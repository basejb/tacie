import XCTest

/// The panel's write proposals (docs/38 12절): reading the engine's `done`, putting a fix or a ledger change into the
/// meeting, and putting it back.
@MainActor
final class PanelProposalTests: XCTestCase {
    private func meeting() -> SavedMeeting {
        var m = SavedMeeting(mode: "inperson", start: Date(timeIntervalSince1970: 1_700_000_000))
        m.end = m.start.addingTimeInterval(3600)
        m.notes = .done
        m.title = "은퇴 관리 개편"
        m.voices = [VoiceEntry(key: "화자 1", name: "지윤")]
        m.transcript = [
            TranscriptLine(key: "e0:s1u1", at: m.start, speaker: nil, text: "은퇴 기록은 은퇴 시스템에서 봐요", question: false, voice: "화자 1"),
            TranscriptLine(key: "e0:s1u2", at: m.start, speaker: nil, text: "아버지 은퇴 얘기는 나중에요", question: false, voice: "화자 1"),
        ]
        m.summary = ["[[화자 1]]이 은퇴 기록을 정리했어요"]
        m.topics = [NoteTopic(title: "은퇴 시스템", points: ["은퇴 기록 보기"])]
        m.ledger = [LedgerEntry(kind: "commitment", text: "은퇴 기록 정리", owner: "화자 1"),
                    LedgerEntry(kind: "decision", text: "다음 주 배포")]
        return m
    }

    private func place(_ at: PanelProposal.At, _ nth: Int, word: String = "은퇴", to: String = "근태") -> PanelProposal.Place {
        PanelProposal.Place(at: at, nth: nth, before: "", word: word, after: "", to: to)
    }

    func testDecodesProposalsFromTheDoneLine() throws {
        let json = """
        {"answer":"아래에서 확인해 주세요","cards":[],"proposals":[
          {"id":"p1","slot":"fix","kind":"fix_words","meeting_id":"m1","heard":"은퇴","meant":"근태","lines":1,"notes":1,
           "places":[{"at":{"line":"e0:s1u1"},"nth":0,"before":"","word":"은퇴","after":" 기록","to":"근태"},
                     {"at":{"topic":0,"point":0},"nth":0,"before":"","word":"은퇴","after":"","to":"근태"}]},
          {"id":"p2","kind":"edit_ledger","meeting_id":"m1","changes":[{"index":0,"was":"은퇴 기록 정리","owner":"민호","done":true}],
           "add":[{"kind":"commitment","text":"보고하기","due":"금요일"}]},
          {"id":"p3","kind":"fix_words"}
        ]}
        """
        let a = try EngineClient.engineDecoder.decode(ChatAnswer.self, from: Data(json.utf8))
        XCTAssertEqual(a.proposals.map(\.id), ["p1", "p2"])  // the one that does not read is left out
        XCTAssertEqual(a.proposals[0].places?[1].at, PanelProposal.At(topic: 0, point: 0))
        XCTAssertEqual(a.proposals[1].changes?.first?.owner, "민호")
        // and it survives the panel's own history file
        let saved = try JSONDecoder().decode(ChatAnswer.self, from: JSONEncoder().encode(a))
        XCTAssertEqual(saved, a)
    }

    func testFixChangesTheChosenPlacesAndKeepsWhatWasHeard() {
        var m = meeting()
        let places = [place(.init(line: "e0:s1u1"), 0), place(.init(line: "e0:s1u1"), 1),
                      place(.init(summary: 0), 0), place(.init(topic: 0), 0), place(.init(topic: 0, point: 0), 0),
                      place(.init(ledger: 0), 0), place(.init(title: true), 0)]
        let (undo, changed) = ProposalApply.fix(&m, places: places)
        XCTAssertEqual(changed, 7)
        XCTAssertEqual(m.transcript[0].text, "근태 기록은 근태 시스템에서 봐요")
        XCTAssertEqual(m.transcript[0].heard, "은퇴 기록은 은퇴 시스템에서 봐요")
        XCTAssertEqual(m.transcript[1].text, "아버지 은퇴 얘기는 나중에요")  // left out: really retirement
        XCTAssertNil(m.transcript[1].heard)
        XCTAssertEqual(m.summary, ["[[화자 1]]이 근태 기록을 정리했어요"])  // the mark stays a mark
        XCTAssertEqual(m.topics, [NoteTopic(title: "근태 시스템", points: ["근태 기록 보기"])])
        XCTAssertEqual(m.ledger[0].text, "근태 기록 정리")
        XCTAssertEqual(m.title, "근태 관리 개편")
        XCTAssertTrue(m.titleEdited)

        ProposalApply.undo(&m, undo)
        XCTAssertEqual(m, meeting())
    }

