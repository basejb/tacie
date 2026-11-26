import Darwin
import Foundation

enum MCPSocketError: Error, Equatable {
    /// The folder is not a real directory owned by this user, or `mcp.sock` exists and is not a socket.
    case unsafeFolder
    /// Another process (another Tacie) already answers on `mcp.sock`; it is left alone.
    case liveListener
    case system(Int32)
}

/// The Unix socket `tacie-mcp` connects to: one connection per tool call, one JSON line in, one line out,
/// then close (docs/38 2-2절). Idle cost is one GCD read source on the listening fd — no thread, no timer.
/// While a connection is open it has a read source and a `readTimeout` timer until its request line arrives.
///
/// The handler is called on a global queue with the request line (without its newline) and a `reply`
/// closure; call `reply` once with the full reply line. Replies after `close()` are dropped.
final class MCPSocket: @unchecked Sendable {
    typealias Handler = @Sendable (Data, @escaping @Sendable (Data) -> Void) -> Void

    let folder: URL
    let maxConnections: Int
    let readTimeout: TimeInterval

    private let q = DispatchQueue(label: "ai.tacie.mcp.socket")
    // Everything below is touched only on `q`.
    private var listenFD: Int32 = -1
    private var listenSource: DispatchSourceRead?
    private var listenClosed: DispatchSemaphore?
    private var boundNode: (dev: dev_t, ino: ino_t)?
    private var handler: Handler?
    private var conns: [Int: Conn] = [:]
    private var nextID = 0
    private var acceptPaused = false

    /// How long to stop accepting after `accept` fails with a resource error (EMFILE, ENFILE…).
    /// The pending connection stays queued, so without a pause the read source would fire in a tight loop.
    var acceptBackoff: TimeInterval = 0.5
    /// Test hook replacing `accept(fd, nil, nil)`; set before `open`.
    var acceptHook: (@Sendable (Int32) -> Int32)?

    private var path: String { folder.appendingPathComponent("mcp.sock").path }

    init(folder: URL, maxConnections: Int = 4, readTimeout: TimeInterval = 5) {
        self.folder = folder
        self.maxConnections = maxConnections
        self.readTimeout = readTimeout
    }

    deinit {
        // Every closure that can close a connection fd (read/timer handlers, the writer) holds `self`
        // strongly, so deinit only runs once none is left: the only connections that can remain are
        // waiting for a handler reply, and `tryClose` here is their one closer. Not on `q`, but nothing
        // else can reach this instance any more.
        _ = shutdownLocked(unlink: true)
    }

    var isOpen: Bool { q.sync { listenFD >= 0 } }

    /// Connections accepted and not yet closed (reading, waiting for the handler, or writing).
    var openConnections: Int { q.sync { conns.count } }

    /// Connections whose request went to the handler and whose reply is not written yet. Closing waits for these to
    /// drain (MCPBridge) so a reply already on its way is not cut to an EOF.
    var owedReplies: Int { q.sync { conns.values.filter { $0.handed }.count } }

    func open(handler: @escaping Handler) throws {
        try q.sync {
            guard listenFD < 0 else { return }
            try prepareFolder()
            let fd = try bindListener()
            self.handler = handler
            listenFD = fd
            let src = DispatchSource.makeReadSource(fileDescriptor: fd, queue: q)
            src.setEventHandler { [weak self] in self?.acceptAll() }
            let closed = DispatchSemaphore(value: 0)
            src.setCancelHandler { Darwin.close(fd); closed.signal() }
            listenClosed = closed
            listenSource = src
            src.resume()
        }
    }

    /// Stops listening, closes every connection (pending replies are dropped) and removes `mcp.sock`.
    func close() {
        // Wait (off `q`) until the listening fd is really closed, so a reopen right after cannot see it alive.
        _ = q.sync { shutdownLocked(unlink: true) }?.wait(timeout: .now() + 2)
    }

    /// Test hook: what a crash leaves behind — the listener is gone but the socket file stays.
    func dropWithoutUnlink() {
        _ = q.sync { shutdownLocked(unlink: false) }?.wait(timeout: .now() + 2)
    }

    // MARK: - path

