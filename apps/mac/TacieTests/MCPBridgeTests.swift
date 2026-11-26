import Darwin
import XCTest

@MainActor
private final class FakeHost: MCPHost {
    var accountID: String? = "acct"
    var engineGeneration = 1
    var engineReady = true
    var modelReady = true
    var meetingRunning = false
    var meetings: [String: SavedMeeting] = [:]
    /// Called with each engine request; return the engine's JSON or throw.
    var answer: (String, [String: Any]) throws -> Data = { _, _ in Data(#"{"cards":[],"confident":false,"ranked":true}"#.utf8) }
    /// When set, `engineTool` waits here until the test resumes it.
    var hold = false
    var held: [CheckedContinuation<Void, Never>] = []
    var calls: [(String, [String: Any])] = []

    func engineTool(_ path: String, _ body: [String: Any]) async throws -> Data {
        calls.append((path, body))
        if hold { await withCheckedContinuation { held.append($0) } }
        return try answer(path, body)
    }

    func meeting(id: String) -> SavedMeeting? { meetings[id] }

    func lineName(_ m: SavedMeeting) -> (TranscriptLine) -> String { { "N-" + ($0.voice ?? "") } }

    func release() {
        let h = held
        held = []
        h.forEach { $0.resume() }
    }
}

@MainActor
final class MCPBridgeTests: XCTestCase {
    private var root: URL!
    private var folder: URL { root.appendingPathComponent("mcp") }
    private var sockFile: String { folder.appendingPathComponent("mcp.sock").path }
    private var bridges: [MCPBridge] = []

    override func setUp() {
        super.setUp()
        // Unix socket paths must stay under 104 bytes.
        root = FileManager.default.temporaryDirectory.appendingPathComponent("tb\(UUID().uuidString.prefix(6))")
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    override func tearDown() {
        bridges.forEach { $0.stop() }
        try? FileManager.default.removeItem(at: root)
        super.tearDown()
    }

    private var events: [AnalyticsEvent] = []

    private func make(_ host: FakeHost, now: @escaping () -> Date = Date.init, allowed: @escaping () -> Bool = { true }) -> MCPBridge {
        let b = MCPBridge(host: host, folder: folder, now: now, allowed: allowed, capture: { [unowned self] in events.append($0) })
        bridges.append(b)
        return b
    }

    private func req(_ tool: MCPTool, _ args: [String: Any] = [:]) -> MCPRequest {
        MCPRequest(tool: tool, args: args, client: .other)
    }

    private func reply(_ d: Data) -> [String: Any] {
        (try? JSONSerialization.jsonObject(with: d) as? [String: Any]) ?? [:]
    }

    private func error(_ d: Data) -> String? { reply(d)["error"] as? String }

    private func ok(_ d: Data) -> Bool { reply(d)["ok"] as? Bool == true }

    private func expect(_ b: MCPBridge, _ r: MCPRequest, _ code: String, line: UInt = #line) async {
        let d = await b.handle(r)
        XCTAssertEqual(error(d), code, line: line)
    }

    private func expectOK(_ b: MCPBridge, _ r: MCPRequest, line: UInt = #line) async {
        let d = await b.handle(r)
        XCTAssertTrue(ok(d), String(decoding: d, as: UTF8.self), line: line)
    }

    /// Lets queued main-actor work run until `cond` holds (or a second passes).
    private func until(_ cond: () -> Bool) async {
        for _ in 0..<200 where !cond() { try? await Task.sleep(for: .milliseconds(5)) }
    }

    func testSwitchMidRequestDropsResult() async {
        let host = FakeHost()
        host.hold = true
        let b = make(host)
        let t = Task { await b.handle(req(.listLedger)) }
        await until { !host.held.isEmpty }
        host.engineGeneration += 1  // the engine started again for another account
        host.release()
        let d = await t.value
        XCTAssertEqual(error(d), "switching")
    }

    func testAccountChangeMidRequestDropsResult() async {
        let host = FakeHost()
        host.hold = true
        host.meetings["m1"] = meeting()
        let b = make(host)
        let t = Task { await b.handle(req(.search, ["query": "why"])) }
        await until { !host.held.isEmpty }
        host.accountID = "other"
        host.release()
        let r = await t.value
        XCTAssertEqual(error(r), "switching")
    }

    func testSearchBusyDuringMeeting() async {
        let host = FakeHost()
        host.meetingRunning = true
        host.answer = { _, _ in Data(#"{"entries":[],"more":false}"#.utf8) }
        let b = make(host)
        await expect(b, req(.search, ["query": "why"]), "busy")
        XCTAssertTrue(host.calls.isEmpty)
        await expectOK(b, req(.listLedger))
    }

    func testSearchMapsQueryToText() async {
        let host = FakeHost()
        let b = make(host)
        let d = await b.handle(req(.search, ["query": "why", "meeting_ids": ["a"]]))
        XCTAssertTrue(ok(d))
        XCTAssertEqual(host.calls.first?.0, "/tools/search")
        XCTAssertEqual(host.calls.first?.1["text"] as? String, "why")
        XCTAssertEqual(host.calls.first?.1["meeting_ids"] as? [String], ["a"])
        XCTAssertNil(host.calls.first?.1["query"])
        await expect(b, req(.search, [:]), "bad_request")
    }

    func testToolCallEventCarriesNumbersOnly() async {
        let host = FakeHost()
        host.answer = { _, _ in Data(#"{"meetings":[{"title":"secret"},{"title":"x"}],"more":false}"#.utf8) }
        let b = make(host)
        await expectOK(b, MCPRequest(tool: .listMeetings, args: [:], client: .cursor))
        await expect(b, req(.search, [:]), "bad_request")
        XCTAssertEqual(events.count, 2)
        guard case let .mcpToolCalled(tool, _, results, failure, client) = events[0] else { return XCTFail() }
        XCTAssertEqual([tool.rawValue, failure.rawValue, client.rawValue], ["list_meetings", "none", "cursor"])
        XCTAssertEqual(results, 2)
        guard case let .mcpToolCalled(_, _, none, bad, other) = events[1] else { return XCTFail() }
        XCTAssertEqual([none, bad.rawValue, other.rawValue] as [AnyHashable], [0, "bad_request", "other"])
        XCTAssertFalse(events.flatMap { $0.properties.values }.contains(.text("secret")))
    }

    func testToolCallEventsLimitedToTenPerMinute() async {
        let host = FakeHost()
        var clock = Date(timeIntervalSince1970: 1_800_000_000)
        let b = make(host, now: { clock })
        for _ in 0..<11 { await expectOK(b, req(.listMeetings)) }
        XCTAssertEqual(events.count, 10)
        clock += 61
        await expectOK(b, req(.listMeetings))
        XCTAssertEqual(events.count, 11)
    }

    func testRateLimits() async {
        let host = FakeHost()
        var clock = Date(timeIntervalSince1970: 1_800_000_000)
        let b = make(host, now: { clock })
        for _ in 0..<60 { await expectOK(b, req(.listMeetings)) }
        await expect(b, req(.listMeetings), "busy")  // the 61st inside the minute
        clock = clock.addingTimeInterval(61)
        await expectOK(b, req(.listMeetings))

        // three at once: the third is refused while two are in flight
        host.hold = true
        let t1 = Task { await b.handle(req(.listMeetings)) }
        let t2 = Task { await b.handle(req(.listMeetings)) }
        await until { host.held.count == 2 }
        await expect(b, req(.listMeetings), "busy")
        host.release()
        let r1 = await t1.value, r2 = await t2.value
        XCTAssertTrue(ok(r1))
        XCTAssertTrue(ok(r2))
    }

    func testNotReady() async {
        let host = FakeHost()
        host.engineReady = false
        let b = make(host)
        await expect(b, req(.listMeetings), "not_ready")
        await expect(b, req(.search, ["query": "x"]), "not_ready")
        XCTAssertTrue(host.calls.isEmpty)
    }

    func testModelMissingStopsSearchOnly() async {
        let host = FakeHost()
        host.modelReady = false
        let b = make(host)
        await expect(b, req(.search, ["query": "x"]), "model")
        await expectOK(b, req(.listMeetings))
    }

    func testCreditMapsToLimit() async {
        let host = FakeHost()
        host.answer = { _, _ in throw EngineError.http(502, #"{"error":"x","error_kind":"credit"}"#) }
        let b = make(host)
        await expect(b, req(.search, ["query": "x"]), "limit")
        host.answer = { _, _ in throw EngineError.http(502, #"{"error":"x","error_kind":"network"}"#) }
        await expect(b, req(.search, ["query": "x"]), "network")
        host.answer = { _, _ in throw EngineError.http(400, #"{"error":"bad"}"#) }
        await expect(b, req(.listLedger, ["limit": "x"]), "bad_request")
        // a search whose rerank ran out of the monthly limit still answers, with the reason in the wire's word
        host.answer = { _, _ in Data(#"{"cards":[],"confident":false,"ranked":false,"reason":"credit"}"#.utf8) }
        let d = await b.handle(req(.search, ["query": "x"]))
        XCTAssertTrue(ok(d))
        XCTAssertEqual((reply(d)["result"] as? [String: Any])?["reason"] as? String, "limit")
    }

    func testTimeoutAnswersBusyOnce() async {
        let host = FakeHost()
        host.hold = true
        let b = make(host)
        b.timeout = 0.05
        let d = await b.handle(req(.listMeetings))
        XCTAssertEqual(error(d), "busy")
        host.release()  // the late result goes nowhere
    }

    private func meeting() -> SavedMeeting {
        var m = SavedMeeting(mode: "inperson", start: Date(timeIntervalSince1970: 1_800_000_000))
        m.transcript = (0..<3).map {
            TranscriptLine(key: "k\($0)", at: m.start.addingTimeInterval(Double($0) * 60), speaker: nil, text: "line \($0)",
                           question: false, voice: "화자 1")
        }
        return m
    }

    func testTranscriptFromMeetingFile() async {
        let host = FakeHost()
        let m = meeting()
        host.meetings[m.id] = m
        let b = make(host)
        let d = await b.handle(req(.readTranscript, ["meeting_id": m.id, "limit": 2]))
        XCTAssertTrue(ok(d))
        let lines = (reply(d)["result"] as? [String: Any])?["lines"] as? [[String: Any]]
        XCTAssertEqual(lines?.map { $0["key"] as? String }, ["k0", "k1"])
        XCTAssertEqual(lines?.first?["speaker"] as? String, "N-화자 1")
        XCTAssertTrue(host.calls.isEmpty)
        await expect(b, req(.readTranscript, ["meeting_id": "nope"]), "bad_request")
        await expect(b, req(.readTranscript, [:]), "bad_request")
    }

    func testRefreshOpensOnlyWhenAllowed() {
        let name = "mcp-bridge-\(UUID().uuidString)"
        let d = UserDefaults(suiteName: name)!
        d.removePersistentDomain(forName: name)
        let space = AccountSpace(id: "aaa")
        var offered = false
        let b = make(FakeHost(), allowed: { MCPAccess.open(d, space: space, offered: offered) })
        MCPAccess.set(true, d, space: space)

        b.refresh()  // not offered
        XCTAssertFalse(FileManager.default.fileExists(atPath: sockFile))
        offered = true
        MCPAccess.remember("off", defaults: d)
        b.refresh()  // the server says off
        XCTAssertFalse(FileManager.default.fileExists(atPath: sockFile))
        MCPAccess.remember("all", defaults: d)
        b.refresh()
        XCTAssertTrue(FileManager.default.fileExists(atPath: sockFile))
        MCPAccess.set(false, d, space: space)
        b.refresh()
        XCTAssertFalse(FileManager.default.fileExists(atPath: sockFile))
    }

    func testSuspendClosesSocket() async {
        let host = FakeHost()
        host.hold = true
        let b = make(host)
        b.refresh()
        XCTAssertTrue(FileManager.default.fileExists(atPath: sockFile))
        let t = Task { await b.handle(req(.listMeetings)) }
        await until { !host.held.isEmpty }
        await b.suspend()  // the request in flight never answers with this account's data
        XCTAssertFalse(FileManager.default.fileExists(atPath: sockFile))
        let r = await t.value
        XCTAssertEqual(error(r), "switching")
        b.refresh()  // still held until the switch is over
        XCTAssertFalse(FileManager.default.fileExists(atPath: sockFile))
        b.resume()
        XCTAssertTrue(FileManager.default.fileExists(atPath: sockFile))
        host.release()
    }

    func testTurningOffAnswersInFlightOff() async {
        let host = FakeHost()
        host.hold = true
        var on = true
        let b = make(host, allowed: { on })
        b.refresh()
        let t = Task { await b.handle(req(.listMeetings)) }
        await until { !host.held.isEmpty }
        on = false
        b.refresh()
        let r = await t.value
        XCTAssertEqual(error(r), "off")
        await until { !FileManager.default.fileExists(atPath: self.sockFile) }
        XCTAssertFalse(FileManager.default.fileExists(atPath: sockFile))
        host.release()
    }

    func testTurningOffFlushesReplyOverSocket() async throws {
        let host = FakeHost()
        host.hold = true
        var on = true
        let b = make(host, allowed: { on })
        b.refresh()
        let path = sockFile
        let client = Task.detached { try Self.ask(path, #"{"v":1,"tool":"list_meetings","args":{},"client":"other"}"#) }
        await until { !host.held.isEmpty }
        on = false
        b.refresh()
        let line = try await client.value
        XCTAssertEqual(line.filter { $0 == "\n" }.count, 1)
        XCTAssertEqual(error(Data(line.utf8)), "off")  // a reply, not a dropped connection
        await until { !FileManager.default.fileExists(atPath: self.sockFile) }
        XCTAssertFalse(FileManager.default.fileExists(atPath: sockFile))
        host.release()
    }

    func testReopenDuringOffDrainKeepsSocket() async {
        let host = FakeHost()
        host.hold = true
        var on = true
        let b = make(host, allowed: { on })
        b.refresh()
        let t = Task { await b.handle(req(.listMeetings)) }
        await until { !host.held.isEmpty }
        on = false
        b.refresh()
        on = true
        b.refresh()
        _ = await t.value
        try? await Task.sleep(for: .milliseconds(200))
        XCTAssertTrue(FileManager.default.fileExists(atPath: sockFile))
        host.release()
    }

    /// Sends one request line and reads until the app closes the connection.
    nonisolated private static func ask(_ path: String, _ request: String) throws -> String {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        var tv = timeval(tv_sec: 5, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let p = Array(path.utf8CString)
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in p.withUnsafeBytes { raw.copyMemory(from: $0) } }
        let r = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard r == 0 else { Darwin.close(fd); throw POSIXError(.ECONNREFUSED) }
        defer { Darwin.close(fd) }
        let msg = Array(request.utf8) + [0x0A]
        _ = msg.withUnsafeBytes { Darwin.write(fd, $0.baseAddress, $0.count) }
        var out = [UInt8]()
        var buf = [UInt8](repeating: 0, count: 4096)
        while true {
            let n = Darwin.read(fd, &buf, buf.count)
            if n <= 0 { break }
            out += buf[..<n]
        }
        return String(decoding: out, as: UTF8.self)
    }

    func testServeOverSocketRepliesOnce() async throws {
        let host = FakeHost()
        host.answer = { _, _ in Data(#"{"meetings":[],"more":false}"#.utf8) }
        let b = make(host)
        b.refresh()
        let path = sockFile
        let line = try await Task.detached { () -> String in
            let fd = socket(AF_UNIX, SOCK_STREAM, 0)
            var tv = timeval(tv_sec: 3, tv_usec: 0)
            setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
            var addr = sockaddr_un()
            addr.sun_family = sa_family_t(AF_UNIX)
            let p = Array(path.utf8CString)
            withUnsafeMutableBytes(of: &addr.sun_path) { raw in p.withUnsafeBytes { raw.copyMemory(from: $0) } }
            let r = withUnsafePointer(to: &addr) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard r == 0 else { throw POSIXError(.ECONNREFUSED) }
            defer { Darwin.close(fd) }
            let msg = Array(#"{"v":1,"tool":"list_meetings","args":{},"client":"other"}"#.utf8) + [0x0A]
            _ = msg.withUnsafeBytes { Darwin.write(fd, $0.baseAddress, $0.count) }
            var out = [UInt8]()
            var buf = [UInt8](repeating: 0, count: 4096)
            while true {
                let n = Darwin.read(fd, &buf, buf.count)
                if n <= 0 { break }
                out += buf[..<n]
            }
            return String(decoding: out, as: UTF8.self)
        }.value
        XCTAssertTrue(line.hasSuffix("\n"))
        XCTAssertEqual(line.filter { $0 == "\n" }.count, 1)
        XCTAssertTrue(ok(Data(line.utf8)))
    }
}