    func testAPlaceNoLongerThereLeavesItsTextAlone() {
        var m = meeting()
        let (_, changed) = ProposalApply.fix(&m, places: [place(.init(line: "e0:s1u2"), 1), place(.init(line: "gone"), 0)])
        XCTAssertEqual(changed, 0)
        XCTAssertEqual(m, meeting())
    }

    func testMiddleDotOfTheRecordIsKept() {
        var m = meeting()
        m.transcript[0].text = "환․환불 유예"
        _ = ProposalApply.fix(&m, places: [place(.init(line: "e0:s1u1"), 0, word: "환․환불", to: "환․반품")])
        XCTAssertEqual(m.transcript[0].text, "환․반품 유예")
    }

    func testUndoLeavesWhatWasChangedAgain() {
        var m = meeting()
        let (undo, _) = ProposalApply.fix(&m, places: [place(.init(line: "e0:s1u1"), 0), place(.init(summary: 0), 0)])
        m.summary = ["손으로 고친 요약"]
        ProposalApply.undo(&m, undo)
        XCTAssertEqual(m.transcript[0].text, meeting().transcript[0].text)
        XCTAssertEqual(m.summary, ["손으로 고친 요약"])
    }

    func testLedgerChangesOnlyWhileTheWordsAreAsRead() {
        var m = meeting()
        let changes = [PanelProposal.Change(index: 0, was: "은퇴 기록 정리", owner: "지윤", due: "금요일", done: true),
                       PanelProposal.Change(index: 1, was: "예전 글", remove: true)]
        let (undo, changed) = ProposalApply.ledger(&m, changes: changes, add: [PanelProposal.Add(kind: "commitment", text: "보고하기")])
        XCTAssertEqual(changed, 2)
        XCTAssertEqual(m.ledger.count, 3)
        XCTAssertEqual(m.ledger[0].owner, "화자 1")  // a speaker's name goes back to the speaker's key
        XCTAssertEqual(m.ledger[0].due, "금요일")
        XCTAssertTrue(m.ledger[0].done)
        XCTAssertEqual(m.ledger[0].edited, true)
        XCTAssertEqual(m.ledger[1].text, "다음 주 배포")  // `was` did not match: not removed
        XCTAssertEqual(m.ledger[2].text, "보고하기")
        ProposalApply.undo(&m, undo)
        XCTAssertEqual(m.ledger, meeting().ledger)
    }

    func testStoreAppliesFixRewritesAndRemembers() {
        var m = meeting()
        var rewrote: [String] = [], remembered: [String] = []
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let store = PanelStore(engine: { nil }, meetings: { [m] id in id == m.id ? m : nil }, folder: { dir },
                               update: { _, change in change(&m) },
                               rewrite: { rewrote.append($0) }, remember: { remembered.append($0) })
        let p = PanelProposal(id: "p1", kind: PanelProposal.fixWords, meetingId: m.id, heard: "은퇴", meant: "근태",
                              places: [place(.init(line: "e0:s1u1"), 0)])
        let turn = PanelTurn(id: UUID(), question: "은퇴는 근태야", at: Date(), steps: [],
                             body: .chat(ChatAnswer(answer: "아래에서 확인해 주세요", cards: [], proposals: [p])))
        store.current = PanelThread(id: UUID(), title: "t", origin: .meeting(m.id), turns: [turn])
        store.applyFix(p, turn: turn.id, choice: .init(places: p.places!, rewrite: true, remember: true))
        XCTAssertEqual(m.transcript[0].text, "근태 기록은 은퇴 시스템에서 봐요")
        XCTAssertEqual(rewrote, [m.id])
        XCTAssertEqual(remembered, ["근태"])
        XCTAssertEqual(store.current?.turns[0].proposalStates?["p1"], .applied)
    }

