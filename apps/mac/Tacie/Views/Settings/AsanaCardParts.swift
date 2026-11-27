import AppKit
import SwiftUI

/// What an Asana card says (docs/32 5절, 5-1절, 8절), the same in the onboarding and in Settings → Connections. Built from
/// plain values so the render tests can draw every state without an engine (AsanaCardRenderTests).
@MainActor
struct AsanaStatus {
    enum State: Equatable {
        case off  // not connected: 연결
        case waiting  // the approval page is open
        case failed(String)  // denied, expired, blocked by an admin…: 다시 연결
        case lost  // the relay said Asana refused the refresh token: 다시 연결 (found tasks stay for seven days)
        case unreadable  // the keychain could not be read: whether there is a connection is unknown, no button
        case signedOut  // the relay wants a Tacie session for the refresh; the connection itself is fine
        case syncing(String)  // "Asana task 받는 중 · 40%"
        case connected
    }

    let link: AsanaLink
    let info: IndexState.AsanaInfo?
    /// The running sync's step line (IndexKeeper.stepLine), nil when no sync runs.
    var stepLine: String? = nil
    var now = Date()

    var state: State {
        switch link.phase {
        case .waiting: return .waiting
        case .failed(let why) where !link.connected: return .failed(why)
        default: break
        }
        if link.unreadable { return .unreadable }
        guard link.connected else { return .off }
        // the engine's lost_since: the connection is lost even if this app has not seen it yet (the engine hides a
        // lost_since a newer token has not been tried with, so a connection made again does not read as lost)
        if link.lost || info?.lostSince != nil { return .lost }
        if link.signedOut { return .signedOut }
        // a sync with nothing from Asana yet is its first one
        if let line = stepLine, info?.tasks == nil { return .syncing(line) }
        return .connected
    }

    /// "연결됨 · 워크스페이스 2개 · task 1,240개", or why not (SettingsText.asanaLine).
    var line: String {
        if link.unreadable { return SettingsText.asanaUnreadable }
        return SettingsText.asanaLine(info, lost: link.lost, lostAt: link.lostAt, signedOut: link.signedOut, now: now)
    }

    /// "Acme 워크스페이스는 관리자가 막아 두어 받지 않아요" under a working connection (docs/32 5-1절).
    var blocked: String? { state == .connected ? SettingsText.asanaBlockedLine(info) : nil }

    /// Who is connected, for the tile's first line.
    var name: String { link.userName.flatMap { $0.isEmpty ? nil : $0 } ?? String(localized: "연결됨") }

    /// "워크스페이스 2개 · task 1,240개" when there is something found (the tile's second line).
    var counts: String? {
        guard let info, (info.tasks ?? 0) > 0 else { return nil }
        return [info.workspaces.map { String(localized: "워크스페이스 \($0)개") }, info.tasks.map { String(localized: "task \($0)개") }]
            .compactMap { $0 }.joined(separator: " · ")
    }

    /// The first fetch is not all in yet (a big workspace comes over several runs).
    var growing: Bool { info?.initialDone == false }

    /// "마지막으로 받은 때: 10월 31일 오후 3:20" when Asana could not be reached on the last sync (docs/32 5-1절).
    var lastFetched: String? {
        guard let info, ["network", "busy", "down"].contains(info.errorKind ?? ""), (info.tasks ?? 0) > 0,
              let at = info.fetchedAt.flatMap(Self.iso) else { return nil }
        return String(localized: "마지막으로 받은 때: \(at.formatted(.dateTime.month().day().hour().minute().locale(AppLocale.locale)))")
    }

    private static func iso(_ s: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.date(from: s) ?? ISO8601DateFormatter().date(from: s)
    }

    /// Disconnecting ends this Mac only; the grant itself ends in Asana's settings, which have no address of their own
    /// (a modal), so the link opens Asana (docs/32 5절 7).
    static let asanaHome = URL(string: "https://app.asana.com")!
}

extension AsanaStatus.State {
    var stage: SourceStage {
        switch self {
        case .off: .off
        case .waiting: .waiting
        case .failed: .failed
        case .lost, .signedOut, .unreadable: .lost
        case .syncing: .syncing
        case .connected: .connected
        }
    }
}

/// What the Asana views can do; the keeper-backed wrappers fill it (AsanaCard.swift).
struct AsanaActions {
    var connect: () -> Void = {}
    var cancel: () -> Void = {}
    var pickScopes: () -> Void = {}
    var disconnect: () -> Void = {}
}

