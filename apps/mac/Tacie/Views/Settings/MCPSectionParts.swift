import SwiftUI

/// One AI app's row in the MCP section: what it says and offers (docs/38 §5 copy table).
struct MCPRow: Equatable {
    let kind: MCPClientKind
    let state: MCPClientState
    /// The server's switch (`MCPAccess.serverAllows`); off, every row on this Mac is dimmed.
    var serverAllows = true
    /// The person's switch (`MCPAccess.isOn`); off, a connected app says so instead of the green check.
    var on = true
    /// Tacie just opened the app or copied the command: the words show for a few seconds in place of the button.
    var flash = false

    var present: Bool { state != .absent }
    var enabled: Bool { serverAllows }
    /// Verified in the app's own settings and answering: the only state with the green check.
    var connected: Bool { serverAllows && on && state == .connected }

    /// A connected app while MCP is off.
    var offLabel: String? {
        guard serverAllows, !on, state == .connected else { return nil }
        return String(localized: "꺼져 있어요")
    }

    /// An asset name, or an SF Symbol for 다른 앱, which stands for any app (`symbol`).
    var logo: String {
        switch kind {
        case .cursor: "ServiceCursor"
        case .claudeDesktop, .claudeCode: "ServiceClaude"
        case .codex: "ServiceCodex"
        case .other: "ellipsis"
        }
    }
    var symbol: Bool { kind == .other }

    var line: String {
        guard present else { return String(localized: "이 Mac에 없어요") }
        guard serverAllows else { return String(localized: "지금은 쓸 수 없어요") }
        switch state {
        case .absent, .notConnected: return firstLine
        case .opened:
            switch kind {
            case .claudeDesktop: return String(localized: "Claude Desktop에서 설치를 눌러 주세요. 연결이 확인되면 여기에 연결됨이 떠요")
            case .cursor: return String(localized: "Cursor에서 설치를 눌러 주세요. 연결이 확인되면 여기에 연결됨이 떠요")
            case .claudeCode, .codex, .other: return firstLine
            }
        case .connected:
            // off, the app is still set up but cannot ask: no "바로 물어볼 수 있어요" next to 꺼져 있어요
            guard on else { return String(localized: "MCP를 켜면 다시 물어볼 수 있어요") }
            switch kind {
            case .claudeDesktop: return String(localized: "Claude에서 회의와 자료를 바로 물어볼 수 있어요")
            case .cursor: return String(localized: "Cursor에서 회의와 자료를 바로 물어볼 수 있어요")
            case .claudeCode: return String(localized: "Claude Code에서 회의와 자료를 바로 물어볼 수 있어요")
            case .codex: return String(localized: "Codex에서 회의와 자료를 바로 물어볼 수 있어요")
            case .other: return firstLine
            }
        case .moved: return String(localized: "Tacie를 옮겼어요. 다시 연결해 주세요.")
        }
    }

    private var firstLine: String {
        switch kind {
        case .claudeDesktop: String(localized: "누르면 Claude Desktop이 열리고 Tacie MCP를 설치할지 물어봐요")
        case .cursor: String(localized: "누르면 Cursor가 열리고 Tacie MCP를 설치할지 물어봐요")
        case .claudeCode, .codex: String(localized: "명령을 복사해 터미널에 붙여 넣어 주세요")
        case .other: String(localized: "설정을 복사해 쓰려는 앱의 MCP 설정에 붙여 넣어 주세요")
        }
    }

    private var startLabel: String {
        switch kind {
        case .claudeCode, .codex: String(localized: "명령 복사")
        case .other: String(localized: "설정 복사")
        case .claudeDesktop, .cursor: String(localized: "연결")
        }
    }

    /// The button on the right (nil: the words after opening, the connected check, the off label, or nothing for an app
    /// not here). After the words, an opened row offers its button again to retry.
    var button: String? {
        guard present else { return nil }
        guard serverAllows else { return startLabel }
        switch state {
        case .absent, .connected: return nil
        case .opened: return flash ? nil : startLabel
        case .notConnected: return startLabel
        case .moved: return String(localized: "다시 연결")
        }
    }

