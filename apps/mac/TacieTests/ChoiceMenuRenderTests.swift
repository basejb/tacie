import AppKit
import SwiftUI
import XCTest

/// Renders the open edition's Choose providers rows with `ChoiceMenu` to PNGs, offscreen, so the menus' right edges
/// can be checked by eye. Runs only when `TACIE_CHOICE_RENDER_DIR` names a folder.
@MainActor
final class ChoiceMenuRenderTests: XCTestCase {
    private struct Rows: View {
        let en: Bool
        @State var stt = "openrouter"
        @State var chat = "other"
        @State var cards = "off"
        var body: some View {
            VStack(alignment: .leading, spacing: 0) {
                SettingsRow(title: en ? "Speech recognition" : "음성 인식", desc: en ? "Turns meeting audio into text." : "회의 소리를 글로 바꿔요.") {
                    ChoiceMenu(value: $stt, items: [("openrouter", "OpenRouter"), ("device", en ? "This Mac" : "이 Mac"),
                                                    ("soniox", "Soniox"), ("elevenlabs", "ElevenLabs")])
                }
                SettingsRow(title: en ? "Meeting notes" : "회의 정리", desc: en ? "Turns the transcript into decisions and to-dos." : "대화 기록으로 결정과 할 일을 정리해요.") {
                    ChoiceMenu(value: $chat, items: [("openrouter", "OpenRouter"), ("device", en ? "This Mac" : "이 Mac"),
                                                     ("openai", "OpenAI"), ("other", en ? "Other address" : "다른 주소")])
                }
                SettingsRow(title: en ? "Answer cards" : "답 카드", desc: en ? "Shows answers to questions that come up in meetings." : "회의 중에 나온 질문에 답을 띄워요.") {
                    ChoiceMenu(value: $cards, items: [("openrouter", "OpenRouter"), ("typesafe", "TypeSafe"), ("off", en ? "Off" : "끄기")])
                }
            }
        }
    }

    func testRender() async throws {
        guard let dir = ProcessInfo.processInfo.environment["TACIE_CHOICE_RENDER_DIR"] else {
            throw XCTSkip("set TACIE_CHOICE_RENDER_DIR to render")
        }
        let out = URL(fileURLWithPath: dir)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        for en in [false, true] {
            for dark in [false, true] {
                AppearanceMode.isDark = dark
                let view = Rows(en: en).padding(24).frame(width: 640).background(Theme.paper)
                    .environment(\.colorScheme, dark ? .dark : .light)
                let host = NSHostingView(rootView: view)
                host.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
                let size = host.fittingSize
                let window = NSWindow(contentRect: NSRect(origin: CGPoint(x: -10_000, y: -10_000), size: size),
                                      styleMask: .borderless, backing: .buffered, defer: false)
                window.appearance = host.appearance
                window.contentView = host
                host.frame = NSRect(origin: .zero, size: size)
                host.layoutSubtreeIfNeeded()
                try? await Task.sleep(for: .milliseconds(300))
                guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { return XCTFail("no image") }
                host.cacheDisplay(in: host.bounds, to: rep)
                try rep.representation(using: .png, properties: [:])?
                    .write(to: out.appendingPathComponent("choice\(en ? "-en" : "")\(dark ? "-dark" : "").png"))
            }
        }
        AppearanceMode.isDark = false
    }
}
