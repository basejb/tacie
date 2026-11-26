import Foundation

/// The AI apps the MCP section offers, in the order of their rows (docs/38 §5).
enum MCPClientKind: String, CaseIterable {
    /// `other` is the last row: any other app that takes a local MCP server, by pasting a settings JSON.
    case claudeDesktop, cursor, claudeCode, codex, other

    /// Product names, never translated; `other` is the row's own words.
    var name: String {
        switch self {
        case .claudeDesktop: "Claude Desktop"
        case .cursor: "Cursor"
        case .claudeCode: "Claude Code"
        case .codex: "Codex"
        case .other: String(localized: "다른 앱")
        }
    }

    /// Found by bundle id (NSWorkspace); Claude Code and Codex are command-line tools and have none.
    var bundleID: String? {
        switch self {
        case .claudeDesktop: "com.anthropic.claudefordesktop"
        case .cursor: "com.todesktop.230313mzl4w4u92"
        case .claudeCode, .codex, .other: nil
        }
    }
}

enum MCPClientState: Equatable {
    /// The app is not on this Mac.
    case absent
    case notConnected
    /// Tacie opened the app (or copied the command) in this settings visit; its settings do not have Tacie yet.
    case opened
    /// The app's own settings have Tacie, pointing at this app.
    case connected
    /// The app's settings have Tacie, but with an older binary or socket path (Tacie was moved).
    case moved
}

/// Whether each AI app has Tacie in its own MCP settings. Only reads those files, never writes them, and spawns
/// nothing; called when the settings page appears or comes to the front again, never on a timer (docs/38 §3, §5).
struct MCPClients {
    let home: URL
    let read: (URL) -> Data?
    let appInstalled: (String) -> Bool
    let serverName: String
    let currentBinary: String?
    var currentSocket: String? = nil
    var exists: (URL) -> Bool = { FileManager.default.fileExists(atPath: $0.path) }

    /// What the app was told: the command (nil for Claude Desktop's extension, which runs its own copy) and the socket.
    struct Entry: Equatable {
        var command: String?
        var socket: String?
    }

    func state(_ k: MCPClientKind, opened: Set<MCPClientKind>, installedPath: String?) -> MCPClientState {
        guard present(k) else { return .absent }
        guard let e = entry(k) else { return opened.contains(k) ? .opened : .notConnected }
        let moved = isMoved(k, e, installedPath: installedPath)
        // connecting again after a move: the old entry stays until the app takes the new one
        if moved, opened.contains(k) { return .opened }
        return moved ? .moved : .connected
    }

    private func isMoved(_ k: MCPClientKind, _ e: Entry, installedPath: String?) -> Bool {
        if let s = e.socket, let now = currentSocket, s != now { return true }
        if let c = e.command, c.hasPrefix("/"), let now = currentBinary, c != now { return true }
        // Claude Desktop runs the copy inside its extension, so where Tacie lives does not matter to it
        if k != .claudeDesktop, let stored = installedPath, let now = currentBinary, stored != now { return true }
        return false
    }

    func present(_ k: MCPClientKind) -> Bool {
        if let id = k.bundleID { return appInstalled(id) }
        switch k {
        case .codex: return exists(home.appendingPathComponent(".codex"))
        case .other: return true
        default: return exists(home.appendingPathComponent(".claude.json")) || exists(home.appendingPathComponent(".claude"))
        }
    }

    /// Tacie's entry in the app's settings, if it is there.
    func entry(_ k: MCPClientKind) -> Entry? {
        switch k {
        case .claudeDesktop:
            let dir = home.appendingPathComponent("Library/Application Support/Claude")
            return server(in: dir.appendingPathComponent("claude_desktop_config.json")) ?? fileExtension(in: dir)
        case .cursor:
            return server(in: home.appendingPathComponent(".cursor/mcp.json"))
        case .claudeCode:
            // user scope only: a project's own entry works in that folder alone
            return server(in: home.appendingPathComponent(".claude.json"))
        case .codex:
            // shared by the Codex CLI, its IDE extension and the ChatGPT desktop app's Codex
            return read(home.appendingPathComponent(".codex/config.toml")).flatMap { Self.codexEntry(String(decoding: $0, as: UTF8.self), name: serverName) }
        case .other:
            return nil  // pasted into an app Tacie does not know: never verified
        }
    }

    /// `[mcp_servers.<name>]` in Codex's `config.toml`, as `codex mcp add` writes it: `command = "…"` and
    /// `args = ["…", …]` on their own lines. Only basic strings (`\"`, `\\` escapes); anything else is skipped.
    static func codexEntry(_ toml: String, name: String) -> Entry? {
        let headers = ["[mcp_servers.\(name)]", "[mcp_servers.\"\(name)\"]"]
        var inside = false
        var entry: Entry?
        var argsText: String?
        for raw in toml.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if argsText != nil {
                argsText! += " " + line
                if line.contains("]") { entry?.socket = socket(in: strings(argsText!)); argsText = nil }
                continue
            }
            if line.hasPrefix("[") {
                if inside { break }
                inside = headers.contains(line)
                if inside { entry = Entry() }
                continue
            }
            guard inside else { continue }
            if line.hasPrefix("command") { entry?.command = strings(line).first }
            if line.hasPrefix("args") {
                if line.contains("]") { entry?.socket = socket(in: strings(line)) } else { argsText = line }
            }
        }
        return entry
    }

    /// The basic strings on a TOML line, in order.
    private static func strings(_ line: String) -> [String] {
        var out: [String] = []
        var cur: String?
        var escaped = false
        for ch in line {
            if var c = cur {
                if escaped { c.append(ch); escaped = false; cur = c; continue }
                if ch == "\\" { escaped = true; continue }
                if ch == "\"" { out.append(c); cur = nil; continue }
                c.append(ch); cur = c
            } else if ch == "\"" {
                cur = ""
            } else if ch == "#" {
                break
            }
        }
        return out
    }

    private func object(_ url: URL) -> [String: Any]? {
        guard let data = read(url) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    /// `{"mcpServers": {"<name>": {"command": ..., "args": [...]}}}`, the shape all three apps use.
    private func server(in url: URL) -> Entry? {
        guard let servers = object(url)?["mcpServers"] as? [String: Any],
              let s = servers[serverName] as? [String: Any] else { return nil }
        return Entry(command: s["command"] as? String, socket: Self.socket(in: s["args"]))
    }

    /// Claude Desktop's installed extensions (`.mcpb`): `extensions-installations.json`, each with its manifest.
    private func fileExtension(in dir: URL) -> Entry? {
        guard let all = object(dir.appendingPathComponent("extensions-installations.json"))?["extensions"] as? [String: Any] else {
            return nil
        }
        for case let ext as [String: Any] in all.values {
            guard let manifest = ext["manifest"] as? [String: Any], manifest["name"] as? String == serverName else { continue }
            let config = (manifest["server"] as? [String: Any])?["mcp_config"] as? [String: Any]
            return Entry(command: nil, socket: Self.socket(in: config?["args"]))
        }
        return nil
    }

    private static func socket(in args: Any?) -> String? {
        guard let args = args as? [String], let i = args.firstIndex(of: "--socket"), i + 1 < args.count else { return nil }
        return args[i + 1]
    }
}
