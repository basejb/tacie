import XCTest

/// Decisions that changed one from an earlier meeting (docs/30 8절).
final class DecisionTrailTests: XCTestCase {
    private lazy var cal: Calendar = {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "Asia/Seoul")!
        return c
    }()

    private func meeting(day: Int, title: String, decisions: [String], notes: SavedMeeting.Notes = .done) -> SavedMeeting {
        let start = cal.date(from: DateComponents(year: 2026, month: 10, day: day, hour: 10))!
        var m = SavedMeeting(mode: "remote", start: start)
        m.end = start.addingTimeInterval(1800)
        m.title = title
        m.notes = notes
        m.ledger = decisions.map { LedgerEntry(kind: "decision", text: $0) }
            + [LedgerEntry(kind: "commitment", text: "할 일은 비교하지 않아요")]
        return m
    }

    func test_past_is_earlier_finished_meetings_newest_first() {
        let now = meeting(day: 30, title: "지금", decisions: ["재시도를 5번으로 늘림"])
        let a = meeting(day: 10, title: "킥오프", decisions: ["배포는 금요일"])
        let b = meeting(day: 20, title: "결제 정리", decisions: ["재시도는 3번까지", "다크 모드는 나중에"])
        let later = meeting(day: 31, title: "다음", decisions: ["나중 결정"])
        let failed = meeting(day: 25, title: "실패", decisions: ["정리 안 됨"], notes: .failed)
        let past = DecisionTrail.past(before: now, in: [a, now, later, failed, b], calendar: cal)
        XCTAssertEqual(past.map(\.text), ["재시도는 3번까지", "다크 모드는 나중에", "배포는 금요일"])
        XCTAssertEqual(past.first?.title, "결제 정리")
        XCTAssertEqual(past.first?.meeting, b.id)
    }

    func test_mark_sets_changes_and_clears_the_rest() {
        var m = meeting(day: 30, title: "지금", decisions: ["재시도를 5번으로 늘림", "다크 모드부터"])
        let was = ReplacedDecision(text: "재시도는 3번까지", meeting: "x", title: "결제 정리", at: Date(timeIntervalSince1970: 0))
        m.ledger[1].replaces = was
        DecisionTrail.mark(&m, ["재시도를 5번으로 늘림": was])
        XCTAssertEqual(m.ledger[0].replaces, was)
        XCTAssertNil(m.ledger[1].replaces)
    }

    func test_digest_and_markdown_carry_the_earlier_decision() throws {
        var m = meeting(day: 30, title: "지금", decisions: ["재시도를 5번으로 늘림"])
        let at = cal.date(from: DateComponents(year: 2026, month: 10, day: 20, hour: 10))!
        m.ledger[0].replaces = ReplacedDecision(text: "재시도는 3번까지", meeting: "x", title: "결제 정리", at: at)
        let d = try XCTUnwrap(m.digest(calendar: cal))
        XCTAssertEqual(d.ledger[0].replaces, MeetingDigest.Replaced(text: "재시도는 3번까지", at: "2026-10-20", title: "결제 정리"))
        XCTAssertNil(d.ledger[1].replaces)
        let entries = try XCTUnwrap(d.json["ledger"] as? [[String: Any]])
        XCTAssertNil(entries[1]["replaces"])
        XCTAssertTrue(m.markdown(calendar: cal).contains("- 재시도를 5번으로 늘림 (이전 결정: 재시도는 3번까지 · 10월 20일 결제 정리)"))
    }

    func test_old_files_without_the_field_still_read() throws {
        let json = #"{"kind":"decision","text":"카드 결제만","done":false}"#
        let e = try JSONDecoder().decode(LedgerEntry.self, from: Data(json.utf8))
        XCTAssertNil(e.replaces)
    }
}
