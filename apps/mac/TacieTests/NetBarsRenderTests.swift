import XCTest
import SwiftUI
@testable import Tacie

/// Draws the pill's running row with each connection level to PNG for a look. Runs only when TACIE_NET_RENDER_DIR
/// names a folder (pass it as TEST_RUNNER_TACIE_NET_RENDER_DIR); nothing opens on screen. The eyes and the glass are
/// left out: only the row's contents, at the pill's height.
@MainActor
final class NetBarsRenderTests: XCTestCase {
    private func row(_ level: NetHealth.Level) -> some View {
        HStack(spacing: 8) {
            Circle().fill(level == .offline ? Theme.warn : Theme.live).frame(width: 6, height: 6)
            Text("대면").font(Theme.chip).foregroundStyle(Theme.ink2)
            Text("12:52").font(Theme.chip).monospacedDigit().foregroundStyle(Theme.ink3)
                .frame(width: 38, alignment: .leading)
            if level != .ok { NetBars(level: level) }
        }
        .padding(.horizontal, 16)
        .frame(height: Theme.pillHeight)
        .background(Theme.fill, in: Capsule())
    }

    func testRender() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_NET_RENDER_DIR"] else { throw XCTSkip("no render dir") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        // light only: Theme's inks follow the app's appearance, not a renderer's colour scheme
        for (name, scheme) in [("light", ColorScheme.light)] {
            let view = VStack(alignment: .leading, spacing: 12) {
                row(.ok); row(.slow); row(.offline)
                NetBars(level: .slow).scaleEffect(4).frame(width: 80, height: 80)
                NetBars(level: .offline).scaleEffect(4).frame(width: 80, height: 80)
            }
            .padding(20).background(scheme == .dark ? Color(white: 0.11) : .white)
            .environment(\.colorScheme, scheme)
            let r = ImageRenderer(content: view)
            r.scale = 2
            guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
                  let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image") }
            try data.write(to: out.appendingPathComponent("net-\(name).png"))
        }
    }
}
