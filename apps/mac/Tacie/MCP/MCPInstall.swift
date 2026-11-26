import AppKit
import zlib

/// Strings and files for adding the local MCP server to AI apps (docs/38 §5).
enum MCPInstall {
    static var serverName: String { Build.current.mcpName }
    /// The binary path an AI app was given, per app (`mcpInstalledPath.cursor`, ...). Not per account: the AI app's
    /// own settings are the same whoever is signed in to Tacie.
    static func installedPathKey(_ kind: MCPClientKind) -> String { "mcpInstalledPath.\(kind.rawValue)" }

    static func binaryPath(bundle: Bundle = .main) -> String? {
        bundle.url(forAuxiliaryExecutable: "tacie-mcp")?.path
    }

    /// `claude mcp remove ...; claude mcp add --scope user <name> -- '<path>' --socket '<sock>'`, single-quote escaped
    /// for the shell. User scope, so Claude Code finds it in every folder (the default scope is the folder it runs in).
    /// `add` refuses a name that already exists and has no force flag, so an old entry (Tacie moved) is removed first;
    /// `;` lets the add run when there was nothing to remove.
    static func claudeCodeCommand(path: String, socket: String = socketPath) -> String {
        "claude mcp remove --scope user \(serverName) 2>/dev/null; "
            + "claude mcp add --scope user \(serverName) -- \(shellQuote(path)) --socket \(shellQuote(socket))"
    }

    /// `codex mcp add <name> -- '<path>' --socket '<sock>'`. `add` replaces an entry of the same name, so no remove
    /// first. The Codex CLI, its IDE extension and the ChatGPT desktop app's Codex share the file it writes.
    static func codexCommand(path: String, socket: String = socketPath) -> String {
        "codex mcp add \(serverName) -- \(shellQuote(path)) --socket \(shellQuote(socket))"
    }

