import XCTest
import SwiftUI
@testable import Tacie

/// Draws the open edition's token window (stage 3) in each state to PNG for a look. Runs only when
/// TACIE_TOKEN_RENDER_DIR names a folder (pass it as TEST_RUNNER_TACIE_TOKEN_RENDER_DIR); nothing opens on screen.
/// Logos are blank here.
@MainActor
final class TokenSheetRenderTests: XCTestCase {
    /// Drawn by AppKit in a window that never comes on screen: text fields and links draw as they do in the app
    /// (ImageRenderer leaves them as placeholders).
    private func png<V: View>(_ view: V, _ name: String, dark: Bool, to out: URL) async {
        AppearanceMode.isDark = dark
        defer { AppearanceMode.isDark = false }
        let framed = view.padding(20).background(dark ? Color(white: 0.11) : Theme.fill)
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
        try? await Task.sleep(for: .milliseconds(150))
        guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { return XCTFail("no image for \(name)") }
        host.cacheDisplay(in: host.bounds, to: rep)
        guard let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no png for \(name)") }
        try? data.write(to: out.appendingPathComponent("\(name)\(dark ? "-dark" : "").png"))
    }

    /// A model brought to `answer` through a fake check (no network, no keychain).
    private func model(_ s: ConnectionsScene.Source, _ answer: PastedTokens.Check?, fill: Bool = true) async -> TokenConnectModel {
        let m = TokenConnectModel(source: s, check: { _, _, _, _ in answer ?? .unknown }, connect: { _, _, _ in }, done: {})
        m.delay = .zero
        m.closeAfter = .seconds(60)
        guard fill else { return m }
        m.token = "fake-token-for-render"
        if s == .atlassian { m.email = "me@acme.com"; m.site = "acme" }
        guard answer != nil else { return m }
        m.changed()
        for _ in 0..<50 where m.state == .checking { try? await Task.sleep(for: .milliseconds(10)) }
        return m
    }

    func testRender() async throws {
        guard let dir = ProcessInfo.processInfo.environment["TACIE_TOKEN_RENDER_DIR"] else { throw XCTSkip("no render dir") }
        let out = URL(fileURLWithPath: dir)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let found = PastedTokens.Found(name: "Acme", id: "o1")
        let cases: [(String, ConnectionsScene.Source, PastedTokens.Check?, Bool)] = [
            ("01-linear-empty", .linear, nil, false),
            ("02-linear-ok", .linear, .ok(found), true),
            ("03-github-rejected", .github, .rejected, true),
            ("04-asana-unknown", .asana, .unknown, true),
            ("05-notion-ok", .notion, .ok(found), true),
            ("06-atlassian-nosite", .atlassian, .noSite, true),
            ("07-atlassian-empty", .atlassian, nil, false),
        ]
        for (name, s, answer, fill) in cases {
            let m = await model(s, answer, fill: fill)
            for dark in [false, true] { await png(TokenSheetFrame(model: m, close: {}), name, dark: dark, to: out) }
        }
    }
}
