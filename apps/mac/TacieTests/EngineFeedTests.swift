import XCTest

/// What the iPhone app hands its engine (docs/40 4-1, 6-1): the session, then the meetings, on every engine start.
@MainActor
final class EngineFeedTests: XCTestCase {
    private final class Fake: EngineFeeding {
        var calls: [String] = []
        var held: [MeetingDigest]?
        /// When set, `setMeetings` waits for it.
        var gate: CheckedContinuation<Void, Never>?
        var hold = false
        /// When set, `setRelaySession` waits for the gate and then fails.
        var holdSession = false
        var failSession = false
        var sessionGate: CheckedContinuation<Void, Never>?
        func setRelaySession(_ token: String) async throws {
            if holdSession { await withCheckedContinuation { sessionGate = $0 } }
            if failSession { throw URLError(.cannotConnectToHost) }
            calls.append("session:\(token)")
        }
        func clearRelaySession() async throws { calls.append("clear") }
        /// When set, `setMeetings` (after the gate) fails once.
        var failMeetingsOnce = false
        func setMeetings(_ list: [MeetingDigest]) async throws -> Int {
            if hold { await withCheckedContinuation { gate = $0 } }
            if failMeetingsOnce { failMeetingsOnce = false; throw URLError(.cannotConnectToHost) }
            calls.append("meetings:\(list.map(\.id).joined(separator: ","))")
            held = list
            return list.count
        }
    }

    private func digest(_ id: String) -> MeetingDigest {
        MeetingDigest(id: id, title: id, start: "2026-11-17T10:00:00+09:00", end: "2026-11-17T11:00:00+09:00",
                      summary: ["요약"], topics: [], ledger: [])
    }

    func testSendsTokenThenMeetingsOnStart() async {
        let feed = EngineFeed(token: { "tok" }, meetings: { [self.digest("b"), self.digest("a")] })
        let e = Fake()
        await feed.engineStarted(e)
        XCTAssertEqual(e.calls, ["session:tok", "meetings:a,b"])
        let ready = await feed.ready()
        XCTAssertTrue(ready)
    }

    func testResendsAfterRestart() async {
        let feed = EngineFeed(token: { "tok" }, meetings: { [self.digest("a")] })
        let first = Fake(), second = Fake()
        await feed.engineStarted(first)
        feed.engineLost()
        let during = await feed.ready()
        XCTAssertFalse(during, "no engine, not ready")
        await feed.engineStarted(second)
        XCTAssertEqual(second.calls, ["session:tok", "meetings:a"])
        XCTAssertEqual(first.calls.count, 2)
    }

    func testSignOutClearsMeetings() async {
        var token: String? = "tok"
        var list = [digest("a")]
        let feed = EngineFeed(token: { token }, meetings: { list })
        let e = Fake()
        await feed.engineStarted(e)
        e.calls = []
        await feed.signedOut()
        XCTAssertEqual(e.calls, ["meetings:", "clear"])
        token = nil; list = []
        await feed.tokenChanged()
        XCTAssertEqual(e.calls, ["meetings:", "clear"], "nothing more to send")
    }

    func testTokenChangeSendsAgainAndMeetingsChangeSendsMeetings() async {
        var token: String? = "one"
        var list = [digest("a")]
        let feed = EngineFeed(token: { token }, meetings: { list })
        let e = Fake()
        await feed.engineStarted(e)
        e.calls = []
        token = "two"
        await feed.tokenChanged()
        XCTAssertEqual(e.calls, ["session:two"])
        list.append(digest("c"))
        await feed.meetingsChanged()
        XCTAssertEqual(e.calls, ["session:two", "meetings:a,c"])
    }

    func testNoTokenNoSession() async {
        let feed = EngineFeed(token: { nil }, meetings: { [] })
        let e = Fake()
        await feed.engineStarted(e)
        XCTAssertEqual(e.calls, ["meetings:"])
        let ready = await feed.ready()
        XCTAssertTrue(ready)
    }

    func testReadyWaitsForMeetings() async {
        let feed = EngineFeed(token: { "tok" }, meetings: { [self.digest("a")] })
        let e = Fake()
        e.hold = true
        let starting = Task { await feed.engineStarted(e) }
        for _ in 0..<50 where e.gate == nil { try? await Task.sleep(for: .milliseconds(10)) }
        XCTAssertNotNil(e.gate, "the meetings post is waiting")
        let ready = Task { await feed.ready() }
        try? await Task.sleep(for: .milliseconds(100))
        XCTAssertNil(e.held, "not sent yet")
        e.hold = false
        e.gate?.resume()
        await starting.value
        let r = await ready.value
        XCTAssertTrue(r)
        XCTAssertEqual(e.held?.map(\.id), ["a"])
    }

    func testReadyGivesUpAfterTheWait() async {
        let feed = EngineFeed(wait: .milliseconds(100), token: { "tok" }, meetings: { [] })
        let ready = await feed.ready()
        XCTAssertFalse(ready, "no engine")
    }

