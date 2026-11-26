import XCTest

private final class FakeChat: PanelChatting, @unchecked Sendable {
    var requests: [PanelChatRequest] = []
    /// One entry per call: steps to emit, then either the answer or the error to throw.
    /// When set, the first call waits for it before answering.
    var gate: CheckedContinuation<Void, Never>?
    var hold = false
    var script: [(steps: [PanelStep], result: Result<ChatAnswer, Error>)] = []

    func panelChat(_ r: PanelChatRequest, onStep: @Sendable (PanelStep) -> Void) async throws -> ChatAnswer {
        requests.append(r)
        let s = script.isEmpty ? (steps: [], result: Result<ChatAnswer, Error>.success(ChatAnswer(answer: "ok", cards: []))) : script.removeFirst()
        s.steps.forEach(onStep)
        if hold { hold = false; await withCheckedContinuation { gate = $0 } }
        return try s.result.get()
    }
}

@MainActor
final class AskThreadTests: XCTestCase {
    private var events: [AnalyticsEvent] = []
    private var fallbackCalls: [(String, [String])] = []

    override func setUp() {
        events = []
        fallbackCalls = []
        transcriptsRead = []
        Analytics.sink = { [unowned self] e in MainActor.assumeIsolated { events.append(e) } }
    }
    override func tearDown() { Analytics.sink = nil }

    private func answer(_ t: String, kind: String? = nil) -> ChatAnswer { ChatAnswer(answer: t, cards: [], errorKind: kind) }

    /// Which meetings' transcripts were read, in order.
    private var transcriptsRead: [String] = []

    private func thread(_ chat: FakeChat?, meeting: String? = nil, fallback: FallbackAnswer? = nil) -> AskThread {
        AskThread(meetingID: meeting,
                  // a careless reader that hands back another meeting too: the request must still carry only the one asked for
                  transcript: { [unowned self] id in
                      transcriptsRead.append(id)
                      return (lines: [id: [["key": "a", "text": "hi"]], "other": [["key": "b", "text": "secret"]]],
                              refs: [id: [["a"]], "other": [["b"]]])
                  },
                  engine: { chat },
                  fallback: { [unowned self] q, ctx in fallbackCalls.append((q, ctx)); return fallback })
    }

    func testHistoryKeepsSix() async {
        let chat = FakeChat()
        let t = thread(chat)
        for i in 1...8 { await t.ask("q\(i)") }
        let h = chat.requests.last!.history
        XCTAssertEqual(h.count, 6)
        XCTAssertEqual(h.first?.q, "q2")
        XCTAssertEqual(h.last?.q, "q7")
        XCTAssertEqual(chat.requests.last?.text, "q8")
    }

