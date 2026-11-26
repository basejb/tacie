import XCTest

/// The panel on POST /panel/chat (docs/38): what each question sends, and how its answers are kept.
@MainActor
final class PanelChatTests: XCTestCase {
    private var dir: URL!
    private var meetings: [String: SavedMeeting] = [:]
    private var sent: [PanelChatRequest] = []
    private var reply: (PanelChatRequest) -> ChatAnswer = { ChatAnswer(answer: "answer to \($0.text) [1]", cards: []) }
    private var oldCalls = 0

    override func setUp() {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        meetings = [:]
        sent = []
        oldCalls = 0
        reply = { ChatAnswer(answer: "answer to \($0.text) [1]", cards: []) }
    }

    override func tearDown() {
        try? FileManager.default.removeItem(at: dir)
    }

    private func store(now: Date = Date(timeIntervalSince1970: 1_745_100_000)) -> PanelStore {
        let engine = PanelEngine(
            prep: { [unowned self] _, _ in oldCalls += 1; throw EngineError.notRunning },
            why: { [unowned self] _, _, _ in oldCalls += 1; throw EngineError.notRunning },
            ask: { [unowned self] _, _, _ in oldCalls += 1; throw EngineError.notRunning },
            chat: { [unowned self] r, onStep in
                sent.append(r)
                onStep(.tool("search", detail: ["query": "환불"], count: nil))
                onStep(.tool("search", detail: [:], count: 2))
                return reply(r)
            })
        return PanelStore(engine: { engine }, meetings: { [unowned self] in meetings[$0] }, folder: { [unowned self] in dir },
                          clock: { now }, allMeetings: { [unowned self] in Array(meetings.values) })
    }

