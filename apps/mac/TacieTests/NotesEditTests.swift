import XCTest

/// People in the notes' prose follow their speaker's name, and what the person changed in the ledger survives Rewrite.
final class NotesEditTests: XCTestCase {
    private let base = Date(timeIntervalSince1970: 1_792_000_000)

    private func line(_ n: Int, _ voice: String?, name: String? = nil) -> TranscriptLine {
        TranscriptLine(key: "e0:s1u\(n)", at: base + Double(n), speaker: nil, text: "말", question: false, voice: voice, name: name)
    }

    private func meeting() -> SavedMeeting {
        var m = SavedMeeting(mode: "inperson", start: base)
        m.transcript = [line(1, "화자 1"), line(2, "화자 2"), line(3, "화자 1", name: "지수")]
        m.rename(key: "화자 1", to: "민재", person: nil)
        return m
    }

    func testNamesInTheSummaryBecomeKeysAndShowTheNameOfNow() {
        var m = meeting()
        m.apply(title: "t", summary: ["[[민재]]가 [[화자 2]]에게 넘김. [[지수]]도 봄. [[하은]]은 없음."],
                topics: [NoteTopic(title: "a", points: ["[[민재]]가 꺼냄."])], entries: [])
        XCTAssertEqual(m.summary, ["[[화자 1]]가 [[화자 2]]에게 넘김. [[@지수]]도 봄. 하은은 없음."])
        XCTAssertEqual(m.topics?.first?.points, ["[[화자 1]]가 꺼냄."])
        XCTAssertEqual(m.shownSummary, ["민재가 화자 2에게 넘김. 지수도 봄. 하은은 없음."])

        // naming the speaker later renames them in the summary, the topics, the copy and the search
        m.rename(key: "화자 2", to: "지윤", person: nil)
        m.rename(key: "화자 1", to: "민호", person: nil)
        XCTAssertEqual(m.shownSummary, ["민호가 지윤에게 넘김. 지수도 봄. 하은은 없음."])
        XCTAssertEqual(m.shownTopics?.first?.points, ["민호가 꺼냄."])
        let md = m.markdown(calendar: Calendar(identifier: .gregorian))
        XCTAssertTrue(md.contains("민호가 지윤에게 넘김."), md)
        XCTAssertFalse(md.contains("[["), md)
        XCTAssertTrue(m.matches("지윤에게"))
    }

    func testParticlesFitTheNameShownNow() {
        var m = meeting()
        m.apply(title: nil, summary: ["[[화자 2]]가 꺼냄. [[화자 2]]는 [[화자 1]]과 [[화자 1]]으로 감. [[화자 2]]를 봄. [[화자 2]] 이번에."], entries: [])
        XCTAssertEqual(m.shownSummary, ["화자 2가 꺼냄. 화자 2는 민재와 민재로 감. 화자 2를 봄. 화자 2 이번에."])
        m.rename(key: "화자 2", to: "지안", person: nil)
        m.rename(key: "화자 1", to: "민철", person: nil)
        XCTAssertEqual(m.shownSummary, ["지안이 꺼냄. 지안은 민철과 민철로 감. 지안을 봄. 지안 이번에."])
        m.rename(key: "화자 1", to: "Alex", person: nil)
        XCTAssertEqual(m.shownSummary, ["지안이 꺼냄. 지안은 Alex과 Alex으로 감. 지안을 봄. 지안 이번에."])
    }

    func testNotesWithoutRefsStayAsTheyAre() {
        var m = meeting()
        m.apply(title: nil, summary: ["배포는 금요일로 정함."], entries: [])
        XCTAssertEqual(m.summary, ["배포는 금요일로 정함."])
        XCTAssertEqual(m.shownSummary, m.summary)
    }

    func testRewriteKeepsWhatThePersonChangedOrAdded() {
        var m = meeting()
        m.apply(title: nil, summary: ["s"], entries: [
            LedgerEntry(kind: "commitment", text: "QA 끝내기", owner: "화자 2"),
            LedgerEntry(kind: "decision", text: "금요일 배포로 정함."),
        ])
        m.ledger[0].owner = "화자 1"
        m.ledger[0].edited = true
        m.ledger.append(LedgerEntry(kind: "commitment", text: "공지 쓰기", edited: true))

        m.apply(title: nil, summary: ["s2"], entries: [
            LedgerEntry(kind: "commitment", text: "QA 끝내기", owner: "화자 2"),
            LedgerEntry(kind: "decision", text: "배포는 금요일로 정함."),
        ])
        XCTAssertEqual(m.ledger.map(\.text), ["배포는 금요일로 정함.", "QA 끝내기", "공지 쓰기"])
        XCTAssertEqual(m.ledger.first { $0.text == "QA 끝내기" }?.owner, "화자 1")
        XCTAssertEqual(m.ledger.filter { $0.edited == true }.count, 2)
    }

    func testKindChangeDropsEditedLines() {
        var m = meeting()
        m.apply(title: nil, summary: ["s"], entries: [LedgerEntry(kind: "commitment", text: "QA 끝내기")],
                kind: .meeting, kindSource: "auto")
        m.ledger.append(LedgerEntry(kind: "commitment", text: "공지 쓰기", edited: true))
        // the same kind again: what the person wrote stays
        m.apply(title: nil, summary: ["s"], entries: [], kind: .meeting, kindSource: "auto")
        XCTAssertEqual(m.ledger.map(\.text), ["공지 쓰기"])
        // another kind: the sections differ, the person's lines go
        m.apply(title: nil, summary: ["s"], entries: [LedgerEntry(kind: "fact", text: "거래액 30억 원임.")],
                kind: .interview, kindSource: "user")
        XCTAssertEqual(m.ledger.map(\.text), ["거래액 30억 원임."])
        XCTAssertEqual(m.kind, .interview)
        XCTAssertEqual(m.kindSource, "user")
    }

    func testOlderFilesReadWithoutTheEditedMark() throws {
        let json = #"{"kind": "commitment", "text": "PR 올리기", "owner": "화자 1", "done": false}"#
        let e = try JSONDecoder().decode(LedgerEntry.self, from: Data(json.utf8))
        XCTAssertNil(e.edited)
    }

    func testSearchFindsSpeakerNamesInQuotes() {
        var m = meeting()
        m.apply(title: nil, summary: ["s"], entries: [LedgerEntry(kind: "quote", text: "[[민재]]: 직접 해 보는 게 빨랐어요.")],
                kind: .interview, kindSource: "auto")
        XCTAssertEqual(m.ledger.first?.text, "[[화자 1]]: 직접 해 보는 게 빨랐어요.")
        XCTAssertTrue(m.matches("민재: 직접"))
    }
}
