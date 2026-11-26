import XCTest

final class CalendarLinkTests: XCTestCase {
    private let t = Date(timeIntervalSince1970: 1_800_000_000)

    private func event(_ id: String, start: TimeInterval, length: TimeInterval = 1800, allDay: Bool = false,
                       texts: [String] = [], title: String = "주간 제품 회의") -> CalendarEvent {
        CalendarEvent(id: id, title: title, start: t + start, end: t + start + length, allDay: allDay, texts: texts, invited: [])
    }

    func testSkipsAllDayAndEnded() {
        let events = [event("all", start: 0, length: 86_400, allDay: true), event("over", start: -1860, length: 1800)]
        XCTAssertNil(CalendarLink.pick(events, at: t, app: nil))
    }

    func testWindowIsTenMinutes() {
        XCTAssertEqual(CalendarLink.pick([event("a", start: 599)], at: t, app: nil)?.id, "a")
        XCTAssertNil(CalendarLink.pick([event("b", start: 601)], at: t, app: nil))
        XCTAssertEqual(CalendarLink.pick([event("c", start: -599)], at: t, app: nil)?.id, "c")
        // started long ago but still running: not a meeting starting now
        XCTAssertNil(CalendarLink.pick([event("d", start: -3000, length: 7200)], at: t, app: nil))
    }

    func testPrefersLinkOfStartingApp() {
        let zoom = event("zoom", start: 0, texts: ["https://us02web.zoom.us/j/1"])
        let teams = event("teams", start: 0, texts: ["https://teams.microsoft.com/l/meetup-join/x"])
        XCTAssertEqual(CalendarLink.pick([zoom, teams], at: t, app: .teams)?.id, "teams")
        XCTAssertEqual(CalendarLink.pick([teams, zoom], at: t, app: .zoom)?.id, "zoom")
    }

    func testThenAnyVideoLink() {
        let plain = event("plain", start: 60)
        let meet = event("meet", start: 300, texts: ["Join: https://meet.google.com/abc-defg-hij"])
        XCTAssertEqual(CalendarLink.pick([plain, meet], at: t, app: nil)?.id, "meet")
    }

    func testThenClosestStart() {
        XCTAssertEqual(CalendarLink.pick([event("early", start: -300), event("soon", start: 120)], at: t, app: nil)?.id, "soon")
    }

    func testBrowserAppMatchesMeet() {
        let zoom = event("zoom", start: 0, texts: ["https://zoom.us/j/2"])
        let meet = event("meet", start: 0, texts: ["https://MEET.google.com/x"])
        XCTAssertEqual(CalendarLink.pick([zoom, meet], at: t, app: .browser)?.id, "meet")
    }

    func testHost() {
        XCTAssertEqual(CalendarLink.host(of: event("a", start: 0, texts: ["https://teams.live.com/meet/1"])), .teams)
        XCTAssertNil(CalendarLink.host(of: event("b", start: 0, texts: ["회의실 3층"])))
    }

    func testMeaningful() {
        XCTAssertFalse(CalendarLink.meaningful("Zoom 회의"))
        XCTAssertFalse(CalendarLink.meaningful(" meeting "))
        XCTAssertFalse(CalendarLink.meaningful("제목 없음"))
        XCTAssertFalse(CalendarLink.meaningful(""))
        XCTAssertTrue(CalendarLink.meaningful("주간 제품 회의"))
    }
}
