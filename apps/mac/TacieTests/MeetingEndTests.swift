import XCTest
@testable import Tacie

final class MeetingEndTests: XCTestCase {
    private let t0 = Date(timeIntervalSince1970: 1_000_000)

    func testSecondsLeftRoundsUpAndStopsAtZero() {
        let deadline = t0.addingTimeInterval(MeetingEnd.grace)
        XCTAssertEqual(MeetingEnd.secondsLeft(until: deadline, now: t0), 10)
        XCTAssertEqual(MeetingEnd.secondsLeft(until: deadline, now: t0.addingTimeInterval(3.2)), 7)
        XCTAssertEqual(MeetingEnd.secondsLeft(until: deadline, now: t0.addingTimeInterval(9.99)), 1)
        XCTAssertEqual(MeetingEnd.secondsLeft(until: deadline, now: t0.addingTimeInterval(12)), 0)
    }

    func testLineCountsDown() {
        let deadline = t0.addingTimeInterval(MeetingEnd.grace)
        XCTAssertEqual(MeetingEnd.line(until: deadline, now: t0), "회의를 끝낼까요? 10초 뒤 정리해요")
        XCTAssertEqual(MeetingEnd.line(until: deadline, now: t0.addingTimeInterval(6.5)), "회의를 끝낼까요? 4초 뒤 정리해요")
    }

    /// 2026-11-09: people talked on after "여기까지 하죠" and the meeting ended under them. Talk said after the closing
    /// words keeps it going; closing words again, a backchannel and lines said before do not. Values from the replay test.
    func testTalkAfterTheClosingWordsKeepsTheMeetingGoing() {
        let from = t0
        let after = t0.addingTimeInterval(2)
        // the closer's own announcement after the closing words read 0.44 in the replay test; a farewell 0.71
        XCTAssertTrue(MeetingEnd.keepsGoing(cutAt: after, closingFrom: from, pEnd: 0.44, backchannel: false))
        XCTAssertTrue(MeetingEnd.keepsGoing(cutAt: after, closingFrom: from, pEnd: 0.71, backchannel: false))
        XCTAssertTrue(MeetingEnd.keepsGoing(cutAt: after, closingFrom: from, pEnd: nil, backchannel: false))
        // "네, 수고하셨습니다" 0.89: closing words again
        XCTAssertFalse(MeetingEnd.keepsGoing(cutAt: after, closingFrom: from, pEnd: 0.89, backchannel: false))
        XCTAssertFalse(MeetingEnd.keepsGoing(cutAt: after, closingFrom: from, pEnd: nil, backchannel: true))
        // the closing words' own pieces, and lines cut before them but heard back later
        XCTAssertFalse(MeetingEnd.keepsGoing(cutAt: from, closingFrom: from, pEnd: 0.05, backchannel: false))
        XCTAssertFalse(MeetingEnd.keepsGoing(cutAt: t0.addingTimeInterval(-1), closingFrom: from, pEnd: 0.05, backchannel: false))
    }

    func testTheDeadlineWaitsForWordsStillBeingSaid() {
        let from = t0
        let deadline = t0.addingTimeInterval(MeetingEnd.grace)
        let now = deadline.addingTimeInterval(1)
        XCTAssertFalse(MeetingEnd.holds(speaking: false, inFlight: [], closingFrom: from, now: now, deadline: deadline))
        XCTAssertTrue(MeetingEnd.holds(speaking: true, inFlight: [], closingFrom: from, now: now, deadline: deadline))
        XCTAssertTrue(MeetingEnd.holds(speaking: false, inFlight: [t0.addingTimeInterval(9)], closingFrom: from,
                                       now: now, deadline: deadline))
        // an utterance cut before the closing words cannot keep the meeting going, so it is not waited for
        XCTAssertFalse(MeetingEnd.holds(speaking: false, inFlight: [t0.addingTimeInterval(-1)], closingFrom: from,
                                        now: now, deadline: deadline))
        // not forever
        XCTAssertFalse(MeetingEnd.holds(speaking: true, inFlight: [], closingFrom: from,
                                        now: deadline.addingTimeInterval(MeetingEnd.holdMax), deadline: deadline))
    }

