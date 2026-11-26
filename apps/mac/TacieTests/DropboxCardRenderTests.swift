import XCTest
import SwiftUI
@testable import Tacie

/// Draws the Dropbox card, its onboarding tile and the folder picker to PNG for a look (docs/33 5절). Runs only when
/// TACIE_DROPBOX_RENDER_DIR names a folder (pass it as TEST_RUNNER_TACIE_DROPBOX_RENDER_DIR); nothing opens on screen.
/// Service logos come from the app's asset catalog and are blank here.
@MainActor
final class DropboxCardRenderTests: XCTestCase {
    private typealias Info = IndexState.DropboxInfo
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

    private func info(files: Int?, done: Bool = true, kind: String? = nil, lost: Date? = nil) -> Info {
        Info(connected: true, account: "jiyoon@example.com", files: files, fetchedAt: iso(now.addingTimeInterval(-3 * 3600)),
             initialDone: done, refused: nil, errorKind: kind, lostSince: lost.map(iso))
    }

    private var pair: DropboxConnection {
        DropboxConnection(token: "t", refreshToken: "r", expiresAt: .distantFuture, accountID: "dbid:A1",
                          account: "jiyoon@example.com")
    }

    /// Every card state as (name, status).
    private func states() -> [(String, DropboxStatus)] {
        func status(_ c: DropboxConnection?, lost: Bool = false, unreadable: Bool = false,
                    phase: DropboxLink.Phase = .idle, info: Info? = nil, step: String? = nil) -> DropboxStatus {
            let link = DropboxLink(load: false)
            link.preview(c, lost: lost, unreadable: unreadable, phase: phase)
            return DropboxStatus(link: link, info: info, stepLine: step, now: now)
        }
        return [
            ("01-off", status(nil)),
            ("02-waiting", status(nil, phase: .waiting)),
            ("03-denied", status(nil, phase: .failed(DropboxLink.words(.denied)))),
            ("04-expired", status(nil, phase: .failed(DropboxLink.words(.expired)))),
            ("05-scope", status(nil, phase: .failed(DropboxLink.words(.scope)))),
            ("06-syncing", status(pair, info: Info(connected: true, account: "jiyoon@example.com", files: nil, fetchedAt: nil,
                                                   initialDone: false, refused: nil, errorKind: nil, lostSince: nil),
                                  step: "\(SettingsText.step("dropbox")) · 40%")),
            ("07-partial", status(pair, info: info(files: 300, done: false))),
            ("08-connected", status(pair, info: info(files: 1240))),
            ("09-empty", status(pair, info: info(files: 0))),
            ("10-offline", status(pair, info: info(files: 1240, kind: "network"))),
            ("11-lost", status(pair, lost: true, info: info(files: 1240, lost: now.addingTimeInterval(-86_400)))),
            ("12-unreadable", status(nil, unreadable: true)),
        ]
    }

    func testRenderStates() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_DROPBOX_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        for (name, status) in states() {
            png(DropboxCardBody(status: status), width: 560, "card-\(name)", to: out)
            png(DropboxTileBody(status: status), width: 270, "tile-\(name)", to: out)
            png(DropboxCardBody(status: status), width: 560, "card-\(name)-dark", dark: true, to: out)
            png(DropboxTileBody(status: status), width: 270, "tile-\(name)-dark", dark: true, to: out)
        }

        let folders = [DropboxFolder(id: "f1", name: "영업", path: "/영업", files: 412, teamFolder: false),
                       DropboxFolder(id: "f2", name: "디자인", path: "/디자인", files: 188, teamFolder: false),
                       DropboxFolder(id: "f3", name: "Acme 팀 폴더", path: "/Acme 팀 폴더", files: 2031, teamFolder: true)]
        func sheet(_ on: Set<String>, _ f: [DropboxFolder]?, unreachable: Bool = false) -> some View {
            DropboxFoldersBody(folders: f, on: .constant(on), unreachable: unreachable, scrolls: false)
        }
        for dark in [false, true] {
            let tail = dark ? "-dark" : ""
            png(sheet(["f1", "f3"], folders), width: 420, "sheet-1-some-off\(tail)", dark: dark, to: out)
            png(sheet(["f1", "f2", "f3"], [folders[2], folders[0]]), width: 420, "sheet-2-team\(tail)", dark: dark, to: out)
            png(sheet([], nil), width: 420, "sheet-3-loading\(tail)", dark: dark, to: out)
            png(sheet([], []), width: 420, "sheet-4-empty\(tail)", dark: dark, to: out)
            png(sheet([], [], unreachable: true), width: 420, "sheet-5-unreachable\(tail)", dark: dark, to: out)
        }
    }
}
