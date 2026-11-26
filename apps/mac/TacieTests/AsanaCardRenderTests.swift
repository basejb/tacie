import XCTest
import SwiftUI
@testable import Tacie

/// Draws the Asana card, its onboarding tile, the workspace · project picker and the disconnect confirmation to PNG
/// for a look (docs/32 8절). Runs only when TACIE_ASANA_RENDER_DIR names a folder (pass it as
/// TEST_RUNNER_TACIE_ASANA_RENDER_DIR); nothing opens on screen. Service logos come from the app's asset catalog and
/// are blank here.
@MainActor
final class AsanaCardRenderTests: XCTestCase {
    private typealias Info = IndexState.AsanaInfo
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

    private func info(workspaces: Int? = 2, tasks: Int?, done: Bool = true, kind: String? = nil, lost: Date? = nil,
                      blocked: [String] = []) -> Info {
        Info(connected: true, refused: nil, workspaces: workspaces, tasks: tasks,
             fetchedAt: iso(now.addingTimeInterval(-3 * 3600)), initialDone: done, errorKind: kind,
             lostSince: lost.map(iso), blockedWorkspaces: blocked)
    }

    private var pair: AsanaConnection {
        AsanaConnection(token: "t", refreshToken: "r", expiresAt: .distantFuture, userGid: "u1", userName: "김지윤")
    }

    /// Every card state as (name, status).
    private func states() -> [(String, AsanaStatus)] {
        func status(_ c: AsanaConnection?, lost: Bool = false, signedOut: Bool = false,
                    phase: AsanaLink.Phase = .idle, info: Info? = nil, step: String? = nil) -> AsanaStatus {
            let link = AsanaLink(load: false)
            link.preview(c, lost: lost, signedOut: signedOut, phase: phase)
            return AsanaStatus(link: link, info: info, stepLine: step, now: now)
        }
        return [
            ("01-off", status(nil)),
            ("02-waiting", status(nil, phase: .waiting)),
            ("03-denied", status(nil, phase: .failed(AsanaLink.words(.denied)))),
            ("04-expired", status(nil, phase: .failed(AsanaLink.words(.expired)))),
            ("05-blocked", status(nil, phase: .failed(AsanaLink.words(.blocked)))),
            ("06-syncing", status(pair, info: Info(connected: true, refused: nil, workspaces: nil, tasks: nil, fetchedAt: nil,
                                                   initialDone: false, errorKind: nil, lostSince: nil, blockedWorkspaces: nil),
                                  step: "\(SettingsText.step("asana")) · 40%")),
            ("07-partial", status(pair, info: info(tasks: 300, done: false))),
            ("08-connected", status(pair, info: info(tasks: 1240))),
            ("09-blocked-workspace", status(pair, info: info(workspaces: 1, tasks: 1240, blocked: ["Acme Corp"]))),
            ("10-empty", status(pair, info: info(tasks: 0))),
            ("11-offline", status(pair, info: info(tasks: 1240, kind: "network"))),
            ("12-lost", status(pair, lost: true, info: info(tasks: 1240, lost: now.addingTimeInterval(-86_400)))),
            ("13-signed-out", status(pair, signedOut: true, info: info(tasks: 1240))),
        ]
    }

    func testRenderStates() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_ASANA_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let all = states()
        for (name, status) in all {
            png(AsanaCardBody(status: status), width: 560, "card-\(name)", to: out)
            png(AsanaTileBody(status: status), width: 270, "tile-\(name)", to: out)
            png(AsanaCardBody(status: status), width: 560, "card-\(name)-dark", dark: true, to: out)
            png(AsanaTileBody(status: status), width: 270, "tile-\(name)-dark", dark: true, to: out)
        }

        typealias W = AsanaScopes.Workspace
        typealias P = AsanaScopes.Project
        let scopes = AsanaScopes(
            workspaces: [W(gid: "w1", name: "Acme", tasks: 1180), W(gid: "w2", name: "개인 워크스페이스", tasks: 60)],
            projects: [P(gid: "p1", workspace: "w1", name: "결제 개편", tasks: 412), P(gid: "p2", workspace: "w1", name: "온보딩", tasks: 268),
                       P(gid: "p3", workspace: "w1", name: "채용", tasks: 77), P(gid: "p4", workspace: "w2", name: "할 일", tasks: 60)])
        func sheet(_ ws: Set<String>, _ ps: Set<String>, _ s: AsanaScopes?, unreachable: Bool = false) -> some View {
            AsanaScopesBody(scopes: s, onWorkspaces: .constant(ws), onProjects: .constant(ps), unreachable: unreachable,
                            scrolls: false)
        }
        for dark in [false, true] {
            let tail = dark ? "-dark" : ""
            png(sheet(["w1", "w2"], ["p1", "p2", "p4"], scopes), width: 420, "sheet-1-some-off\(tail)", dark: dark, to: out)
            png(sheet(["w1"], ["p1", "p2", "p3"], scopes), width: 420, "sheet-2-workspace-off\(tail)", dark: dark, to: out)
            png(sheet([], [], scopes), width: 420, "sheet-3-all-off\(tail)", dark: dark, to: out)
            png(AsanaDisconnectBody(), width: 420, "disconnect-confirm\(tail)", dark: dark, to: out)
        }
        let blocked = AsanaScopes(
            workspaces: [W(gid: "w1", name: "Acme", tasks: 0, blocked: true), W(gid: "w2", name: "개인 워크스페이스", tasks: 60)],
            projects: [P(gid: "p4", workspace: "w2", name: "할 일", tasks: 60)])
        png(sheet(["w1", "w2"], ["p4"], blocked), width: 420, "sheet-7-blocked", to: out)
        png(sheet([], [], nil), width: 420, "sheet-4-loading", to: out)
        png(sheet([], [], AsanaScopes(workspaces: [], projects: [])), width: 420, "sheet-5-empty", to: out)
        png(sheet([], [], AsanaScopes(workspaces: [], projects: []), unreachable: true), width: 420, "sheet-6-unreachable", to: out)
    }
}