/// The onboarding's Asana tile (SourcesStep), after Jira · Confluence's.
struct AsanaTileBody: View {
    let status: AsanaStatus
    var actions = AsanaActions()

    var body: some View {
        let state = status.state
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                // on the logo tile's own light ground, like the settings card, so the coral mark keeps its contrast in dark mode
                Image("ServiceAsana").resizable().scaledToFit().frame(width: 13, height: 13)
                    .frame(width: 20, height: 20)
                    .background(Theme.logoTile, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 5, style: .continuous).stroke(Theme.ink.opacity(0.1)))
                VStack(alignment: .leading, spacing: 1) {
                    Text(verbatim: "Asana").uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink)
                    Text("task, 댓글, 프로젝트").uiFont(11.5).foregroundStyle(Theme.ink3)
                }
                Spacer(minLength: 0)
                if state == .connected {
                    TileMenu { Button("워크스페이스·프로젝트 고르기", action: actions.pickScopes) }
                }
            }
            switch state {
            case .off:
                Button("연결", action: actions.connect).buttonStyle(InkButtonStyle())
            case .waiting:
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.mini)
                        Text("브라우저에서 기다리는 중").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                        Spacer(minLength: 0)
                        Button("취소", action: actions.cancel).buttonStyle(.plain)
                            .uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink3)
                    }
                    Text("Asana에서 허용해 주세요. Tacie는 읽기만 해요.")
                        .uiFont(11).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                }
            case .failed(let why):
                problem(why)
            case .lost:
                problem(status.line)
            case .signedOut, .unreadable:
                Text(status.line).uiFont(11.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            case .syncing(let line):
                HStack(spacing: 6) {
                    ProgressView().controlSize(.mini)
                    Text("동기화 중 · \(line)").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                        .lineLimit(2).fixedSize(horizontal: false, vertical: true)
                }
            case .connected:
                VStack(alignment: .leading, spacing: 3) {
                    if let counts = status.counts {
                        TileStatusLines(name: status.name,
                                        detail: status.growing ? String(localized: "\(counts) · 더 받는 중") : counts,
                                        growing: status.growing)
                    } else {
                        // nothing to fetch, or not reached: the line needs its whole width
                        TileStatusLines(name: status.name)
                        Text(status.line).uiFont(11).foregroundStyle(Theme.ink3)
                            .fixedSize(horizontal: false, vertical: true)
                            .padding(.leading, 17)
                    }
                    if let blocked = status.blocked {
                        Text(blocked).uiFont(11).foregroundStyle(Theme.ink3)
                            .fixedSize(horizontal: false, vertical: true)
                            .padding(.leading, 17)
                    }
                }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 100, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.paper))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
            .strokeBorder(state == .connected ? Theme.ink.opacity(0.35) : Theme.ink.opacity(0.1)))
    }

    private func problem(_ why: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(why).uiFont(11.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            Button("다시 연결", action: actions.connect).buttonStyle(InkButtonStyle())
        }
    }
}

/// 설정 → 연결's Asana card, under Jira · Confluence's.
struct AsanaCardBody: View {
    let status: AsanaStatus
    var actions = AsanaActions()
    @State private var justConnected = false

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            LogoTile(logo: "ServiceAsana")
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: "Asana").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                switch status.state {
                case .off:
                    Text("연결되지 않았어요").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .waiting:
                    Text("브라우저에서 기다리는 중이에요. Asana에서 허용해 주세요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                        .fixedSize(horizontal: false, vertical: true)
                case .failed(let why):
                    Text(why).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                case .lost, .signedOut, .unreadable:
                    Text(status.line).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                case .syncing(let line):
                    Text("동기화 중 · \(line)").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .connected:
                    Text(status.line).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                    if let blocked = status.blocked {
                        Text(blocked).uiFont(12).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                    }
                    if let last = status.lastFetched {
                        Text(last).uiFont(12).foregroundStyle(Theme.ink3)
                    }
                }
            }
            Spacer(minLength: 8)
            if justConnected {
                DoneMark(text: String(localized: "연결했어요")).padding(.top, -5)
            } else {
                switch status.state {
                case .off:
                    Button(action: actions.connect) { OutlineButton(label: String(localized: "연결")) }.buttonStyle(PressStyle())
                case .waiting:
                    Button(action: actions.cancel) { OutlineButton(label: String(localized: "취소")) }.buttonStyle(PressStyle())
                case .unreadable:
                    // connecting again would write over an item that may still be there: the 30-minute timer reads it again
                    EmptyView()
                case .failed, .lost:
                    Button(action: actions.connect) { OutlineButton(label: String(localized: "다시 연결")) }.buttonStyle(PressStyle())
                case .signedOut, .syncing, .connected:
                    Menu {
                        if status.state == .connected { Button("워크스페이스·프로젝트 고르기", action: actions.pickScopes) }
                        Button("연결 해제", role: .destructive, action: actions.disconnect)
                    } label: {
                        Image(systemName: "ellipsis").uiFont(13, weight: .semibold).foregroundStyle(Theme.ink3)
                            .frame(width: 24, height: 20)
                            .contentShape(Rectangle())
                    }
                    .menuStyle(.button)
                    .buttonStyle(.plain)
                    .menuIndicator(.hidden)
                    .fixedSize()
                    .padding(.top, 2)
                }
            }
            // the first sync's wait, at the right edge where 연결했어요's check will close it
            if !justConnected, case .syncing(let line) = status.state {
                SyncRing(fraction: SyncRing.fraction(in: line)).padding(.top, -5)
            }
        }
        .sourceCardMotion(status.state.stage, justConnected: $justConnected)
        .frame(maxWidth: .infinity, alignment: .leading)  // a cell of 설정 → 연결's two columns, which draw the row
    }
}

