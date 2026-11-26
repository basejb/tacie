import XCTest
@testable import Tacie

final class DueDateTests: XCTestCase {
    private let cal: Calendar = {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "Asia/Seoul")!
        c.firstWeekday = 2
        return c
    }()

    private func date(_ y: Int, _ m: Int, _ d: Int, _ h: Int = 0) -> Date {
        cal.date(from: DateComponents(year: y, month: m, day: d, hour: h))!
    }

    /// "M-d" of what `said` reads as, in a meeting on Tuesday 2026-10-27 at 14:00.
    private func read(_ said: String?, from start: Date? = nil) -> String? {
        DueDate.day(said, from: start ?? date(2026, 10, 27, 14), calendar: cal).map {
            "\(cal.component(.month, from: $0))-\(cal.component(.day, from: $0))"
        }
    }

    func testWeekdaysCountFromTheMeetingDay() {
        XCTAssertEqual(read("금요일까지"), "10-30")
        XCTAssertEqual(read("이번 주 금요일"), "10-30")
        XCTAssertEqual(read("다음 주 수요일"), "11-4")
        XCTAssertEqual(read("다다음 주 월요일"), "11-9")
        XCTAssertEqual(read("월요일까지"), "11-2")  // Monday has passed: the next one
        XCTAssertEqual(read("이번 주 월요일"), "10-26")  // said so: this week's, even past
    }

    func testWeeksWithoutADay() {
        XCTAssertEqual(read("이번 주 안에"), "10-30")
        XCTAssertEqual(read("다음 주 초"), "11-2")
        XCTAssertEqual(read("다음 주 중반"), "11-4")
        XCTAssertEqual(read("다음 주까지"), "11-6")
        XCTAssertEqual(read("주말까지"), "10-31")
        XCTAssertEqual(read("이번 주 안에", from: date(2026, 10, 31, 10)), "11-1")  // Saturday: Friday has passed
    }

    func testDaysAndDates() {
        XCTAssertEqual(read("오늘 중으로"), "10-27")
        XCTAssertEqual(read("퇴근 전에"), "10-27")
        XCTAssertEqual(read("내일"), "10-28")
        XCTAssertEqual(read("모레까지"), "10-29")
        XCTAssertEqual(read("3일 안에"), "10-30")
        XCTAssertEqual(read("2주 뒤"), "11-10")
        XCTAssertEqual(read("11월 7일까지"), "11-7")
        XCTAssertEqual(read("11/7"), "11-7")
        XCTAssertEqual(read("15일까지"), "11-15")  // the 15th has passed this month
        XCTAssertEqual(read("30일까지"), "10-30")
        XCTAssertEqual(read("월말까지"), "10-31")
        XCTAssertEqual(read("다음 달 초"), "11-1")
        XCTAssertEqual(read("다음 달 중순"), "11-15")
    }

    func testJanuarySaidInDecemberIsNextYear() {
        let d = DueDate.day("1월 5일", from: date(2026, 12, 20), calendar: cal)!
        XCTAssertEqual(cal.component(.year, from: d), 2027)
    }

    func testUnreadableStaysWithoutADay() {
        XCTAssertNil(read(nil))
        XCTAssertNil(read(""))
        XCTAssertNil(read("다음 분기"))
        XCTAssertNil(read("배포 전에"))
        XCTAssertNil(read("2월 30일"))
    }

    func testBuckets() {
        let now = date(2026, 11, 2, 9)  // Monday
        XCTAssertEqual(DueBucket(date(2026, 10, 30), now: now, calendar: cal), .overdue)
        XCTAssertEqual(DueBucket(date(2026, 11, 2), now: now, calendar: cal), .today)
        XCTAssertEqual(DueBucket(date(2026, 11, 8), now: now, calendar: cal), .week)
        XCTAssertEqual(DueBucket(date(2026, 11, 9), now: now, calendar: cal), .later)
        XCTAssertEqual(DueBucket(nil, now: now, calendar: cal), .none)
        XCTAssertEqual(DueBucket.days(to: date(2026, 10, 30), now: now, calendar: cal), -3)
    }

    func testTodosByDueAndFinishedOrder() {
        func meeting(_ day: Int, _ ledger: [LedgerEntry]) -> SavedMeeting {
            var m = SavedMeeting(mode: "remote", start: date(2026, 10, day, 10))
            m.end = date(2026, 10, day, 11)
            m.ledger = ledger
            return m
        }
        let old = meeting(27, [
            LedgerEntry(kind: "commitment", text: "금요일 일", due: "금요일까지"),
            LedgerEntry(kind: "decision", text: "정한 것"),
            LedgerEntry(kind: "commitment", text: "끝낸 일", done: true, doneAt: date(2026, 10, 28)),
        ])
        let new = meeting(29, [
            LedgerEntry(kind: "commitment", text: "기한 없는 일"),
            LedgerEntry(kind: "commitment", text: "예전에 끝낸 일", done: true),
            LedgerEntry(kind: "commitment", text: "방금 끝낸 일", done: true, doneAt: date(2026, 10, 30)),
        ])
        let all = Collected.allTodos([old, new], calendar: cal)
        XCTAssertEqual(all.map(\.entry.text), ["기한 없는 일", "예전에 끝낸 일", "방금 끝낸 일", "금요일 일", "끝낸 일"])
        let open = all.filter { !$0.entry.done }
        let groups = Collected.byDue(open, now: date(2026, 11, 2), calendar: cal)
        XCTAssertEqual(groups.map(\.bucket), [.overdue, .none])
        XCTAssertEqual(groups.map { $0.todos.map(\.entry.text) }, [["금요일 일"], ["기한 없는 일"]])
        XCTAssertEqual(Collected.finished(all.filter(\.entry.done)).map(\.entry.text), ["방금 끝낸 일", "끝낸 일", "예전에 끝낸 일"])
    }
}
