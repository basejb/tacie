import XCTest
import SwiftUI
@testable import Tacie

/// Draws the open edition's Settings → AI page (scene and cards) in each key state to PNG for a look. Runs only when
/// TACIE_AI_RENDER_DIR names a folder (pass it as TEST_RUNNER_TACIE_AI_RENDER_DIR); nothing opens on screen.
@MainActor
final class AIPageRenderTests: XCTestCase {
    private func stt() -> LocalSTT {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let pack = ModelPack(name: "stt", base: URL(string: "http://x/")!, dir: dir, files: [ModelFile(name: "m.bin", size: 1, sha256: "g")])
        return LocalSTT(pack: pack, defaults: UserDefaults(suiteName: UUID().uuidString)!, memory: 16 << 30,
                        free: { 1 << 40 }, fetch: { _, _ in }, hash: { _ in "g" }, online: { true }, engine: { nil })
    }

    /// `after`: runs once the view is up (its onAppear reads the saved state), to set the state drawn.
    private func png<V: View>(_ view: V, _ name: String, dark: Bool, to out: URL, width: CGFloat = 640,
                              after: () -> Void) async {
        AppearanceMode.isDark = dark
        defer { AppearanceMode.isDark = false }
        let framed = view.padding(24).frame(width: width).background(Theme.paper)
            .environment(\.colorScheme, dark ? .dark : .light)
        let host = NSHostingView(rootView: framed)
        host.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        let size = host.fittingSize
        let window = NSWindow(contentRect: NSRect(origin: CGPoint(x: -10_000, y: -10_000), size: size),
                              styleMask: .borderless, backing: .buffered, defer: false)
        window.appearance = host.appearance
        window.contentView = host
        host.frame = NSRect(origin: .zero, size: size)
        host.layoutSubtreeIfNeeded()
        try? await Task.sleep(for: .milliseconds(100))
        after()
        host.layoutSubtreeIfNeeded()
        try? await Task.sleep(for: .milliseconds(400))
        host.frame = NSRect(origin: .zero, size: host.fittingSize)
        host.layoutSubtreeIfNeeded()
        guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { return XCTFail("no image for \(name)") }
        host.cacheDisplay(in: host.bounds, to: rep)
        try? rep.representation(using: .png, properties: [:])?.write(to: out.appendingPathComponent("\(name)\(dark ? "-dark" : "").png"))
    }

    // 업체 직접 선택 (2026-11-24): the scene names the providers that were picked, in job order, without repeats, and
    // stays on this Mac when every job does.
    func testCustomSceneNamesThePickedProviders() {
        let c = AIPathScene.Custom(stt: .soniox, chat: .openai, cards: .openrouter)
        XCTAssertEqual(c.title, "Soniox · OpenAI · OpenRouter")
        XCTAssertFalse(c.onDevice)
        let shared = AIPathScene.Custom(stt: .openrouter, chat: .device, cards: .openrouter)
        XCTAssertEqual(shared.title, "OpenRouter")
        let home = AIPathScene.Custom(stt: .device, chat: .device, cards: .off)
        XCTAssertTrue(home.onDevice)
        XCTAssertEqual(AIPathScene.Custom(stt: .device, chat: .other, cards: .typesafe).title, "다른 주소 · TypeSafe")
    }

