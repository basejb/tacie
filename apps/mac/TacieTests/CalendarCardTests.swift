import SwiftUI
import XCTest

@MainActor
final class CalendarCardTests: XCTestCase {
    func testLines() {
        XCTAssertEqual(CalendarStatus(access: .off, chosen: nil, total: 0).line, "회의를 시작하면 지금 열리는 일정의 제목과 초대된 사람을 채워요")
        XCTAssertNil(CalendarStatus(access: .off, chosen: nil, total: 0).note)
        XCTAssertEqual(CalendarStatus(access: .granted, chosen: nil, total: 5).line, "모든 캘린더")
        XCTAssertEqual(CalendarStatus(access: .granted, chosen: 5, total: 5).line, "모든 캘린더")
        XCTAssertEqual(CalendarStatus(access: .granted, chosen: 2, total: 5).line, "캘린더 2개")
        XCTAssertEqual(CalendarStatus(access: .granted, chosen: 2, total: 5).note,
                       "일정은 이 Mac 밖으로 보내지 않아요. 초대된 사람의 이름만 받아쓰기에 보내요.")
        XCTAssertEqual(CalendarStatus(access: .granted, chosen: nil, total: 0).line, "캘린더 앱에 계정이 없어요")
        XCTAssertEqual(CalendarStatus(access: .denied, chosen: nil, total: 0).line, "캘린더 접근이 허용되지 않았어요")
        XCTAssertTrue(CalendarStatus(access: .granted, chosen: nil, total: 1).on)
        XCTAssertFalse(CalendarStatus(access: .denied, chosen: nil, total: 1).on)
    }

    private func png<V: View>(_ view: V, width: CGFloat, _ name: String, dark: Bool, to out: URL) {
        AppearanceMode.isDark = dark
        defer { AppearanceMode.isDark = false }
        let framed = view.frame(width: width).padding(20).background(dark ? Color(white: 0.11) : Theme.fill)
            .environment(\.colorScheme, dark ? .dark : .light)
        let r = ImageRenderer(content: framed)
        r.scale = 2
        guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
        try? data.write(to: out.appendingPathComponent("\(name)\(dark ? "-dark" : "").png"))
    }

    func testColourFromCGColor() {
        XCTAssertEqual(CalendarInfo.rgb(CGColor(srgbRed: 1, green: 0x29 / 255.0, blue: 0x68 / 255.0, alpha: 1)), 0xFF2968)
        XCTAssertNil(CalendarInfo.rgb(nil))
    }

    func testRenderStates() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_CALENDAR_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let states: [(String, CalendarStatus)] = [
            ("off", .init(access: .off, chosen: nil, total: 0)), ("all", .init(access: .granted, chosen: nil, total: 5)),
            ("some", .init(access: .granted, chosen: 2, total: 5)), ("none", .init(access: .granted, chosen: nil, total: 0)),
            ("denied", .init(access: .denied, chosen: nil, total: 0)),
        ]
        // the Calendar app's default colours
        let cals = [CalendarInfo(id: "1", title: "개인", account: "iCloud", color: 0x1BADF8),
                    CalendarInfo(id: "2", title: "가족", account: "iCloud", color: 0x63DA38),
                    CalendarInfo(id: "3", title: "업무", account: "minho@company.com", color: 0xFF2968),
                    CalendarInfo(id: "4", title: "팀 공유 일정", account: "minho@company.com", color: 0xCC73E1),
                    CalendarInfo(id: "5", title: "휴가", account: "minho@company.com", color: 0xFF9500)]
        for dark in [false, true] {
            for (name, s) in states { png(CalendarCardBody(status: s), width: 560, "card-\(name)", dark: dark, to: out) }
            png(CalendarPickerBody(calendars: cals, on: .constant(["3", "4"])), width: 420, "picker", dark: dark, to: out)
            // the row in 설정 → 일반 → 회의, between its neighbours
            for (name, st) in states {
                png(SettingsSection(title: "회의") {
                    SettingsRow(title: "회의 시작 알림", desc: "회의 앱이 마이크를 쓰기 시작하면 화면 위 Tacie가 같이 들을지 물어요. 누르기 전에는 듣지 않아요.") {
                        InkToggle(on: .constant(true))
                    }
                    SettingsRow(title: "알림 받을 앱", desc: "Meet처럼 브라우저로 하는 회의는 브라우저로 알아채요.") { MenuText(label: "모든 회의 앱") }
                    CalendarRowBody(status: st)
                    SettingsRow(title: "회의 정리 자동 열기", desc: "회의가 끝나면 메인 창에서 그 회의를 열어요.") { InkToggle(on: .constant(true)) }
                }, width: 640, "row-\(name)", dark: dark, to: out)
            }
        }
    }
}
