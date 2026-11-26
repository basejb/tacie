import XCTest

/// A past meeting found as an answer (docs/30): its card opens that meeting in the main window.
@MainActor
final class MeetingCardTests: XCTestCase {
    private func card(_ path: String, kind: String) -> Card {
        Card(id: "1", path: path, kind: kind, source: kind, byline: "회의 · 2026-11-03", url: "", lineStart: 1, lineEnd: 1,
             title: "결제 화면 정리", snippet: "결정: 카드 결제만 먼저 내요", score: 0.9, githubUrl: nil, localPath: nil, chunk: 7,
             repeat: nil)
    }

    override func tearDown() async throws {
        MeetingCards.openMeeting = nil
        MeetingCards.meetingExists = nil
    }

    func test_meeting_card_id_from_path() {
        let m = card("meeting/20261103-140210", kind: "meeting")
        XCTAssertEqual(m.meetingID, "20261103-140210")
        XCTAssertNil(card("notion/abc", kind: "notion").meetingID)
        XCTAssertNil(card("meeting/x", kind: "docs").meetingID, "a repo folder named meeting is a file")
        XCTAssertNil(m.logo)
        XCTAssertEqual(m.symbol, "person.2")
        XCTAssertEqual(m.opensIn, "메인 창")
        XCTAssertNil(m.link)
    }

    func test_open_meeting_card_opens_main_window() {
        var opened: [String] = []
        MeetingCards.openMeeting = { opened.append($0) }
        MeetingCards.meetingExists = { _ in true }
        _ = MeetingCards.open(card("meeting/20261103-140210", kind: "meeting"))
        XCTAssertEqual(opened, ["20261103-140210"])
    }

    func test_open_deleted_meeting_does_nothing() {
        var opened: [String] = []
        MeetingCards.openMeeting = { opened.append($0) }
        MeetingCards.meetingExists = { _ in false }
        let c = card("meeting/20261103-140210", kind: "meeting")
        XCTAssertTrue(MeetingCards.gone(c))
        _ = MeetingCards.open(c)
        XCTAssertEqual(opened, [])
        XCTAssertFalse(MeetingCards.gone(card("notion/abc", kind: "notion")))
    }

    func test_meeting_is_a_card_source() {
        XCTAssertEqual(ACardSource(rawValue: "meeting"), .meeting)
    }
}
