import XCTest
@testable import Tacie

final class MCPInstallTests: XCTestCase {
    func testClaudeCodeCommandQuotesSpaceAndApostrophe() {
        let cmd = MCPInstall.claudeCodeCommand(path: "/Applications/My Apps/Tacie's.app/Contents/MacOS/tacie-mcp", socket: "/Users/a b/Library/Application Support/Tacie Dev/mcp/mcp.sock")
        XCTAssertEqual(cmd, "claude mcp remove --scope user \(MCPInstall.serverName) 2>/dev/null; claude mcp add --scope user \(MCPInstall.serverName) -- '/Applications/My Apps/Tacie'\\''s.app/Contents/MacOS/tacie-mcp' --socket '/Users/a b/Library/Application Support/Tacie Dev/mcp/mcp.sock'")
    }

    func testCodexCommandAndSettingsJSON() throws {
        let cmd = MCPInstall.codexCommand(path: "/A B/it's/tacie-mcp", socket: "/s/mcp.sock")
        XCTAssertEqual(cmd, "codex mcp add \(MCPInstall.serverName) -- '/A B/it'\\''s/tacie-mcp' --socket '/s/mcp.sock'")
        let json = MCPInstall.settingsJSON(path: "/A B/tacie-mcp", socket: "/s/mcp.sock")
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
        let entry = try XCTUnwrap((obj["mcpServers"] as? [String: Any])?[MCPInstall.serverName] as? [String: Any])
        XCTAssertEqual(entry["command"] as? String, "/A B/tacie-mcp")
        XCTAssertEqual(entry["args"] as? [String], ["--socket", "/s/mcp.sock"])
        XCTAssertFalse(json.contains("\\/"), "paths stay readable")
    }