    func testTermsKeepTheNewestOnce() throws {
        let space = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: space) }
        MeetingTerms.remember("근태", in: space)
        MeetingTerms.remember("공매", in: space)
        MeetingTerms.remember("근태", in: space)
        XCTAssertEqual(MeetingTerms.words(space), ["근태", "공매"])
    }

    func testMergeAndNameGoToTheSpeakersKeys() {
        var m = meeting()
        m.voices = [VoiceEntry(key: "화자 1", name: "서연"), VoiceEntry(key: "화자 3"), VoiceEntry(key: "화자 5")]
        var merged: [(String, String)] = [], named: [(String, String)] = []
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let store = PanelStore(engine: { nil }, meetings: { [m] id in id == m.id ? m : nil }, folder: { dir },
                               nameSpeaker: { _, k, n in named.append((k, n)) }, mergeSpeaker: { _, k, into in merged.append((k, into)) })
        let merge = PanelProposal(id: "p1", kind: PanelProposal.mergeSpeakers, meetingId: m.id, lines: 3,
                                  from: [.init(key: "화자 3", name: "화자 3"), .init(key: "화자 9", name: "화자 9")],
                                  into: .init(key: "화자 1", name: "서연"))
        XCTAssertEqual(store.mergeKeys(merge)?.from, ["화자 3"])  // a speaker the meeting no longer has is left out
        // a speaker heard but never named has no voices entry: it still merges
        var heard = m
        heard.transcript[1].voice = "화자 5"
        let s5 = PanelStore(engine: { nil }, meetings: { [heard] id in id == heard.id ? heard : nil }, folder: { dir })
        let five = PanelProposal(id: "p5", kind: PanelProposal.mergeSpeakers, meetingId: m.id,
                                 from: [.init(key: "화자 5", name: "화자 5")], into: .init(key: "화자 1", name: "서연"))
        XCTAssertEqual(s5.mergeKeys(five)?.from, ["화자 5"])
        let turn = UUID()
        store.applyMerge(merge, turn: turn)
        XCTAssertEqual(merged.map(\.0), ["화자 3"])
        XCTAssertEqual(merged.map(\.1), ["화자 1"])
        // a name points at the speaker by its key, or by the name it shows now
        let byName = PanelProposal(id: "p2", kind: PanelProposal.nameSpeaker, meetingId: m.id, speaker: "서연", name: "한결")
        XCTAssertEqual(store.speakerKey(byName), "화자 1")
        store.applyName(byName, turn: turn)
        XCTAssertEqual(named.first?.0, "화자 1")
    }

    func testTheChatRequestCarriesEachLinesSpeakerKey() {
        var m = meeting()
        m.voices = [VoiceEntry(key: "화자 1", name: "지윤"), VoiceEntry(key: "화자 2", same: "화자 1")]
        m.transcript[1].voice = "화자 2"
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let store = PanelStore(engine: { nil }, meetings: { [m] id in id == m.id ? m : nil }, folder: { dir })
        let r = store.chatRequest(.ask, text: "q", scope: .meeting(m.id), history: [])
        XCTAssertEqual(r.transcripts[m.id]?.map { $0["voice"] }, ["화자 1", "화자 1"])  // a merged speaker reads as the one kept
    }

    func testTitleRewriteKindAndMoveGoToTheirPaths() {
        var m = meeting()
        m.voices = [VoiceEntry(key: "화자 1", name: "지윤"), VoiceEntry(key: "화자 2")]
        var rewrote: [(String, String?)] = [], kinds: [NotesKind] = [], moved: [([String], String?, String)] = []
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let store = PanelStore(engine: { nil }, meetings: { id in id == m.id ? m : nil }, folder: { dir },
                               update: { _, change in change(&m) },
                               moveLines: { _, lines, to, name in moved.append((lines, to, name)) },
                               rewriteAsked: { rewrote.append(($0, $1)) }, changeKind: { kinds.append($1) })
        let turn = UUID()
        let title = PanelProposal(id: "t", kind: PanelProposal.renameTitle, meetingId: m.id, title: "근태 관리 개편", was: "은퇴 관리 개편")
        store.applyTitle(title, turn: turn)
        XCTAssertEqual(m.title, "근태 관리 개편")
        XCTAssertTrue(m.titleEdited)
        XCTAssertTrue(store.canUndo(title, turn: turn))
        store.undo(title, turn: turn)
        XCTAssertEqual(m.title, "은퇴 관리 개편")

        let ask = PanelProposal(id: "r", kind: PanelProposal.rewriteNotes, meetingId: m.id, request: "요약을 더 자세히")
        store.applyRewrite(ask, turn: turn)
        XCTAssertEqual(rewrote.first?.1, "요약을 더 자세히")
        let kind = PanelProposal(id: "k", kind: PanelProposal.changeKind, meetingId: m.id, notesKind: "interview")
        store.applyRewrite(kind, turn: turn)
        XCTAssertEqual(kinds, [.interview])

        let move = PanelProposal(id: "m", kind: PanelProposal.moveLines, meetingId: m.id,
                                 moved: [.init(key: "e0:s1u2"), .init(key: "gone")], target: .init(key: "화자 2", name: "화자 2"))
        XCTAssertEqual(store.movedKeys(move), ["e0:s1u2"])
        store.applyMove(move, turn: turn)
        XCTAssertEqual(moved.first?.0, ["e0:s1u2"])
        XCTAssertEqual(moved.first?.1, "화자 2")
        let newName = PanelProposal(id: "n", kind: PanelProposal.moveLines, meetingId: m.id,
                                    moved: [.init(key: "e0:s1u1")], target: .init(name: "민호"))
        store.applyMove(newName, turn: turn)
        XCTAssertNil(moved.last?.1)
        XCTAssertEqual(moved.last?.2, "민호")
    }

    func testRewriteUndoBringsTheNotesAndKindBack() {
        var m = meeting()
        m.kind = .meeting
        let before = m
        let u = ProposalUndo(notes: ProposalUndo.Notes(m), rewrote: true)
        m.summary = ["다시 쓴 요약"]
        m.kind = .interview
        ProposalApply.undo(&m, u)
        XCTAssertEqual(m.summary, before.summary)
        XCTAssertEqual(m.kind, .meeting)
    }

    func testAssignGivesEachChosenLineToItsSpeaker() {
        var m = meeting()
        m.voices = [VoiceEntry(key: "화자 1", name: "지윤"), VoiceEntry(key: "화자 2", name: "민호"), VoiceEntry(key: "화자 9")]
        m.transcript[0].voice = "화자 9"
        m.transcript[1].voice = "화자 9"
        m.transcript.append(TranscriptLine(key: "e0:s1u3", at: m.start, speaker: nil, text: "네", question: false, voice: "화자 2"))
        var moved: [([String], String?)] = []
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let store = PanelStore(engine: { nil }, meetings: { [m] id in id == m.id ? m : nil }, folder: { dir },
                               moveLines: { _, lines, to, _ in moved.append((lines, to)) })
        let p = PanelProposal(id: "a", kind: PanelProposal.assignLines, meetingId: m.id, assigned: [
            .init(key: "e0:s1u1", to: .init(key: "화자 1", name: "지윤")),
            .init(key: "e0:s1u2", to: .init(key: "화자 2", name: "민호")),
            .init(key: "gone", to: .init(key: "화자 1", name: "지윤")),
        ])
        store.applyAssign(p, turn: UUID(), keys: ["e0:s1u1", "e0:s1u2", "gone"])
        XCTAssertEqual(Set(moved.map { $0.0 }), [["e0:s1u1"], ["e0:s1u2"]])
        XCTAssertEqual(Set(moved.compactMap { $0.1 }), ["화자 1", "화자 2"])
        moved = []
        // a line left unchecked stays
        store.applyAssign(p, turn: UUID(), keys: ["e0:s1u2"])
        XCTAssertEqual(moved.map { $0.0 }, [["e0:s1u2"]])
    }
}