    private func finished(_ title: String, at start: TimeInterval = 0) -> SavedMeeting {
        var m = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: start))
        m.title = title
        m.end = Date(timeIntervalSince1970: start + 1800)
        return m
    }

    private func line(_ n: Int, _ text: String) -> TranscriptLine {
        TranscriptLine(key: "e0:s1u\(n)", at: Date(timeIntervalSince1970: Double(n)), speaker: nil, text: text, question: false)
    }

    func testEveryMeetingSendsNoTranscripts() async {
        let s = store()
        s.open(scope: .all, via: .page)
        s.ask("이번 달 정한 거")
        await s.settle()
        XCTAssertEqual(sent.count, 1)
        XCTAssertEqual(sent[0].text, "이번 달 정한 거")
        XCTAssertEqual(sent[0].meetingIDs, [])
        XCTAssertTrue(sent[0].transcripts.isEmpty)
        XCTAssertEqual(sent[0].today.count, 10)
        XCTAssertEqual(oldCalls, 0)
        XCTAssertEqual(s.current?.turns.first?.steps.count, 2)
    }

    /// On a meeting page: the meeting and the one it goes on from, each with its transcript lines and ledger refs.
    func testMeetingPageSendsTheChainWithTranscriptsAndRefs() async {
        var a = finished("결제 정책")
        a.transcript = [line(1, "환불 수수료는 누가 내죠"), line(2, "우리가 내요")]
        var d = LedgerEntry(kind: "decision", text: "수수료는 회사가 내요", owner: nil, due: nil)
        d.refs = ["e0:s1u2"]
        a.ledger = [d, LedgerEntry(kind: "commitment", text: "문서 정리", owner: nil, due: nil)]
        var b = finished("결제 정책 2", at: 86_400)
        b.linkedFrom = a.id
        b.transcript = [line(1, "지난번에 이어서")]
        meetings[a.id] = a
        meetings[b.id] = b
        let s = store()
        s.open(scope: .meeting(b.id), via: .page)
        s.ask("왜 회사가 내기로 했어?")
        await s.settle()
        let r = sent[0]
        XCTAssertEqual(r.meetingIDs, [b.id, a.id])
        XCTAssertEqual(r.transcripts[a.id]?.map { $0["key"] }, ["e0:s1u1", "e0:s1u2"])
        XCTAssertEqual(r.transcripts[a.id]?[1]["text"], "우리가 내요")
        XCTAssertEqual(r.refs[a.id], [["e0:s1u2"], []])
        XCTAssertEqual(r.refs[b.id], [])
    }

    /// "이유 보기" shows the decision cut in its bubble but asks with all of it.
    func testWhyAsksWithTheWholeDecision() async {
        var a = finished("결제 정책")
        let long = "결제 실패 시 재시도는 세 번까지 하기로 해요. 다섯 번이면 결제사 차단에 걸려요."
        a.ledger = [LedgerEntry(kind: "decision", text: long, owner: nil, due: nil)]
        meetings[a.id] = a
        let s = store()
        s.open(scope: .meeting(a.id), via: .why)
        s.suggest(.why(meetingID: a.id, ledgerIndex: 0))
        await s.settle()
        XCTAssertTrue(sent[0].text.contains(long))
        XCTAssertTrue(s.current!.turns[0].question.contains("…"))
    }

    /// A chat answer that prepared the meeting counts as its prep: the example opens it again instead of asking.
    func testChatPrepIsReused() async {
        let a = finished("결제 정책")
        meetings[a.id] = a
        reply = { _ in
            ChatAnswer(answer: "수수료부터 정해요", cards: [],
                       prep: PrepAnswer(advice: "a", open: [], todos: [], sources: [], meetingsRead: 1, sourcesRead: 0, errorKind: nil))
        }
        let s = store()
        s.open(scope: .meeting(a.id), via: .page)
        s.suggest(.prep)
        await s.settle()
        let first = s.current!.id
        s.newThread()
        s.suggest(.prep)
        await s.settle()
        XCTAssertEqual(s.current?.id, first)
        XCTAssertEqual(sent.count, 1)
    }

    func testChatTurnsRoundTripThroughHistory() async throws {
        let s = store()
        reply = { _ in
            ChatAnswer(answer: "정했어요 [1]", cards: [], items: [.init(meetingId: "a", index: 2)],
                       lines: [.init(meetingId: "a", key: "e0:s1u3")])
        }
        s.ask("q")
        await s.settle()
        let again = store()
        XCTAssertEqual(again.history.first?.turns.first?.body, s.history.first?.turns.first?.body)
        guard case .chat(let a)? = again.history.first?.turns.first?.body else { return XCTFail("no chat body") }
        XCTAssertEqual(a.lines, [.init(meetingId: "a", key: "e0:s1u3")])
        XCTAssertEqual(a.items, [.init(meetingId: "a", index: 2)])
    }

    /// The engine's done line: snake_case keys, a prep that failed half way left out, a failure with no answer.
    func testDoneLineDecodes() throws {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        let ok = try d.decode(ChatAnswer.self, from: Data("""
        {"answer":"a [1]","cards":[],"items":[{"meeting_id":"m","index":1}],"keys":["k"],
         "lines":[{"meeting_id":"m","key":"k"}],"rounds":1,"tools":["search"],"cost":0.001,
         "prep":{"error":"x","error_kind":"down","open":[],"todos":[]}}
        """.utf8))
        XCTAssertEqual(ok.items, [.init(meetingId: "m", index: 1)])
        XCTAssertNil(ok.prep)
        let failed = try d.decode(ChatAnswer.self, from: Data("""
        {"answer":null,"cards":[],"items":[],"keys":[],"lines":[],"rounds":0,"tools":[],"cost":0,"error":"e","error_kind":"network"}
        """.utf8))
        XCTAssertNil(failed.answer)
        XCTAssertEqual(failed.errorKind, "network")
    }
}

/// The chat's step line and answer lines (docs/38 7절).
@MainActor
final class PanelChatViewTests: XCTestCase {
    private func tool(_ name: String, _ d: [String: String] = [:], _ count: Int? = nil) -> PanelStep { .tool(name, detail: d, count: count) }

    func testStepLineFollowsTheRunningTool() {
        XCTAssertEqual(PanelPhase.toolText([tool("start")]), PanelText.stepFinding)
        XCTAssertEqual(PanelPhase.toolText([tool("start"), tool("search", ["query": "환불 수수료"])]), "'환불 수수료' 찾는 중…")
        XCTAssertEqual(PanelPhase.toolText([tool("filter_ledger", ["owner": "민호", "kinds": "commitment"])]), "민호 할 일 모으는 중…")
        XCTAssertEqual(PanelPhase.toolText([tool("filter_ledger")]), "결정·할 일·남은 논의 모으는 중…")
        XCTAssertEqual(PanelPhase.toolText([tool("transcript_window", ["meeting_id": "m", "find": "민호 환불"])]),
                       "대화 기록에서 '민호 환불' 찾는 중…")
        // the second of two at once is done: the first still runs
        XCTAssertEqual(PanelPhase.toolText([tool("search", ["query": "a"]), tool("prepare_meeting", ["meeting_id": "m"]),
                                            tool("prepare_meeting", [:], 5)]), "'a' 찾는 중…")
        XCTAssertEqual(PanelPhase.toolText([tool("search", ["query": "a"]), tool("search", [:], 3)]), PanelText.stepWriting)
    }