/// 워크스페이스·프로젝트 고르기: each workspace with its projects under it. A workspace turned off is not fetched at
/// all, so its projects go grey with it; a project turned off is left out of search. Only what is turned off is
/// remembered, so new workspaces and projects come in on their own (docs/32 5절 6).
struct AsanaScopesBody: View {
    /// nil while the engine is asked.
    let scopes: AsanaScopes?
    @Binding var onWorkspaces: Set<String>
    @Binding var onProjects: Set<String>
    /// The workspaces and projects could not be asked for (the engine did not answer).
    var unreachable = false
    var cancel: () -> Void = {}
    var save: () -> Void = {}
    /// The list scrolls past 300 points; the render tests draw it flat (ImageRenderer leaves a ScrollView blank).
    var scrolls = true

    /// Every workspace turned off would leave nothing to search: saving waits for one pick.
    private var noneOn: Bool { Self.noneOn(scopes, onWorkspaces: onWorkspaces) }

    /// Whether no workspace that can be picked is on. A workspace an admin blocks cannot be picked, so it neither
    /// counts as on nor asks for a pick when it is the only one.
    static func noneOn(_ scopes: AsanaScopes?, onWorkspaces: Set<String>) -> Bool {
        let open = (scopes?.workspaces ?? []).filter { !$0.blocked }.map(\.gid)
        guard !open.isEmpty else { return false }
        return onWorkspaces.isDisjoint(with: open)
    }

    private var empty: Bool { scopes.map { $0.workspaces.isEmpty } ?? false }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("워크스페이스·프로젝트 고르기").uiFont(16, weight: .semibold).foregroundStyle(Theme.ink)
            Text("끈 워크스페이스는 받지 않고, 끈 프로젝트의 task는 찾지 않아요. 새 프로젝트는 저절로 들어와요.")
                .uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            Group {
                if let scopes {
                    if unreachable {
                        Text("Asana 목록을 불러오지 못했어요. 잠시 뒤에 다시 열어 주세요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, minHeight: 60, alignment: .leading)
                    } else if empty {
                        Text("고를 수 있는 워크스페이스가 없어요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                            .frame(maxWidth: .infinity, minHeight: 60, alignment: .leading)
                    } else {
                        let list = VStack(alignment: .leading, spacing: 2) {
                            ForEach(scopes.workspaces) { w in
                                let wOn = !w.blocked && onWorkspaces.contains(w.gid)
                                row(name: w.name, count: String(localized: "task \(w.tasks)개"), isOn: wOn, bold: true,
                                    enabled: !w.blocked) {
                                    toggle(w.gid, in: $onWorkspaces)
                                }
                                if w.blocked {
                                    Text(String(localized: "\(w.name) 워크스페이스는 관리자가 막아 두어 받지 않아요"))
                                        .uiFont(12).foregroundStyle(Theme.ink3)
                                        .fixedSize(horizontal: false, vertical: true)
                                        .padding(.leading, 36).padding(.bottom, 4)
                                }
                                ForEach(scopes.projects.filter { $0.workspace == w.gid }) { p in
                                    row(name: p.name, count: String(localized: "task \(p.tasks)개"),
                                        isOn: wOn && onProjects.contains(p.gid), bold: false, enabled: wOn) {
                                        toggle(p.gid, in: $onProjects)
                                    }
                                    .padding(.leading, 26)
                                }
                            }
                        }
                        if scrolls {
                            ScrollView { list }
                                .scrollIndicators(.never)
                                .frame(maxHeight: 300)
                                .fixedSize(horizontal: false, vertical: true)
                        } else {
                            list
                        }
                    }
                } else {
                    ProgressView().controlSize(.small).frame(maxWidth: .infinity, minHeight: 60)
                }
            }
            .padding(.top, 14)
            HStack(spacing: 8) {
                if noneOn {
                    Text("워크스페이스를 하나 이상 골라요").uiFont(12).foregroundStyle(Theme.ink3)
                }
                Spacer(minLength: 8)
                Button(action: cancel) { OutlineButton(label: String(localized: "취소")) }
                    .buttonStyle(PressStyle()).keyboardShortcut(.cancelAction)
                let off = scopes == nil || empty || noneOn
                Button("저장", action: save)
                    .buttonStyle(InkButtonStyle())
                    .keyboardShortcut(.defaultAction)
                    .disabled(off)
                    .opacity(off ? 0.35 : 1)
            }
            .padding(.top, 18)
        }
        .padding(22)
        .frame(width: 420)
        .background(Theme.paper)
    }

