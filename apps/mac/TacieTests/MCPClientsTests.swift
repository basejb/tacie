import XCTest
@testable import Tacie

/// Whether each AI app has Tacie in its own MCP settings (docs/38 §5). Reads only fixture files in a temp home.
final class MCPClientsTests: XCTestCase {
    private var home: URL!
    private let binary = "/Applications/Tacie.app/Contents/MacOS/tacie-mcp"
    private let socket = "/Users/a/Library/Application Support/Tacie/mcp/mcp.sock"
    private let desktopID = "com.anthropic.claudefordesktop"
    private let cursorID = "com.todesktop.230313mzl4w4u92"

    override func setUpWithError() throws {
        home = FileManager.default.temporaryDirectory.appendingPathComponent("mcpclients-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws { try? FileManager.default.removeItem(at: home) }

    private func clients(apps: Set<String> = [], name: String = "tacie", binary: String? = nil) -> MCPClients {
        MCPClients(home: home, read: { try? Data(contentsOf: $0) }, appInstalled: { apps.contains($0) }, serverName: name,
                   currentBinary: binary ?? self.binary, currentSocket: socket)
    }

    private func write(_ rel: String, _ text: String) throws {
        let url = home.appendingPathComponent(rel)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(text.utf8).write(to: url)
    }

    private func servers(_ name: String, command: String? = nil, socket: String? = nil) -> String {
        """
        {"mcpServers": {"other": {"command": "/x", "args": []},
          "\(name)": {"type": "stdio", "command": "\(command ?? binary)", "args": ["--socket", "\(socket ?? self.socket)"], "env": {}}}}
        """
    }

    private let desktopConfig = "Library/Application Support/Claude/claude_desktop_config.json"
    private let desktopExtensions = "Library/Application Support/Claude/extensions-installations.json"
    private let cursorConfig = ".cursor/mcp.json"
    private let codeConfig = ".claude.json"
    private let codexConfig = ".codex/config.toml"
    /// The rows Tacie can verify (`other` is pasted into an app it does not know).
    private let known = MCPClientKind.allCases.filter { $0 != .other }

    private func codexServer(_ name: String, command: String? = nil, socket: String? = nil) -> String {
        """
        model = "gpt-5"

        [mcp_servers.other]
        command = "npx"

        [mcp_servers.\(name)]
        command = "\(command ?? binary)"
        args = ["--socket", "\(socket ?? self.socket)"]

        [mcp_servers.\(name).env]
        A = "1"
        """
    }

    func testAbsentWhenNoAppIsHere() {
        let c = clients()
        for k in known { XCTAssertEqual(c.state(k, opened: [], installedPath: nil), .absent, "\(k)") }
        XCTAssertEqual(c.state(.other, opened: [], installedPath: nil), .notConnected, "always offered, never verified")
    }

    func testClaudeCodeIsHereWithItsFolderOrFile() throws {
        try FileManager.default.createDirectory(at: home.appendingPathComponent(".claude"), withIntermediateDirectories: true)
        XCTAssertEqual(clients().state(.claudeCode, opened: [], installedPath: nil), .notConnected)
    }

    func testNotConnectedWhenOnlyOtherServers() throws {
        try write(desktopConfig, #"{"mcpServers": {"excalidraw": {"command": "npx", "args": []}}}"#)
        try write(cursorConfig, #"{"mcpServers": {"supabase": {"command": "npx"}}}"#)
        try write(codeConfig, #"{"mcpServers": {"codex": {"type": "stdio", "command": "codex"}}}"#)
        try write(codexConfig, "[mcp_servers.notion]\ncommand = \"npx\"\n")
        let c = clients(apps: [desktopID, cursorID])
        for k in MCPClientKind.allCases { XCTAssertEqual(c.state(k, opened: [], installedPath: nil), .notConnected, "\(k)") }
    }

    func testConnectedFromEachConfig() throws {
        try write(desktopConfig, servers("tacie"))
        try write(cursorConfig, servers("tacie"))
        try write(codeConfig, servers("tacie"))
        try write(codexConfig, codexServer("tacie"))
        let c = clients(apps: [desktopID, cursorID])
        for k in known { XCTAssertEqual(c.state(k, opened: [], installedPath: binary), .connected, "\(k)") }
    }

    func testDevAndReleaseNamesStayApart() throws {
        try write(cursorConfig, servers("tacie"))
        try write(codeConfig, servers("tacie-dev"))
        let dev = clients(apps: [cursorID], name: "tacie-dev")
        XCTAssertEqual(dev.state(.cursor, opened: [], installedPath: nil), .notConnected)
        XCTAssertEqual(dev.state(.claudeCode, opened: [], installedPath: nil), .connected)
        let release = clients(apps: [cursorID], name: "tacie")
        XCTAssertEqual(release.state(.cursor, opened: [], installedPath: nil), .connected)
        XCTAssertEqual(release.state(.claudeCode, opened: [], installedPath: nil), .notConnected)
    }

    func testDesktopExtensionCountsAndItsCopiedBinaryIsNotAMove() throws {
        try write(desktopExtensions, """
        {"extensions": {"local.mcpb.tacie.tacie": {"id": "local.mcpb.tacie.tacie", "manifest": {"name": "tacie",
          "server": {"type": "binary", "mcp_config": {"command": "${__dirname}/server/tacie-mcp", "args": ["--socket", "\(socket)"]}}}}}}
        """)
        let c = clients(apps: [desktopID])
        XCTAssertEqual(c.state(.claudeDesktop, opened: [], installedPath: nil), .connected)
        XCTAssertEqual(clients(apps: [desktopID], name: "tacie-dev").state(.claudeDesktop, opened: [], installedPath: nil), .notConnected)
    }

    func testDesktopExtensionWithAnotherSocketMoved() throws {
        try write(desktopExtensions, """
        {"extensions": {"x": {"manifest": {"name": "tacie", "server": {"mcp_config": {"args": ["--socket", "/old/mcp.sock"]}}}}}}
        """)
        XCTAssertEqual(clients(apps: [desktopID]).state(.claudeDesktop, opened: [], installedPath: nil), .moved)
    }

    func testMovedWhenTheRecordedBinaryOrSocketDiffers() throws {
        try write(cursorConfig, servers("tacie", command: "/Users/a/Downloads/Tacie.app/Contents/MacOS/tacie-mcp"))
        try write(codeConfig, servers("tacie", socket: "/elsewhere/mcp.sock"))
        let c = clients(apps: [cursorID])
        XCTAssertEqual(c.state(.cursor, opened: [], installedPath: nil), .moved)
        XCTAssertEqual(c.state(.claudeCode, opened: [], installedPath: nil), .moved)
        XCTAssertEqual(c.state(.cursor, opened: [.cursor], installedPath: nil), .opened, "connecting again after the move")
    }

    func testMovedWhenTheStoredInstallPathDiffers() throws {
        try write(codeConfig, #"{"mcpServers": {"tacie": {"type": "stdio", "command": "tacie-mcp"}}}"#)
        let c = clients()
        XCTAssertEqual(c.state(.claudeCode, opened: [], installedPath: "/Users/a/Downloads/Tacie.app/Contents/MacOS/tacie-mcp"), .moved)
        XCTAssertEqual(c.state(.claudeCode, opened: [], installedPath: binary), .connected)
    }

    func testOpenedUntilTheConfigHasIt() throws {
        try write(cursorConfig, #"{"mcpServers": {}}"#)
        let c = clients(apps: [cursorID])
        XCTAssertEqual(c.state(.cursor, opened: [.cursor], installedPath: nil), .opened)
        XCTAssertEqual(c.state(.claudeDesktop, opened: [.claudeDesktop], installedPath: nil), .absent)
        try write(cursorConfig, servers("tacie"))
        XCTAssertEqual(c.state(.cursor, opened: [.cursor], installedPath: nil), .connected)
    }

    func testMalformedFilesReadAsNotConnected() throws {
        try write(desktopConfig, "{not json")
        try write(desktopExtensions, #"{"extensions": [1, "x", {"manifest": 3}]}"#)
        try write(cursorConfig, #"{"mcpServers": ["tacie"]}"#)
        try write(codeConfig, #"[1, 2]"#)
        try write(codexConfig, "[mcp_servers.tacie\ncommand = [\n")
        let c = clients(apps: [desktopID, cursorID])
        for k in MCPClientKind.allCases { XCTAssertEqual(c.state(k, opened: [], installedPath: nil), .notConnected, "\(k)") }
    }

    func testCodexReadsItsTomlEntry() throws {
        try write(codexConfig, codexServer("tacie-dev", socket: #"/a b/\"q\"/mcp.sock"#))
        let c = clients(name: "tacie-dev")
        XCTAssertEqual(c.entry(.codex), .init(command: binary, socket: "/a b/\"q\"/mcp.sock"))
        XCTAssertNil(clients(name: "tacie").entry(.codex))
        let multi = "[mcp_servers.\"tacie\"]\ncommand = \"\(binary)\"\nargs = [\n  \"--socket\",\n  \"\(socket)\",\n]\n"
        XCTAssertEqual(MCPClients.codexEntry(multi, name: "tacie"), .init(command: binary, socket: socket))
    }

    func testCodexMovedWhenSocketDiffers() throws {
        try write(codexConfig, codexServer("tacie", socket: "/old/mcp.sock"))
        XCTAssertEqual(clients().state(.codex, opened: [], installedPath: nil), .moved)
    }

    func testClaudeCodeProjectScopeAloneIsNotConnected() throws {
        try write(codeConfig, """
        {"mcpServers": {}, "projects": {"/Users/a/repo": {"mcpServers": {"tacie": {"type": "stdio", "command": "\(binary)"}}}}}
        """)
        XCTAssertEqual(clients().state(.claudeCode, opened: [], installedPath: nil), .notConnected)
    }
}

/// What each row says and offers (docs/38 §5 copy table).
final class MCPRowTests: XCTestCase {
    func testLinesFollowTheCopyTable() {
        typealias R = MCPRow
        XCTAssertEqual(R(kind: .claudeDesktop, state: .notConnected).line, "누르면 Claude Desktop이 열리고 Tacie MCP를 설치할지 물어봐요")
        XCTAssertEqual(R(kind: .cursor, state: .notConnected).line, "누르면 Cursor가 열리고 Tacie MCP를 설치할지 물어봐요")
        XCTAssertEqual(R(kind: .claudeCode, state: .notConnected).line, "명령을 복사해 터미널에 붙여 넣어 주세요")
        XCTAssertEqual(R(kind: .claudeDesktop, state: .notConnected).button, "연결")
        XCTAssertEqual(R(kind: .claudeCode, state: .notConnected).button, "명령 복사")

        XCTAssertEqual(R(kind: .claudeDesktop, state: .opened, flash: true).line, "Claude Desktop에서 설치를 눌러 주세요. 연결이 확인되면 여기에 연결됨이 떠요")
        XCTAssertEqual(R(kind: .cursor, state: .opened, flash: true).line, "Cursor에서 설치를 눌러 주세요. 연결이 확인되면 여기에 연결됨이 떠요")
        XCTAssertEqual(R(kind: .claudeDesktop, state: .opened, flash: true).done, "Claude Desktop을 열었어요")
        XCTAssertEqual(R(kind: .cursor, state: .opened, flash: true).done, "Cursor를 열었어요")
        XCTAssertNil(R(kind: .cursor, state: .opened, flash: true).button)
        XCTAssertEqual(R(kind: .claudeCode, state: .opened, flash: true).line, "명령을 복사해 터미널에 붙여 넣어 주세요")
        XCTAssertEqual(R(kind: .claudeCode, state: .opened, flash: true).done, "복사했어요")
        XCTAssertNil(R(kind: .claudeCode, state: .opened, flash: true).button)

        XCTAssertEqual(R(kind: .claudeDesktop, state: .connected).line, "Claude에서 회의와 자료를 바로 물어볼 수 있어요")
        XCTAssertEqual(R(kind: .cursor, state: .connected).line, "Cursor에서 회의와 자료를 바로 물어볼 수 있어요")
        XCTAssertEqual(R(kind: .claudeCode, state: .connected).line, "Claude Code에서 회의와 자료를 바로 물어볼 수 있어요")
        XCTAssertTrue(R(kind: .claudeCode, state: .connected).connected)
        XCTAssertNil(R(kind: .claudeCode, state: .connected).button)

        XCTAssertEqual(R(kind: .cursor, state: .moved).line, "Tacie를 옮겼어요. 다시 연결해 주세요.")
        XCTAssertEqual(R(kind: .cursor, state: .moved).button, "다시 연결")

        XCTAssertEqual(R(kind: .cursor, state: .absent).line, "이 Mac에 없어요")
        XCTAssertNil(R(kind: .cursor, state: .absent).button)
    }

    func testAfterTheWordsTheButtonComesBackAndTheHintStays() {
        let desktop = MCPRow(kind: .claudeDesktop, state: .opened)
        XCTAssertNil(desktop.done)
        XCTAssertEqual(desktop.button, "연결")
        XCTAssertEqual(desktop.line, "Claude Desktop에서 설치를 눌러 주세요. 연결이 확인되면 여기에 연결됨이 떠요")
        let code = MCPRow(kind: .claudeCode, state: .opened)
        XCTAssertNil(code.done)
        XCTAssertEqual(code.button, "명령 복사")
    }

    func testOffConnectedRowSaysOffWithoutTheCheck() {
        let row = MCPRow(kind: .claudeCode, state: .connected, on: false)
        XCTAssertFalse(row.connected)
        XCTAssertEqual(row.offLabel, "꺼져 있어요")
        XCTAssertNil(row.button)
        XCTAssertNil(MCPRow(kind: .claudeCode, state: .connected).offLabel)
        XCTAssertEqual(MCPRow(kind: .cursor, state: .notConnected, on: false).button, "연결")
    }

    func testServerOffDimsEveryPresentRow() {
        let off = MCPRow(kind: .claudeCode, state: .connected, serverAllows: false)
        XCTAssertEqual(off.line, "지금은 쓸 수 없어요")
        XCTAssertEqual(off.button, "명령 복사")
        XCTAssertFalse(off.enabled)
        XCTAssertFalse(off.connected)
        XCTAssertEqual(MCPRow(kind: .cursor, state: .absent, serverAllows: false).line, "이 Mac에 없어요")
        XCTAssertTrue(MCPRow(kind: .cursor, state: .notConnected).enabled)
    }
}
