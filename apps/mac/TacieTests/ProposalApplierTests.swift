import XCTest

/// The apply rules both apps share (`ProposalApplier`, docs/40 6-4): a card's change into the meeting, the notes
/// written again and the word remembered when chosen, and Undo.
@MainActor
final class ProposalApplierTests: XCTestCase {
    private var m = SavedMeeting(mode: "inperson", start: Date(timeIntervalSince1970: 1_700_000_000))
    private var rewrote: [String] = []
    private var remembered: [String] = []

    override func setUp() {
        m = SavedMeeting(mode: "inperson", start: Date(timeIntervalSince1970: 1_700_000_000))
        m.end = m.start.addingTimeInterval(3600)
        m.notes = .done
        m.title = "은퇴 관리 개편"
        m.voices = [VoiceEntry(key: "화자 1", name: "지윤")]
        m.transcript = [
            TranscriptLine(key: "e0:s1u1", at: m.start, speaker: nil, text: "은퇴 기록은 은퇴 시스템에서 봐요", question: false, voice: "화자 1"),
            TranscriptLine(key: "e0:s1u2", at: m.start, speaker: nil, text: "은퇴 처리는 금요일까지요", question: false, voice: "화자 2"),
        ]
        m.summary = ["[[화자 1]]이 은퇴 기록을 정리했어요"]
        m.topics = [NoteTopic(title: "은퇴 시스템", points: ["은퇴 기록 보기"])]
        m.ledger = [LedgerEntry(kind: "commitment", text: "은퇴 기록 정리", owner: "화자 1"),
                    LedgerEntry(kind: "decision", text: "다음 주 배포")]
        rewrote = []
        remembered = []
    }

    private func applier() -> ProposalApplier {
        ProposalApplier(update: { [unowned self] id, change in if id == m.id { change(&m) } },
                        rewrite: { [unowned self] in rewrote.append($0) },
                        remember: { [unowned self] in remembered.append($0) },
                        canRewrite: { !$0.transcript.isEmpty && $0.end != nil },
                        place: .iphoneMeeting)
    }

    private func place(_ at: PanelProposal.At, _ nth: Int) -> PanelProposal.Place {
        PanelProposal.Place(at: at, nth: nth, before: "", word: "은퇴", after: "", to: "근태")
    }

    private func fix(_ places: [PanelProposal.Place]) -> PanelProposal {
        PanelProposal(id: "p1", kind: PanelProposal.fixWords, meetingId: m.id, heard: "은퇴", meant: "근태", places: places)
    }

    func testFixAppliesChosenPlacesAndRewrites() throws {
        let p = fix([place(.init(line: "e0:s1u1"), 0), place(.init(line: "e0:s1u1"), 1), place(.init(line: "e0:s1u2"), 0),
                     place(.init(summary: 0), 0)])
        // the person unchecked the second line
        let chosen = Array(p.places![0...1]) + [p.places![3]]
        let undo = try XCTUnwrap(applier().apply(p, choice: .init(places: chosen, rewrite: true, remember: true)))
        XCTAssertEqual(m.transcript[0].text, "근태 기록은 근태 시스템에서 봐요")
        XCTAssertEqual(m.transcript[0].heard, "은퇴 기록은 은퇴 시스템에서 봐요")
        XCTAssertEqual(m.transcript[1].text, "은퇴 처리는 금요일까지요")
        XCTAssertEqual(m.summary, ["[[화자 1]]이 근태 기록을 정리했어요"])
        XCTAssertEqual(rewrote, [m.id])
        XCTAssertEqual(remembered, ["근태"])
        XCTAssertTrue(undo.rewrote)
        XCTAssertEqual(undo.notes?.summary, ["[[화자 1]]이 은퇴 기록을 정리했어요"])
    }

    /// The meeting AI's rewrite_notes carries what the person asked ("한글로 다시 요약해줘"): the rewrite gets it, as on the Mac.
    func testRewriteNotesPassesTheRequest() throws {
        var asked: [(String, String?)] = []
        var a = applier()
        a.rewriteAsked = { asked.append(($0, $1)) }
        let p = PanelProposal(id: "p1", kind: PanelProposal.rewriteNotes, meetingId: m.id, request: "한글로 다시 요약해줘")
        let undo = try XCTUnwrap(a.apply(p, choice: nil))
        XCTAssertEqual(asked.map(\.0), [m.id])
        XCTAssertEqual(asked.map(\.1), ["한글로 다시 요약해줘"])
        XCTAssertEqual(rewrote, [])
        XCTAssertTrue(undo.rewrote)
    }

    func testFixWithNoTranscriptPlacesSkipsRewrite() throws {
        let p = fix([place(.init(topic: 0), 0), place(.init(topic: 0, point: 0), 0)])
        let undo = try XCTUnwrap(applier().apply(p, choice: .init(places: p.places!, rewrite: true, remember: false)))
        XCTAssertEqual(m.topics, [NoteTopic(title: "근태 시스템", points: ["근태 기록 보기"])])
        XCTAssertEqual(rewrote, [])  // no transcript line changed: the notes stay as they are
        XCTAssertEqual(remembered, [])
        XCTAssertFalse(undo.rewrote)
        // nothing for a meeting still going on
        m.end = nil
        let again = fix([place(.init(line: "e0:s1u2"), 0)])
        XCTAssertNil(applier().apply(again, choice: .init(places: again.places!, rewrite: true, remember: true)))
        XCTAssertEqual(m.transcript[1].text, "은퇴 처리는 금요일까지요")
    }

