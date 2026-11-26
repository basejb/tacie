import XCTest
import SwiftUI

/// The main window's panel (docs/36 1-2–1-4절): what each answer shows, where the panel sits, and PNGs of its states for
/// a look. The PNGs are written only when TACIE_PANEL_RENDER_DIR names a folder (pass it as
/// TEST_RUNNER_TACIE_PANEL_RENDER_DIR); nothing opens on screen. The text field is AppKit and draws blank in renders.
@MainActor
final class PanelRenderTests: XCTestCase {
    private let cal: Calendar = {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "Asia/Seoul")!
        c.firstWeekday = 2
        return c
    }()

    private func day(_ m: Int, _ d: Int, _ h: Int = 14, _ min: Int = 0) -> Date {
        cal.date(from: DateComponents(year: 2026, month: m, day: d, hour: h, minute: min))!
    }

    private var meetings: [String: SavedMeeting] = [:]

    override func setUp() {
        var a = SavedMeeting(mode: "inperson", start: day(11, 7))
        a.end = day(11, 7, 14, 42)
        a.notes = .done
        a.title = "주간 결제 싱크"
        a.voices = [VoiceEntry(key: "화자 1", name: "지윤"), VoiceEntry(key: "화자 2", name: "민호"), VoiceEntry(key: "화자 3", name: "서연")]
        a.transcript = [
            TranscriptLine(key: "e0:s1u1", at: day(11, 7, 14, 7), speaker: nil, text: "실패하면 다섯 번은 다시 해 봐야 하지 않을까요?",
                           question: true, voice: "화자 1"),
            TranscriptLine(key: "e0:s1u2", at: day(11, 7, 14, 8), speaker: nil, text: "다섯 번이면 결제사 한도에 걸리고, 중복 결제가 날 수 있어요.",
                           question: false, voice: "화자 2"),
            TranscriptLine(key: "e0:s1u3", at: day(11, 7, 14, 9), speaker: nil, text: "그럼 세 번으로 하고, 한도는 지윤 님이 확인해 주세요.",
                           question: false, voice: "화자 3"),
        ]
        a.ledger = [
            LedgerEntry(kind: "decision", text: "결제 실패 시 재시도는 세 번까지 해요.", refs: ["e0:s1u3"]),
            LedgerEntry(kind: "commitment", text: "재시도 정책 문서 정리", owner: "화자 2", due: "11월 12일"),
            LedgerEntry(kind: "open_question", text: "부분 환불 수수료를 누가 낼지", refs: ["e0:s1u2"]),
        ]
        var b = SavedMeeting(mode: "remote", start: day(10, 31))
        b.end = day(10, 31, 15)
        b.notes = .done
        b.title = "결제 정책 논의"
        meetings = [a.id: a, b.id: b]
    }

    private var a: SavedMeeting { meetings.values.first { $0.title == "주간 결제 싱크" }! }
    private var b: SavedMeeting { meetings.values.first { $0.title == "결제 정책 논의" }! }
    private func lookup(_ id: String) -> SavedMeeting? { meetings[id] }

    private func card(_ id: String, kind: String, title: String, path: String? = nil) -> Card {
        Card(id: id, path: path ?? "\(kind)/\(id)", kind: kind, source: kind, byline: nil, url: "https://example.com/\(id)",
             lineStart: 0, lineEnd: 0, title: title, snippet: "", score: 0.9, githubUrl: nil, localPath: nil, chunk: 1, repeat: nil)
    }

    private func prep(open: Bool = true, todos: Bool = true, error: String? = nil) -> PrepAnswer {
        PrepAnswer(
            advice: "부분 환불이 두 번의 회의에서 계속 미뤄졌어요. 이번에는 수수료 문제부터 정하면 좋겠어요.",
            open: open ? [PanelItem(meetingId: a.id, index: 2, kind: "open_question", text: "부분 환불 수수료를 누가 낼지",
                                    owner: nil, due: nil, also: [b.id]),
                          PanelItem(meetingId: b.id, index: 0, kind: "open_question", text: "부분 환불을 이번 분기에 넣을지",
                                    owner: nil, due: nil, also: [])] : [],
            todos: todos ? [PanelItem(meetingId: a.id, index: 1, kind: "commitment", text: "재시도 정책 문서 정리",
                                      owner: "화자 2", due: "11월 12일", also: nil)] : [],
            sources: [card("pay142", kind: "linear", title: "PAY-142 부분 환불 API"),
                      card("slack1", kind: "slack", title: "#payments 환불 수수료 정책 초안")],
            meetingsRead: 2, sourcesRead: 2, errorKind: error)
    }

    /// A long chain: the prepared meeting `a`, an earlier `b`, and an older `c`; 4 of the 9 items come from `a` or came up twice.
    private func longPrep(c: String) -> PrepAnswer {
        func q(_ m: String, _ i: Int, _ t: String, also: [String] = []) -> PanelItem {
            PanelItem(meetingId: m, index: i, kind: "open_question", text: t, owner: nil, due: nil, also: also)
        }
        func t(_ m: String, _ i: Int, _ x: String) -> PanelItem {
            PanelItem(meetingId: m, index: i, kind: "commitment", text: x, owner: "화자 2", due: "11월 12일", also: nil)
        }
        return PrepAnswer(
            advice: "부분 환불이 두 번의 회의에서 계속 미뤄졌어요. 이번에는 수수료 문제부터 정하면 좋겠어요.",
            open: [q(b.id, 0, "환불 사유별 수수료 표를 따로 둘지"), q(a.id, 2, "부분 환불 수수료를 누가 낼지", also: [b.id]),
                   q(c, 1, "정산 주기를 주 단위로 바꿀지"), q(b.id, 3, "해외 카드 환불은 같은 정책을 쓸지", also: [c]),
                   q(c, 4, "환불 알림 문구를 누가 쓸지")],
            todos: [t(c, 2, "결제사 한도 문서 받기"), t(a.id, 1, "재시도 정책 문서 정리"), t(b.id, 1, "환불 API 초안 공유"),
                    t(c, 5, "정산 표 업데이트")],
            sources: [card("pay142", kind: "linear", title: "PAY-142 부분 환불 API")],
            meetingsRead: 3, sourcesRead: 1, errorKind: nil)
    }

    private func longMeetingID() -> String {
        var c = SavedMeeting(mode: "remote", start: day(10, 24))
        c.end = day(10, 24, 15)
        c.notes = .done
        c.title = "정산 주기 점검"
        meetings[c.id] = c
        return c.id
    }

    // MARK: Behaviour

    func testPrepShowsRecentAndRepeatedFirstAndCountsTheRest() {
        let p = longPrep(c: longMeetingID())
        let short = PanelShown.prep(p, recent: a.id, expanded: false)
        // shown: the prepared meeting's items and those that came up in more than one meeting, group order and own order kept
        XCTAssertEqual(short.groups.map(\.title), ["아직 안 정한 것", "안 끝난 할 일"])
        XCTAssertEqual(short.groups[0].items.map(\.text), ["부분 환불 수수료를 누가 낼지", "해외 카드 환불은 같은 정책을 쓸지"])
        XCTAssertEqual(short.groups[1].items.map(\.text), ["재시도 정책 문서 정리"])
        XCTAssertEqual(short.hidden, 6)
        XCTAssertTrue(short.hasItems)
        XCTAssertEqual(PanelText.prepMore(short.hidden), "나머지 항목 6개 더")
        // expanded: everything, nothing hidden; the first view stays on top and the rest follows in its own order
        let full = PanelShown.prep(p, recent: a.id, expanded: true)
        XCTAssertEqual(full.groups.map(\.items.count), [5, 4])
        XCTAssertEqual(full.groups[0].items.map(\.text), ["부분 환불 수수료를 누가 낼지", "해외 카드 환불은 같은 정책을 쓸지",
                                                         "환불 사유별 수수료 표를 따로 둘지", "정산 주기를 주 단위로 바꿀지",
                                                         "환불 알림 문구를 누가 쓸지"])
        XCTAssertEqual(full.groups[1].items.map(\.text), ["재시도 정책 문서 정리", "결제사 한도 문서 받기", "환불 API 초안 공유",
                                                         "정산 표 업데이트"])
        XCTAssertEqual(full.hidden, 0)
    }

    /// At most five first per group: those raised in more than one meeting, then the prepared meeting's own, each in its
    /// own order; the prepared meeting's sixth item waits behind the button too.
    func testPrepFirstViewIsFiveRepeatedBeforeOwn() {
        func q(_ m: String, _ t: String, also: [String] = []) -> PanelItem {
            PanelItem(meetingId: m, index: 0, kind: "open_question", text: t, owner: nil, due: nil, also: also)
        }
        let open = [q(a.id, "own1"), q(b.id, "rep1", also: [a.id]), q(a.id, "own2"), q(b.id, "old1"), q(a.id, "rep2", also: [b.id]),
                    q(a.id, "own3"), q(a.id, "own4"), q(b.id, "rep3", also: [a.id])]
        let p = PrepAnswer(advice: "x", open: open, todos: [], sources: [], meetingsRead: 2, sourcesRead: 0, errorKind: nil)
        let short = PanelShown.prep(p, recent: a.id, expanded: false)
        XCTAssertEqual(short.groups.first?.items.map(\.text), ["rep1", "rep2", "rep3", "own1", "own2"])
        XCTAssertEqual(short.hidden, 3)
        let full = PanelShown.prep(p, recent: a.id, expanded: true)
        XCTAssertEqual(full.groups.first?.items.map(\.text), ["rep1", "rep2", "rep3", "own1", "own2", "old1", "own3", "own4"])
        // what a prep carries into the next meeting follows the same order
        XCTAssertEqual(PanelStore.preparedItems(p, recent: a.id).map(\.text), ["rep1", "rep2", "rep3", "own1", "own2", "old1", "own3", "own4"])
        // the copy has every item
        let md = PanelShown.markdown(p, meeting: lookup, calendar: cal)
        for item in open { XCTAssertTrue(md.contains("- \(item.text)"), item.text) }
        XCTAssertEqual(PanelText.prepMore(1), "나머지 항목 1개 더")
    }

    /// The read line leaves out a part that is zero, and is gone when nothing was read.
    func testReadLineOmitsZeroParts() {
        XCTAssertEqual(PanelText.read(meetings: 2, sources: 3), "회의 2개와 연결한 서비스 3개를 읽고 답했어요")
        XCTAssertEqual(PanelText.read(meetings: 5, sources: 0), "회의 5개를 읽고 답했어요")
        XCTAssertEqual(PanelText.read(meetings: 0, sources: 2), "연결한 서비스 2개를 읽고 답했어요")
        XCTAssertNil(PanelText.read(meetings: 0, sources: 0))
        let none = PrepAnswer(advice: "", open: [], todos: [], sources: [], meetingsRead: 0, sourcesRead: 0, errorKind: nil)
        XCTAssertNil(PanelShown.prep(none).read)
    }

    func testPrepHasNoMoreButtonWhenNothingIsHidden() {
        // every item is from the prepared meeting or repeated
        let shown = PanelShown.prep(prep(), recent: a.id, expanded: false)
        XCTAssertEqual(shown.hidden, 1, "the fixture's own b-only question hides")
        let onlyRecent = PrepAnswer(advice: "", open: [prep().open[0]], todos: prep().todos, sources: [], meetingsRead: 1,
                                    sourcesRead: 0, errorKind: nil)
        XCTAssertEqual(PanelShown.prep(onlyRecent, recent: a.id, expanded: false).hidden, 0)
        // no prepared meeting known: only the repeated ones lead
        XCTAssertEqual(PanelShown.prep(prep(), recent: nil, expanded: false).groups.map(\.items.count), [1])
    }

    func testPrepAllHiddenKeepsAdviceAndCopyAndGroupsSkipped() {
        let p = longPrep(c: longMeetingID())
        // a different meeting prepared: group with nothing to show is left out, but the answer still has items
        let s = PanelShown.prep(PrepAnswer(advice: "x", open: [], todos: [p.todos[0]], sources: [], meetingsRead: 1,
                                           sourcesRead: 0, errorKind: nil), recent: a.id, expanded: false)
        XCTAssertEqual(s.groups, [])
        XCTAssertEqual(s.hidden, 1)
        XCTAssertTrue(s.hasItems)
        XCTAssertEqual(s.advice, "x", "not the nothing-open sentence")
    }

    func testPrepCopyHasEveryItemEvenWhenCollapsed() {
        let p = longPrep(c: longMeetingID())
        let md = PanelShown.markdown(p, meeting: lookup, calendar: cal)
        for item in p.open + p.todos { XCTAssertTrue(md.contains("- \(item.text)"), item.text) }
    }

    func testPrepViewRendersCollapsedAndExpanded() {
        let p = longPrep(c: longMeetingID())
        func view(_ expanded: Bool) -> some View {
            PrepAnswerView(answer: p, steps: [], meeting: lookup, actions: PanelActions(), calendar: cal,
                           preparedID: a.id, expanded: expanded).frame(width: 340)
        }
        let collapsed = image(view(false)), full = image(view(true))
        XCTAssertNotNil(collapsed)
        XCTAssertNotNil(full)
        XCTAssertGreaterThan(full?.size.height ?? 0, collapsed?.size.height ?? 0)
    }

    func testRendersPrepAnswerWithReadLine() throws {
        let shown = PanelShown.prep(prep())
        XCTAssertEqual(shown.read, "회의 2개와 연결한 서비스 2개를 읽고 답했어요")
        XCTAssertEqual(shown.groups.map(\.title), ["아직 안 정한 것", "안 끝난 할 일"])
        XCTAssertEqual(shown.advice, prep().advice)
        XCTAssertNil(shown.reason)
        let line = PanelShown.itemLine(prep().open[0], meeting: lookup, calendar: cal).map(\.text).joined()
        XCTAssertEqual(line, "11월 7일 · 10월 31일에도 나왔어요")
        let todo = PanelShown.itemLine(prep().todos[0], meeting: lookup, calendar: cal, now: day(11, 14))
        XCTAssertEqual(todo.map(\.text).joined(), "민호 · 11월 12일 · 기한 지남")
        XCTAssertEqual(todo.last?.warn, true)
        let md = PanelShown.markdown(prep(), meeting: lookup, calendar: cal)
        XCTAssertTrue(md.contains("### 아직 안 정한 것\n- 부분 환불 수수료를 누가 낼지"), md)
        XCTAssertTrue(md.contains("### 안 끝난 할 일\n- 재시도 정책 문서 정리"), md)
        XCTAssertTrue(md.hasPrefix("부분 환불이 두 번의 회의에서"), md)
        XCTAssertEqual(PanelShown.markdown(PrepAnswer(advice: "먼저 정해요[1].", open: prep().open, todos: [], sources: [],
                                                      meetingsRead: 1, sourcesRead: 0, errorKind: nil),
                                           meeting: lookup, calendar: cal).components(separatedBy: "\n").first, "먼저 정해요.")
        XCTAssertNotNil(image(PrepAnswerView(answer: prep(), steps: [.meetings([a.id]), .related([b.id])], meeting: lookup,
                                             actions: PanelActions(), calendar: cal).frame(width: 340)))
    }

    func testHidesEmptyGroups() {
        let onlyTodos = PanelShown.prep(prep(open: false))
        XCTAssertEqual(onlyTodos.groups.map(\.title), ["안 끝난 할 일"])
        let none = PanelShown.prep(prep(open: false, todos: false))
        XCTAssertEqual(none.groups, [])
        XCTAssertEqual(none.advice, "아직 정하지 않은 것은 없어요.")
        XCTAssertFalse(PanelShown.markdown(prep(open: false, todos: false), meeting: lookup, calendar: cal).contains("###"))
    }

    func testShowsErrorKindReason() {
        // the model's part failed: its reason in place of the advice, the ledger groups still there
        let model = PanelShown.prep(prep(error: "model"))
        XCTAssertEqual(model.reason, "검색 모델을 받고 있어요")
        XCTAssertNil(model.advice)
        XCTAssertEqual(model.groups.count, 2)
        // the app's words win for the kinds the engine knows
        XCTAssertEqual(PanelText.reason("credit", say: { _ in "이번 달 AI 사용량을 다 썼어요" }), "이번 달 AI 사용량을 다 썼어요")
        // no engine to ask: the island's words for it, not "외부 서비스가 응답하지 않아요"
        XCTAssertEqual(PanelText.reason("engine"), "검색 엔진에 연결하지 못했어요")
        // any other kind (the store's "none") still says something
        for kind in ["none", "", "weird"] {
            XCTAssertEqual(PanelText.reason(kind, say: { _ in "x" }), "답을 가져오지 못했어요.")
        }
        // no answer: the engine's sentence, nil, or no sources
        XCTAssertFalse(PanelShown.found("찾은 자료에는 답이 없어요.", sources: 2))
        XCTAssertFalse(PanelShown.found(nil, sources: 2))
        XCTAssertFalse(PanelShown.found("세 번이에요[1]", sources: 0))
        XCTAssertTrue(PanelShown.found("세 번이에요[1]", sources: 1))
        XCTAssertEqual(PanelText.notFound("주간 결제 싱크"), "주간 결제 싱크에서는 이야기가 나오지 않았어요")
    }

    func testNarrowWindowOverlaysPanel() {
        // a 900pt window with the 300pt sidebar leaves 600 for the page and the panel: the page would get 228
        XCTAssertTrue(PanelLayout<EmptyView, EmptyView>.overlays(width: 900 - 300))
        // the default 1200pt window keeps the panel beside the page (528 left for it)
        XCTAssertFalse(PanelLayout<EmptyView, EmptyView>.overlays(width: 1200 - 300))
        XCTAssertTrue(PanelLayout<EmptyView, EmptyView>.overlays(width: 372 + 479))
        XCTAssertFalse(PanelLayout<EmptyView, EmptyView>.overlays(width: 372 + 480))
        // over the page, the page keeps its whole width
        let probe = WidthProbe()
        _ = image(PanelLayout(open: true, close: {}) { probe.view } panel: { Color.gray }.frame(width: 600, height: 400))
        XCTAssertEqual(probe.width ?? 0, 600, accuracy: 0.5)
        let wide = WidthProbe()
        _ = image(PanelLayout(open: true, close: {}) { wide.view } panel: { Color.gray }.frame(width: 1100, height: 400))
        XCTAssertEqual(wide.width ?? 0, 1100 - 372, accuracy: 0.5)
    }

    func testPrepItemOpensItsMeetingWithTheLedgerRowLit() {
        let jump = PanelJump()
        let t = PanelShown.target(for: prep().open[0], meeting: lookup)
        XCTAssertEqual(t, PanelTarget(meetingID: a.id, keys: ["e0:s1u2"], ledgerIndex: 2))
        // an item from a meeting without refs still names its row
        XCTAssertEqual(PanelShown.target(for: prep().open[1], meeting: lookup), PanelTarget(meetingID: b.id, ledgerIndex: 0))
        XCTAssertEqual(jump.go(t, shown: b.id, exists: true, listed: true), .select(clearQuery: false))
        XCTAssertNil(jump.take(for: b.id))  // the page being left does not take it
        let mark = jump.take(for: a.id)
        XCTAssertEqual(mark?.ledgerIndex, 2)
        XCTAssertEqual(mark?.keys, ["e0:s1u2"])
        XCTAssertNil(jump.take(for: a.id))
        // pressed again on the page already showing it: lit again, no page change
        XCTAssertEqual(jump.go(t, shown: a.id, exists: true, listed: true), .stay)
        XCTAssertEqual(jump.take(for: a.id)?.ledgerIndex, 2)
        XCTAssertNotNil(image(Text("부분 환불 수수료를 누가 낼지").modifier(LedgerMarkStyle(on: true)).padding(12)))
    }

    func testSourceHiddenBySearchClearsTheQueryAndNeverLeavesTheMark() {
        let jump = PanelJump()
        // the list's search hides the meeting: the window clears it so the page really changes
        XCTAssertEqual(jump.go(PanelTarget(meetingID: a.id), shown: b.id, exists: true, listed: false), .select(clearQuery: true))
        // a deleted meeting: nothing moves, nothing is marked
        XCTAssertEqual(jump.go(PanelTarget(meetingID: "gone", keys: ["k"]), shown: b.id, exists: false, listed: false), .none)
        XCTAssertNil(jump.mark)
        // the page already shows it: no move, so no mark that a change would have to clear
        XCTAssertEqual(jump.go(PanelTarget(meetingID: b.id), shown: b.id, exists: true, listed: true), .stay)
    }

    /// docs/36 0-1: the head names the conversation and where it started; the empty screen names the place.
    func testHeadAndEmptyScreenWords() {
        XCTAssertEqual(PanelText.headTitle(nil), "새 대화")
        let t = PanelThread(id: UUID(), title: "이 회의에서 안 정한 것", origin: .meeting(a.id), turns: [])
        XCTAssertEqual(PanelText.headTitle(t), "이 회의에서 안 정한 것")
        XCTAssertEqual(PanelText.place(.meeting(a.id), meeting: lookup, calendar: cal), "주간 결제 싱크")
        XCTAssertEqual(PanelText.place(.all, meeting: lookup, calendar: cal), "모든 회의")
        XCTAssertEqual(PanelText.place(.meeting("gone"), meeting: lookup, calendar: cal), "지운 회의")
        XCTAssertEqual(PanelText.emptyTitle(.meeting(a.id), meeting: lookup, calendar: cal), "주간 결제 싱크에 대해 물어보세요")
        XCTAssertEqual(PanelText.emptyTitle(.all, meeting: lookup, calendar: cal), "회의와 연결한 서비스에 물어보세요")
        XCTAssertEqual(PanelText.emptyBody, "회의 기록과 연결한 서비스에서 찾아 근거와 함께 답해요.")
    }

    /// The conversation list: by day (오늘, 어제, then the date), each conversation once, newest first as kept.
    func testConversationListGroupsByDay() {
        let now = day(11, 15, 10)
        func thread(_ title: String, _ at: Date) -> PanelThread {
            PanelThread(id: UUID(), title: title, origin: .all, turns: [turn("q", nil)].map {
                PanelTurn(id: $0.id, question: $0.question, at: at, steps: [], body: nil)
            })
        }
        let list = [thread("a", day(11, 15, 9)), thread("b", day(11, 15, 8)), thread("c", day(11, 14, 20)),
                    thread("d", day(11, 10, 12)), PanelThread(id: UUID(), title: "empty", origin: .all, turns: [])]
        let days = PanelConversationList.days(list, calendar: cal, now: now)
        XCTAssertEqual(days.map(\.title), ["오늘", "어제", "11월 10일 화요일"])
        XCTAssertEqual(days.map { $0.threads.map(\.title) }, [["a", "b"], ["c"], ["d"]])
    }

    func testCitationNumbersOpenTheirSource() {
        let styled = Citations.styled("세 번으로 정했어요[2].", size: 14.5, linked: true)
        let links = styled.runs.compactMap(\.link)
        XCTAssertEqual(links.compactMap(Citations.number(of:)), [2])
        XCTAssertEqual(Citations.styled("a[1]", size: 14, linked: false).runs.compactMap(\.link), [])
        XCTAssertNil(Citations.number(of: URL(string: "https://example.com/2")!))
        let why = WhyAnswer(reason: "r[1][2]", keys: ["e0:s1u1", "e0:s1u2"], errorKind: nil)
        XCTAssertEqual(PanelShown.whyTarget(2, answer: why, meetingID: a.id), PanelTarget(meetingID: a.id, keys: ["e0:s1u2"]))
        XCTAssertNil(PanelShown.whyTarget(3, answer: why, meetingID: a.id))
    }

    func testOneBlackButtonWhenPanelOpen() {
        XCTAssertTrue(PanelInk.pageButtonStrong(panelOpen: false))
        XCTAssertFalse(PanelInk.pageButtonStrong(panelOpen: true))
        XCTAssertFalse(PanelInk.sendStrong(draft: "  "))
        XCTAssertTrue(PanelInk.sendStrong(draft: "왜요?"))
        // the page under an open panel is told so (its black button steps back)
        let seen = OpenProbe()
        _ = image(PanelLayout(open: true, close: {}) { seen.view } panel: { Color.gray }.frame(width: 1200, height: 300))
        XCTAssertEqual(seen.open, true)
        let shut = OpenProbe()
        _ = image(PanelLayout(open: false, close: {}) { shut.view } panel: { Color.gray }.frame(width: 1200, height: 300))
        XCTAssertEqual(shut.open, false)
    }

    /// docs/36 1-1, mockup v7 step 7: grey words with the small face and the ⌘J hint, no filled capsule behind them.
    func testCollectedAskButtonIsGreyWordsWithoutAFill() throws {
        let s = store(scope: .all, turns: [])
        func pixel(_ v: some View, x: Int, y: Int) throws -> NSColor {
            let img = try XCTUnwrap(image(v.environment(\.colorScheme, .light)))
            let rep = try XCTUnwrap(NSBitmapImageRep(data: try XCTUnwrap(img.tiffRepresentation)))
            return try XCTUnwrap(rep.colorAt(x: x, y: y)?.usingColorSpace(.sRGB))
        }
        let button = AskEntryButton(store: s).background(Theme.paper)
        let size = try XCTUnwrap(image(button)).size
        let edge = try pixel(button, x: 2, y: Int(size.height / 2))
        let paper = try pixel(Theme.paper.frame(width: 10, height: 10), x: 5, y: 5)
        XCTAssertEqual(edge.redComponent, paper.redComponent, accuracy: 0.01)
        XCTAssertEqual(edge.greenComponent, paper.greenComponent, accuracy: 0.01)
        XCTAssertEqual(edge.blueComponent, paper.blueComponent, accuracy: 0.01)
    }

    /// The head row takes the clicks on 지난 질문, 새 질문 and × even with a long answer scrolled under it (E2E 2026-11-14:
    /// on macOS 26 the scroll view's top edge reached up under the row and took them, as TitleRowPage notes).
    func testHeaderControlsAreAboveTheScrollView() throws {
        let long = longPrep(c: longMeetingID())
        let s = store(scope: .meeting(a.id), turns: [turn("다음 회의 준비", steps: [.meetings([a.id])], .prep(long)),
                                                    turn("다음 회의 준비", steps: [.meetings([a.id])], .prep(long))])
        let size = NSSize(width: PanelLayout<EmptyView, EmptyView>.width, height: 520)
        // the main window as the app makes it (MainWindow.makeWindow): a hosting controller, the 52 pt title bar, no safe area
        let controller = NSHostingController(rootView: PanelView(store: s, meeting: lookup).ignoresSafeArea())
        controller.sizingOptions = []
        let window = NSWindow(contentViewController: controller)
        MainWindowShape.apply(window)
        window.isReleasedWhenClosed = false
        window.setContentSize(size)
        let host = controller.view
        host.layoutSubtreeIfNeeded()
        let scrolls = try XCTUnwrap(Self.views(in: host).compactMap { $0 as? NSScrollView }.first, "the panel scrolls")
        // scrolled to the middle, as in the user's screenshot
        scrolls.contentView.scroll(to: NSPoint(x: 0, y: max(0, (scrolls.documentView?.frame.height ?? 0) / 2)))
        host.layoutSubtreeIfNeeded()
        let frame = try XCTUnwrap(host.superview)
        for x in [150.0, 230, 300, size.width - 28] {
            // window points: y up from the bottom, the head row's middle 26 pt below the top
            let hit = frame.hitTest(NSPoint(x: x, y: frame.bounds.height - 26))
            XCTAssertNotNil(hit, "x \(x)")
            var v = hit
            while let cur = v {
                XCTAssertFalse(cur is NSScrollView, "x \(x): the scroll view takes the click (\(String(describing: type(of: hit!))))")
                XCTAssertFalse(String(describing: type(of: cur)).contains("Backdrop"), "x \(x): \(type(of: cur))")
                v = cur.superview
            }
        }
        window.close()
    }

    private static func views(in v: NSView) -> [NSView] { [v] + v.subviews.flatMap { views(in: $0) } }

    // MARK: Renders

    private func image<V: View>(_ view: V, scale: CGFloat = 1) -> NSImage? {
        let r = ImageRenderer(content: view)
        r.scale = scale
        return r.nsImage
    }

    private func png<V: View>(_ view: V, size: CGSize, _ name: String, dark: Bool, scale: CGFloat, to out: URL) {
        AppearanceMode.isDark = dark
        defer { AppearanceMode.isDark = false }
        let framed = view.frame(width: size.width, height: size.height).background(Theme.paper)
            .environment(\.colorScheme, dark ? .dark : .light)
            .environment(\.displayScale, scale)
        let r = ImageRenderer(content: framed)
        r.scale = scale
        guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
        try? data.write(to: out.appendingPathComponent("\(name).png"))
    }

    private func store(scope: PanelScope, turns: [PanelTurn]) -> PanelStore {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let s = PanelStore(engine: { nil }, meetings: { [meetings] in meetings[$0] }, folder: { dir })
        s.open(scope: scope, via: .page)
        if !turns.isEmpty { s.current = PanelThread(id: UUID(), title: PanelThread.title(from: turns[0].question), origin: scope, turns: turns) }
        return s
    }

    private func turn(_ q: String, steps: [PanelStep] = [], _ body: PanelTurnBody?) -> PanelTurn {
        PanelTurn(id: UUID(), question: q, at: day(11, 14, 13, 55), steps: steps, body: body)
    }

    /// The chat-first layout (mockup v9, layout B): the empty screen on a meeting and on every meeting, a
    /// conversation with a finished answer, the conversation list, and the pages' entry. Written as light and dark @2x.
    func testRenderChatFirstLayout() async throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_PANEL_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let here = PanelScope.meeting(a.id)
        let answer = PanelAsk(answer: "부분 환불 수수료를 누가 낼지가 아직 안 정해졌어요[1]. 10월 31일 회의에서도 같은 이유로 미뤄졌고[2], Linear 이슈도 그 결정을 기다리고 있어요[3].",
                              cited: [], cards: [card(a.id, kind: "meeting", title: "주간 결제 싱크", path: "meeting/\(a.id)"),
                                                 card(b.id, kind: "meeting", title: "결제 정책 논의", path: "meeting/\(b.id)"),
                                                 card("pay142", kind: "linear", title: "PAY-142 부분 환불 API")],
                              errorKind: nil)
        let talk = store(scope: here, turns: [turn("이 회의에서 아직 안 정한 거 뭐야?", .ask(answer))])
        // the list: conversations of today, yesterday and last week; the one on screen tinted
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        var at = Date().addingTimeInterval(-6 * 86400)
        let list = PanelStore(engine: { nil }, meetings: { [meetings] in meetings[$0] }, folder: { dir }, clock: { at })
        let asked: [(PanelScope, String, TimeInterval)] = [(.all, "가격 정책 어디서 정했지", -6 * 86400), (.meeting(b.id), "부분 환불을 이번 분기에 넣을지", -86400 - 3600),
                                                           (.all, "이번 달에 미뤄진 결정 뭐 있어?", -7200), (here, "이 회의에서 아직 안 정한 거 뭐야?", -600)]
        for (scope, q, ago) in asked {
            at = Date().addingTimeInterval(ago)
            list.open(scope: scope, via: .page)
            list.newThread()
            list.ask(q)
        }
        await list.settle()
        let views: [(String, PanelStore, Bool)] = [("t3-empty-meeting", store(scope: here, turns: []), false),
                                                   ("t3-empty-all", store(scope: .all, turns: []), false),
                                                   ("t3-conversation", talk, false),
                                                   ("t3-list", list, true)]
        for (name, s, listed) in views {
            for dark in [false, true] {
                png(PanelView(store: s, meeting: lookup, scrolls: false, showingList: listed), size: CGSize(width: 372, height: 640),
                    "\(name)-\(dark ? "dark" : "light")@2x", dark: dark, scale: 2, to: out)
            }
        }
        for dark in [false, true] {
            png(AskEntryButton(store: talk, meetingID: a.id).padding(10), size: CGSize(width: 220, height: 48),
                "t3-entry-\(dark ? "dark" : "light")@2x", dark: dark, scale: 2, to: out)
        }
    }

    /// The chat-first layout (docs/36 0-2절, mockup v9): Tacie at work, frame by frame. A request in flight at
    /// 0.5 s (only the bubble), 1.5 s (eyes looking, brighter), 4 s (step line), 12 s (slower), the answer half in, done;
    /// and the face's glow steps close up. Light and dark @2x.
    func testRenderMotionFrames() async throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_PANEL_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let here = PanelScope.meeting(a.id)
        let q = "이 회의에서 아직 안 정한 거 뭐야?"
        let answer = PanelAsk(answer: "부분 환불 수수료를 누가 낼지가 아직 안 정해졌어요[1]. 10월 31일 회의에서도 같은 이유로 미뤄졌고[2], Linear 이슈도 그 결정을 기다리고 있어요[3].",
                              cited: [], cards: [card(a.id, kind: "meeting", title: "주간 결제 싱크", path: "meeting/\(a.id)"),
                                                 card(b.id, kind: "meeting", title: "결제 정책 논의", path: "meeting/\(b.id)"),
                                                 card("pay142", kind: "linear", title: "PAY-142 부분 환불 API")],
                              errorKind: nil)
        let read: [PanelStep] = [.meetings([a.id]), .related([b.id]), .sources(1)]
        // the answer half in: the first clause in, the second half-way, the third not yet
        let half = PanelReveal(t: 0, staggered: true).chunkStart(chips: 3, 1) + PanelMotion.chunkStep
        // a frame `ago` seconds after the question, with the answer `answered` seconds ago; the views read the clock,
        // so the turn is placed in the past
        let frames: [(String, TimeInterval?, [PanelStep], TimeInterval?, PanelAsk?)] = [
            ("t4-idle", nil, [], nil, nil),
            ("t4-0.5s", 0.5, [], nil, nil),
            ("t4-1.5s-looking", 1.5, [], nil, nil),
            ("t4-3.8s-working-bright", 3.8, read, nil, nil),
            ("t4-4.6s-working-dim", 4.6, read, nil, nil),
            ("t4-7s-ask-writing", 7, [], nil, nil),
            ("t4-12s-slow", 12, read, nil, nil),
            ("t4-answer-arriving", 5, read, half, answer),
            ("t4-done", 5, read, 3, answer),
        ]
        for (name, ago, steps, answered, body) in frames {
            for dark in [false, true] {
                let now = Date()
                let t = ago.map {
                    PanelTurn(id: UUID(), question: q, at: now.addingTimeInterval(-$0), steps: steps, body: body.map { .ask($0) },
                              answeredAt: answered.map { now.addingTimeInterval(-$0) })
                }
                let s = store(scope: here, turns: t.map { [$0] } ?? [])
                png(PanelView(store: s, meeting: lookup, scrolls: false), size: CGSize(width: 372, height: 420),
                    "\(name)-\(dark ? "dark" : "light")@2x", dark: dark, scale: 2, to: out)
            }
        }
        for dark in [false, true] {
            let faces = HStack(spacing: 10) {
                PanelFace(size: .head)
                PanelFace(size: .head, look: true, glow: 1)
                PanelFace(size: .head, glow: 1)
                PanelFace(size: .head, glow: 2)
            }
            .padding(8)
            png(faces, size: CGSize(width: 166, height: 42), "t4-face-steps-\(dark ? "dark" : "light")@4x", dark: dark, scale: 4, to: out)
        }
    }

    /// docs/38 7절: the chat's step lines from its tools, a list answer, and a prep with the model's lead. Light and
    /// dark, 1x and 2x.
    /// The write tools' cards (docs/38 12-6절): a fix with places in the transcript and the notes, the same applied, a
    /// ledger change, and a fix of an iPhone meeting (notes only, no 다시 쓰기). Light and dark, 1x and 2x.
    func testRenderProposals() async throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_PANEL_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let here = PanelScope.meeting(a.id)
        func pl(_ at: PanelProposal.At, _ before: String, _ after: String, word: String = "다섯 번", to: String = "세 번") -> PanelProposal.Place {
            PanelProposal.Place(at: at, nth: 0, before: before, word: word, after: after, to: to)
        }
        let fix = PanelProposal(id: "p1", kind: PanelProposal.fixWords, meetingId: a.id, heard: "다섯 번", meant: "세 번", places: [
            pl(.init(line: "e0:s1u1"), "실패하면 ", "은 다시 해 봐야 하지 않을까요?"),
            pl(.init(line: "e0:s1u2"), "", "이면 결제사 한도에 걸리고, 중복 결제가 날 수 있어요."),
            pl(.init(summary: 0), "결제 실패 시 재시도를 ", "까지 하기로 했어요"),
            pl(.init(ledger: 0), "결제 실패 시 재시도는 ", "까지 해요."),
        ])
        let ledger = PanelProposal(id: "p2", kind: PanelProposal.editLedger, meetingId: a.id, changes: [
            .init(index: 1, was: "재시도 정책 문서 정리", owner: "서연", due: "금요일"),
            .init(index: 2, was: "부분 환불 수수료를 누가 낼지", remove: true),
        ], add: [.init(kind: "commitment", text: "결제사 한도 확인", owner: "지윤")])
        let say = "아래에서 바뀔 곳을 확인해 주세요. 원래 다섯 번이 맞는 곳은 체크를 풀면 돼요."
        func t(_ q: String, _ ps: [PanelProposal], states: [String: ProposalState]? = nil) -> PanelTurn {
            var x = turn(q, .chat(ChatAnswer(answer: say, cards: [], proposals: ps)))
            x.proposalStates = states
            return x
        }
        var phone = a
        phone.source = "iphone"
        phone.transcript = []
        let phoneFix = PanelProposal(id: "p3", kind: PanelProposal.fixWords, meetingId: a.id, heard: "다섯 번", meant: "세 번",
                                     places: [fix.places![2], fix.places![3]])
        let frames: [(String, PanelStore)] = [
            ("w1-fix", store(scope: here, turns: [t("다섯 번이 아니라 세 번이야", [fix])])),
            ("w2-applied", store(scope: here, turns: [t("다섯 번이 아니라 세 번이야", [fix], states: ["p1": .applied])])),
            ("w3-ledger", store(scope: here, turns: [t("문서는 서연 님이 금요일까지 해", [ledger])])),
        ]
        for (name, s) in frames {
            for dark in [false, true] {
                for scale in [1.0, 2.0] {
                    png(PanelView(store: s, meeting: lookup, scrolls: false), size: CGSize(width: 372, height: 760),
                        "\(name)-\(dark ? "dark" : "light")@\(Int(scale))x", dark: dark, scale: scale, to: out)
                }
            }
        }
        meetings[a.id] = phone
        png(PanelView(store: store(scope: here, turns: [t("다섯 번이 아니라 세 번이야", [phoneFix])]), meeting: lookup, scrolls: false),
            size: CGSize(width: 372, height: 560), "w4-iphone-light@2x", dark: false, scale: 2, to: out)
    }

    func testRenderHarness() async throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_PANEL_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let here = PanelScope.meeting(a.id)
        let mc = [card(a.id, kind: "meeting", title: "주간 결제 싱크", path: "meeting/\(a.id)"),
                  card(b.id, kind: "meeting", title: "결제 정책 논의", path: "meeting/\(b.id)"),
                  card("pay142", kind: "linear", title: "PAY-142 부분 환불 API")]
        let list = ChatAnswer(answer: """
        이번 달에 미뤄진 결정은 세 가지예요.

        - **부분 환불 수수료**: 누가 낼지 두 번의 회의에서 정하지 못했어요 [1][2]
        - **재시도 횟수 상한**: 결제사 한도를 확인한 뒤 다시 정하기로 했어요 [1]
        - **환불 API 공개 시점**: 수수료가 정해질 때까지 기다려요 [3]

        가장 오래 미뤄진 건 부분 환불 수수료예요 [2].
        """, cards: mc)
        let lead = ChatAnswer(answer: """
        먼저 챙길 것은 두 가지예요.

        1. 부분 환불 수수료를 누가 낼지 정해요 [1]
        2. 민호 님의 재시도 정책 문서가 나왔는지 확인해요 [2]
        """, cards: [], prep: prep())
        let now = Date()
        func inFlight(_ ago: TimeInterval, _ steps: [PanelStep]) -> PanelTurn {
            PanelTurn(id: UUID(), question: "이번 달에 미뤄진 결정 뭐 있어?", at: now.addingTimeInterval(-ago), steps: steps, body: nil)
        }
        let started: [PanelStep] = [.tool("start", detail: [:], count: nil)]
        let ledger = started + [.tool("filter_ledger", detail: ["kinds": "decision", "from": "2026-11-01", "to": "2026-11-30"], count: nil)]
        let transcript = ledger + [.tool("filter_ledger", detail: [:], count: 3),
                                   .tool("transcript_window", detail: ["meeting_id": a.id, "find": "환불 수수료 지윤"], count: nil)]
        let writing = transcript + [.tool("transcript_window", detail: [:], count: 12)]
        let frames: [(String, PanelStore)] = [
            ("h1-step-ledger", store(scope: here, turns: [inFlight(1.6, ledger)])),
            ("h2-step-transcript", store(scope: here, turns: [inFlight(3.4, transcript)])),
            ("h3-step-writing", store(scope: here, turns: [inFlight(5, writing)])),
            ("h4-list-answer", store(scope: .all, turns: [turn("이번 달에 미뤄진 결정 뭐 있어?", steps: writing, .chat(list))])),
            ("h5-prep-lead", store(scope: here, turns: [turn("다음 회의 준비해 줘", steps: [], .chat(lead))])),
            ("h6-chitchat", store(scope: .all, turns: [turn("고마워!", .chat(ChatAnswer(
                answer: "천만에요. 다음엔 \"지난주에 정한 거 뭐야?\"처럼 물어보셔도 돼요.", cards: [])))])),
        ]
        for (name, s) in frames {
            for dark in [false, true] {
                for scale in [1.0, 2.0] {
                    png(PanelView(store: s, meeting: lookup, scrolls: false), size: CGSize(width: 372, height: name.hasPrefix("h5") ? 720 : 520),
                        "\(name)-\(dark ? "dark" : "light")@\(Int(scale))x", dark: dark, scale: scale, to: out)
                }
            }
        }
    }

    func testRenderStates() async throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_PANEL_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let here = PanelScope.meeting(a.id)
        let why = WhyAnswer(reason: "결제사의 분당 요청 한도 때문이에요. 지윤 님이 다섯 번을 제안했지만[1], 민호 님이 중복 결제가 날 수 있다고 반대해서[2] 세 번으로 정했어요[3].",
                            keys: ["e0:s1u1", "e0:s1u2", "e0:s1u3"], errorKind: nil)
        let steps: [PanelStep] = [.meetings([a.id]), .related([b.id])]
        var live = SavedMeeting(mode: "inperson", start: day(11, 14, 13))
        live.title = "디자인 리뷰"
        meetings[live.id] = live
        let states: [(String, PanelStore)] = [
            ("1-empty-meeting", store(scope: here, turns: [])),
            ("2-empty-all", store(scope: .all, turns: [])),
            ("2b-empty-live-meeting", store(scope: .meeting(live.id), turns: [])),
            ("3-working", store(scope: here, turns: [turn("다음 회의 준비", steps: steps, nil)])),
            ("4-prep", store(scope: here, turns: [turn("다음 회의 준비", steps: steps + [.sources(2), .writing], .prep(prep()))])),
            ("5-why", store(scope: here, turns: [turn("\"결제 실패 시 재시도는 세 번까지 해요.\" 왜 이렇게 정했어요?", .why(why, meetingID: a.id))])),
            ("6-not-found", store(scope: here, turns: [turn("연간 할인은 몇 퍼센트로 했어요?",
                                                             .ask(PanelAsk(answer: "찾은 자료에는 답이 없어요.", cited: [], cards: [], errorKind: nil)))])),
            ("7-failed", store(scope: here, turns: [turn("다음 회의 준비", .failed(kind: "none")),
                                                   turn("다음 회의 준비", steps: steps, .prep(prep(error: "model")))])),
        ]
        for (name, s) in states {
            for dark in [false, true] {
                for scale in [1.0, 2.0] {
                    png(PanelView(store: s, meeting: lookup, scrolls: false), size: CGSize(width: 372, height: 760),
                        "\(name)-\(dark ? "dark" : "light")@\(Int(scale))x", dark: dark, scale: scale, to: out)
                }
            }
        }
        // the user's case: a long prep scrolled to the middle (its text passes under the head row), then the empty panel and a why answer
        let fixPrep = store(scope: here, turns: [turn("다음 회의 준비", steps: steps + [.sources(2), .writing], .prep(longPrep(c: longMeetingID()))),
                                                 turn("다음 회의 준비", steps: steps + [.sources(2), .writing], .prep(prep()))])
        for dark in [false, true] {
            let tag = dark ? "dark" : "light"
            png(PanelView(store: fixPrep, meeting: lookup, scrolls: false, renderScroll: 420), size: CGSize(width: 372, height: 640),
                "fix-prep-scrolled-\(tag)@2x", dark: dark, scale: 2, to: out)
            png(PanelView(store: store(scope: here, turns: []), meeting: lookup, scrolls: false), size: CGSize(width: 372, height: 640),
                "fix-empty-\(tag)@2x", dark: dark, scale: 2, to: out)
            png(PanelView(store: states.first { $0.0 == "5-why" }!.1, meeting: lookup, scrolls: false), size: CGSize(width: 372, height: 640),
                "fix-why-\(tag)@2x", dark: dark, scale: 2, to: out)
            png(PanelFace(size: .head).padding(6), size: CGSize(width: 38, height: 34), "fix-face-head-\(tag)@4x", dark: dark, scale: 4, to: out)
        }
        let long = longPrep(c: longMeetingID())
        for dark in [false, true] {
            for (name, expanded) in [("4b-prep-short", false), ("4c-prep-expanded", true)] {
                let v = PrepAnswerView(answer: long, steps: [.meetings([a.id]), .related([b.id])], meeting: lookup, actions: PanelActions(),
                                       calendar: cal, preparedID: a.id, expanded: expanded).padding(16)
                png(v, size: CGSize(width: 372, height: expanded ? 700 : 460), "\(name)-\(dark ? "dark" : "light")@1x", dark: dark, scale: 1, to: out)
            }
        }
        let hist = store(scope: here, turns: [])
        for q in ["다음 회의 준비", "가격 정책 어디서 정했지", "이번 달에 미뤄진 결정 뭐 있어?"] {
            hist.newThread()
            if q != "다음 회의 준비" { hist.pageChanged(to: nil) }
            hist.send(q, kind: .ask)
        }
        await hist.settle()
        hist.newThread()
        for dark in [false, true] {
            png(PanelView(store: hist, meeting: lookup, scrolls: false, showingList: true), size: CGSize(width: 372, height: 500),
                "9-history-\(dark ? "dark" : "light")@2x", dark: dark, scale: 2, to: out)
        }
        // a 900pt window: the sidebar, the page, and the panel over the page
        let s = store(scope: here, turns: [turn("다음 회의 준비", steps: steps + [.sources(2), .writing], .prep(prep()))])
        for dark in [false, true] {
            for scale in [1.0, 2.0] {
                let window = HStack(spacing: 0) {
                    FakeSidebar().frame(width: 300)
                    PanelLayout(open: true, close: {}) { FakePage() } panel: { PanelView(store: s, meeting: lookup, scrolls: false) }
                }
                png(window, size: CGSize(width: 900, height: 700), "10-narrow-overlay-\(dark ? "dark" : "light")@\(Int(scale))x",
                    dark: dark, scale: scale, to: out)
            }
        }
    }
}

