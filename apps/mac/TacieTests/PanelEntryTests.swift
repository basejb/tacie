import XCTest

@MainActor
final class PanelEntryTests: XCTestCase {
    private var dir: URL!
    private var meetings: [String: SavedMeeting] = [:]
    private var prepCalls: [[String]] = []
    private var asks: [(text: String, ids: [String]?)] = []
    private var whys: [String] = []

    override func setUp() {
        PanelFeatures.current = .allOn
        dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        meetings = [:]
        prepCalls = []
        asks = []
        whys = []
    }

    override func tearDown() {
        PanelFeatures.current = .allOff
        try? FileManager.default.removeItem(at: dir)
    }

    private func store() -> PanelStore {
        let engine = PanelEngine(
            prep: { [unowned self] ids, _ in
                prepCalls.append(ids)
                return PrepAnswer(advice: "", open: [], todos: [], sources: [], meetingsRead: ids.count, sourcesRead: 0, errorKind: nil)
            },
            why: { [unowned self] decision, _, _ in
                whys.append(decision)
                return WhyAnswer(reason: "because", keys: [], errorKind: nil)
            },
            ask: { [unowned self] text, ids, _ in
                asks.append((text, ids))
                return PanelAsk(answer: "a", cited: [], cards: [], errorKind: nil)
            })
        return PanelStore(engine: { engine }, meetings: { [unowned self] in meetings[$0] }, folder: { [unowned self] in dir },
                          account: { AccountSpace(id: "aaa") })
    }

