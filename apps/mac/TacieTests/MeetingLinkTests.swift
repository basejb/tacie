import XCTest

@MainActor
final class MeetingLinkTests: XCTestCase {
    private var dir: URL!
    private var now = Date(timeIntervalSince1970: 1_000_000)
    private var meetings: [String: SavedMeeting] = [:]
    private var follow: (([String], [LedgerEntry]) async throws -> [FollowResult])?

    override func setUp() {
        PanelFeatures.current = .allOn
        dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        now = Date(timeIntervalSince1970: 1_000_000)
        meetings = [:]
        follow = nil
    }

    override func tearDown() {
        PanelFeatures.current = .allOff
        try? FileManager.default.removeItem(at: dir) }

    private func ended(_ id: String) {
        var m = SavedMeeting(mode: "remote", start: now.addingTimeInterval(-7200))
        m.end = now.addingTimeInterval(-3600)
        meetings[id] = m
    }

    private func store() -> PanelStore {
        let engine = PanelEngine(
            prep: { ids, _ in
                PrepAnswer(advice: "", open: [PanelItem(meetingId: ids[0], index: 0, kind: "open_question", text: "수수료 정하기 \(ids[0])", owner: nil, due: nil, also: nil)],
                           todos: [PanelItem(meetingId: ids[0], index: 1, kind: "commitment", text: "문서 정리", owner: nil, due: nil, also: nil)],
                           sources: [], meetingsRead: 1, sourcesRead: 0, errorKind: nil)
            },
            why: { _, _, _ in WhyAnswer(reason: "", keys: [], errorKind: nil) },
            ask: { _, _, _ in PanelAsk(answer: nil, cited: [], cards: [], errorKind: nil) },
            followup: { [unowned self] p, l in try await follow!(p, l) })
        return PanelStore(engine: { engine }, meetings: { [unowned self] in meetings[$0] }, folder: { [unowned self] in dir },
                          account: { AccountSpace(id: "aaa") }, clock: { [unowned self] in now },
                          update: { [unowned self] id, change in if var m = meetings[id] { change(&m); meetings[id] = m } })
    }

    private func prep(_ s: PanelStore, _ id: String) async {
        s.open(scope: .meeting(id), via: .page)
        s.newThread()
        s.send("다음 회의 준비", kind: .prep)
        await s.settle()
    }

    func testMeetingStartedWithinTwoHoursOfPrepIsLinked() async {
        ended("a")
        let s = store()
        await prep(s, "a")
        let link = s.prepLink(startingAt: now.addingTimeInterval(3600))
        XCTAssertEqual(link?.from, "a")
        XCTAssertEqual(link?.items.map(\.text), ["수수료 정하기 a", "문서 정리"])
        XCTAssertEqual(link?.items.map(\.kind), ["open_question", "commitment"])
        XCTAssertNotNil(s.prepLink(startingAt: now.addingTimeInterval(7200)))
    }

    func testMeetingStartedAfterTwoHoursIsNot() async {
        ended("a")
        let s = store()
        await prep(s, "a")
        XCTAssertNil(s.prepLink(startingAt: now.addingTimeInterval(7201)))
        XCTAssertNil(s.prepLink(startingAt: now.addingTimeInterval(-1)))
    }

    func testLastPrepWinsWhenTwo() async {
        ended("a"); ended("b")
        let s = store()
        await prep(s, "a")
        now = now.addingTimeInterval(1800)
        await prep(s, "b")
        XCTAssertEqual(s.prepLink(startingAt: now.addingTimeInterval(600))?.from, "b")
    }

    func testPrepOfAMeetingStillLiveDoesNotCount() async {
        meetings["live"] = SavedMeeting(mode: "remote", start: now)
        let s = store()
        await prep(s, "live")
        XCTAssertNil(s.prepLink(startingAt: now.addingTimeInterval(60)))
    }

    func testCutLinkClearsBoth() {
        var m = SavedMeeting(mode: "remote", start: now)
        m.linkedFrom = "a"
        m.prepared = [PreparedItem(text: "x", kind: "open_question")]
        m.cutLink()
        XCTAssertNil(m.linkedFrom)
        XCTAssertNil(m.prepared)
    }

    func testFollowupFillsDecidedAfterNotes() async {
        var m = SavedMeeting(mode: "remote", start: now)
        m.notes = .done
        m.ledger = [LedgerEntry(kind: "decision", text: "B안으로 간다")]
        m.linkedFrom = "a"
        m.prepared = [PreparedItem(text: "수수료", kind: "open_question"), PreparedItem(text: "문서", kind: "commitment")]
        meetings["n"] = m
        follow = { p, l in
            XCTAssertEqual(p, ["수수료", "문서"])
            XCTAssertEqual(l.count, 1)
            return [FollowResult(i: 0, decided: true, how: "B안으로 간다"), FollowResult(i: 1, decided: false, how: nil)]
        }
        await store().followUp("n")
        XCTAssertEqual(meetings["n"]?.prepared?.map(\.decided), [true, false])
        XCTAssertEqual(meetings["n"]?.prepared?.first?.how, "B안으로 간다")
    }