    func testTodayIsLocalDay() async {
        let chat = FakeChat()
        await thread(chat).ask("q")
        XCTAssertTrue(chat.requests[0].today.range(of: #"^\d{4}-\d{2}-\d{2}$"#, options: .regularExpression) != nil)
    }

    func testMeetingThreadAttachesTranscript() async {
        let chat = FakeChat()
        await thread(chat, meeting: "m1").ask("q")
        XCTAssertEqual(chat.requests[0].meetingIDs, ["m1"])
        XCTAssertEqual(chat.requests[0].transcripts["m1"]?.first?["text"], "hi")
        XCTAssertEqual(chat.requests[0].refs["m1"], [["a"]])
        XCTAssertEqual(Array(chat.requests[0].transcripts.keys), ["m1"], "no other meeting's lines")
    }

    // MARK: Hand-off (docs/40 12절)

    private func handedOff(to id: String) -> ChatAnswer { ChatAnswer(answer: "", cards: [], handoff: .init(meetingId: id)) }

    func testHandoffAsksAgainOnThatMeetingOnly() async {
        let chat = FakeChat()
        let p = PanelProposal(id: "p1", kind: PanelProposal.nameSpeaker, meetingId: "m2", speaker: "화자 1", name: "나", key: "화자 1")
        let open: PanelStep = .tool("open_meeting", detail: ["meeting_id": "m2", "title": "주간 회의"], count: nil)
        let named: [PanelStep] = [.tool("start", detail: [:], count: nil), .tool("meeting_notes", detail: [:], count: nil)]
        chat.script = [([], .success(answer("「주간 회의」 하나가 있어요."))),
                       ([.tool("start", detail: [:], count: nil), open], .success(handedOff(to: "m2"))),
                       (named, .success(ChatAnswer(answer: "아래에서 확인해 주세요.", cards: [], proposals: [p])))]
        let t = thread(chat)
        await t.ask("오늘 회의 뭐했지")
        await t.ask("거기서 화자 1을 나로 바꿔")
        XCTAssertEqual(chat.requests.count, 3)
        let first = chat.requests[1], again = chat.requests[2]
        XCTAssertTrue(first.meetingIDs.isEmpty)
        XCTAssertTrue(first.transcripts.isEmpty, "the all-meetings request carries no transcript")
        XCTAssertEqual(again.text, "거기서 화자 1을 나로 바꿔")
        XCTAssertEqual(again.meetingIDs, ["m2"])
        XCTAssertEqual(Array(again.transcripts.keys), ["m2"], "only the handed-off meeting's transcript")
        XCTAssertEqual(Array(again.refs.keys), ["m2"])
        XCTAssertEqual(again.history.map(\.q), ["오늘 회의 뭐했지"], "earlier turns stay as context")
        XCTAssertEqual(again.history.map(\.a), first.history.map(\.a))
        XCTAssertEqual(again.today, first.today)
        XCTAssertEqual(transcriptsRead, ["m2"])
        XCTAssertEqual(t.turns.count, 2, "the same turn, no new one")
        XCTAssertEqual(t.turns[1].handoff, "m2")
        XCTAssertEqual(t.turns[1].proposals, [p])
        XCTAssertEqual(t.turns[1].answer?.answer, "아래에서 확인해 주세요.")
        XCTAssertEqual(t.turns[1].steps, [.tool("start", detail: [:], count: nil), open] + named, "the hand-off's line leads into the meeting's")
        XCTAssertFalse(t.isWaiting)
        XCTAssertTrue(fallbackCalls.isEmpty)
    }

    func testHandoffStepLineNamesTheMeeting() {
        let open: PanelStep = .tool("open_meeting", detail: ["meeting_id": "m2", "title": "대면 회의 음성 인식 테스트"], count: nil)
        let lines = PanelSteps.lines([.tool("start", detail: [:], count: nil), open])
        XCTAssertEqual(lines.last, "「대면 회의 음성 인식 테스트」의 대화 기록 읽는 중…")
        // the meeting's own tools take its place, and with them done it is writing
        let more = PanelSteps.lines([open, .tool("find_words", detail: ["word": "화자"], count: nil), .tool("find_words", detail: [:], count: 2)])
        XCTAssertEqual(more.last, PanelText.stepWriting)
    }

    func testHandoffHappensOncePerQuestion() async {
        let chat = FakeChat()
        chat.script = [([], .success(handedOff(to: "m2"))),
                       ([], .success(ChatAnswer(answer: "그 회의예요.", cards: [], handoff: .init(meetingId: "m3"))))]
        let t = thread(chat)
        await t.ask("q")
        XCTAssertEqual(chat.requests.count, 2, "no second hand-off")
        XCTAssertEqual(t.turns[0].handoff, "m2")
        XCTAssertEqual(t.turns[0].answer?.answer, "그 회의예요.")
    }

    func testHandoffFailureHasNoOldAsk() async {
        let chat = FakeChat()
        chat.script = [([], .success(handedOff(to: "m2"))),
                       ([], .failure(EngineError.http(0, "{}"))), ([], .failure(EngineError.http(0, "{}")))]
        let t = thread(chat, fallback: FallbackAnswer(answer: "old", meetings: [], near: []))
        await t.ask("q")
        XCTAssertEqual(chat.requests.count, 3, "the meeting's request is retried once")
        XCTAssertEqual(t.turns[0].failure, .down)
        XCTAssertTrue(fallbackCalls.isEmpty, "the old ask cannot fix a meeting")
        XCTAssertFalse(t.isWaiting)
        // 다시 묻기 starts over from all meetings
        chat.script = [([], .success(handedOff(to: "m2"))), ([], .success(answer("now")))]
        await t.retry(t.turns[0].id)
        XCTAssertEqual(chat.requests[3].meetingIDs, [])
        XCTAssertEqual(chat.requests[4].meetingIDs, ["m2"])
        XCTAssertEqual(t.turns[0].answer?.answer, "now")
    }

    func testMeetingPageIgnoresHandoff() async {
        let chat = FakeChat()
        chat.script = [([], .success(ChatAnswer(answer: "a", cards: [], handoff: .init(meetingId: "m2"))))]
        let t = thread(chat, meeting: "m1")
        await t.ask("q")
        XCTAssertEqual(chat.requests.count, 1)
        XCTAssertNil(t.turns[0].handoff)
        XCTAssertEqual(t.turns[0].answer?.answer, "a")
    }

    func testHandoffDecodes() throws {
        let d = EngineClient.engineDecoder
        let a = try d.decode(ChatAnswer.self, from: Data(#"{"answer":"","cards":[],"proposals":[],"handoff":{"meeting_id":"m2"}}"#.utf8))
        XCTAssertEqual(a.handoff?.meetingId, "m2")
        let none = try d.decode(ChatAnswer.self, from: Data(#"{"answer":"a","cards":[]}"#.utf8))
        XCTAssertNil(none.handoff)
        let bad = try d.decode(ChatAnswer.self, from: Data(#"{"answer":"a","cards":[],"handoff":{"id":3}}"#.utf8))
        XCTAssertNil(bad.handoff, "a hand-off that does not read is none")
        XCTAssertEqual(bad.answer, "a")
    }

    func testAskThreadOmitsTranscript() async {
        let chat = FakeChat()
        await thread(chat).ask("q")
        XCTAssertTrue(chat.requests[0].meetingIDs.isEmpty)
        XCTAssertTrue(chat.requests[0].transcripts.isEmpty)
        XCTAssertTrue(chat.requests[0].refs.isEmpty)
    }

    func testRetriesOnceAfterStreamBreak() async {
        let chat = FakeChat()
        chat.script = [([], .failure(EngineError.http(0, "{}"))), ([], .success(answer("late")))]
        let t = thread(chat)
        await t.ask("q")
        XCTAssertEqual(chat.requests.count, 2)
        XCTAssertEqual(t.turns.count, 1)
        XCTAssertEqual(t.turns[0].answer?.answer, "late")
        XCTAssertNil(t.turns[0].failure)
        XCTAssertEqual(events.filter { $0 == .askRetried(succeeded: true) }.count, 1)
        XCTAssertFalse(t.isWaiting)
    }

    func testSecondBreakShowsFailure() async {
        let chat = FakeChat()
        chat.script = [([], .failure(EngineError.http(0, "{}"))), ([], .failure(EngineError.http(0, "{}")))]
        let t = thread(chat)
        await t.ask("q")
        XCTAssertEqual(chat.requests.count, 2)
        XCTAssertEqual(t.turns[0].failure, .down)
        XCTAssertNil(t.turns[0].answer)
        XCTAssertEqual(events.filter { $0 == .askRetried(succeeded: false) }.count, 1)
        // retry by hand runs once more and can succeed
        chat.script = [([], .success(answer("now")))]
        await t.retry(t.turns[0].id)
        XCTAssertEqual(t.turns.count, 1)
        XCTAssertEqual(t.turns[0].answer?.answer, "now")
        XCTAssertNil(t.turns[0].failure)
    }

    func testCreditMapsToLimit() async {
        let chat = FakeChat()
        chat.script = [([], .success(answer("", kind: "credit")))]
        let t = thread(chat)
        await t.ask("q")
        XCTAssertEqual(t.turns[0].failure, .limit)
        XCTAssertEqual(chat.requests.count, 1)
        chat.script = [([], .failure(EngineError.http(402, "{\"error_kind\":\"credit\"}")))]
        await t.ask("q2")
        XCTAssertEqual(t.turns[1].failure, .limit)
        XCTAssertEqual(chat.requests.count, 2, "no retry on a credit error")
        XCTAssertTrue(fallbackCalls.isEmpty, "the limit state stays, no old ask")
        XCTAssertFalse(t.turns[1].usedFallback)
    }

    func testServerErrorFallsBackOnAskScreen() async {
        let chat = FakeChat()
        // the engine fails (a relay 400 for an unknown prompt name reads as down), and again on the automatic retry
        chat.script = [([], .failure(EngineError.http(500, "{\"error_kind\":\"down\"}"))),
                       ([], .failure(EngineError.http(500, "{\"error_kind\":\"down\"}")))]
        let fb = FallbackAnswer(answer: "old", meetings: [], near: [])
        let t = thread(chat, fallback: fb)
        await t.ask("q")
        XCTAssertEqual(chat.requests.count, 2, "one automatic retry first")
        XCTAssertTrue(t.turns[0].usedFallback)
        XCTAssertEqual(t.turns[0].fallback?.answer, "old")
        XCTAssertNil(t.turns[0].failure)
        XCTAssertEqual(fallbackCalls.map(\.0), ["q"])
        XCTAssertEqual(events.filter { $0 == .askFallback(reason: .engineFailed) }.count, 1)

        chat.script = [([], .success(answer("", kind: "model")))]
        await t.ask("q2")
        XCTAssertEqual(t.turns[1].fallback?.answer, "old", "an error kind in the answer falls back too")
        XCTAssertEqual(fallbackCalls.last?.1, ["q"])
        XCTAssertFalse(t.isWaiting)
    }

    func testServerErrorWithFailedFallbackKeepsEngineFailure() async {
        let chat = FakeChat()
        chat.script = [([], .success(answer("", kind: "network")))]
        let t = thread(chat)
        await t.ask("q")
        XCTAssertEqual(fallbackCalls.count, 1)
        XCTAssertEqual(t.turns[0].failure, .network)
    }

    func testKeyErrorDoesNotFallBack() async {
        let chat = FakeChat()
        chat.script = [([], .success(answer("", kind: "key")))]
        let t = thread(chat, fallback: FallbackAnswer(answer: "old", meetings: [], near: []))
        await t.ask("q")
        XCTAssertEqual(t.turns[0].failure, .signedOut)
        XCTAssertTrue(fallbackCalls.isEmpty, "the old ask sends the same session")
    }

    func testEngineErrorOnMeetingPageDoesNotFallBack() async {
        let chat = FakeChat()
        chat.script = [([], .success(answer("", kind: "down")))]
        let t = thread(chat, meeting: "m1", fallback: FallbackAnswer(answer: "old", meetings: [], near: []))
        await t.ask("q")
        XCTAssertEqual(t.turns[0].failure, .down)
        XCTAssertTrue(fallbackCalls.isEmpty)
    }

    func testKindsMap() {
        XCTAssertEqual(AskThreadFailure(kind: "key"), .signedOut)
        XCTAssertEqual(AskThreadFailure(kind: "network"), .network)
        XCTAssertEqual(AskThreadFailure(kind: "busy"), .down)
        XCTAssertEqual(AskThreadFailure(kind: nil), .down)
    }

    func testNoEngineUsesFallbackOnAskScreen() async {
        let fb = FallbackAnswer(answer: "old", meetings: [.init(id: "m", title: "T", start: "s", quote: "q")], near: [])
        let t = thread(nil, fallback: fb)
        await t.ask("first")
        await t.ask("second")
        XCTAssertEqual(t.turns[1].usedFallback, true)
        XCTAssertEqual(t.turns[1].fallback?.answer, "old")
        XCTAssertNil(t.turns[1].failure)
        XCTAssertEqual(fallbackCalls[1].0, "second")
        XCTAssertEqual(fallbackCalls[1].1, ["first"])
        XCTAssertEqual(events.filter { $0 == .askFallback(reason: .engineMissing) }.count, 2)
    }

    func testNoEngineFailsOnMeetingPage() async {
        let t = thread(nil, meeting: "m1", fallback: FallbackAnswer(answer: "old", meetings: [], near: []))
        await t.ask("q")
        XCTAssertEqual(t.turns[0].failure, .down)
        XCTAssertFalse(t.turns[0].usedFallback)
        XCTAssertTrue(fallbackCalls.isEmpty)
    }

    func testStepsArriveInOrder() async {
        let chat = FakeChat()
        let steps: [PanelStep] = [.tool("search", detail: ["q": "x"], count: nil), .tool("search", detail: [:], count: 3), .writing]
        chat.script = [(steps, .success(answer("a")))]
        let t = thread(chat)
        await t.ask("q")
        XCTAssertEqual(t.turns[0].steps, steps)
    }

    func testProposalsComeFromAnswer() async {
        let chat = FakeChat()
        let p = PanelProposal(id: "p1", kind: PanelProposal.renameTitle, meetingId: "m1", title: "New")
        chat.script = [([], .success(ChatAnswer(answer: "a", cards: [], proposals: [p])))]
        let t = thread(chat, meeting: "m1")
        await t.ask("q")
        XCTAssertEqual(t.turns[0].proposals, [p])
    }

    func testRetryDuringFlightDoesNothing() async {
        let chat = FakeChat()
        chat.hold = true
        let t = thread(chat)
        let running = Task { await t.ask("q") }
        while chat.gate == nil { await Task.yield() }
        await t.retry(t.turns[0].id)
        XCTAssertEqual(chat.requests.count, 1, "no second request while the first is streaming")
        chat.gate?.resume()
        await running.value
        XCTAssertEqual(t.turns[0].answer?.answer, "ok")
        XCTAssertEqual(chat.requests.count, 1)
    }

    func testCancellationIsNotRetriedNorNetwork() async {
        let chat = FakeChat()
        chat.script = [([], .failure(URLError(.cancelled)))]
        let t = thread(chat, fallback: FallbackAnswer(answer: "old", meetings: [], near: []))
        await t.ask("q")
        XCTAssertEqual(chat.requests.count, 1)
        XCTAssertEqual(t.turns[0].failure, .down)
        XCTAssertTrue(events.isEmpty)
        XCTAssertTrue(fallbackCalls.isEmpty, "a cancel does not fall back")
    }
}
