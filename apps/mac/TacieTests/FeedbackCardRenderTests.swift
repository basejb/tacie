import XCTest
import SwiftUI
@testable import Tacie

/// Draws the feedback card to PNG for a look. Runs only when TACIE_FEEDBACK_RENDER_DIR names a folder (pass it as
/// TEST_RUNNER_TACIE_FEEDBACK_RENDER_DIR); nothing opens on screen. The text field is AppKit and draws blank here.
@MainActor
final class FeedbackCardRenderTests: XCTestCase {
    private func png<V: View>(_ view: V, _ name: String, scheme: ColorScheme, to out: URL) {
        let framed = view.padding(28).background(Theme.paper).environment(\.colorScheme, scheme)
        let r = ImageRenderer(content: framed)
        r.scale = 2
        guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
        try? data.write(to: out.appendingPathComponent("\(name).png"))
    }

    func testRenderStates() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_FEEDBACK_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let p = PointerTracker()
        for scheme in [ColorScheme.light, .dark] {
            let tag = scheme == .light ? "light" : "dark"
            png(FeedbackCard(pointer: p, notes: 3) { _ in }, "1-ask-\(tag)", scheme: scheme, to: out)
            png(FeedbackCard(pointer: p, notes: 3, picked: [.faster, .sources]) { _ in }, "2-picked-\(tag)", scheme: scheme, to: out)
            png(FeedbackCard(pointer: p, notes: 3, sent: true) { _ in }, "3-sent-\(tag)", scheme: scheme, to: out)
        }
    }
}
