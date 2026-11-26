import Darwin
import XCTest

final class MCPSocketTests: XCTestCase {
    private var root: URL!
    private var folder: URL { root.appendingPathComponent("mcp") }
    private var sockPath: String { folder.appendingPathComponent("mcp.sock").path }
    private var sockets: [MCPSocket] = []

    override func setUp() {
        super.setUp()
        // Unix socket paths must stay under 104 bytes.
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("tm\(UUID().uuidString.prefix(6))")
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    override func tearDown() {
        sockets.forEach { $0.close() }
        try? FileManager.default.removeItem(at: root)
        super.tearDown()
    }

    private func make(max: Int = 4, timeout: TimeInterval = 5, at dir: URL? = nil) -> MCPSocket {
        let s = MCPSocket(folder: dir ?? folder, maxConnections: max, readTimeout: timeout)
        sockets.append(s)
        return s
    }

    private let echo: @Sendable (Data, @escaping @Sendable (Data) -> Void) -> Void = { line, reply in
        var d = line
        d.append(0x0A)
        reply(d)
    }

    // MARK: client helpers

    private func connectClient(_ path: String? = nil) -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        var tv = timeval(tv_sec: 3, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let p = Array((path ?? sockPath).utf8CString)
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            p.withUnsafeBytes { raw.copyMemory(from: $0) }
        }
        let r = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        XCTAssertEqual(r, 0, "connect failed: \(errno)")
        return fd
    }

    private func send(_ fd: Int32, _ data: Data) {
        data.withUnsafeBytes { raw in
            var off = 0
            while off < raw.count {
                let n = Darwin.write(fd, raw.baseAddress! + off, raw.count - off)
                if n <= 0 { return } // peer closed; fine for the oversize test
                off += n
            }
        }
    }

    /// Reads until EOF (or the 3 s receive timeout). Returns the bytes and whether EOF was seen.
    private func readAll(_ fd: Int32) -> (Data, Bool) {
        var out = Data()
        var buf = [UInt8](repeating: 0, count: 4096)
        while true {
            let n = Darwin.read(fd, &buf, buf.count)
            if n == 0 { return (out, true) }
            if n < 0 { return (out, false) }
            out.append(contentsOf: buf[0..<n])
        }
    }

    private func roundTrip(_ text: String) -> String? {
        let fd = connectClient()
        defer { Darwin.close(fd) }
        send(fd, Data((text + "\n").utf8))
        let (data, eof) = readAll(fd)
        XCTAssertTrue(eof)
        return String(data: data, encoding: .utf8)
    }

    private func errorCode(_ data: Data) -> String? {
        let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        return obj?["error"] as? String
    }

    // MARK: tests

    func testRoundTrip() throws {
        let s = make()
        try s.open(handler: echo)
        XCTAssertTrue(s.isOpen)
        XCTAssertEqual(roundTrip("{\"v\":1}"), "{\"v\":1}\n")
    }

    func testMaxSizeLineAccepted() throws {
        let s = make()
        try s.open(handler: echo)
        let line = String(repeating: "a", count: MCPWire.maxRequest)
        let fd = connectClient()
        defer { Darwin.close(fd) }
        let writer = Thread { [self] in send(fd, Data((line + "\n").utf8)) }
        writer.start()
        let (data, eof) = readAll(fd)
        XCTAssertTrue(eof)
        XCTAssertEqual(data.count, MCPWire.maxRequest + 1)
    }

    func testSilentClientClosedAfterTimeout() throws {
        let s = make(timeout: 0.3)
        try s.open(handler: echo)
        let fd = connectClient()
        defer { Darwin.close(fd) }
        let start = Date()
        let (data, eof) = readAll(fd)
        XCTAssertTrue(eof)
        XCTAssertTrue(data.isEmpty)
        XCTAssertLessThan(Date().timeIntervalSince(start), 1.0)
        XCTAssertEqual(roundTrip("again"), "again\n")
    }

    func testOversizedRequestRejected() throws {
        let s = make()
        try s.open(handler: { line, reply in
            XCTAssertLessThanOrEqual(line.count, MCPWire.maxRequest, "oversized line reached the handler")
            reply(line + Data([0x0A]))
        })
        let fd = connectClient()
        defer { Darwin.close(fd) }
        let big = Data(repeating: 0x61, count: 70 * 1024)
        let writer = Thread { [self] in send(fd, big) }
        writer.start()
        let (data, eof) = readAll(fd)
        XCTAssertTrue(eof)
        XCTAssertEqual(errorCode(data), "bad_request")
        XCTAssertEqual(roundTrip("ok"), "ok\n")
    }

