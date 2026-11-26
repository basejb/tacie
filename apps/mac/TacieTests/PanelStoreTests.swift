import XCTest

@MainActor
final class PanelStoreTests: XCTestCase {
    private var dir: URL!
    private var account = AccountSpace(id: "aaa")
    private var meetings: [String: SavedMeeting] = [:]
    private var asks: [(text: String, ids: [String]?, history: [(q: String, a: String)])] = []
    private var whyCalls: [(decision: String, window: [TranscriptLine])] = []
    private var gate: CheckedContinuation<Void, Never>?
    private var gated = false
    private var prepCalls = 0

    override func setUp() {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        account = AccountSpace(id: "aaa")
        meetings = [:]
        asks = []
        whyCalls = []
        gated = false
        prepCalls = 0
    }

    override func tearDown() {
        try? FileManager.default.removeItem(at: dir)
    }

    private func store() -> PanelStore {
        let engine = PanelEngine(
            prep: { [unowned self] ids, onStep in
                prepCalls += 1
                if gated { await withCheckedContinuation { gate = $0 } }
                onStep(.writing)
                return PrepAnswer(advice: "prep \(ids.joined(separator: ","))", open: [], todos: [], sources: [],
                                  meetingsRead: ids.count, sourcesRead: 0, errorKind: nil)
            },
            why: { [unowned self] decision, window, _ in
                whyCalls.append((decision, window))
                return WhyAnswer(reason: "because", keys: [], errorKind: nil)
            },
            ask: { [unowned self] text, ids, history in
                asks.append((text, ids, history))
                if gated { await withCheckedContinuation { gate = $0 } }
                return PanelAsk(answer: "answer to \(text)", cited: [], cards: [], errorKind: nil)
            })
        return PanelStore(engine: { engine }, meetings: { [unowned self] in meetings[$0] }, folder: { [unowned self] in dir },
                          account: { [unowned self] in account },
                          allMeetings: { [unowned self] in Array(meetings.values) })
    }

