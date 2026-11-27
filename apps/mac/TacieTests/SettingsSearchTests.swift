import XCTest
import SwiftUI
@testable import Tacie

/// Settings search: which rows a query finds, and (with TACIE_SETTINGS_SEARCH_RENDER_DIR, passed as
/// TEST_RUNNER_TACIE_SETTINGS_SEARCH_RENDER_DIR) the sidebar drawn to PNG for a look. Nothing opens on screen.
@MainActor
final class SettingsSearchTests: XCTestCase {
    private let entries: [SettingsSearchEntry] = [
        .init(pane: .general, title: "테마", words: ["다크", "dark"]),
        .init(pane: .general, title: "회의 모드", words: ["답 카드"]),
        .init(pane: .privacy, title: "회의 녹음", words: ["녹음", "record"]),
        .init(pane: .privacy, title: "회의 기록 삭제", words: ["삭제"]),
        .init(pane: .privacy, title: "녹음 없는 줄", words: []),
    ]

    private func titles(_ q: String) -> [String] { SettingsSearch.matches(q, in: entries).map(\.title) }

    func testOtherWordsFindTheRow() {
        XCTAssertEqual(titles("다크"), ["테마"])
        XCTAssertEqual(titles("DARK"), ["테마"])
    }

    func testSearchFindsCalendar() {
        XCTAssertEqual(SettingsSearch.matches("일정", in: SettingsSearch.entries).first?.title, "캘린더")
        XCTAssertEqual(SettingsSearch.matches("calendar", in: SettingsSearch.entries).first?.pane, .meeting)
    }

    func testEveryWordMustMatch() {
        XCTAssertEqual(titles("회의 삭제"), ["회의 기록 삭제"])
    }

    func testTitleMatchesComeFirst() {
        // "녹음" is in 회의 녹음's title and in its words; 녹음 없는 줄 has it only in the title. Both are title matches,
        // kept in list order; a words-only match would come after them.
        XCTAssertEqual(titles("녹음"), ["회의 녹음", "녹음 없는 줄"])
        XCTAssertEqual(titles("record"), ["회의 녹음"])
    }

    func testPaneNameFindsItsRows() {
        XCTAssertEqual(Set(titles(SettingsPane.privacy.title)), ["회의 녹음", "회의 기록 삭제", "녹음 없는 줄"])
    }

    func testOpenEditionLeavesOutConnectionsItDoesNotOffer() {
        let all: [SettingsSearchEntry] = [
            .init(pane: .connections, title: "Google Drive", anchor: .top, words: ["드라이브"], source: .gdrive),
            .init(pane: .connections, title: "Linear", anchor: .top, source: .linear),
            .init(pane: .connections, title: "지금 동기화", anchor: .top),
        ] + SettingsSearch.openAI
        let open = SettingsSearch.shown(all, panes: SettingsPane.panes(hasAccount: false),
                                        sources: ConnectionsScene.Source.available(open: true, dev: false))
        XCTAssertEqual(SettingsSearch.matches("드라이브", in: open), [])
        XCTAssertEqual(SettingsSearch.matches("Linear", in: open).map(\.title), ["Linear"])
        XCTAssertEqual(SettingsSearch.matches("지금 동기화", in: open).count, 1)
        XCTAssertEqual(SettingsSearch.matches("Soniox", in: open).first?.pane, .ai)
        XCTAssertEqual(SettingsSearch.matches("Ollama", in: open).first?.pane, .ai)
        let official = SettingsSearch.shown(all, panes: SettingsPane.panes(hasAccount: true),
                                            sources: ConnectionsScene.Source.available(open: false, dev: false))
        XCTAssertEqual(SettingsSearch.matches("드라이브", in: official).map(\.title), ["Google Drive"])
        XCTAssertEqual(SettingsSearch.matches("Soniox", in: official).first?.pane, .ai)  // one app: AI is always a pane (#56)
    }

    func testBlankFindsNothing() {
        XCTAssertEqual(titles("   "), [])
    }

    func testCatalogHasNoDuplicates() {
        let ids = SettingsSearch.entries.map(\.id)
        XCTAssertEqual(ids.count, Set(ids).count)
    }

    func testRenderSidebar() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_SETTINGS_SEARCH_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let all = SettingsSearch.entries
        for (name, q, current) in [("1-dark", "다크", 0), ("2-meeting", "회의", 1), ("3-none", "없는말", 0)] {
            for scheme in [ColorScheme.light, .dark] {
                let view = VStack(alignment: .leading, spacing: 2) {
                    SettingsSearchResults(results: SettingsSearch.matches(q, in: all), current: current, pick: { _ in })
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 10).padding(.top, 14)
                .frame(width: 214, height: 360, alignment: .top)
                .background(Theme.settingsSidebar)
                .environment(\.colorScheme, scheme)
                let r = ImageRenderer(content: view)
                r.scale = 2
                guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
                      let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
                try data.write(to: out.appendingPathComponent("\(name)-\(scheme == .dark ? "dark" : "light").png"))
            }
        }
    }
}