    func testRender() async throws {
        guard let dir = ProcessInfo.processInfo.environment["TACIE_AI_RENDER_DIR"] else { throw XCTSkip("no render dir") }
        let out = URL(fileURLWithPath: dir)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let cases: [(String, AIChoiceModel.KeyState)] = [("01-empty", .empty), ("02-checking", .checking), ("03-ok", .ok),
                                                         ("04-rejected", .rejected), ("05-saved", .saved)]
        for (name, state) in cases {
            let m = AIChoiceModel(localSTT: stt()) { _, _ in }
            if state != .empty && state != .saved { m.keyDraft = "sk-or-v1-render" }
            let page = Page(model: m)
            for dark in [false, true] { await png(page, name, dark: dark, to: out) { m.keyState = state } }
        }
        // the onboarding's AI step (540-wide window, 32 on each side): scene and cards as OnboardingView lays them out
        for (name, open) in [("08-onboarding-key", AIMode.key), ("09-onboarding-local", .local)] {
            let m = AIChoiceModel(localSTT: stt()) { _, _ in }
            let page = Page(model: m, inSettings: false)
            for dark in [false, true] { await png(page, name, dark: dark, to: out, width: 524) { m.open = open } }
        }
        // "Mac 안에서만" with no notes model yet: what to get (오픈소스판 e2e, 2026-11-25)
        let pulls: [(String, AIChoiceModel.Search, AIChoiceModel.Pull)] = [
            ("10-local-none", .none, .idle),
            ("13-local-pulling", .none, .pulling(done: 2_140_000_000, total: 4_977_171_584)),
            ("14-local-pull-failed", .none, .failed("받지 못했어요. 인터넷 연결을 확인한 뒤 다시 받아 주세요.")),
        ]
        for (name, search, pull) in pulls {
            let m = AIChoiceModel(localSTT: stt()) { _, _ in }
            let page = Page(model: m)
            for dark in [false, true] {
                await png(page, name, dark: dark, to: out) { m.open = .local; m.search = search; m.pull = pull }
            }
        }
        // the onboarding's card with 받기 in the 회의 정리 row (the window is a fixed 540×620)
        let inline: [(String, AIChoiceModel.Search, AIChoiceModel.Pull)] = [
            ("15-onboarding-pull", .none, .idle),
            ("16-onboarding-pulling", .none, .pulling(done: 2_140_000_000, total: 4_977_171_584)),
            ("17-onboarding-pull-failed", .none, .failed("받지 못했어요")),
            ("18-onboarding-pulled", .found(LocalServer(name: BundledLLM.name, url: "http://127.0.0.1:11535/v1", model: BundledLLM.model)), .idle),
            ("19-onboarding-ollama", .found(LocalServer(name: "Ollama", url: "http://127.0.0.1:11434/v1", model: AIChecks.notesModel)), .idle),
        ]
        for (name, search, pull) in inline {
            let m = AIChoiceModel(localSTT: stt()) { _, _ in }
            let page = Page(model: m, inSettings: false)
            for dark in [false, true] {
                await png(page, name, dark: dark, to: out, width: 524) { m.open = .local; m.search = search; m.pull = pull }
            }
        }
        // one app (#56): Tacie AI on top, signed out (the sign-in in its card), in settings and onboarding
        for (name, inSettings, width) in [("20-tacie-settings", true, CGFloat(640)), ("21-tacie-onboarding", false, CGFloat(524))] {
            let m = AIChoiceModel(localSTT: stt()) { _, _ in }
            let page = Page(model: m, inSettings: inSettings)
            for dark in [false, true] { await png(page, name, dark: dark, to: out, width: width) { m.open = .tacie } }
        }
        for dark in [false, true] { await png(SignedOutNotice {}, "22-signed-out-notice", dark: dark, to: out) {} }
        for (name, custom) in [("06-custom", AIPathScene.Custom(stt: .soniox, chat: .openai, cards: .openrouter)),
                               ("07-custom-home", AIPathScene.Custom(stt: .device, chat: .device, cards: .off))] {
            let m = AIChoiceModel(localSTT: stt()) { _, _ in }
            m.open = .custom
            let page = Page(model: m, custom: custom)
            for dark in [false, true] { await png(page, name, dark: dark, to: out) {} }
        }
    }
}

/// The page's scene and cards as `OpenAIPage` lays them out, reading the model in its body so the scene follows it.
private struct Page: View {
    @Bindable var model: AIChoiceModel
    var custom: AIPathScene.Custom? = nil
    var inSettings = true
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            AIPathScene(local: model.open == .local, keyOn: model.keyOn, custom: custom, tacie: model.open == .tacie ? TacieAccount.shared.signedIn : nil, short: !inSettings)
                .padding(.bottom, inSettings ? 14 : 12)
            AIChoiceCards(model: model, inSettings: inSettings)
        }
    }
}
