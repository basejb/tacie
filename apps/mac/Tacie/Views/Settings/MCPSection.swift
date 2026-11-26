import AppKit
import SwiftUI

/// The live MCP section (docs/38 §5): reads the AI apps' own settings when the page appears and when Tacie comes to
/// the front while it is open (no timer, no file watcher, docs/38 §3), and starts adding Tacie to one of them.
struct MCPSection: View {
    /// Opens or closes the socket after the switch changed (`MCPBridge.refresh`).
    let refresh: () -> Void

    @State private var states: [MCPClientKind: MCPClientState] = [:]
    @State private var opened: Set<MCPClientKind> = []
    @State private var on = MCPAccess.isOn()
    @State private var serverAllows = MCPAccess.serverAllows()
    @State private var flashing: Set<MCPClientKind> = []
    /// The confirm question is up: for an app's first 연결 (then that app), or for "MCP 켜기" (nil inside).
    @State private var asking: Ask?

    private enum Ask: Equatable { case client(MCPClientKind), turnOn }

    /// How long "열었어요" / "복사했어요" stays before the button comes back.
    private static let flashTime: Duration = .seconds(3)

    private static var clients: MCPClients {
        MCPClients(home: FileManager.default.homeDirectoryForCurrentUser, read: { try? Data(contentsOf: $0) },
                   appInstalled: { NSWorkspace.shared.urlForApplication(withBundleIdentifier: $0) != nil },
                   serverName: MCPInstall.serverName, currentBinary: MCPInstall.binaryPath(), currentSocket: MCPInstall.socketPath)
    }

    var body: some View {
        MCPSectionBody(status: MCPStatus(states: states, on: on, serverAllows: serverAllows, flashing: flashing),
                       actions: MCPActions(connect: connect, turnOff: turnOff, turnOn: { asking = .turnOn }))
            .onAppear(perform: reload)
            .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in reload() }
            .alert("MCP를 켤까요?", isPresented: Binding(get: { asking != nil }, set: { if !$0 { asking = nil } })) {
                Button("켜고 연결하기") {
                    guard let ask = asking else { return }
                    asking = nil
                    MCPAccess.set(true)
                    Analytics.capture(.settingChanged(setting: .mcpAccess, value: .on))
                    refresh()
                    if case .client(let kind) = ask { start(kind) } else { reload() }
                }
                .keyboardShortcut(.defaultAction)
                Button("취소", role: .cancel) { asking = nil }
            } message: {
                Text("찾은 회의 기록과 연결한 서비스의 내용은 AI 앱으로 넘어가 그 앱의 AI가 읽어요. 화자 이름은 회의 페이지에 보이는 그대로 보내고, 기억한 참석자의 보이스 모델은 보내지 않아요.")
            }
    }

    private func reload() {
        on = MCPAccess.isOn()
        serverAllows = MCPAccess.serverAllows()
        let c = Self.clients
        var next: [MCPClientKind: MCPClientState] = [:]
        for k in MCPClientKind.allCases {
            next[k] = c.state(k, opened: opened, installedPath: MCPInstall.installedPath(k))
        }
        states = next
    }

    /// The first "연결" asks once; after that each button goes straight to its app.
    private func connect(_ kind: MCPClientKind) {
        guard MCPAccess.serverAllows() else { return reload() }
        if MCPAccess.isOn() { start(kind) } else { asking = .client(kind) }
    }

    private func start(_ kind: MCPClientKind) {
        guard let path = MCPInstall.binaryPath() else {
            Log.info("mcp: tacie-mcp is missing from the app bundle")
            return reload()
        }
        switch kind {
        case .claudeDesktop:
            do {
                let dir = try MCPInstall.mcpbFolder()
                let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.0.0"
                let file = try MCPInstall.makeMCPB(path: path, version: version, into: dir)
                if let id = kind.bundleID, let app = NSWorkspace.shared.urlForApplication(withBundleIdentifier: id) {
                    if NSRunningApplication.runningApplications(withBundleIdentifier: id).isEmpty {
                        // Claude Desktop drops a file handed over while it starts (seen 2026-11-16): start it,
                        // wait once until it is ready, then hand the file over (one-off, not a poll).
                        Task.detached {
                            _ = try? await NSWorkspace.shared.openApplication(at: app, configuration: NSWorkspace.OpenConfiguration())
                            try? await Task.sleep(for: MCPInstall.launchSettle)
                            _ = try? await NSWorkspace.shared.open([file], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
                        }
                    } else {
                        NSWorkspace.shared.open([file], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
                    }
                } else {
                    NSWorkspace.shared.open(file)
                }
                // one-off cleanup once Claude Desktop has had time to copy it (not a poll)
                Task.detached {
                    try? await Task.sleep(for: MCPInstall.mcpbKeep)
                    try? FileManager.default.removeItem(at: dir)
                }
            } catch {
                Log.info("mcp: could not make the Claude Desktop extension: \(error)")
                return reload()
            }
        case .cursor:
            NSWorkspace.shared.open(MCPInstall.cursorURL(path: path))
        case .claudeCode, .codex, .other:
            let text = switch kind {
            case .codex: MCPInstall.codexCommand(path: path)
            case .other: MCPInstall.settingsJSON(path: path)
            default: MCPInstall.claudeCodeCommand(path: path)
            }
            let board = NSPasteboard.general
            board.clearContents()
            board.setString(text, forType: .string)
        }
        Analytics.capture(.mcpInstallClicked(app: {
            switch kind {
            case .claudeDesktop: .claudeDesktop
            case .cursor: .cursor
            case .claudeCode: .claudeCode
            case .codex: .codex
            case .other: .other
            }
        }()))
        MCPInstall.remember(kind, path: path)
        opened.insert(kind)
        flashing.insert(kind)
        reload()
        // a one-off wait after a click (not a poll): the words go and the button is back for a retry
        Task { @MainActor in
            try? await Task.sleep(for: Self.flashTime)
            withAnimation(.easeOut(duration: 0.2)) { _ = flashing.remove(kind) }
        }
    }

    private func turnOff() {
        MCPAccess.set(false)
        Analytics.capture(.settingChanged(setting: .mcpAccess, value: .off))
        refresh()
        opened = []
        flashing = []
        reload()
    }
}