    func testCursorURLDecodesBackToCommand() throws {
        let path = "/Applications/Tacie App.app/Contents/MacOS/tacie-mcp"
        let url = MCPInstall.cursorURL(path: path, socket: "/s/Tacie Dev/mcp/mcp.sock")
        let comps = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))
        XCTAssertEqual(comps.scheme, "cursor")
        XCTAssertEqual(comps.host, "anysphere.cursor-deeplink")
        XCTAssertEqual(comps.path, "/mcp/install")
        let items = Dictionary(uniqueKeysWithValues: (comps.queryItems ?? []).map { ($0.name, $0.value ?? "") })
        XCTAssertEqual(items["name"], MCPInstall.serverName)
        let data = try XCTUnwrap(Data(base64Encoded: try XCTUnwrap(items["config"])))
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(json["command"] as? String, path)
        XCTAssertEqual(json["args"] as? [String], ["--socket", "/s/Tacie Dev/mcp/mcp.sock"])
    }

    func testMCPBContainsManifestAndBinaryCopy() throws {
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("mcpb-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        let bin = tmp.appendingPathComponent("tacie-mcp")
        try Data("#!/bin/sh\n".utf8).write(to: bin)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: bin.path)
        let out = tmp.appendingPathComponent("out")
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)

        let mcpb = try MCPInstall.makeMCPB(path: bin.path, version: "1.2.3", into: out, socket: "/s/Tacie Dev/mcp/mcp.sock")
        XCTAssertEqual(mcpb.pathExtension, "mcpb")

        let ex = tmp.appendingPathComponent("ex")
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/unzip")
        p.arguments = ["-q", mcpb.path, "-d", ex.path]
        try p.run(); p.waitUntilExit()
        XCTAssertEqual(p.terminationStatus, 0)

        let m = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: ex.appendingPathComponent("manifest.json"))) as? [String: Any])
        XCTAssertEqual(m["name"] as? String, MCPInstall.serverName)
        XCTAssertEqual(m["version"] as? String, "1.2.3")
        XCTAssertNotNil(m["manifest_version"])
        let server = try XCTUnwrap(m["server"] as? [String: Any])
        XCTAssertEqual(server["type"] as? String, "binary")
        XCTAssertEqual(server["entry_point"] as? String, "server/tacie-mcp")
        let cfg = try XCTUnwrap(server["mcp_config"] as? [String: Any])
        XCTAssertEqual(cfg["command"] as? String, "${__dirname}/server/tacie-mcp")
        XCTAssertEqual(cfg["args"] as? [String], ["--socket", "/s/Tacie Dev/mcp/mcp.sock"])
        let copied = ex.appendingPathComponent("server/tacie-mcp")
        XCTAssertTrue(FileManager.default.isExecutableFile(atPath: copied.path))
    }

    func testMCPBCarriesIconDescriptionsAndTools() throws {
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("mcpb-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        let bin = tmp.appendingPathComponent("tacie-mcp")
        try Data("#!/bin/sh\n".utf8).write(to: bin)
        let png = try XCTUnwrap(MCPInstall.appIconPNG())
        XCTAssertEqual(Array(png.prefix(4)), [0x89, 0x50, 0x4E, 0x47])

        for korean in [true, false] {
            let out = tmp.appendingPathComponent("out-\(korean)")
            try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
            let mcpb = try MCPInstall.makeMCPB(path: bin.path, version: "1.0.0", into: out, socket: "/s", icon: png, korean: korean)
            let ex = tmp.appendingPathComponent("ex-\(korean)")
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/usr/bin/unzip")
            p.arguments = ["-q", mcpb.path, "-d", ex.path]
            try p.run(); p.waitUntilExit()
            XCTAssertEqual(p.terminationStatus, 0)
            XCTAssertEqual(try Data(contentsOf: ex.appendingPathComponent("icon.png")), png)

            let m = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: ex.appendingPathComponent("manifest.json"))) as? [String: Any])
            XCTAssertEqual(m["icon"] as? String, "icon.png")
            let copy = MCPBCopy(korean: korean)
            XCTAssertEqual(m["description"] as? String, copy.description)
            XCTAssertEqual(m["long_description"] as? String, copy.longDescription)
            XCTAssertEqual(m["privacy_policies"] as? [String], Build.isOpen ? [] : ["https://auth.tacie.app/privacy"])
            XCTAssertEqual((m["compatibility"] as? [String: Any])?["platforms"] as? [String], ["darwin"])
            let tools = try XCTUnwrap(m["tools"] as? [[String: String]])
            XCTAssertEqual(tools.map { $0["name"] }, ["search", "list_meetings", "list_ledger", "read_transcript"])
            XCTAssertTrue(tools.allSatisfy { !($0["description"] ?? "").isEmpty })
        }
    }

    func testMCPBWithoutIconHasNoIconField() throws {
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("mcpb-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        let bin = tmp.appendingPathComponent("tacie-mcp")
        try Data("x".utf8).write(to: bin)
        let mcpb = try MCPInstall.makeMCPB(path: bin.path, version: "1.0.0", into: tmp, socket: "/s", icon: nil, korean: true)
        let ex = tmp.appendingPathComponent("ex")
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/unzip")
        p.arguments = ["-q", mcpb.path, "-d", ex.path]
        try p.run(); p.waitUntilExit()
        let m = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: ex.appendingPathComponent("manifest.json"))) as? [String: Any])
        XCTAssertNil(m["icon"])
        XCTAssertFalse(FileManager.default.fileExists(atPath: ex.appendingPathComponent("icon.png").path))
    }

    func testMCPBFolderRemovesEarlierOnes() throws {
        let base = FileManager.default.temporaryDirectory.appendingPathComponent("mcpb-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: base) }
        let first = try MCPInstall.mcpbFolder(in: base)
        try Data("x".utf8).write(to: first.appendingPathComponent("tacie.mcpb"))
        let other = base.appendingPathComponent("someone-else")
        try FileManager.default.createDirectory(at: other, withIntermediateDirectories: true)
        let second = try MCPInstall.mcpbFolder(in: base)
        XCTAssertNotEqual(first, second)
        XCTAssertFalse(FileManager.default.fileExists(atPath: first.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: second.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: other.path))
    }

    func testMovedOnlyWhenStoredPathDiffersPerClient() {
        let suite = "mcpinstall-\(UUID().uuidString)"
        let d = UserDefaults(suiteName: suite)!
        defer { d.removePersistentDomain(forName: suite) }
        XCTAssertFalse(MCPInstall.moved(.cursor, defaults: d, current: "/a"))
        MCPInstall.remember(.cursor, path: "/a", defaults: d)
        XCTAssertEqual(MCPInstall.installedPath(.cursor, defaults: d), "/a")
        XCTAssertNil(MCPInstall.installedPath(.claudeCode, defaults: d))
        XCTAssertFalse(MCPInstall.moved(.cursor, defaults: d, current: "/a"))
        XCTAssertTrue(MCPInstall.moved(.cursor, defaults: d, current: "/b"))
        XCTAssertFalse(MCPInstall.moved(.claudeCode, defaults: d, current: "/b"))
    }
}