    /// The prep's first view (per group, those raised more than once, then the prepared meeting's own) is kept first,
    /// so it is what the follow-up judges when there are more than it sends.
    func testPreparedItemsKeepTheFirstViewFirst() async {
        ended("a")
        func item(_ m: String, _ text: String, kind: String = "open_question", also: [String]? = nil) -> PanelItem {
            PanelItem(meetingId: m, index: 0, kind: kind, text: text, owner: nil, due: nil, also: also)
        }
        let engine = PanelEngine(
            prep: { _, _ in
                PrepAnswer(advice: "", open: [item("old", "old open"), item("a", "a open"), item("old", "repeated", also: ["a"])],
                           todos: [item("old", "old todo", kind: "commitment"), item("a", "a todo", kind: "commitment")],
                           sources: [], meetingsRead: 2, sourcesRead: 0, errorKind: nil)
            },
            why: { _, _, _ in WhyAnswer(reason: "", keys: [], errorKind: nil) },
            ask: { _, _, _ in PanelAsk(answer: nil, cited: [], cards: [], errorKind: nil) })
        let s = PanelStore(engine: { engine }, meetings: { [unowned self] in meetings[$0] }, folder: { [unowned self] in dir },
                           account: { AccountSpace(id: "aaa") }, clock: { [unowned self] in now })
        await prep(s, "a")
        let items = s.prepLink(startingAt: now.addingTimeInterval(60))?.items
        XCTAssertEqual(items?.map(\.text), ["repeated", "a open", "a todo", "old open", "old todo"])
        XCTAssertEqual(items?.map(\.kind), ["open_question", "open_question", "commitment", "open_question", "commitment"])
    }

    /// At most `followUpLimit` items go to the engine (a longer list cut its reply short); the rest get no mark.
    func testFollowupSendsAtMostTwentyAndLeavesTheRestUnmarked() async {
        var m = SavedMeeting(mode: "remote", start: now)
        m.notes = .done
        m.ledger = [LedgerEntry(kind: "decision", text: "x")]
        m.prepared = (0..<30).map { PreparedItem(text: "item \($0)", kind: "open_question") }
        meetings["n"] = m
        var sent: [String] = []
        follow = { p, _ in
            sent = p
            return p.indices.map { FollowResult(i: $0, decided: $0 == 3, how: $0 == 3 ? "정했어요" : nil) }
        }
        await store().followUp("n")
        XCTAssertEqual(PanelStore.followUpLimit, 20)
        XCTAssertEqual(sent, (0..<20).map { "item \($0)" })
        let items = meetings["n"]?.prepared ?? []
        XCTAssertEqual(items.count, 30)
        XCTAssertEqual(items.prefix(20).map(\.decided), (0..<20).map { $0 == 3 })
        XCTAssertEqual(items.dropFirst(20).map(\.decided), Array(repeating: nil, count: 10))
        XCTAssertEqual(items[3].how, "정했어요")
        XCTAssertTrue(PreparedItem.shows(items))
    }

    func testFailedFollowupLeavesItemsUnjudged() async {
        var m = SavedMeeting(mode: "remote", start: now)
        m.notes = .done
        m.ledger = [LedgerEntry(kind: "decision", text: "x")]
        m.prepared = [PreparedItem(text: "수수료", kind: "open_question")]
        meetings["n"] = m
        follow = { _, _ in throw EngineError.http(503, "") }
        await store().followUp("n")
        XCTAssertNil(meetings["n"]?.prepared?.first?.decided)
        XCTAssertFalse(PreparedItem.shows(meetings["n"]?.prepared))
    }

    func testOldFilesDecodeWithoutNewFields() throws {
        let m = SavedMeeting(mode: "remote", start: now)
        var json = try JSONSerialization.jsonObject(with: JSONEncoder().encode(m)) as! [String: Any]
        json.removeValue(forKey: "linkedFrom")
        json.removeValue(forKey: "prepared")
        let back = try JSONDecoder().decode(SavedMeeting.self, from: JSONSerialization.data(withJSONObject: json))
        XCTAssertNil(back.linkedFrom)
        XCTAssertNil(back.prepared)
    }

    func testLinkingIsOffByDefault() async {
        ended("a")
        let s = store()
        await prep(s, "a")
        PanelFeatures.current = .allOff
        XCTAssertNil(s.prepLink(startingAt: now.addingTimeInterval(3600)))
        var m = SavedMeeting(mode: "remote", start: now)
        m.notes = .done
        m.ledger = [LedgerEntry(kind: "decision", text: "B안으로 간다")]
        m.prepared = [PreparedItem(text: "수수료", kind: "open_question")]
        meetings["n"] = m
        var asked = false
        follow = { _, _ in asked = true; return [] }
        await s.followUp("n")
        XCTAssertFalse(asked)
        XCTAssertNil(meetings["n"]?.prepared?.first?.decided)
    }
}
