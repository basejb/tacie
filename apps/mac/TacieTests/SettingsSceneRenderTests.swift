import XCTest
import SwiftUI
@testable import Tacie

/// The scene of Settings → Meetings drawn to PNG for a look, one file per row it explains, light and dark. Only with
/// TACIE_SETTINGS_SCENE_RENDER_DIR (passed as TEST_RUNNER_TACIE_SETTINGS_SCENE_RENDER_DIR). Nothing opens on screen.
@MainActor
final class SettingsSceneRenderTests: XCTestCase {
    func testRenderMeetingScene() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_SETTINGS_SCENE_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let focuses: [(String, MeetingSettingsScene.Focus?)] = [
            ("00-none", nil), ("01-mode", .mode), ("02-sound", .sound), ("03-prompt", .prompt), ("04-notice", .notice),
            ("05-music", .music), ("06-translation", .translation), ("07-notes", .notes), ("08-openAfter", .openAfter),
            ("09-record", .record), ("10-people", .people), ("11-keep", .keep),
        ]
        for (name, focus) in focuses {
            for scheme in [ColorScheme.light, .dark] {
                let view = VStack(alignment: .leading, spacing: 0) {
                    PageHeader(title: SettingsPane.meeting.title, desc: String(localized: "회의를 시작할 때부터 끝난 뒤까지, 순서대로 정해요."))
                        .padding(.bottom, -12)
                    MeetingSettingsScene(focus: focus)
                    SettingsSection(title: String(localized: "회의를 시작할 때"), step: 1, lit: focus?.step == 1) {
                        SettingsRow(title: String(localized: "회의 모드"), desc: String(localized: "기록만은 회의 중에 답 카드를 띄우지 않고 녹음, 대화 기록, 회의 정리만 해요.")) {
                            ChoiceStrip(choices: [(false, String(localized: "답 카드")), (true, String(localized: "기록만"))], value: .constant(false))
                        }
                    }
                    .padding(.top, 26)
                }
                .padding(.horizontal, 48).padding(.top, 34)
                .frame(width: 640, height: 470, alignment: .top)
                .background(Theme.paper)
                .environment(\.colorScheme, scheme)
                // Theme's colours read AppearanceMode.isDark, not the SwiftUI colour scheme
                AppearanceMode.isDark = scheme == .dark
                defer { AppearanceMode.isDark = false }
                let r = ImageRenderer(content: view)
                r.scale = 2
                let image = r.nsImage
                guard let tiff = image?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
                      let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
                try data.write(to: out.appendingPathComponent("\(name)-\(scheme == .dark ? "dark" : "light").png"))
            }
        }
    }

    func testRenderConnectionsScene() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_SETTINGS_SCENE_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let some: [ConnectionsScene.Source: Bool] = [.github: true, .notion: true, .linear: true, .notes: true,
                                                     .gdrive: false, .atlassian: false, .asana: false, .dropbox: false]
        let none = some.mapValues { _ in false }
        let states: [(String, [ConnectionsScene.Source: Bool], ConnectionsScene.Source?, String?)] = [
            ("c0-some", some, nil, nil), ("c1-github", some, .github, nil), ("c2-asana", some, .asana, nil),
            ("c3-sync", some, nil, "동기화하는 중 · Linear 이슈 받는 중 40%"), ("c4-none", none, nil, nil),
        ]
        for (name, on, focus, syncing) in states {
            for scheme in [ColorScheme.light, .dark] {
                let view = VStack(alignment: .leading, spacing: 0) {
                    PageHeader(title: SettingsPane.connections.title, desc: String(localized: "연결한 서비스에서 답을 찾아요. 내 계정으로 볼 수 있는 것만 가져와요."))
                        .padding(.bottom, -12)
                    ConnectionsScene(on: on, focus: focus, syncing: syncing)
                }
                .padding(.horizontal, 48).padding(.top, 34)
                .frame(width: 640, height: 330, alignment: .top)
                .background(Theme.paper)
                .environment(\.colorScheme, scheme)
                AppearanceMode.isDark = scheme == .dark
                defer { AppearanceMode.isDark = false }
                let r = ImageRenderer(content: view)
                r.scale = 2
                guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
                      let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
                try data.write(to: out.appendingPathComponent("\(name)-\(scheme == .dark ? "dark" : "light").png"))
            }
        }
    }
}