    @discardableResult
    private func add(_ n: Double, ended: Bool = true, linkedFrom: String? = nil, ledger: [LedgerEntry] = []) -> SavedMeeting {
        var m = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 1_700_000_000 + n * 3600))
        if ended { m.end = m.start.addingTimeInterval(1800) }
        m.linkedFrom = linkedFrom
        m.ledger = ledger
        meetings[m.id] = m
        return m
    }

    func testPrepButtonOnlyOnEndedMeetings() async {
        let done = add(1), going = add(2, ended: false)
        XCTAssertTrue(PanelEntries.showsPrep(done))
        XCTAssertFalse(PanelEntries.showsPrep(going))
        let s = store()
        PanelEntries.prep(done, in: s)
        XCTAssertTrue(s.isOpen)
        XCTAssertEqual(s.scope, .meeting(done.id))
        // through the example's path: its question, a title with the meeting's name, reused on the next press
        XCTAssertEqual(s.current?.turns.first?.question, PanelStore.prepQuestion)
        XCTAssertTrue(s.current?.title.hasSuffix("다음 회의 준비") ?? false)
        await s.settle()
        XCTAssertEqual(prepCalls, [[done.id]])
        let first = s.current?.id
        s.newThread()
        PanelEntries.prep(done, in: s)
        XCTAssertEqual(s.current?.id, first)
        await s.settle()
        XCTAssertEqual(prepCalls, [[done.id]])
        // a meeting still going on opens nothing
        let t = store()
        PanelEntries.prep(going, in: t)
        XCTAssertFalse(t.isOpen)
        XCTAssertNil(t.current)
    }

    func testWhyOnlyOnDecisionRows() async {
        let decision = LedgerEntry(kind: "decision", text: "재시도는 세 번까지")
        let todo = LedgerEntry(kind: "commitment", text: "수수료 확인")
        let open = LedgerEntry(kind: "open_question", text: "누가 낼지")
        XCTAssertTrue(PanelEntries.showsWhy(decision))
        XCTAssertFalse(PanelEntries.showsWhy(todo))
        XCTAssertFalse(PanelEntries.showsWhy(open))
        let m = add(1, ledger: [todo, decision])
        let s = store()
        PanelEntries.why(m, index: 1, in: s)
        XCTAssertEqual(s.scope, .meeting(m.id))
        XCTAssertEqual(s.current?.turns.first?.question, PanelStore.whyQuestion("재시도는 세 번까지"))
        XCTAssertEqual(s.current?.title, "'재시도는 세 번까지' 정한 이유")
        await s.settle()
        XCTAssertEqual(whys, ["재시도는 세 번까지"])
        // a row that is not a decision sends nothing
        let t = store()
        PanelEntries.why(m, index: 0, in: t)
        XCTAssertNil(t.current)
    }

    func testSearchRowSendsQueryWithAllScope() async {
        let s = store()
        s.open(scope: .meeting("x"), via: .page)
        PanelEntries.searchAsk("가격 정책 어디서 정했지", in: s)
        XCTAssertEqual(s.scope, .all)
        XCTAssertEqual(s.current?.turns.first?.question, "가격 정책 어디서 정했지")
        await s.settle()
        XCTAssertEqual(asks.count, 1)
        XCTAssertNil(asks[0].ids)
        // an empty or blank query sends nothing
        let t = store()
        PanelEntries.searchAsk("   ", in: t)
        XCTAssertNil(t.current)
        XCTAssertFalse(t.isOpen)
    }

    /// Return in the sidebar search did nothing before the panel, so the first Return asks Tacie (no arming step).
    func testSearchReturnAsksOnTheFirstPress() async {
        XCTAssertEqual(PanelEntries.searchReturn("  가격 정책  "), "가격 정책")
        XCTAssertNil(PanelEntries.searchReturn(" \n "))
        let s = store()
        if let q = PanelEntries.searchReturn("가격 정책") { PanelEntries.searchAsk(q, in: s) }
        XCTAssertTrue(s.isOpen)
        XCTAssertEqual(s.current?.turns.first?.question, "가격 정책")
        await s.settle()
        XCTAssertEqual(asks.count, 1)
    }

    func testPrepScopeFollowsLinkedMeetingsUpToFive() {
        let a = add(1)
        let b = add(2, linkedFrom: a.id)
        let c = add(3, linkedFrom: b.id)
        let s = store()
        XCTAssertEqual(s.prepMeetingIDs(for: a.id), [a.id])
        XCTAssertEqual(s.prepMeetingIDs(for: c.id), [c.id, b.id, a.id])
        // a chain of seven stops at five
        var prev = a.id
        var chain = [a.id]
        for n in 10..<16 { let m = add(Double(n), linkedFrom: prev); prev = m.id; chain.append(m.id) }
        XCTAssertEqual(s.prepMeetingIDs(for: prev), Array(chain.reversed().prefix(5)))
        // a cycle stops where it closes
        var x = add(30), y = add(31, linkedFrom: x.id)
        x.linkedFrom = y.id
        meetings[x.id] = x
        XCTAssertEqual(s.prepMeetingIDs(for: y.id), [y.id, x.id])
        // a link to a meeting that is gone ends the chain
        let z = add(40, linkedFrom: "gone")
        XCTAssertEqual(s.prepMeetingIDs(for: z.id), [z.id])
        _ = y
    }

    func testOldMeetingFileWithoutLinkedFromDecodes() throws {
        let json = """
        {"version":1,"id":"20261101-100000","mode":"remote","start":"2026-11-01T01:00:00Z","titleEdited":false,
         "notes":"done","summary":[],"ledger":[],"questions":[],"transcript":[]}
        """
        let d = JSONDecoder()
        d.dateDecodingStrategy = .iso8601
        let m = try d.decode(SavedMeeting.self, from: Data(json.utf8))
        XCTAssertNil(m.linkedFrom)
        let e = JSONEncoder()
        e.dateEncodingStrategy = .iso8601
        var linked = m
        linked.linkedFrom = "20261031-100000"
        let back = try d.decode(SavedMeeting.self, from: try e.encode(linked))
        XCTAssertEqual(back.linkedFrom, "20261031-100000")
    }
    /// The meeting page's "Tacie에게 물어보기 ⌘J" opens on that meeting; the collected pages' on every meeting.
    func testAskEntryOpensOnItsPlace() async {
        let m = add(1)
        let s = store()
        PanelEntries.ask(meetingID: m.id, in: s)
        XCTAssertTrue(s.isOpen)
        XCTAssertEqual(s.scope, .meeting(m.id))
        XCTAssertNil(s.current)
        let t = store()
        PanelEntries.ask(meetingID: nil, in: t)
        XCTAssertEqual(t.scope, .all)
    }

    // MARK: held back (docs/36 0-1절): the app leaves these off; setUp turns them on for the tests above

    func testHeldBackEntriesAreAbsentByDefault() async {
        PanelFeatures.current = .allOff
        var done = SavedMeeting(mode: "remote", start: Date()); done.end = Date()
        XCTAssertFalse(PanelEntries.showsPrep(done))
        XCTAssertFalse(PanelEntries.showsWhy(LedgerEntry(kind: "decision", text: "재시도는 세 번까지")))
        XCTAssertFalse(PanelEntries.showsSearchAsk)
        XCTAssertNil(PanelEntries.searchReturn("가격 정책"))
        let s = store()
        PanelEntries.prep(done, in: s)
        XCTAssertFalse(s.isOpen)
        XCTAssertEqual(PanelFeatures.current, .allOff)
    }
}