    func testLedgerChangesAndUndo() throws {
        let before = m.ledger
        let p = PanelProposal(id: "p2", kind: PanelProposal.editLedger, meetingId: m.id,
                              changes: [.init(index: 0, was: "은퇴 기록 정리", owner: "민호", done: true),
                                        .init(index: 1, was: "다른 말", remove: true)],  // the entry reads otherwise now: left as it is
                              add: [.init(kind: "commitment", text: "보고하기", due: "금요일")])
        let a = applier()
        let undo = try XCTUnwrap(a.apply(p, choice: nil))
        XCTAssertEqual(m.ledger.count, 3)
        XCTAssertEqual(m.ledger[0].done, true)
        XCTAssertEqual(m.ledger[0].edited, true)
        XCTAssertEqual(m.ledger[1].text, "다음 주 배포")
        XCTAssertEqual(m.ledger[2].text, "보고하기")
        XCTAssertTrue(a.canUndo(undo, in: m))
        a.undo(p, undo)
        XCTAssertEqual(m.ledger, before)
    }

    func testNameSpeakerRenamesVoiceOnly() throws {
        m.voices = [VoiceEntry(key: "화자 1", name: "지윤", person: nil)]
        let summary = m.summary
        let p = PanelProposal(id: "p3", kind: PanelProposal.nameSpeaker, meetingId: m.id, speaker: "지윤", name: "한결", key: "화자 1")
        let a = applier()
        let undo = try XCTUnwrap(a.apply(p, choice: nil))
        XCTAssertEqual(m.displayName(key: "화자 1"), "한결")
        XCTAssertNil(m.voices?.first { $0.key == "화자 1" }?.person)  // no remembered person on the iPhone
        XCTAssertEqual(m.summary, summary)  // the notes keep the key: they show the new name by themselves
        XCTAssertEqual(m.title, "은퇴 관리 개편")
        // a speaker heard but never named takes the name too, by its key
        let two = PanelProposal(id: "p4", kind: PanelProposal.nameSpeaker, meetingId: m.id, speaker: "화자 2", name: "민호")
        XCTAssertNotNil(a.apply(two, choice: nil))
        XCTAssertEqual(m.displayName(key: "화자 2"), "민호")
        // 되돌리기 puts the first name back while nothing else renamed the speaker since
        XCTAssertFalse(a.canUndo(undo, in: m))
        a.undo(p, undo)
        XCTAssertEqual(m.displayName(key: "화자 1"), "한결")
    }

    func testNameSpeakerUndoPutsTheNameBack() throws {
        let before = m.voices
        let p = PanelProposal(id: "p5", kind: PanelProposal.nameSpeaker, meetingId: m.id, speaker: "지윤", name: "한결", key: "화자 1")
        let a = applier()
        let undo = try XCTUnwrap(a.apply(p, choice: nil))
        XCTAssertEqual(m.displayName(key: "화자 1"), "한결")
        XCTAssertTrue(a.canUndo(undo, in: m))  // nothing renamed the speaker since
        a.undo(p, undo)
        XCTAssertEqual(m.voices, before)
        XCTAssertEqual(m.displayName(key: "화자 1"), "지윤")
    }

    func testTitleRenameMarksTitleEdited() throws {
        let p = PanelProposal(id: "t", kind: PanelProposal.renameTitle, meetingId: m.id, title: " 근태 관리 개편 ", was: "은퇴 관리 개편")
        let a = applier()
        let undo = try XCTUnwrap(a.apply(p, choice: nil))
        XCTAssertEqual(m.title, "근태 관리 개편")
        XCTAssertTrue(m.titleEdited)
        a.undo(p, undo)
        XCTAssertEqual(m.title, "은퇴 관리 개편")
        XCTAssertFalse(m.titleEdited)
        XCTAssertNil(a.apply(PanelProposal(id: "e", kind: PanelProposal.renameTitle, meetingId: m.id, title: "  "), choice: nil))
    }

    func testUndoRestoresWhenUntouched() throws {
        let p = fix([place(.init(line: "e0:s1u1"), 0), place(.init(line: "e0:s1u2"), 0)])
        let a = applier()
        let undo = try XCTUnwrap(a.apply(p, choice: .init(places: p.places!, rewrite: false, remember: false)))
        XCTAssertEqual(rewrote, [])
        // the person edited the second line since: it stays theirs
        m.transcript[1].text = "근태 처리는 목요일까지요"
        XCTAssertTrue(a.canUndo(undo, in: m))
        a.undo(p, undo)
        XCTAssertEqual(m.transcript[0].text, "은퇴 기록은 은퇴 시스템에서 봐요")
        XCTAssertNil(m.transcript[0].heard)
        XCTAssertEqual(m.transcript[1].text, "근태 처리는 목요일까지요")
        // not while the notes are being written again
        m.notes = .waiting
        XCTAssertFalse(a.canUndo(undo, in: m))
    }
}
