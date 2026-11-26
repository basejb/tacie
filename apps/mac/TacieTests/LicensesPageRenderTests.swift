import XCTest
import SwiftUI
@testable import Tacie

/// Draws Settings → General → Open source licenses to PNG for a look. Runs only when TACIE_LICENSES_RENDER_DIR names a
/// folder (pass it as TEST_RUNNER_TACIE_LICENSES_RENDER_DIR); nothing opens on screen.
@MainActor
final class LicensesPageRenderTests: XCTestCase {
    func testRender() async throws {
        guard let dir = ProcessInfo.processInfo.environment["TACIE_LICENSES_RENDER_DIR"] else { throw XCTSkip("no render dir") }
        let out = URL(fileURLWithPath: dir)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        for dark in [false, true] {
            AppearanceMode.isDark = dark
            let page = VStack(alignment: .leading, spacing: 0) { LicensesPage(back: {}) }
                .frame(width: 560, alignment: .topLeading).padding(32).background(Theme.paper)
                .environment(\.colorScheme, dark ? .dark : .light)
            let host = NSHostingView(rootView: page)
            host.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
            let size = host.fittingSize
            let window = NSWindow(contentRect: NSRect(origin: CGPoint(x: -10_000, y: -10_000), size: size),
                                  styleMask: .borderless, backing: .buffered, defer: false)
            window.contentView = host
            host.frame = NSRect(origin: .zero, size: size)
            host.layoutSubtreeIfNeeded()
            try? await Task.sleep(for: .milliseconds(150))
            let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: rep)
            try rep.representation(using: .png, properties: [:])?.write(to: out.appendingPathComponent("licenses\(dark ? "-dark" : "").png"))
        }
        AppearanceMode.isDark = false
    }
}