    private func prepareFolder() throws {
        let dir = folder.path
        if mkdir(dir, 0o700) != 0, errno != EEXIST { throw MCPSocketError.system(errno) }
        var st = stat()
        guard lstat(dir, &st) == 0 else { throw MCPSocketError.system(errno) }
        guard st.st_mode & S_IFMT == S_IFDIR, st.st_uid == getuid() else { throw MCPSocketError.unsafeFolder }
        if st.st_mode & 0o777 != 0o700, chmod(dir, 0o700) != 0 { throw MCPSocketError.system(errno) }

        guard lstat(path, &st) == 0 else {
            if errno == ENOENT { return }
            throw MCPSocketError.system(errno)
        }
        guard st.st_mode & S_IFMT == S_IFSOCK else { throw MCPSocketError.unsafeFolder }
        // A leftover socket: remove it only when nothing answers.
        let probe = socket(AF_UNIX, SOCK_STREAM, 0)
        guard probe >= 0 else { throw MCPSocketError.system(errno) }
        defer { Darwin.close(probe) }
        var addr = try address()
        let r = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(probe, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        if r == 0 { throw MCPSocketError.liveListener }
        guard errno == ECONNREFUSED || errno == ENOENT else { throw MCPSocketError.system(errno) }
        if unlink(path) != 0, errno != ENOENT { throw MCPSocketError.system(errno) }
    }

    private func address() throws -> sockaddr_un {
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8CString) // includes the trailing NUL
        guard bytes.count <= MemoryLayout.size(ofValue: addr.sun_path) else {
            throw MCPSocketError.system(ENAMETOOLONG)
        }
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            bytes.withUnsafeBytes { raw.copyMemory(from: $0) }
        }
        return addr
    }

    private func bindListener() throws -> Int32 {
        var addr = try address()
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw MCPSocketError.system(errno) }
        func fail() -> MCPSocketError {
            let e = errno
            Darwin.close(fd)
            return .system(e)
        }
        guard Self.configure(fd, nonBlocking: true) else { throw fail() }
        let r = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard r == 0 else { throw fail() }
        var st = stat()
        guard chmod(path, 0o600) == 0, lstat(path, &st) == 0, listen(fd, 8) == 0 else {
            let e = errno
            unlink(path)
            Darwin.close(fd)
            throw MCPSocketError.system(e)
        }
        boundNode = (st.st_dev, st.st_ino)
        return fd
    }

    /// Close-on-exec (the app spawns the engine), no SIGPIPE, and the blocking mode.
    private static func configure(_ fd: Int32, nonBlocking: Bool) -> Bool {
        var on: Int32 = 1
        guard fcntl(fd, F_SETFD, FD_CLOEXEC) == 0,
              setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size)) == 0
        else { return false }
        let flags = fcntl(fd, F_GETFL)
        guard flags >= 0 else { return false }
        return fcntl(fd, F_SETFL, nonBlocking ? flags | O_NONBLOCK : flags & ~O_NONBLOCK) == 0
    }

    // MARK: - connections

    /// Touched only on `q` (the `reply` closure hops to `q` before reading it), hence unchecked.
    private final class Conn: @unchecked Sendable {
        let id: Int
        var fd: Int32
        var read: DispatchSourceRead?
        var timer: DispatchSourceTimer?
        var buffer = Data()
        var readCancelled = false
        var writing = false
        var wantClose = false
        var replied = false
        /// The request line went to the handler: a reply is owed or being written.
        var handed = false
        init(id: Int, fd: Int32) { self.id = id; self.fd = fd }
    }

    private func acceptAll() {
        while listenFD >= 0 {
            let fd = acceptHook?(listenFD) ?? accept(listenFD, nil, nil)
            if fd < 0 {
                switch errno {
                case EINTR, ECONNABORTED: continue
                case EAGAIN: return // drained: wait for the next event
                default: return pauseAccepting() // EMFILE, ENFILE, ENOBUFS…: don't spin on the queued peer
                }
            }
            guard Self.configure(fd, nonBlocking: true) else { Darwin.close(fd); continue }
            if conns.count >= maxConnections {
                let busy = MCPWire.fail(.busy) // tiny: fits the empty send buffer without blocking
                _ = busy.withUnsafeBytes { Darwin.write(fd, $0.baseAddress!, $0.count) }
                Darwin.close(fd)
                continue
            }
            startConnection(fd)
        }
    }

    private func pauseAccepting() {
        guard let src = listenSource, !acceptPaused else { return }
        acceptPaused = true
        src.suspend()
        q.asyncAfter(deadline: .now() + acceptBackoff) { [weak self] in
            guard let self, self.acceptPaused, self.listenSource === src else { return }
            self.acceptPaused = false
            src.resume()
        }
    }

    private func startConnection(_ fd: Int32) {
        nextID += 1
        let c = Conn(id: nextID, fd: fd)
        conns[c.id] = c

        let read = DispatchSource.makeReadSource(fileDescriptor: fd, queue: q)
        // Handlers hold `self` and `c` strongly so `tryClose` is the only closer of `c.fd`;
        // GCD drops them once the source is cancelled, which breaks the cycle.
        read.setEventHandler { self.readAvailable(c) }
        read.setCancelHandler {
            c.readCancelled = true
            self.tryClose(c)
        }
        c.read = read

        let timer = DispatchSource.makeTimerSource(queue: q)
        timer.schedule(deadline: .now() + readTimeout)
        timer.setEventHandler { self.finish(c) } // silent client: just close
        c.timer = timer

        read.resume()
        timer.resume()
    }

    private func readAvailable(_ c: Conn) {
        guard !c.wantClose, c.read != nil, !(c.read!.isCancelled) else { return }
        let cap = MCPWire.maxRequest + 1 // the line plus its newline
        var chunk = [UInt8](repeating: 0, count: 16_384)
        while true {
            let room = min(chunk.count, cap - c.buffer.count)
            let n = Darwin.read(c.fd, &chunk, room)
            if n < 0 {
                if errno == EINTR { continue }
                if errno == EAGAIN { return }
                return finish(c)
            }
            if n == 0 { return finish(c) } // peer closed before a full line
            let start = c.buffer.count
            c.buffer.append(contentsOf: chunk[0..<n])
            if let nl = c.buffer[start...].firstIndex(of: 0x0A) {
                return gotLine(c, Data(c.buffer[..<nl]))
            }
            if c.buffer.count >= cap { return reject(c) }
        }
    }

    private func reject(_ c: Conn) {
        stopReading(c)
        send(c, MCPWire.fail(.badRequest))
    }

    private func gotLine(_ c: Conn, _ line: Data) {
        stopReading(c)
        c.buffer = Data()
        guard let handler else { return finish(c) }
        c.handed = true
        let reply: @Sendable (Data) -> Void = { [weak self] data in
            guard let self else { return }
            self.q.async { self.send(c, data) }
        }
        DispatchQueue.global(qos: .userInitiated).async { handler(line, reply) }
    }

    /// Stops the request timer and read source; the fd stays open for the reply.
    private func stopReading(_ c: Conn) {
        c.timer?.cancel()
        c.timer = nil
        if let r = c.read, !r.isCancelled { r.cancel() }
    }

    /// Writes one reply line on a global queue (blocking, with a send timeout) and then closes.
    private func send(_ c: Conn, _ reply: Data) {
        guard !c.wantClose, !c.replied, c.fd >= 0 else { return }
        c.replied = true
        c.writing = true
        var data = reply.count > MCPWire.maxResponse ? MCPWire.fail(.badRequest) : reply
        if data.last != 0x0A { data.append(0x0A) }
        let fd = c.fd
        var tv = timeval(tv_sec: Int(max(readTimeout, 1)), tv_usec: 0)
        guard Self.configure(fd, nonBlocking: false),
              setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size)) == 0
        else {
            c.writing = false
            return finish(c)
        }
        // Holds `self` strongly until the write is done, so the fd is closed by `tryClose` only.
        DispatchQueue.global(qos: .userInitiated).async {
            data.withUnsafeBytes { raw in
                var off = 0
                while off < raw.count {
                    let n = Darwin.write(fd, raw.baseAddress! + off, raw.count - off)
                    if n < 0, errno == EINTR { continue }
                    if n <= 0 { break } // peer gone (EPIPE, no signal), timed out, or shut down by close()
                    off += n
                }
            }
            self.q.async {
                c.writing = false
                self.finish(c)
            }
        }
    }

    private func finish(_ c: Conn) {
        c.wantClose = true
        stopReading(c)
        tryClose(c)
    }

    /// Closes the fd once GCD has let go of it (read source cancelled) and no write is using it.
    private func tryClose(_ c: Conn) {
        guard c.wantClose, c.readCancelled, !c.writing else { return }
        if c.fd >= 0 { Darwin.close(c.fd); c.fd = -1 }
        conns[c.id] = nil
    }

    /// Returns a semaphore signalled once the listening fd is closed; wait on it off `q`.
    private func shutdownLocked(unlink removeFile: Bool) -> DispatchSemaphore? {
        guard listenFD >= 0 else { return nil }
        let closed = listenClosed
        listenClosed = nil
        if acceptPaused { acceptPaused = false; listenSource?.resume() } // a suspended source never runs its cancel handler
        listenSource?.cancel() // its cancel handler closes the listening fd
        listenSource = nil
        listenFD = -1
        handler = nil
        if removeFile, let node = boundNode {
            // Only remove the file this instance bound, not one another process put there since.
            var st = stat()
            if lstat(path, &st) == 0, st.st_dev == node.dev, st.st_ino == node.ino { unlink(path) }
        }
        boundNode = nil
        for c in conns.values {
            if c.writing { Darwin.shutdown(c.fd, SHUT_RDWR) } // unblocks the writer; it closes after
            finish(c)
        }
        return closed
    }
}