    private func toggle(_ id: String, in set: Binding<Set<String>>) {
        if set.wrappedValue.contains(id) { set.wrappedValue.remove(id) } else { set.wrappedValue.insert(id) }
    }

    /// One workspace or project: an ink check when it is searched, its name, how many tasks. The whole row toggles.
    private func row(name: String, count: String, isOn: Bool, bold: Bool, enabled: Bool = true,
                     action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 10) {
                ZStack {
                    RoundedRectangle(cornerRadius: 5, style: .continuous)
                        .fill(isOn ? Theme.ink : .clear)
                    RoundedRectangle(cornerRadius: 5, style: .continuous)
                        .strokeBorder(Theme.ink.opacity(isOn ? 0 : 0.25), lineWidth: 1.2)
                    Image(systemName: "checkmark").uiFont(9.5, weight: .bold).foregroundStyle(Theme.onStrong)
                        .opacity(isOn ? 1 : 0)
                }
                .frame(width: 16, height: 16)
                Text(verbatim: name).uiFont(13.5, weight: bold ? .semibold : .medium)
                    .foregroundStyle(isOn ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3)).lineLimit(1)
                Spacer(minLength: 8)
                Text(count).uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize()
            }
            .padding(.horizontal, 10)
            .frame(height: 34)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .opacity(enabled ? 1 : 0.45)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

/// The disconnect confirmation (docs/32 5절 7): only this Mac lets go, and how to end the grant in Asana itself.
/// Asana's revoke would end every Mac's connection, so Tacie never calls it.
struct AsanaDisconnectBody: View {
    var cancel: () -> Void = {}
    var confirm: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Asana 연결을 해제할까요?").uiFont(16, weight: .semibold).foregroundStyle(Theme.ink)
            Text("이 Mac에 받아 둔 Asana 자료를 지워요. 다른 Mac의 Tacie는 그대로 연결돼 있어요.")
                .uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            VStack(alignment: .leading, spacing: 8) {
                Text(PastedTokens.pasted(.asana) ? String(localized: "Asana에서도 끊으려면 Asana의 My apps → Personal access tokens에서 이 토큰을 지워요.")
                     : String(localized: "Asana에서도 끊으려면 Asana → 설정 → 연동 → 권한이 부여된 연동에서 Tacie의 '권한 부여 취소'를 눌러요."))
                    .uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                Button { NSWorkspace.shared.open(PastedTokens.pasted(.asana) ? PastedTokens.makeURL(.asana) : AsanaStatus.asanaHome) } label: {
                    HStack(spacing: 4) {
                        Text("Asana 열기").uiFont(12.5, weight: .semibold)
                        Image(systemName: "arrow.up.right").uiFont(10, weight: .semibold)
                    }
                    .foregroundStyle(Theme.ink)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Theme.hairline))
            .padding(.top, 14)
            HStack(spacing: 8) {
                Spacer(minLength: 8)
                Button(action: cancel) { OutlineButton(label: String(localized: "취소")) }
                    .buttonStyle(PressStyle()).keyboardShortcut(.cancelAction)
                Button("연결 해제", action: confirm)
                    .buttonStyle(InkButtonStyle())
            }
            .padding(.top, 18)
        }
        .padding(22)
        .frame(width: 420)
        .background(Theme.paper)
    }
}
