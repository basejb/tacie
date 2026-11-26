import XCTest
import SwiftUI
@testable import Tacie

/// Draws every state of the on-device speech recognition screens to PNG for a look (docs/22 section 4). Runs only
/// when TACIE_STT_RENDER_DIR names a folder (pass it as TEST_RUNNER_TACIE_STT_RENDER_DIR); nothing opens on screen.
@MainActor
final class LocalSTTRenderTests: XCTestCase {
    private let gib: UInt64 = 1024 * 1024 * 1024
    private var dir: URL!
    private let size: Int64 = 570_000_000

    override func setUp() { dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString) }
    override func tearDown() { try? FileManager.default.removeItem(at: dir) }

    private enum Want { case off, unsupported, space, downloading, waiting, failed, ready }

    private func make(_ want: Want) async -> LocalSTT {
        let defaults = UserDefaults(suiteName: UUID().uuidString)!
        let file = ModelFile(name: "m.bin", size: size, sha256: "g")
        let pack = ModelPack(name: "stt", base: URL(string: "http://x/")!, dir: dir.appendingPathComponent("\(want)"), files: [file])
        let s = LocalSTT(pack: pack, defaults: defaults, memory: (want == .unsupported ? 8 : 16) * gib,
                         free: { want == .space ? 160_000_000 : 100_000_000_000 },
                         fetch: { p, progress in
                             switch want {
                             case .downloading: progress(240_000_000); try await Task.sleep(for: .seconds(60))
                             case .failed: throw URLError(.cannotConnectToHost)
                             case .ready:
                                 try Data(count: Int(self.size)).write(to: p.dir.appendingPathComponent("m.bin"))
                             default: break
                             }
                         },
                         hash: { _ in "g" }, online: { want != .waiting }, engine: { nil })
        switch want {
        case .off, .unsupported: break
        case .space: s.turnOn(consented: true)
        default: s.turnOn(consented: true)
        }
        if want == .ready { await s.task?.value }
        try? await Task.sleep(for: .milliseconds(120))
        return s
    }

    private func png<V: View>(_ view: V, width: CGFloat, _ name: String, to out: URL) {
        let framed = view.padding(20).frame(width: width + 40, alignment: .leading).background(Theme.paper)
            .environment(\.colorScheme, .light)
        let r = ImageRenderer(content: framed)
        r.scale = 2
        guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
        try? data.write(to: out.appendingPathComponent("\(name).png"))
    }

    /// An alert drawn by hand (ImageRenderer cannot draw the system's): copy and buttons only.
    private func alertCopy(_ title: String, _ body: String, _ buttons: [String]) -> some View {
        VStack(spacing: 10) {
            Text(title).font(.system(size: 13, weight: .semibold))
            Text(body).font(.system(size: 11.5)).multilineTextAlignment(.center).fixedSize(horizontal: false, vertical: true)
            VStack(spacing: 6) {
                ForEach(buttons, id: \.self) { Text($0).font(.system(size: 12)).frame(maxWidth: .infinity).padding(.vertical, 4)
                    .background(Color.gray.opacity(0.18), in: RoundedRectangle(cornerRadius: 6)) }
            }
        }
        .foregroundStyle(Theme.ink).padding(18).frame(width: 260)
        .background(RoundedRectangle(cornerRadius: 12).fill(Color.white))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color.gray.opacity(0.3)))
    }

    func testRenderAllStates() async throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_STT_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

        // settings row, one per phase
        for (n, want) in [("off", Want.off), ("unsupported", .unsupported), ("downloading", .downloading), ("waiting", .waiting),
                          ("failed", .failed), ("ready", .ready)] {
            let s = await make(want)
            png(VStack(spacing: 0) { LocalSTTRow(stt: s) }, width: 520, "1-settings-\(n)", to: out)
        }
        // alerts
        png(alertCopy(LocalSTTCopy.consentTitle, LocalSTTCopy.consentBody, [LocalSTTCopy.consentButton, LocalSTTCopy.cancel]),
            width: 260, "2-alert-consent", to: out)
        png(alertCopy(LocalSTTCopy.spaceTitle, LocalSTTCopy.spaceBody(more: 400_000_000), [LocalSTTCopy.spaceButton]),
            width: 260, "2-alert-space", to: out)
        png(alertCopy(LocalSTTCopy.deleteTitle, LocalSTTCopy.deleteBody, [LocalSTTCopy.deleteButton, LocalSTTCopy.cancel]),
            width: 260, "2-alert-delete", to: out)
        // sidebar
        for (n, want) in [("downloading", Want.downloading), ("waiting", .waiting), ("failed", .failed)] {
            let s = await make(want)
            png(STTSidebarRow(stt: s, open: {}), width: 300, "3-sidebar-\(n)", to: out)
        }
        png(DoneMark(text: LocalSTTCopy.doneText).padding(.horizontal, 14), width: 300, "3-sidebar-done", to: out)
        // meeting: the once-per-meeting line and the paused warning (flat stand-ins for the glass)
        for (n, phase) in [("downloading", LocalSTT.Phase.downloading(done: 240_000_000, total: size)), ("failed", .failed("x"))] {
            png(Text(LocalSTTCopy.meetingLine(phase) ?? "").font(Theme.chip).foregroundStyle(Theme.ink2)
                .padding(.horizontal, 14).frame(height: 28).background(Capsule().fill(Color.white))
                .overlay(Capsule().strokeBorder(Theme.ink.opacity(0.1))), width: 420, "4-meeting-notice-\(n)", to: out)
        }
        png(HStack(spacing: 8) { Circle().fill(Theme.warn).frame(width: 6, height: 6)
            Text(LocalSTTCopy.paused).font(Theme.chip).foregroundStyle(Theme.ink2) }
            .padding(.horizontal, 14).frame(height: 38).background(Capsule().fill(Color.white))
            .overlay(Capsule().strokeBorder(Theme.ink.opacity(0.1))), width: 300, "4-meeting-paused", to: out)
        // onboarding
        for (n, want) in [("off", Want.off), ("on", .downloading)] {
            let s = await make(want)
            png(OnboardingSTTRow(stt: s), width: 460, "5-onboarding-\(n)", to: out)
        }
        // 설정 → 회의 scene for the speech recognition row, both ways
        for on in [false, true] {
            UserDefaults.standard.set(on, forKey: LocalSTT.key)
            png(MeetingSettingsScene(focus: .stt).environment(\.forcedReduceMotion, true), width: 560, "6-scene-\(on ? "device" : "server")", to: out)
        }
        UserDefaults.standard.removeObject(forKey: LocalSTT.key)
    }
}