    /// A finished meeting (a meeting still going on starts the panel on every meeting, `pageScope`).
    private func meeting(_ id: String, title: String) -> SavedMeeting {
        var m = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 0))
        m.title = title
        m.end = Date(timeIntervalSince1970: 1800)
        return m
    }

    private func line(_ n: Int, _ text: String = "") -> TranscriptLine {
        TranscriptLine(key: "e0:s1u\(n)", at: Date(timeIntervalSince1970: Double(n)), speaker: nil,
                       text: text.isEmpty ? "line \(n)" : text, question: false)
    }

    func testMovingToAnotherMeetingKeepsTheConversationAndNewOnesStartThere() async {
        meetings["b"] = meeting("b", title: "주간 싱크")
        let s = store()
        s.open(scope: .meeting("a"), via: .page)
        s.ask("q1")
        await s.settle()
        let first = s.current!.id
        s.pageChanged(to: "b")
        XCTAssertEqual(s.scope, .meeting("b"))
        XCTAssertEqual(s.current?.id, first)
        s.ask("q2")
        await s.settle()
        XCTAssertEqual(s.current?.id, first)
        XCTAssertEqual(s.current?.origin, .meeting("a"))
        XCTAssertEqual(s.current?.turns.count, 2)
        XCTAssertEqual(asks[1].ids, ["a"])
        XCTAssertEqual(asks[1].history.map(\.q), ["q1"])
        s.newThread()
        s.ask("q3")
        await s.settle()
        XCTAssertNotEqual(s.current?.id, first)
        XCTAssertEqual(s.current?.origin, .meeting("b"))
        XCTAssertEqual(asks[2].ids, ["b"])
        XCTAssertEqual(asks[2].history.count, 0)
        XCTAssertEqual(s.history.count, 2)
    }

    func testTitleIsTheFirstMessageCutTo28() async {
        let s = store()
        s.open(scope: .all, via: .page)
        s.ask("  짧은 질문  ")
        await s.settle()
        XCTAssertEqual(s.current?.title, "짧은 질문")
        s.ask("두 번째 질문은 제목이 되지 않아요")
        await s.settle()
        XCTAssertEqual(s.current?.title, "짧은 질문")
        s.newThread()
        let long = String(repeating: "가", count: 40)
        s.ask(long)
        XCTAssertEqual(s.current?.title, String(repeating: "가", count: 28) + "…")
        XCTAssertEqual(PanelThread.title(from: String(repeating: "나", count: 28)), String(repeating: "나", count: 28))
    }

    func testOldThreadsWithScopeOnlyStillRead() throws {
        let id = UUID()
        let json = """
        [{"id":"\(id.uuidString)","scope":{"meeting":{"_0":"a"}},"turns":[{"id":"\(UUID().uuidString)","question":"옛 질문",
        "at":"2026-11-10T10:00:00Z","steps":[],"body":{"failed":{"kind":"none"}}}]}]
        """
        let file = dir.appendingPathComponent("panel/history.json")
        try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(json.utf8).write(to: file)
        let s = store()
        XCTAssertEqual(s.history.count, 1)
        XCTAssertEqual(s.history[0].origin, .meeting("a"))
        XCTAssertEqual(s.history[0].title, "옛 질문")
    }

    func testSuggestionsForAMeetingAndForAll() {
        var m = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 0))
        m.end = Date(timeIntervalSince1970: 1800)
        m.ledger = [LedgerEntry(kind: "commitment", text: "문서 정리", owner: nil, due: nil),
                    LedgerEntry(kind: "decision", text: "결제 실패 시 재시도는 세 번까지 하기로 해요", owner: nil, due: nil)]
        meetings[m.id] = m
        var bare = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 7200))
        bare.end = Date(timeIntervalSince1970: 9000)
        bare.ledger = [LedgerEntry(kind: "commitment", text: "문서 정리", owner: nil, due: nil)]
        meetings[bare.id] = bare
        let s = store()
        XCTAssertEqual(s.suggestions(for: .meeting(m.id)), [.ask("이 회의에서 아직 안 정한 거 뭐야?"), .prep, .why(meetingID: m.id, ledgerIndex: 1)])
        XCTAssertEqual(s.text(of: .why(meetingID: m.id, ledgerIndex: 1)), "'결제 실패 시 재시도는 세 번까지…' 왜 이렇게 정했어?")
        XCTAssertEqual(s.suggestions(for: .meeting(bare.id)), [.ask("이 회의에서 아직 안 정한 거 뭐야?"), .prep, .ask("이 회의에서 정한 거 정리해 줘")])
        XCTAssertEqual(s.suggestions(for: .all), [.ask("이번 달에 미뤄진 결정 뭐 있어?"), .ask("요즘 회의에서 자주 나온 이야기 뭐야?"), .ask("내가 맡은 할 일 뭐 남았어?")])
        XCTAssertEqual(s.text(of: .prep), "다음 회의 준비해 줘")
    }

    func testWhySuggestionRunsWhyInItsOwnConversation() async {
        var m = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 0))
        m.ledger = [LedgerEntry(kind: "decision", text: "배포는 금요일로 미뤄요", owner: nil, due: nil)]
        meetings[m.id] = m
        let s = store()
        s.open(scope: .meeting(m.id), via: .page)
        s.suggest(.why(meetingID: m.id, ledgerIndex: 0))
        await s.settle()
        XCTAssertEqual(whyCalls.map(\.decision), ["배포는 금요일로 미뤄요"])
        XCTAssertEqual(s.current?.title, "'배포는 금요일로 미뤄요' 정한 이유")
        XCTAssertEqual(s.current?.turns.first?.question, "'배포는 금요일로 미뤄요' 왜 이렇게 정했어?")
        XCTAssertTrue(asks.isEmpty)
    }

    func testAskKindsSeparateSuggestionsFromTyped() async {
        var m = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 0))
        m.end = Date(timeIntervalSince1970: 1800)
        m.ledger = [LedgerEntry(kind: "decision", text: "배포는 금요일로 미뤄요", owner: nil, due: nil)]
        meetings[m.id] = m
        var got: [AnalyticsEvent] = []
        Analytics.sink = { got.append($0) }
        defer { Analytics.sink = nil }
        let s = store()
        s.open(scope: .meeting(m.id), via: .page)
        s.suggest(.ask("예시 질문"))
        await s.settle()
        s.newThread(); s.ask("직접 쓴 질문")
        await s.settle()
        s.suggest(.prep)
        await s.settle()
        s.suggest(.why(meetingID: m.id, ledgerIndex: 0))
        await s.settle()
        let kinds = got.compactMap { e -> String? in
            if case let .panelAsked(kind, _, _, _) = e { return kind.rawValue } else { return nil }
        }
        XCTAssertEqual(kinds, ["suggestion_ask", "typed", "suggestion_prep", "suggestion_why"])
    }

    func testSuggestedQuestionGoesToAsk() async {
        let s = store()
        s.open(scope: .all, via: .page)
        s.suggest(.ask("이번 달에 미뤄진 결정 뭐 있어?"))
        await s.settle()
        XCTAssertEqual(asks.map(\.text), ["이번 달에 미뤄진 결정 뭐 있어?"])
    }

    func testPrepSuggestionIsReusedUntilAnotherMeetingEnds() async {
        var a = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 0))
        a.end = Date(timeIntervalSince1970: 1800)
        a.title = "결제 정책"
        meetings[a.id] = a
        let s = store()
        s.open(scope: .meeting(a.id), via: .page)
        s.suggest(.prep)
        await s.settle()
        let first = s.current!.id
        XCTAssertEqual(s.current?.title, "결제 정책 다음 회의 준비")
        XCTAssertEqual(s.current?.turns.first?.question, "다음 회의 준비해 줘")
        // another conversation is on screen; the prep comes back, the engine is not called
        s.newThread()
        s.ask("딴 질문")
        await s.settle()
        s.suggest(.prep)
        XCTAssertEqual(s.current?.id, first)
        await s.settle()
        XCTAssertEqual(prepCalls, 1)
        XCTAssertEqual(s.history.count, 2)
        // a meeting that goes on from it ends later: a new prep, in a new conversation
        var b = SavedMeeting(mode: "remote", start: Date().addingTimeInterval(60))
        b.end = Date().addingTimeInterval(1800)
        b.linkedFrom = a.id
        meetings[b.id] = b
        s.suggest(.prep)
        await s.settle()
        XCTAssertNotEqual(s.current?.id, first)
        XCTAssertEqual(prepCalls, 2)
        XCTAssertEqual(s.history.count, 3)
    }

    /// ⌘J and reopening the panel show the conversation on screen, wherever it started; "+ 새 대화" starts a new one.
    func testReopeningShowsTheCurrentConversation() async {
        let s = store()
        s.open(scope: .meeting("a"), via: .shortcut)
        s.ask("q1")
        await s.settle()
        let first = s.current!.id
        s.isOpen = false
        s.open(scope: .meeting("a"), via: .shortcut)
        XCTAssertEqual(s.current?.id, first)
        s.isOpen = false
        s.pageChanged(to: "b")
        s.open(scope: .meeting("b"), via: .shortcut)
        XCTAssertTrue(s.isOpen)
        XCTAssertEqual(s.current?.id, first)
        XCTAssertEqual(s.scope, .meeting("b"))
        s.newThread()
        XCTAssertNil(s.current)
        s.ask("q2")
        await s.settle()
        XCTAssertEqual(s.current?.origin, .meeting("b"))
    }

    /// A prep or why title keeps its end ("다음 회의 준비", "이유") and is as long as a typed one at most.
    func testPrepAndWhyTitlesAreCutLikeTypedOnes() async {
        var a = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 0))
        a.end = Date(timeIntervalSince1970: 1800)
        a.title = String(repeating: "가", count: 40)
        a.ledger = [LedgerEntry(kind: "decision", text: String(repeating: "나", count: 40), owner: nil, due: nil)]
        meetings[a.id] = a
        let s = store()
        s.open(scope: .meeting(a.id), via: .page)
        s.suggest(.prep)
        let prep = s.current?.title ?? ""
        XCTAssertEqual(prep.count, PanelThread.titleLimit + 1)
        XCTAssertTrue(prep.hasSuffix("… 다음 회의 준비"), prep)
        XCTAssertTrue(prep.hasPrefix("가가가"))
        s.suggest(.why(meetingID: a.id, ledgerIndex: 0))
        // a why title quotes the decision cut to 18 characters, no particle after the cut
        XCTAssertEqual(s.current?.title, "'" + String(repeating: "나", count: 18) + "…' 정한 이유")
        XCTAssertEqual(s.current?.turns.first?.question, "'" + String(repeating: "나", count: 18) + "…' 왜 이렇게 정했어?")
        // words around too long by themselves: the whole is cut like a typed title
        let wide = PanelThread.title("가", around: { "\($0) " + String(repeating: "다", count: 40) })
        XCTAssertEqual(wide.count, PanelThread.titleLimit + 1)
        await s.settle()
    }

    /// A second tap on the prep while the first is still on its way opens that conversation; nothing else is sent.
    func testSecondPrepTapWhileRunningOpensThePendingOne() async {
        var a = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 0))
        a.end = Date(timeIntervalSince1970: 1800)
        meetings[a.id] = a
        gated = true
        let s = store()
        s.open(scope: .meeting(a.id), via: .page)
        s.suggest(.prep)
        let first = s.current!.id
        while gate == nil { await Task.yield() }
        s.newThread()
        s.suggest(.prep)
        XCTAssertEqual(s.current?.id, first)
        XCTAssertEqual(s.history.count, 1)
        gate?.resume()
        gate = nil
        await s.settle()
        XCTAssertEqual(prepCalls, 1)
        // once answered it is reused as before
        s.newThread()
        s.suggest(.prep)
        XCTAssertEqual(s.current?.id, first)
        await s.settle()
        XCTAssertEqual(prepCalls, 1)
    }

    /// A meeting still going on is not searchable yet (the engine reads finished meetings): "Tacie AI" on its page
    /// starts on every meeting with every meeting's examples, none about this meeting. Once it ends, its page is as before.
    func testLiveMeetingPageStartsOnEveryMeeting() async {
        var live = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 0))
        live.ledger = [LedgerEntry(kind: "decision", text: "배포는 금요일", owner: nil, due: nil)]
        meetings[live.id] = live
        let s = store()
        let every: [PanelSuggestion] = [.ask("이번 달에 미뤄진 결정 뭐 있어?"), .ask("요즘 회의에서 자주 나온 이야기 뭐야?"), .ask("내가 맡은 할 일 뭐 남았어?")]
        PanelEntries.ask(meetingID: live.id, in: s)
        XCTAssertTrue(s.isOpen)
        XCTAssertEqual(s.scope, .all)
        XCTAssertEqual(s.suggestions(for: s.scope), every)
        XCTAssertEqual(s.suggestions(for: .meeting(live.id)), every)
        s.ask("q1")
        await s.settle()
        XCTAssertEqual(s.current?.origin, .all)
        XCTAssertNil(asks[0].ids)
        // moving onto the live page also starts new conversations on every meeting
        s.pageChanged(to: live.id)
        XCTAssertEqual(s.scope, .all)
        // the meeting ended: its page starts on it again, with its own examples
        live.end = Date(timeIntervalSince1970: 1800)
        meetings[live.id] = live
        PanelEntries.ask(meetingID: live.id, in: s)
        XCTAssertEqual(s.scope, .meeting(live.id))
        XCTAssertNil(s.current)
        XCTAssertEqual(s.suggestions(for: s.scope),
                       [.ask("이 회의에서 아직 안 정한 거 뭐야?"), .prep, .why(meetingID: live.id, ledgerIndex: 0)])
    }

    /// The pages' "Tacie에게 물어보기": a conversation that started on the same place stays, even with the panel shut;
    /// one that started elsewhere gives way to a new one there.
    func testEnteringKeepsAConversationOfTheSamePlace() async {
        let s = store()
        s.open(scope: .meeting("a"), via: .page, startHere: true)
        XCTAssertTrue(s.isOpen)
        s.ask("q1")
        await s.settle()
        let first = s.current!.id
        s.isOpen = false
        s.open(scope: .meeting("a"), via: .page, startHere: true)
        XCTAssertEqual(s.current?.id, first)
        s.open(scope: .meeting("b"), via: .page, startHere: true)
        XCTAssertNil(s.current)
        XCTAssertEqual(s.scope, .meeting("b"))
        s.reopen(first)
        s.open(scope: .all, via: .page, startHere: true)
        XCTAssertNil(s.current)
        XCTAssertEqual(s.scope, .all)
    }

    /// A source in the panel moved the page: the conversation stays with its own meeting, but "+ 새 대화" starts on
    /// the meeting now on screen (docs/36 0-1절).
    func testFollowingAPanelLinkKeepsThreadAndNewOnesStartOnThePage() async {
        let s = store()
        s.open(scope: .meeting("a"), via: .page)
        s.send("q1", kind: .ask)
        await s.settle()
        let id = s.current!.id
        s.pageChanged(to: "b")
        XCTAssertEqual(s.scope, .meeting("b"))
        s.send("q2", kind: .ask)
        await s.settle()
        XCTAssertEqual(s.current?.id, id)
        XCTAssertEqual(s.current?.origin, .meeting("a"))
        XCTAssertEqual(s.current?.turns.count, 2)
        XCTAssertEqual(asks[1].ids, ["a"])
        XCTAssertEqual(asks[1].history.map(\.q), ["q1"])
        XCTAssertEqual(asks[1].history.map(\.a), ["answer to q1"])
        s.newThread()
        s.send("q3", kind: .ask)
        await s.settle()
        XCTAssertEqual(s.current?.origin, .meeting("b"))
        XCTAssertEqual(asks[2].ids, ["b"])
    }

    /// docs/36 2-2: a meeting's scope is that meeting and the meetings it goes on from.
    func testMeetingScopeAskSendsTheLinkedChain() async {
        var b = meeting("b", title: "2차")
        b.linkedFrom = "a"
        meetings["b"] = b
        meetings["a"] = meeting("a", title: "1차")
        let s = store()
        s.open(scope: .meeting("b"), via: .page)
        s.send("q", kind: .ask)
        await s.settle()
        XCTAssertEqual(asks[0].ids, ["b", "a"])
    }

    func testAllScopeSendsNilMeetingIDs() async {
        let s = store()
        s.open(scope: .all, via: .page)
        s.send("q", kind: .ask)
        await s.settle()
        XCTAssertNil(asks[0].ids)
    }

    func testResultArrivesAfterPanelClosedIsKept() async {
        let s = store()
        gated = true
        s.open(scope: .meeting("a"), via: .page)
        s.send("slow", kind: .ask)
        let id = s.current!.id
        s.isOpen = false
        s.open(scope: .meeting("b"), via: .shortcut)  // reopened: the same conversation
        XCTAssertEqual(s.current?.id, id)
        s.newThread()
        XCTAssertNil(s.current)
        while gate == nil { await Task.yield() }
        gate?.resume()
        await s.settle()
        let saved = s.history.first { $0.id == id }
        XCTAssertEqual(saved?.turns.first?.body, .ask(PanelAsk(answer: "answer to slow", cited: [], cards: [], errorKind: nil)))
        XCTAssertNil(s.current)
        let onDisk = PanelStore(engine: { nil }, meetings: { _ in nil }, folder: { [unowned self] in dir }, account: { [unowned self] in account })
        XCTAssertEqual(onDisk.history.first { $0.id == id }?.turns.count, 1)
    }

    /// An ask or why that failed does not go along with later questions as an empty answer.
    func testFailedTurnsStayOutOfHistory() {
        func turn(_ body: PanelTurnBody) -> PanelTurn { PanelTurn(id: UUID(), question: "q", at: Date(), steps: [], body: body) }
        XCTAssertNil(turn(.ask(PanelAsk(answer: nil, cited: [], cards: [], errorKind: "network"))).answerText)
        XCTAssertNil(turn(.why(WhyAnswer(reason: "", keys: [], errorKind: "credit"), meetingID: "a")).answerText)
        XCTAssertEqual(turn(.ask(PanelAsk(answer: nil, cited: [], cards: [], errorKind: nil))).answerText, "")
        XCTAssertEqual(turn(.why(WhyAnswer(reason: "비용", keys: [], errorKind: nil), meetingID: "a")).answerText, "비용")
    }

    func testAccountSwitchStartsOnEveryMeeting() {
        let s = store()
        s.open(scope: .meeting("a"), via: .page)
        account = AccountSpace(id: "bbb")
        s.accountChanged()
        XCTAssertEqual(s.scope, .all)
        XCTAssertNil(s.current)
    }

    func testAccountSwitchDropsInFlightResult() async {
        let s = store()
        gated = true
        s.open(scope: .all, via: .page)
        s.send("slow", kind: .ask)
        while gate == nil { await Task.yield() }
        account = AccountSpace(id: "bbb")
        gate?.resume()
        await s.settle()
        XCTAssertNil(s.current?.turns.first?.body)
        XCTAssertNil(s.history.first?.turns.first?.body)
    }

    func testHistoryKeepsAtMost200() async {
        let s = store()
        for i in 0..<205 {
            s.newThread()
            s.send("q\(i)", kind: .ask)
        }
        await s.settle()
        XCTAssertEqual(s.history.count, 200)
        XCTAssertEqual(s.history.first?.turns.first?.question, "q204")
        XCTAssertEqual(s.history.last?.turns.first?.question, "q5")
        let again = PanelStore(engine: { nil }, meetings: { _ in nil }, folder: { [unowned self] in dir }, account: { [unowned self] in account })
        XCTAssertEqual(again.history.count, 200)
    }

    func testPrepSendsMeetingIDAndRecordsSteps() async {
        let s = store()
        s.open(scope: .meeting("a"), via: .page)
        s.send("다음 회의 준비", kind: .prep)
        await s.settle()
        await Task.yield()
        XCTAssertEqual(s.prepMeetingIDs(for: "a"), ["a"])
        guard case .prep(let a)? = s.current?.turns.first?.body else { return XCTFail("no prep answer") }
        XCTAssertEqual(a.advice, "prep a")
    }

    func testWhyWindowUsesRefsThenWordOverlapThenEmpty() async {
        var m = meeting("a", title: "t")
        m.transcript = (0..<30).map { line($0) }
        m.transcript[20] = line(20, "배포는 금요일로 미뤄요")
        m.ledger = [
            LedgerEntry(kind: "decision", text: "x", refs: ["e0:s1u10"]),
            LedgerEntry(kind: "decision", text: "배포는 금요일로 미루기", refs: nil),
            LedgerEntry(kind: "decision", text: "전혀 다른 이야기", refs: []),
        ]
        meetings["a"] = m
        let s = store()
        s.open(scope: .meeting("a"), via: .page)
        for i in 0..<3 { s.send("q\(i)", kind: .why(meetingID: "a", ledgerIndex: i)) }
        await s.settle()
        XCTAssertEqual(whyCalls.count, 3)
        let byDecision = Dictionary(uniqueKeysWithValues: whyCalls.map { ($0.decision, $0.window.map(\.key)) })
        XCTAssertEqual(byDecision["x"], (4...16).map { "e0:s1u\($0)" })
        XCTAssertEqual(byDecision["배포는 금요일로 미루기"], (14...26).map { "e0:s1u\($0)" })
        XCTAssertEqual(byDecision["전혀 다른 이야기"], [])
    }

    func testPrepAndWhyTurnsRoundTripThroughHistory() async throws {
        let card = Card(id: "c1", path: "linear/PAY-142", kind: "linear", source: "linear", byline: "Linear · 진행 중",
                        url: "https://linear.app/x", lineStart: 0, lineEnd: 0, title: "PAY-142 부분 환불 API", snippet: "s",
                        score: 0.9, githubUrl: nil, localPath: nil, chunk: 3, repeat: nil)
        let prep = PrepAnswer(advice: "수수료부터 정해요", open: [PanelItem(meetingId: "a", index: 2, kind: "open_question",
                              text: "누가 낼지", owner: nil, due: nil, also: ["b"])],
                              todos: [PanelItem(meetingId: "a", index: 1, kind: "commitment", text: "문서 정리", owner: "화자 1",
                                                due: "금요일까지", also: nil)],
                              sources: [card], meetingsRead: 2, sourcesRead: 1, errorKind: nil)
        let why = WhyAnswer(reason: "한도 때문이에요", keys: ["e0:s1u3"], errorKind: nil)
        let at = Date(timeIntervalSince1970: 1_800_000_000)
        let thread = PanelThread(id: UUID(), title: "다음 회의 준비", origin: .meeting("a"), turns: [
            PanelTurn(id: UUID(), question: "다음 회의 준비", at: at, steps: [.meetings(["a"]), .related(["b"]), .sources(1), .writing],
                      body: .prep(prep)),
            PanelTurn(id: UUID(), question: "왜", at: at, steps: [], body: .why(why, meetingID: "a")),
            PanelTurn(id: UUID(), question: "실패", at: at, steps: [], body: .failed(kind: "model")),
        ])
        let e = JSONEncoder()
        e.dateEncodingStrategy = .iso8601
        let file = dir.appendingPathComponent("panel/history.json")
        try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try e.encode([thread]).write(to: file)
        let s = store()
        XCTAssertEqual(s.history, [thread])
    }

    func testOneUnreadableThreadDoesNotWipeTheHistory() async throws {
        let s = store()
        s.send("q1", kind: .ask)
        await s.settle()
        s.newThread()
        s.send("q2", kind: .ask)
        await s.settle()
        let file = dir.appendingPathComponent("panel/history.json")
        var list = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [Any])
        XCTAssertEqual(list.count, 2)
        list.insert(["id": "not a uuid", "scope": 7], at: 1)
        try JSONSerialization.data(withJSONObject: list).write(to: file)
        let again = store()
        XCTAssertEqual(again.history.map { $0.turns.first?.question }, ["q2", "q1"])
        again.send("q3", kind: .ask)
        await again.settle()
        let third = store()
        XCTAssertEqual(third.history.count, 3)
    }

    func testUnreadableFileIsSetAsideBeforeTheNextSave() async throws {
        let file = dir.appendingPathComponent("panel/history.json")
        try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("{broken".utf8).write(to: file)
        let s = store()
        XCTAssertEqual(s.history, [])
        XCTAssertTrue(FileManager.default.fileExists(atPath: dir.appendingPathComponent("panel/history.broken.json").path))
    }

    func testNoEngineSaysTheEngineIsNotThere() async {
        let s = PanelStore(engine: { nil }, meetings: { _ in nil }, folder: { [unowned self] in dir }, account: { [unowned self] in account })
        s.open(scope: .all, via: .page)
        s.send("q", kind: .ask)
        await s.settle()
        XCTAssertEqual(s.current?.turns.first?.body, .failed(kind: "engine"))
    }

    // MARK: usage numbers (docs/16)

    func testUsageEventsFollowAThread() async {
        var got: [AnalyticsEvent] = []
        Analytics.sink = { got.append($0) }
        defer { Analytics.sink = nil }
        let s = store()
        s.open(scope: .meeting("a"), via: .page)
        s.send("q1", kind: .ask)
        await s.settle()
        s.send("q2", kind: .ask)
        await s.settle()
        // the fake answers a sentence with no sources, which the screen shows as not found
        XCTAssertEqual(got, [
            .panelOpened(via: .page, scope: .meeting),
            .panelAsked(kind: .typed, scope: .meeting, turn: 1, place: .mac),
            .panelAnswered(ms: 0, sources: 0, notFound: true, errorKind: nil, place: .mac),
            .panelAsked(kind: .followUp, scope: .meeting, turn: 2, place: .mac),
            .panelAnswered(ms: 0, sources: 0, notFound: true, errorKind: nil, place: .mac),
        ])
    }

    func testUsageOfAFailedAnswerKeepsOnlyTheKind() async {
        var got: [AnalyticsEvent] = []
        Analytics.sink = { got.append($0) }
        defer { Analytics.sink = nil }
        let s = PanelStore(engine: { nil }, meetings: { _ in nil }, folder: { [unowned self] in dir }, account: { [unowned self] in account })
        s.open(scope: .all, via: .search)
        s.send("secret question", kind: .ask)
        await s.settle()
        XCTAssertEqual(got.last, .panelAnswered(ms: 0, sources: 0, notFound: false, errorKind: .engine, place: .mac))
        XCTAssertEqual(PanelTurnBody.ask(PanelAsk(answer: nil, cited: [], cards: [], errorKind: nil)).usage.notFound, true)
        XCTAssertEqual(PanelTurnBody.failed(kind: "none").usage.error, nil)
    }

    /// not_found counts what the screen shows as "이야기가 나오지 않았어요" (PanelShown.found), not only a null answer.
    func testUsageNotFoundMatchesTheScreen() {
        let card = Card(id: "c1", path: "meeting/m1", kind: "meeting", source: "meeting", byline: nil, url: nil, lineStart: 0,
                        lineEnd: 0, title: "t", snippet: "s", score: nil, githubUrl: nil, localPath: nil, chunk: nil, repeat: nil)
        func ask(_ answer: String?, cards: [Card], kind: String? = nil) -> Bool {
            PanelTurnBody.ask(PanelAsk(answer: answer, cited: [], cards: cards, errorKind: kind)).usage.notFound
        }
        XCTAssertTrue(ask(PanelText.noAnswer, cards: []))
        XCTAssertTrue(ask(PanelText.noAnswer, cards: [card]))
        XCTAssertTrue(ask("답이에요[1]", cards: []))
        XCTAssertFalse(ask("답이에요[1]", cards: [card]))
        XCTAssertFalse(ask(nil, cards: [], kind: "credit"))
        func why(_ reason: String, keys: [String], kind: String? = nil) -> Bool {
            PanelTurnBody.why(WhyAnswer(reason: reason, keys: keys, errorKind: kind), meetingID: "m1").usage.notFound
        }
        XCTAssertTrue(why(PanelText.noAnswer, keys: []))
        XCTAssertTrue(why("", keys: ["e0:s1u1"]))
        XCTAssertFalse(why("비용 때문이에요[1]", keys: ["e0:s1u1"]))
        XCTAssertFalse(why("", keys: [], kind: "network"))
    }
}