    /// For any other app that takes a local MCP server: the `mcpServers` JSON most of them read, to paste into its settings.
    static func settingsJSON(path: String, socket: String = socketPath) -> String {
        let config: [String: Any] = ["mcpServers": [serverName: ["command": path, "args": ["--socket", socket]]]]
        let data = (try? JSONSerialization.data(withJSONObject: config, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }

    private static func shellQuote(_ s: String) -> String {
        "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    /// The socket the app listens on; same folder `MCPBridge` uses by default.
    static var socketPath: String {
        AccountSpace.base.appendingPathComponent("mcp").appendingPathComponent("mcp.sock").path
    }

    static func cursorURL(path: String, socket: String = socketPath) -> URL {
        let json = (try? JSONSerialization.data(withJSONObject: ["command": path, "args": ["--socket", socket]] as [String: Any], options: [.sortedKeys])) ?? Data()
        var c = URLComponents()
        c.scheme = "cursor"
        c.host = "anysphere.cursor-deeplink"
        c.path = "/mcp/install"
        // Base64 can hold "+", "/" and "=", which must be escaped inside a query value.
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-._~"))
        let cfg = json.base64EncodedString().addingPercentEncoding(withAllowedCharacters: allowed) ?? ""
        c.percentEncodedQuery = "name=\(serverName)&config=\(cfg)"
        return c.url!
    }

    /// Builds `<name>.mcpb` (a zip with manifest.json and a copy of the binary; the copy may go stale
    /// because the binary only talks to the socket). No process is spawned. NSFileCoordinator's
    /// `.forUploading` wraps a folder's files in a top-level folder, which MCPB rejects (manifest.json
    /// must sit at the archive root), so the zip is written here (stored entries, no compression).
    /// The extension page in Claude Desktop shows `icon`, the descriptions, the tool list and the links, in the
    /// app's language at the time of the click.
    static func makeMCPB(path: String, version: String, into dir: URL, socket: String = socketPath,
                         icon: Data? = appIconPNG(), korean: Bool = AppLocale.isKorean) throws -> URL {
        let copy = MCPBCopy(korean: korean)
        var manifest: [String: Any] = [
            "manifest_version": "0.3",
            "name": serverName,
            "display_name": "Tacie",
            "version": version,
            "description": copy.description,
            "long_description": copy.longDescription,
            "author": ["name": "Tacie", "url": "https://tacie.app"],
            "homepage": "https://tacie.app",
            // an open-source build is supported where its code lives
            "support": Build.isOpen ? "https://github.com/basejb/tacie/issues"
                : korean ? "https://tacie.app/ko/contact/" : "https://tacie.app/contact/",
            // the official app's privacy policy; an open-source build has no Tacie service to cover
            "privacy_policies": Build.isOpen ? [] : ["https://auth.tacie.app/privacy"],
            "keywords": copy.keywords,
            "tools": copy.tools.map { ["name": $0.0, "description": $0.1] },
            "compatibility": ["platforms": ["darwin"]],
            "server": [
                "type": "binary",
                "entry_point": "server/tacie-mcp",
                "mcp_config": ["command": "${__dirname}/server/tacie-mcp", "args": ["--socket", socket]],
            ] as [String: Any],
        ]
        if icon != nil { manifest["icon"] = "icon.png" }
        let manifestData = try JSONSerialization.data(withJSONObject: manifest, options: [.prettyPrinted, .sortedKeys])
        let binary = try Data(contentsOf: URL(fileURLWithPath: path))
        var entries: [ZipWriter.Entry] = [
            .init(name: "manifest.json", data: manifestData, mode: 0o644),
            .init(name: "server/tacie-mcp", data: binary, mode: 0o755),
        ]
        if let icon { entries.append(.init(name: "icon.png", data: icon, mode: 0o644)) }
        let zip = ZipWriter.archive(entries)
        let dest = dir.appendingPathComponent("\(serverName).mcpb")
        try zip.write(to: dest, options: .atomic)
        return dest
    }

    static let mcpbFolderPrefix = "tacie-mcpb-"
    /// How long a built `.mcpb` is kept after it was handed to Claude Desktop, which copies it when it opens it.
    static let mcpbKeep: Duration = .seconds(300)
    /// How long to wait after starting Claude Desktop before handing it the `.mcpb`.
    static let launchSettle: Duration = .seconds(6)

    /// A new empty folder for one `.mcpb` under `base`. Folders left by earlier clicks (the app quit before their
    /// one-off cleanup) are removed first, so at most one copy of the binary sits in the temp folder.
    static func mcpbFolder(in base: URL = FileManager.default.temporaryDirectory) throws -> URL {
        let fm = FileManager.default
        for name in (try? fm.contentsOfDirectory(atPath: base.path)) ?? [] where name.hasPrefix(mcpbFolderPrefix) {
            try? fm.removeItem(at: base.appendingPathComponent(name))
        }
        let dir = base.appendingPathComponent(mcpbFolderPrefix + UUID().uuidString)
        try fm.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    static func remember(_ kind: MCPClientKind, path: String, defaults: UserDefaults = .standard) {
        defaults.set(path, forKey: installedPathKey(kind))
    }

    static func installedPath(_ kind: MCPClientKind, defaults: UserDefaults = .standard) -> String? {
        defaults.string(forKey: installedPathKey(kind))
    }

    /// True when a path was stored when this app was added and Tacie now lives elsewhere.
    static func moved(_ kind: MCPClientKind, defaults: UserDefaults = .standard, current: String) -> Bool {
        guard let stored = installedPath(kind, defaults: defaults) else { return false }
        return stored != current
    }
}

extension MCPInstall {
    /// Tacie's app icon as a 512 pt PNG for the `.mcpb` (Claude Desktop shows it on the extension page).
    /// nil when it cannot be drawn; the extension then has no icon.
    static func appIconPNG(bundlePath: String = Bundle.main.bundlePath) -> Data? {
        let icon = NSWorkspace.shared.icon(forFile: bundlePath)
        let side = 512
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: side, pixelsHigh: side, bitsPerSample: 8,
                                         samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                         bytesPerRow: 0, bitsPerPixel: 0) else { return nil }
        rep.size = NSSize(width: side, height: side)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        icon.draw(in: NSRect(x: 0, y: 0, width: side, height: side))
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .png, properties: [:])
    }
}

/// The words Claude Desktop shows for the extension (docs/38 §5). Tool names match `tacie-mcp`'s `tools/list`.
struct MCPBCopy {
    let description: String
    let longDescription: String
    let keywords: [String]
    let tools: [(String, String)]

    init(korean: Bool) {
        if korean {
            description = "회의 기록과 연결한 서비스에서 지난 결정과 할 일을 찾아요."
            longDescription = """
            Tacie가 이 Mac에 저장한 회의 기록과 연결한 서비스(GitHub, 노션, 슬랙, Linear, Jira 등)를 Claude에서 찾아볼 수 있어요. "지난주 회의에서 왜 그렇게 정했지?", "아직 안 끝난 할 일이 뭐야?"처럼 물어보세요.

            찾은 내용은 Claude로 넘어가 Claude가 읽어요. 화자 이름은 회의 페이지에 보이는 그대로 보내고, 기억한 참석자의 보이스 모델은 보내지 않아요. Tacie 앱이 켜져 있어야 하고, 끄려면 Tacie 설정 → 연결 → MCP에서 꺼요.
            """
            keywords = ["회의", "회의록", "결정", "할 일", "meetings"]
            tools = [
                ("search", "회의 기록과 연결한 서비스에서 찾기"),
                ("list_meetings", "날짜나 제목으로 회의 찾기"),
                ("list_ledger", "결정 사항, 할 일, 남은 논의 모아 보기"),
                ("read_transcript", "회의 대화 기록 읽기"),
            ]
        } else {
            description = "Find past decisions and to-dos in your meetings and connected services."
            longDescription = """
            Search the meetings Tacie saved on this Mac and connected services such as GitHub, Notion, Slack, Linear and Jira from Claude. Ask things like "Why did we decide that in last week's meeting?" or "Which to-dos are still open?"

            What Tacie finds goes to Claude, and Claude reads it. Speaker names are sent as shown on the meeting page. Voice models of people Tacie remembers are never sent. Tacie needs to be running. To turn MCP off, go to Tacie Settings → Connections → MCP.
            """
            keywords = ["meetings", "meeting notes", "decisions", "to-dos"]
            tools = [
                ("search", "Search meetings and connected services"),
                ("list_meetings", "Find meetings by date or title"),
                ("list_ledger", "See decisions, to-dos and open questions"),
                ("read_transcript", "Read a meeting's transcript"),
            ]
        }
    }
}

/// Minimal zip (stored entries) so the archive has files at its root without spawning `zip`.
enum ZipWriter {
    struct Entry { let name: String; let data: Data; let mode: UInt16 }

    static func archive(_ entries: [Entry]) -> Data {
        var out = Data(), central = Data()
        for e in entries {
            let name = Data(e.name.utf8)
            let crc = e.data.withUnsafeBytes { UInt32(crc32(0, $0.bindMemory(to: UInt8.self).baseAddress, uInt(e.data.count))) }
            let offset = UInt32(out.count)
            let size = UInt32(e.data.count)
            out.le32(0x04034b50); out.le16(20); out.le16(0x0800); out.le16(0)
            out.le16(0); out.le16(0x21) // fixed DOS time/date (1980-01-01)
            out.le32(crc); out.le32(size); out.le32(size); out.le16(UInt16(name.count)); out.le16(0)
            out.append(name); out.append(e.data)
            central.le32(0x02014b50); central.le16(0x031E); central.le16(20); central.le16(0x0800); central.le16(0)
            central.le16(0); central.le16(0x21)
            central.le32(crc); central.le32(size); central.le32(size); central.le16(UInt16(name.count))
            central.le16(0); central.le16(0); central.le16(0); central.le16(0)
            central.le32(UInt32(0o100000 | UInt32(e.mode)) << 16); central.le32(offset)
            central.append(name)
        }
        let cdOffset = UInt32(out.count)
        out.append(central)
        out.le32(0x06054b50); out.le16(0); out.le16(0)
        out.le16(UInt16(entries.count)); out.le16(UInt16(entries.count))
        out.le32(UInt32(central.count)); out.le32(cdOffset); out.le16(0)
        return out
    }
}

private extension Data {
    mutating func le16(_ v: UInt16) { append(UInt8(v & 0xff)); append(UInt8(v >> 8)) }
    mutating func le32(_ v: UInt32) { le16(UInt16(v & 0xffff)); le16(UInt16(v >> 16)) }
}
