import XCTest
import SwiftUI
@testable import Tacie

/// Draws the MCP section of Settings → Connections in the mockup's states 1–5 (docs/38 §5), light and dark, to PNG
/// for a look.
/// Runs only when TACIE_MCP_RENDER_DIR names a folder (pass it as TEST_RUNNER_TACIE_MCP_RENDER_DIR); nothing opens on
/// screen. Logos come from the app's asset catalog and are blank here.
@MainActor
final class MCPSectionRenderTests: XCTestCase {
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

    /// The mockup's states as (name, status), and the fix round's: words after open (no check), retry, off.
    static func states() -> [(String, MCPStatus)] {
        func s(_ d: MCPClientState, _ code: MCPClientState, on: Bool, server: Bool = true,
               flashing: Set<MCPClientKind> = []) -> MCPStatus {
            MCPStatus(states: [.claudeDesktop: d, .cursor: .absent, .claudeCode: code, .codex: code, .other: .notConnected],
                      on: on, serverAllows: server,
                      flashing: flashing)
        }
        return [
            ("1-first", s(.notConnected, .notConnected, on: false)),
            ("2-opened", s(.opened, .notConnected, on: true, flashing: [.claudeDesktop])),
            ("3-connected", s(.connected, .connected, on: true)),
            ("4-moved", s(.moved, .connected, on: true)),
            ("5-server-off", s(.notConnected, .notConnected, on: false, server: false)),
            ("6-copied", s(.notConnected, .opened, on: true, flashing: [.claudeCode])),
            ("7-retry", s(.opened, .opened, on: true)),
            ("8-off-connected", s(.connected, .notConnected, on: false)),
        ]
    }

    func testStatusFooter() {
        let all = Self.states()
        XCTAssertNil(all[0].1.footer)
        XCTAssertEqual(all[2].1.footer, "MCP 끄기")
        XCTAssertEqual(all[7].1.footer, "MCP 켜기")
        XCTAssertEqual(all[0].1.rows.map(\.kind), [.claudeDesktop, .cursor, .claudeCode, .codex, .other])
        XCTAssertEqual(all[0].1.rows.last?.button, "설정 복사")
        XCTAssertEqual(all[0].1.rows[3].button, "명령 복사")
        XCTAssertEqual(all[2].1.rows[3].line, "Codex에서 회의와 자료를 바로 물어볼 수 있어요")
        XCTAssertEqual(all[4].1.rows.first?.enabled, false)
        XCTAssertEqual(all[1].1.rows.first?.done, "Claude Desktop을 열었어요")
        XCTAssertEqual(all[6].1.rows.first?.button, "연결")
        XCTAssertEqual(all[7].1.rows.first?.line, "MCP를 켜면 다시 물어볼 수 있어요")
        XCTAssertEqual(all[7].1.rows.first?.offLabel, "꺼져 있어요")
        XCTAssertEqual(all[2].1.rows.first?.line, "Claude에서 회의와 자료를 바로 물어볼 수 있어요")
    }

    func testRenderStates() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_MCP_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        for (name, status) in Self.states() {
            png(MCPSectionBody(status: status), width: 560, "mcp-\(name)", to: out)
            png(MCPSectionBody(status: status), width: 560, "mcp-\(name)-dark", dark: true, to: out)
        }
    }
}