    func testConnectionCap() throws {
        let s = make(max: 2)
        try s.open(handler: echo)
        let a = connectClient(), b = connectClient()
        defer { Darwin.close(a); Darwin.close(b) }
        // Make sure both are accepted before the third arrives.
        let deadline = Date().addingTimeInterval(2)
        while s.openConnections < 2, Date() < deadline { usleep(5_000) }
        XCTAssertEqual(s.openConnections, 2)
        let c = connectClient()
        defer { Darwin.close(c) }
        let (data, eof) = readAll(c)
        XCTAssertTrue(eof)
        XCTAssertEqual(errorCode(data), "busy")
        // The first two still work.
        send(a, Data("one\n".utf8))
        XCTAssertEqual(readAll(a).0, Data("one\n".utf8))
    }

    func testStaleSocketReplaced() throws {
        let first = make()
        try first.open(handler: echo)
        // Simulate a crash: the listening fd goes away but the file stays.
        first.dropWithoutUnlink()
        XCTAssertTrue(FileManager.default.fileExists(atPath: sockPath))
        let second = make()
        try second.open(handler: echo)
        XCTAssertEqual(roundTrip("fresh"), "fresh\n")
    }

    func testLiveListenerKept() throws {
        let first = make()
        try first.open(handler: echo)
        let second = make()
        XCTAssertThrowsError(try second.open(handler: echo)) { e in
            guard case MCPSocketError.liveListener = e else { return XCTFail("got \(e)") }
        }
        XCTAssertFalse(second.isOpen)
        XCTAssertEqual(roundTrip("still"), "still\n")
    }

    func testNonSocketFileRefused() throws {
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: sockPath, contents: Data("x".utf8))
        XCTAssertThrowsError(try make().open(handler: echo)) { e in
            guard case MCPSocketError.unsafeFolder = e else { return XCTFail("got \(e)") }
        }
        XCTAssertTrue(FileManager.default.fileExists(atPath: sockPath))
    }

    func testSymlinkFolderRefused() throws {
        let real = root.appendingPathComponent("real")
        try FileManager.default.createDirectory(at: real, withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(at: folder, withDestinationURL: real)
        XCTAssertThrowsError(try make().open(handler: echo)) { e in
            guard case MCPSocketError.unsafeFolder = e else { return XCTFail("got \(e)") }
        }
        XCTAssertFalse(FileManager.default.fileExists(atPath: real.appendingPathComponent("mcp.sock").path))
    }

    func testPermissions() throws {
        let s = make()
        try s.open(handler: echo)
        var st = stat()
        XCTAssertEqual(lstat(folder.path, &st), 0)
        XCTAssertEqual(st.st_mode & 0o777, 0o700)
        XCTAssertEqual(lstat(sockPath, &st), 0)
        XCTAssertEqual(st.st_mode & S_IFMT, S_IFSOCK)
        XCTAssertEqual(st.st_mode & 0o777, 0o600)
    }

    func testCloseRemovesFile() throws {
        let s = make()
        try s.open(handler: echo)
        XCTAssertTrue(FileManager.default.fileExists(atPath: sockPath))
        s.close()
        XCTAssertFalse(s.isOpen)
        XCTAssertFalse(FileManager.default.fileExists(atPath: sockPath))
        // Reopen on the same instance works.
        try s.open(handler: echo)
        XCTAssertEqual(roundTrip("re"), "re\n")
    }

    func testCloseWhileHandlerPendingDropsReply() throws {
        let s = make()
        let held = Holder()
        try s.open(handler: { _, reply in held.set(reply) })
        let fd = connectClient()
        defer { Darwin.close(fd) }
        send(fd, Data("x\n".utf8))
        let deadline = Date().addingTimeInterval(2)
        while held.get() == nil, Date() < deadline { usleep(5_000) }
        s.close()
        let (data, eof) = readAll(fd)
        XCTAssertTrue(eof)
        XCTAssertTrue(data.isEmpty)
        held.get()?(Data("late\n".utf8)) // must not crash or write
        XCTAssertEqual(s.openConnections, 0)
    }

    func testAcceptResourceErrorDoesNotSpin() throws {
        let s = make()
        s.acceptBackoff = 0.2
        let calls = Counter()
        let failUntil = Date().addingTimeInterval(0.1)
        s.acceptHook = { fd in
            if Date() < failUntil { calls.bump(); errno = EMFILE; return -1 }
            return accept(fd, nil, nil)
        }
        try s.open(handler: echo)
        let start = Date()
        XCTAssertEqual(roundTrip("after"), "after\n") // accepted once the pause ends
        XCTAssertGreaterThanOrEqual(Date().timeIntervalSince(start), 0.1)
        XCTAssertLessThanOrEqual(calls.value, 2, "accept retried in a tight loop")
    }
}

private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var n = 0
    func bump() { lock.lock(); n += 1; lock.unlock() }
    var value: Int { lock.lock(); defer { lock.unlock() }; return n }
}

private final class Holder: @unchecked Sendable {
    private let lock = NSLock()
    private var reply: (@Sendable (Data) -> Void)?
    func set(_ r: @escaping @Sendable (Data) -> Void) { lock.lock(); reply = r; lock.unlock() }
    func get() -> (@Sendable (Data) -> Void)? { lock.lock(); defer { lock.unlock() }; return reply }
}