    func testSignOutWhileASendIsInFlight() async {
        var token: String? = "A"
        var list = [digest("a")]
        let feed = EngineFeed(signedIn: { token != nil }, token: { token }, meetings: { list })
        let e = Fake()
        e.hold = true
        let starting = Task { await feed.engineStarted(e) }
        for _ in 0..<50 where e.gate == nil { try? await Task.sleep(for: .milliseconds(10)) }
        XCTAssertNotNil(e.gate)
        await feed.signedOut()
        e.hold = false
        e.gate?.resume()
        await starting.value
        XCTAssertEqual(e.held, [], "the engine ends with nothing of A")
        XCTAssertEqual(e.calls.last, "clear")
        token = "B"; list = []
        await feed.tokenChanged()
        XCTAssertEqual(e.calls.last, "session:B")
        XCTAssertEqual(e.held, [])
        let ready = await feed.ready()
        XCTAssertTrue(ready)
    }

    func testSignOutThenOtherAccountSendsItsList() async {
        var token: String? = "A"
        var list = [digest("a")]
        let feed = EngineFeed(token: { token }, meetings: { list })
        let e = Fake()
        await feed.engineStarted(e)
        await feed.signedOut()
        token = nil; list = []
        await feed.tokenChanged()
        token = "B"; list = [digest("b")]
        await feed.tokenChanged()
        XCTAssertEqual(e.held?.map(\.id), ["b"])
        XCTAssertEqual(e.calls.last, "meetings:b")
        XCTAssertTrue(e.calls.contains("session:B"))
    }

    func testKeyFilesModeSendsNoClear() async {
        let feed = EngineFeed(signedIn: { true }, token: { nil }, meetings: { [self.digest("a")] })
        let e = Fake()
        await feed.engineStarted(e)
        await feed.signedOut()
        XCTAssertFalse(e.calls.contains("clear"))
        await feed.tokenChanged()
        XCTAssertEqual(e.calls.last, "meetings:a", "signed in again: the list goes back")
    }

    func testNewEngineWhileOldSendIsInFlight() async {
        let feed = EngineFeed(token: { "tok" }, meetings: { [self.digest("a")] })
        let first = Fake(), second = Fake()
        first.holdSession = true
        first.failSession = true
        let starting = Task { await feed.engineStarted(first) }
        for _ in 0..<50 where first.sessionGate == nil { try? await Task.sleep(for: .milliseconds(10)) }
        XCTAssertNotNil(first.sessionGate)
        await feed.engineStarted(second)
        first.sessionGate?.resume()
        await starting.value
        XCTAssertEqual(second.calls, ["session:tok", "meetings:a"])
        let ready = await feed.ready()
        XCTAssertTrue(ready)
    }

    func testNewEngineWhileOldSendSucceeds() async {
        let feed = EngineFeed(token: { "tok" }, meetings: { [self.digest("a")] })
        let first = Fake(), second = Fake()
        first.holdSession = true
        let starting = Task { await feed.engineStarted(first) }
        for _ in 0..<50 where first.sessionGate == nil { try? await Task.sleep(for: .milliseconds(10)) }
        await feed.engineStarted(second)
        first.sessionGate?.resume()
        await starting.value
        XCTAssertEqual(second.calls, ["session:tok", "meetings:a"], "the old engine's success is not the new one's")
    }

    func testSignOutDuringAFailingSendStillClears() async {
        let feed = EngineFeed(token: { "A" }, meetings: { [self.digest("a")] })
        let e = Fake()
        e.hold = true
        e.failMeetingsOnce = true
        let starting = Task { await feed.engineStarted(e) }
        for _ in 0..<50 where e.gate == nil { try? await Task.sleep(for: .milliseconds(10)) }
        XCTAssertNotNil(e.gate, "the meetings post is waiting, and will fail")
        await feed.signedOut()
        e.hold = false
        e.gate?.resume()
        await starting.value
        XCTAssertEqual(e.held, [], "the sign-out that came during the failed send cleared the meetings")
        XCTAssertEqual(e.calls.last, "clear", "and the session")
    }

    func testNoMeetingsWhenNotFed() async {
        var fed = false
        var read = 0
        let feed = EngineFeed(feedsMeetings: { fed }, token: { "tok" }, meetings: { read += 1; return [self.digest("a")] })
        let e = Fake()
        await feed.engineStarted(e)
        await feed.meetingsChanged()
        XCTAssertEqual(e.calls, ["session:tok"], "only the session")
        let ready = await feed.ready()
        XCTAssertTrue(ready, "ready without meetings")
        await feed.signedOut()
        XCTAssertEqual(e.calls, ["session:tok", "clear"])
        XCTAssertEqual(read, 0, "the meetings are not even read")
        await feed.tokenChanged()
        XCTAssertEqual(e.calls.last, "session:tok")
        fed = true
        await feed.meetingsChanged()
        XCTAssertEqual(e.calls.last, "meetings:a", "turned on: the list goes")
    }
}
