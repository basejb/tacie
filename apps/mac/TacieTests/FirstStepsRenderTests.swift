import XCTest
import SwiftUI
@testable import Tacie

/// Draws Getting started to PNG for a look (docs/11 14절). Runs only when TACIE_FIRST_STEPS_RENDER_DIR names a folder
/// (pass it as TEST_RUNNER_TACIE_FIRST_STEPS_RENDER_DIR); nothing opens on screen. Service logos come from the app's
/// asset catalog and are blank here.
@MainActor
final class FirstStepsRenderTests: XCTestCase {
    private func png<V: View>(_ view: V, width: CGFloat, _ name: String, to out: URL) {
        let framed = view.padding(20).frame(width: width + 40, alignment: .leading).background(Theme.paper)
            .environment(\.colorScheme, .light)
        let r = ImageRenderer(content: framed)
        r.scale = 2
        guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
        try? data.write(to: out.appendingPathComponent("\(name).png"))
    }

    func testRenderStates() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_FIRST_STEPS_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let t = Date()
        let github = FirstStepsInfo(connected: [.init(tool: .github, label: "GitHub · basejb/tacie")],
                                    unconnected: [.notion, .linear, .gdrive, .atlassian], unanswered: 0)
        var s = FirstStepsState()
        s.done[.source] = t
        png(FirstStepsCard(state: s, info: github, actions: FirstStepsActions()), width: 560, "1a-github", to: out)
        png(FirstStepsCard(state: FirstStepsState(),
                           info: FirstStepsInfo(unconnected: [.github, .notion, .linear, .gdrive, .atlassian, .asana, .dropbox], unanswered: 0),
                           actions: FirstStepsActions()), width: 560, "1b-none", to: out)
        let atlassian = FirstStepsInfo(connected: [.init(tool: .atlassian, label: FirstStepsInfo.name(.atlassian))],
                                       unconnected: [.github, .notion, .linear, .gdrive], unanswered: 0)
        png(FirstStepsCard(state: s, info: atlassian, actions: FirstStepsActions()), width: 560, "1c-atlassian", to: out)
        var after = s
        after.done[.meeting] = t
        var withQs = github
        withQs.unanswered = 2
        png(FirstStepsCard(state: after, info: withQs, actions: FirstStepsActions(), width: 500, framed: false), width: 500, "2-popover", to: out)
        png(FirstStepsFooterRow(state: after, info: withQs, actions: FirstStepsActions(), finished: {}).frame(width: 300), width: 300, "2-footer", to: out)
        var skipped = after
        skipped.skipped = [.card]
        png(FirstStepsCard(state: skipped, info: withQs, actions: FirstStepsActions(), width: 500, framed: false), width: 500, "2-skipped", to: out)
        var all = after
        all.done[.card] = t
        all.done[.ask] = t
        png(FirstStepsFooterRow(state: all, info: withQs, actions: FirstStepsActions(), finished: {}).frame(width: 300), width: 300, "3-finish", to: out)
    }
}