    /// What Tacie just did for this app, plain words in ink2: nothing is verified yet, so no check (the green check is
    /// kept for 연결됨).
    var done: String? {
        guard serverAllows, state == .opened, flash else { return nil }
        switch kind {
        case .claudeDesktop: return String(localized: "Claude Desktop을 열었어요")
        case .cursor: return String(localized: "Cursor를 열었어요")
        case .claudeCode, .codex, .other: return String(localized: "복사했어요")
        }
    }
}

/// What the MCP section shows. Built from plain values so the render tests can draw every state without the app.
struct MCPStatus {
    var states: [MCPClientKind: MCPClientState]
    /// The person's switch (`MCPAccess.isOn`).
    var on: Bool
    var serverAllows: Bool
    /// The apps whose "열었어요" / "복사했어요" words are showing now.
    var flashing: Set<MCPClientKind> = []

    var rows: [MCPRow] {
        MCPClientKind.allCases.map {
            MCPRow(kind: $0, state: states[$0] ?? .absent, serverAllows: serverAllows, on: on, flash: flashing.contains($0))
        }
    }

    /// The link under the rows: "MCP 끄기" while on; "MCP 켜기" while off with an app still connected (with none, each
    /// row's own button turns it on).
    var footer: String? {
        if on { return String(localized: "MCP 끄기") }
        if serverAllows, states.values.contains(.connected) { return String(localized: "MCP 켜기") }
        return nil
    }
}

struct MCPActions {
    var connect: (MCPClientKind) -> Void = { _ in }
    var turnOff: () -> Void = {}
    /// Asks the same question as the first 연결, then turns it on.
    var turnOn: () -> Void = {}
}

/// 설정 → 연결's MCP section, after 동기화: one row per AI app, no on/off switch (docs/38 §5).
struct MCPSectionBody: View {
    let status: MCPStatus
    var actions = MCPActions()

    var body: some View {
        SettingsSection(title: "MCP") {
            Text("Claude Desktop, Cursor, Claude Code, Codex 같은 AI 앱이 이 Mac의 회의 기록과 연결한 서비스에서 찾아 답할 수 있어요. Tacie 앱이 열려 있을 때만 돼요.")
                .uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 2)
            VStack(spacing: 0) {
                ForEach(Array(status.rows.enumerated()), id: \.element.kind) { i, row in
                    if i > 0 { Rectangle().fill(Theme.hairline).frame(height: 1) }
                    MCPRowView(row: row, connect: { actions.connect(row.kind) })
                }
            }
            .background(Theme.paper, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Theme.ink.opacity(0.1)))
            .shadow(color: Theme.cardShadow, radius: 3, y: 1)
            .padding(.top, 10)
            if let footer = status.footer {
                Button(action: status.on ? actions.turnOff : actions.turnOn) {
                    Text(footer).uiFont(12).foregroundStyle(Theme.ink2).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .padding(.top, 10)
            }
        }
        .id("MCP")  // 설정 검색 scrolls here
    }
}

private struct MCPRowView: View {
    let row: MCPRow
    let connect: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Group {
                if row.symbol {
                    // the tile stays light in dark mode (like the brand logos), so a fixed dark grey, not an ink
                    Image(systemName: row.logo).uiFont(13, weight: .bold).foregroundStyle(Color(white: 0.4))
                } else {
                    Image(row.logo).resizable().scaledToFit().frame(width: 20, height: 20)
                }
            }
                .frame(width: 30, height: 30)
                .background(Theme.logoTile, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Theme.ink.opacity(0.1)))
                .saturation(row.present ? 1 : 0)
                .opacity(row.present ? 1 : 0.5)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: row.kind.name).uiFont(13.5, weight: .semibold)
                    .foregroundStyle(row.present ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3))
                Text(row.line).uiFont(12).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            if let label = row.button {
                Button(action: connect) { OutlineButton(label: label) }
                    .buttonStyle(PressStyle())
                    .disabled(!row.enabled)
                    .opacity(row.enabled ? 1 : 0.4)
            } else if let done = row.done {
                Text(done).uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink2)
                    .frame(height: 28)
                    .transition(.opacity)
            } else if let off = row.offLabel {
                Text(off).uiFont(12).foregroundStyle(Theme.ink3)
            } else if row.connected {
                HStack(spacing: 5) {
                    Image(systemName: "checkmark.circle.fill").uiFont(12).foregroundStyle(Theme.live)
                    Text("연결됨").uiFont(12).foregroundStyle(Theme.ink2)
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
    }
}