/// Reads the page's width inside a layout.
@MainActor
private final class WidthProbe {
    var width: CGFloat?
    var view: some View {
        GeometryReader { g in
            let _ = { self.width = g.size.width }()
            Color.clear
        }
    }
}

/// Reads `panelOpen` inside a layout.
@MainActor
private final class OpenProbe {
    var open: Bool?
    var view: some View { Reader { self.open = $0 } }
    private struct Reader: View {
        let seen: (Bool) -> Void
        @Environment(\.panelOpen) private var open
        var body: some View {
            let _ = seen(open)
            Color.clear
        }
    }
}

private struct FakeSidebar: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(["주간 결제 싱크", "결제 정책 논의", "온보딩 개편 리뷰"], id: \.self) { t in
                Text(t).uiFont(13).foregroundStyle(Theme.ink)
            }
            Spacer()
        }
        .padding(.top, 52).padding(.horizontal, 20)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Theme.ink.opacity(0.03))
        .overlay(alignment: .trailing) { Rectangle().fill(Theme.hairline).frame(width: 1) }
    }
}

private struct FakePage: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("대면 회의 · 민호, 지윤, 서연").uiFont(12, weight: .semibold).foregroundStyle(Theme.ink3)
            Text("주간 결제 싱크").uiFont(24, weight: .bold).foregroundStyle(Theme.ink)
            Text("결제 재시도 횟수와 부분 환불 일정을 다뤘어요. 재시도는 세 번으로 정했고, 부분 환불은 수수료를 누가 낼지 정하지 못해 다음 주로 넘겼어요.")
                .uiFont(14).foregroundStyle(Theme.ink).lineSpacing(4).padding(.top, 16)
            Spacer()
        }
        .padding(.horizontal, 44).padding(.top, 60)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}