    /// The iPhone's stacked step lines: each tool's line once, "정리하는 중…" only as the last line.
    func testStackedStepLines() {
        XCTAssertEqual(PanelSteps.lines([]), [PanelText.stepFinding])
        XCTAssertEqual(PanelSteps.lines([tool("start")]), [PanelText.stepFinding])
        let steps = [tool("start"), tool("search", ["query": "이번 달 결정"]), tool("search", [:], 6),
                     tool("filter_ledger", ["kinds": "decision"])]
        XCTAssertEqual(PanelSteps.lines(steps), [PanelText.stepFinding, "'이번 달 결정' 찾는 중…", "결정 모으는 중…"])
        XCTAssertEqual(PanelSteps.lines(steps + [tool("filter_ledger", [:], 2)]),
                       [PanelText.stepFinding, "'이번 달 결정' 찾는 중…", "결정 모으는 중…", PanelText.stepWriting])
        // the same tool again right after itself adds no line
        XCTAssertEqual(PanelSteps.lines([tool("list_meetings"), tool("list_meetings", [:], 3), tool("list_meetings")]),
                       [PanelText.stepFinding, "회의 목록 보는 중…"])
    }

    func testToolStepShowsTheLineBeforeThreeSeconds() {
        XCTAssertEqual(PanelPhase.at(elapsed: 0.5, steps: [tool("search", ["query": "a"])], done: false), .quiet)
        XCTAssertEqual(PanelPhase.at(elapsed: 1.5, steps: [tool("start")], done: false), .looking)
        XCTAssertEqual(PanelPhase.at(elapsed: 1.5, steps: [tool("search", ["query": "a"])], done: false), .working("'a' 찾는 중…"))
    }

    func testPeriodReadsLikeSpeech() {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "Asia/Seoul")!
        let today = c.date(from: DateComponents(year: 2025, month: 4, day: 20))!
        let ko = Locale(identifier: "ko")
        XCTAssertEqual(PanelPhase.period(from: "2025-04-01", to: "2025-04-30", calendar: c, today: today, locale: ko), "4월")
        XCTAssertEqual(PanelPhase.period(from: "2024-11-01", to: "2024-11-30", calendar: c, today: today, locale: ko), "2024년 11월")
        XCTAssertEqual(PanelPhase.period(from: "2025-01-01", to: "2025-12-31", calendar: c, today: today, locale: ko), "2025년")
        XCTAssertEqual(PanelPhase.period(from: "2025-04-03", to: "2025-04-17", calendar: c, today: today, locale: ko), "4/3–4/17")
        XCTAssertEqual(PanelPhase.period(from: "2025-04-03", to: nil, calendar: c, today: today, locale: ko), "4/3부터")
        XCTAssertNil(PanelPhase.period(from: nil, to: nil, calendar: c, today: today, locale: ko))
        let en = Locale(identifier: "en")
        XCTAssertEqual(PanelPhase.period(from: "2025-04-01", to: "2025-04-30", calendar: c, today: today, locale: en), "April")
        XCTAssertEqual(PanelPhase.period(from: "2024-11-01", to: "2024-11-30", calendar: c, today: today, locale: en), "November 2024")
    }

    func testAnswerLinesSplitIntoParagraphsAndListItems() {
        let b = PanelBlocks.of("세 가지예요.\n\n- **하나** [1]\n* 둘\n2. 셋\n\n끝이에요.")
        XCTAssertEqual(b.map(\.mark), [nil, "•", "•", "2.", nil])
        XCTAssertEqual(b.map(\.text), ["세 가지예요.", "**하나** [1]", "둘", "셋", "끝이에요."])
        XCTAssertEqual(b.map(\.gap), [false, true, false, false, true])
        // one plain sentence is one paragraph, clauses as before
        let one = "결제사 한도 때문이에요, 그래서 세 번으로 정했어요."
        XCTAssertEqual(PanelBlocks.chunkCount(one), PanelMotion.chunks(one).count)
    }
}
