import XCTest
import SwiftUI
@testable import Tacie

/// Draws the Jira · Confluence card, its onboarding tile and the project · space picker to PNG for a look (docs/31 8절).
/// Runs only when TACIE_ATLASSIAN_RENDER_DIR names a folder (pass it as TEST_RUNNER_TACIE_ATLASSIAN_RENDER_DIR); nothing
/// opens on screen. Service logos come from the app's asset catalog and are blank here.
@MainActor
final class AtlassianCardRenderTests: XCTestCase {
    private typealias Info = IndexState.AtlassianInfo
    private let now = Date()

    private func png<V: View>(_ view: V, width: CGFloat, _ name: String, dark: Bool = false, to out: URL) {
        AppearanceMode.isDark = dark
        defer { AppearanceMode.isDark = false }
        let framed = view.frame(width: width).padding(20).background(dark ? Color(white: 0.11) : Theme.fill)
            .environment(\.colorScheme, dark ? .dark : .light)
        let r = ImageRenderer(content: framed)
        r.scale = 2
        guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
        try? data.write(to: out.appendingPathComponent("\(name).png"))
    }

    private func iso(_ d: Date) -> String { ISO8601DateFormatter().string(from: d) }

    private func info(issues: Int?, pages: Int?, done: Bool = true, kind: String? = nil, lost: Date? = nil,
                      paused: Date? = nil) -> Info {
        Info(connected: true, site: "acme", refused: nil, issues: issues, pages: pages,
             fetchedAt: iso(now.addingTimeInterval(-3 * 3600)), initialDone: done, errorKind: kind,
             lostSince: lost.map(iso), pausedUntil: paused.map(iso))
    }

    private var pair: AtlassianConnection {
        AtlassianConnection(token: "t", refreshToken: "r", expiresAt: .distantFuture, cloud: "c", site: "acme",
                            siteURL: "https://acme.atlassian.net")
    }

    /// Every card state as (name, status).
    private func states() -> [(String, AtlassianStatus)] {
        func status(_ c: AtlassianConnection?, lost: Bool = false, signedOut: Bool = false,
                    phase: AtlassianLink.Phase = .idle, info: Info? = nil, step: String? = nil) -> AtlassianStatus {
            let link = AtlassianLink(load: false)
            link.preview(c, lost: lost, signedOut: signedOut, phase: phase)
            return AtlassianStatus(link: link, info: info, stepLine: step, now: now)
        }
        let paused = Calendar.current.date(bySettingHour: 15, minute: 0, second: 0, of: now.addingTimeInterval(86_400))!
        return [
            ("01-off", status(nil)),
            ("02-waiting", status(nil, phase: .waiting)),
            ("03-denied", status(nil, phase: .failed(AtlassianLink.words(.denied)))),
            ("04-expired", status(nil, phase: .failed(AtlassianLink.words(.expired)))),
            ("05-blocked", status(nil, phase: .failed(AtlassianLink.words(.blocked)))),
            ("06-syncing-first", status(pair, info: Info(connected: true, site: "acme", refused: nil, issues: nil, pages: nil,
                                                         fetchedAt: nil, initialDone: false, errorKind: nil, lostSince: nil,
                                                         pausedUntil: nil),
                                        step: SettingsText.step("atlassian"))),
            ("07-syncing-partial", status(pair, info: info(issues: 300, pages: 40, done: false))),
            ("08-paused", status(pair, info: info(issues: 300, pages: 40, done: false, paused: paused))),
            ("09-connected", status(pair, info: info(issues: 1240, pages: 380))),
            ("10-empty", status(pair, info: info(issues: 0, pages: 0))),
            ("11-offline", status(pair, info: info(issues: 1240, pages: 380, kind: "network"))),
            ("12-unreached", status(pair, info: info(issues: 0, pages: 0, done: false, kind: "network"))),
            ("13-lost", status(pair, lost: true, info: info(issues: 1240, pages: 380, lost: now.addingTimeInterval(-86_400)))),
            ("14-signed-out", status(pair, signedOut: true, info: info(issues: 1240, pages: 380))),
        ]
    }

    func testRenderStates() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_ATLASSIAN_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let all = states()
        for (name, status) in all {
            png(AtlassianCardBody(status: status), width: 560, "card-\(name)", to: out)
            png(AtlassianTileBody(status: status), width: 270, "tile-\(name)", to: out)
        }
        let off = all[0].1
        png(AtlassianCardBody(status: off, disconnected: true), width: 560, "card-15-after-disconnect", to: out)
        for (name, status) in all where ["01-off", "09-connected", "13-lost"].contains(name) {
            png(AtlassianCardBody(status: status), width: 560, "card-\(name)-dark", dark: true, to: out)
            png(AtlassianTileBody(status: status), width: 270, "tile-\(name)-dark", dark: true, to: out)
        }

        typealias Scope = AtlassianScopes.Scope
        let scopes = AtlassianScopes(
            projects: [Scope(id: "p1", key: "ENG", name: "Engineering", count: 912),
                       Scope(id: "p2", key: "OPS", name: "Operations", count: 328)],
            spaces: [Scope(id: "s1", key: "ENG", name: "엔지니어링", count: 241), Scope(id: "s2", key: "PM", name: "제품", count: 117),
                     Scope(id: "s3", key: "ARC", name: "아카이브", count: 22)])
        func sheet(_ projects: Set<String>, _ spaces: Set<String>, _ s: AtlassianScopes?, unreachable: Bool = false) -> some View {
            AtlassianScopesBody(scopes: s, onProjects: .constant(projects), onSpaces: .constant(spaces), unreachable: unreachable,
                                scrolls: false)
        }
        png(sheet(["p1", "p2"], ["s1", "s2"], scopes), width: 400, "sheet-1-some-off", to: out)
        png(sheet([], [], scopes), width: 400, "sheet-2-all-off", to: out)
        png(sheet([], [], nil), width: 400, "sheet-3-loading", to: out)
        png(sheet([], [], AtlassianScopes(projects: [], spaces: [])), width: 400, "sheet-4-empty", to: out)
        png(sheet([], [], AtlassianScopes(projects: [], spaces: []), unreachable: true), width: 400, "sheet-5-unreachable", to: out)
        png(sheet(["p1", "p2"], ["s1", "s2"], scopes), width: 400, "sheet-1-some-off-dark", dark: true, to: out)
    }
}