    func testHeardDecodesEndingFromOlderAndNewerEngines() throws {
        let newer = try EngineClient.engineDecoder.decode(Heard.self, from: Data(#"""
        {"utterance_id":"s1u3","text":"오늘은 여기까지 하죠","fired":false,"p_lookup":0.02,"p_end":0.95,"ending":true}
        """#.utf8))
        XCTAssertEqual(newer.ending, true)
        XCTAssertEqual(newer.pEnd, 0.95)
        let older = try EngineClient.engineDecoder.decode(Heard.self, from: Data(#"""
        {"utterance_id":"s1u3","text":"오늘은 여기까지 하죠","fired":false,"p_lookup":0.02}
        """#.utf8))
        XCTAssertNil(older.ending)
    }

    func testOpensOnlyForMeetings() {
        XCTAssertTrue(NotesFlow.opensAfterNotes(asked: false, kind: .meeting, hasLedger: true))
        XCTAssertFalse(NotesFlow.opensAfterNotes(asked: false, kind: .interview, hasLedger: true))
        XCTAssertFalse(NotesFlow.opensAfterNotes(asked: false, kind: .talk, hasLedger: true))
        XCTAssertFalse(NotesFlow.opensAfterNotes(asked: true, kind: .meeting, hasLedger: true))
        XCTAssertFalse(NotesFlow.opensAfterNotes(asked: false, kind: .meeting, hasLedger: false))
    }

    func testChangeKindMarksThePersonsPickAndWaits() {
        var m = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 1_792_000_000))
        m.notes = .done
        m.kind = .meeting
        m.kindSource = "auto"
        XCTAssertFalse(NotesFlow.markKindChange(&m, to: .meeting))  // the same kind: nothing to write
        XCTAssertEqual(m.notes, .done)
        XCTAssertTrue(NotesFlow.markKindChange(&m, to: .interview))
        XCTAssertEqual(m.kind, .interview)
        XCTAssertEqual(m.kindSource, "user")
        XCTAssertEqual(m.notes, .waiting)
        // the rewrite failing keeps the picked kind, so 다시 쓰기 writes that kind
        m.notes = .failed
        XCTAssertEqual(NotesFlow.rewriteKind(m), .interview)
        m.kindSource = "auto"
        XCTAssertNil(NotesFlow.rewriteKind(m))
    }

    /// Review finding 1–2: the person's lines go once they confirm, and the old kind's notes do not stay under the new
    /// headings (an empty reply would otherwise keep them).
    func testChangeKindDropsTheOldNotesAndThePersonsLines() {
        var m = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 1_792_000_000))
        m.notes = .done
        m.summary = ["회의 요약."]
        m.topics = [NoteTopic(title: "주제", points: ["말함."])]
        m.ledger = [LedgerEntry(kind: "commitment", text: "공지 쓰기", edited: true),
                    LedgerEntry(kind: "decision", text: "금요일 배포로 정함.")]
        XCTAssertTrue(NotesFlow.markKindChange(&m, to: .interview))
        XCTAssertEqual(m.ledger, [])
        XCTAssertEqual(m.summary, [])
        XCTAssertEqual(m.topics, [])
        // the reply comes back as the picked kind: nothing of the person's comes back with it
        m.apply(title: nil, summary: ["인터뷰 요약."], entries: [LedgerEntry(kind: "fact", text: "30억 원임.")],
                kind: .interview, kindSource: "user")
        XCTAssertEqual(m.ledger.map(\.text), ["30억 원임."])
    }

    /// Review finding 3: two picks in a row; the reply for the earlier pick is not kept.
    func testAReplyForAnEarlierPickIsStale() {
        var m = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 1_792_000_000))
        m.kind = .talk
        m.kindSource = "user"
        XCTAssertTrue(NotesFlow.isStale(m, replyKind: "interview"))
        XCTAssertFalse(NotesFlow.isStale(m, replyKind: "talk"))
        m.kindSource = "auto"
        XCTAssertFalse(NotesFlow.isStale(m, replyKind: "interview"))  // the engine picked: any reply is the answer
    }

    /// Checked to-dos count as the person's work: changing the kind asks first.
    func testCheckedToDosAskBeforeAKindChange() {
        var m = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 1_792_000_000))
        m.ledger = [LedgerEntry(kind: "commitment", text: "QA 끝내기")]
        XCTAssertFalse(NotesFlow.hasPersonsWork(m))
        m.ledger[0].done = true
        XCTAssertTrue(NotesFlow.hasPersonsWork(m))
    }
}
