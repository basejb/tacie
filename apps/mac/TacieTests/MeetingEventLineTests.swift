import SwiftUI
import XCTest

@MainActor
final class MeetingEventLineTests: XCTestCase {
    private var cal: Calendar = {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "Asia/Seoul")!
        c.locale = Locale(identifier: "ko_KR")
        return c
    }()

    private func at(_ h: Int, _ m: Int = 0, day: Int = 14) -> Date {
        cal.date(from: DateComponents(year: 2026, month: 11, day: day, hour: h, minute: m))!
    }

    func testText() {
        let e = MeetingEvent(id: "e", title: "주간 제품 회의", start: at(14), end: at(15))
        XCTAssertEqual(MeetingEventLine.text(e, invited: 5, calendar: cal), "일정 오후 2:00–3:00 · 초대된 사람 5명")
        XCTAssertEqual(MeetingEventLine.text(e, invited: 0, calendar: cal), "일정 오후 2:00–3:00")
        XCTAssertEqual(MeetingEventLine.span(MeetingEvent(id: "e", title: "", start: at(11, 30), end: at(12, 30)), calendar: cal),
                       "오전 11:30–오후 12:30")
        XCTAssertEqual(MeetingEventLine.span(MeetingEvent(id: "e", title: "", start: at(23), end: at(1, day: 15)), calendar: cal),
                       "오후 11:00–11월 15일 오전 1:00")
    }

    func testChangingEventKeepsUserTitle() {
        var m = SavedMeeting(mode: "remote", start: at(14)).renamed("내 제목")
        m.link(MeetingEvent(id: "a", title: "주간 제품 회의", start: at(14), end: at(15)), invited: [], meaningful: true)
        m.link(MeetingEvent(id: "b", title: "채용 인터뷰", start: at(16), end: at(17)), invited: [], meaningful: true)
        XCTAssertEqual(m.title, "내 제목")
        var n = SavedMeeting(mode: "remote", start: at(14))
        n.link(MeetingEvent(id: "a", title: "주간 제품 회의", start: at(14), end: at(15)), invited: [], meaningful: true)
        n.link(MeetingEvent(id: "b", title: "Zoom 회의", start: at(14), end: at(15)), invited: [], meaningful: false)
        XCTAssertNil(n.title)  // the old event's title left with it; the notes give one again
    }

    private func png<V: View>(_ view: V, width: CGFloat, _ name: String, dark: Bool, to out: URL) {
        AppearanceMode.isDark = dark
        defer { AppearanceMode.isDark = false }
        let framed = view.frame(width: width, alignment: .leading).padding(20).background(dark ? Color(white: 0.11) : Theme.paper)
            .environment(\.colorScheme, dark ? .dark : .light)
        let r = ImageRenderer(content: framed)
        r.scale = 2
        guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
        try? data.write(to: out.appendingPathComponent("\(name)\(dark ? "-dark" : "").png"))
    }

    func testRender() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_CALENDAR_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let e = MeetingEvent(id: "b", title: "주간 제품 회의", start: at(14), end: at(15))
        let day = [CalendarEvent(id: "a", title: "1:1 지윤", start: at(11), end: at(11, 30), allDay: false, texts: [], invited: []),
                   CalendarEvent(id: "b", title: "주간 제품 회의", start: at(14), end: at(15), allDay: false, texts: [], invited: []),
                   CalendarEvent(id: "c", title: "채용 인터뷰", start: at(16), end: at(16, 30), allDay: false, texts: [], invited: [])]
        for dark in [false, true] {
            png(VStack(alignment: .leading, spacing: 8) {
                Text("주간 제품 회의").uiFont(26, weight: .bold).foregroundStyle(Theme.ink)
                MeetingEventLine(event: e, invited: 5, calendar: cal)
            }, width: 600, "line", dark: dark, to: out)
            png(MeetingEventMenu(day: at(14), linked: "b", events: day, calendar: cal), width: 300, "event-menu", dark: dark, to: out)
            png(MeetingEventMenu(day: at(14), linked: "b", events: [], calendar: cal), width: 300, "event-menu-empty", dark: dark, to: out)
        }
    }
}
