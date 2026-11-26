import AppKit
import SwiftUI

/// What a Jira · Confluence card says (docs/31 5절, 5-1절, 8절), the same in the onboarding and in Settings → Connections.
/// Built from plain values so the render tests can draw every state without an engine (AtlassianCardRenderTests).
@MainActor
struct AtlassianStatus {
    enum State: Equatable {
        case off  // not connected: 연결
        case waiting  // the approval page is open
        case failed(String)  // denied, expired, blocked by an admin…: 다시 연결
        case lost  // Atlassian refused the refresh token: 다시 연결 (found items stay for seven days)
        case unreadable  // the keychain could not be read: whether there is a connection is unknown, no button
        case signedOut  // the relay wants a Tacie session for the refresh; the connection itself is fine
        case syncing(String)  // "Jira 이슈와 Confluence 페이지 받는 중"
        case connected
    }

    let link: AtlassianLink
    let info: IndexState.AtlassianInfo?
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
        // the engine's lost_since: Atlassian refused for good even if this app has not seen it yet
        if link.lost || info?.lostSince != nil { return .lost }
        if link.signedOut { return .signedOut }
        // a sync with nothing from Atlassian yet is its first one
        if let line = stepLine, info?.issues == nil, info?.pages == nil { return .syncing(line) }
        return .connected
    }

    /// "연결됨 · acme · 이슈 1,240개 · 페이지 380개", or why not (SettingsText.atlassianLine).
    var line: String {
        if link.unreadable { return SettingsText.atlassianUnreadable }
        return SettingsText.atlassianLine(info, lost: link.lost, signedOut: link.signedOut, now: now)
    }

    /// The site's name for the tile's first line.
    var site: String { info?.site ?? link.site ?? String(localized: "연결됨") }

    private var paused: Bool { info?.pausedUntil.flatMap(Self.iso).map { $0 > now } ?? false }

    /// "이슈 1,240개 · 페이지 380개" when there is something found and nothing else to say (the tile's second line).
    var counts: String? {
        guard let info, !paused else { return nil }
        let found = (info.issues ?? 0) + (info.pages ?? 0)
        guard found > 0 else { return nil }
        return [info.issues.map { String(localized: "이슈 \($0)개") }, info.pages.map { String(localized: "페이지 \($0)개") }]
            .compactMap { $0 }.joined(separator: " · ")
    }

    /// The first fetch has not reached two years back yet (it comes newest first).
    var growing: Bool { info?.initialDone == false }

    /// "마지막으로 받은 때: 10월 31일 오후 3:20" when Atlassian could not be reached on the last sync (docs/31 5-1절).
    var lastFetched: String? {
        guard let info, !paused, ["network", "busy", "down"].contains(info.errorKind ?? ""),
              (info.issues ?? 0) + (info.pages ?? 0) > 0,
              let at = info.fetchedAt.flatMap(Self.iso) else { return nil }
        return String(localized: "마지막으로 받은 때: \(at.formatted(.dateTime.month().day().hour().minute().locale(AppLocale.locale)))")
    }

    private static func iso(_ s: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.date(from: s) ?? ISO8601DateFormatter().date(from: s)
    }

    /// Atlassian has no revoke API: the user removes Tacie under the account's connected apps (docs/31 5절 7).
    static let accountApps = URL(string: "https://id.atlassian.com/manage-profile/apps")!
}

extension AtlassianStatus.State {
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

/// What the Jira · Confluence views can do; the keeper-backed wrappers fill it (AtlassianCard.swift).
struct AtlassianActions {
    var connect: () -> Void = {}
    var cancel: () -> Void = {}
    var pickScopes: () -> Void = {}
    var disconnect: () -> Void = {}
}

/// The tile's mark: Jira's, the one logo like the other cards (one card holds both products), on the logo tile ground.
struct AtlassianMarks: View {
    var size: CGFloat = 17
    var body: some View {
        // on the logo tile's own light ground, like the settings card, so the blue mark keeps its contrast in dark mode
        Image("ServiceJira").resizable().scaledToFit().frame(width: size * 0.78, height: size * 0.78)
            .frame(width: size + 3, height: size + 3)
            .background(Theme.logoTile, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 5, style: .continuous).stroke(Theme.ink.opacity(0.1)))
    }
}

/// The onboarding's Jira · Confluence tile (SourcesStep), next to Google Drive's.
struct AtlassianTileBody: View {
    let status: AtlassianStatus
    var actions = AtlassianActions()

    var body: some View {
        let state = status.state
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                AtlassianMarks().frame(width: 20, height: 20)
                VStack(alignment: .leading, spacing: 1) {
                    Text(verbatim: "Jira · Confluence").uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink)
                    Text("이슈, 페이지, 댓글").uiFont(11.5).foregroundStyle(Theme.ink3)
                }
                Spacer(minLength: 0)
                if state == .connected {
                    TileMenu { Button("프로젝트·스페이스 고르기", action: actions.pickScopes) }
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
                    Text("사이트를 고르고 허용해 주세요. Tacie는 읽기만 해요.")
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
                    // Jira and Confluence's step line is long: two lines rather than cut
                    Text("동기화 중 · \(line)").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                        .lineLimit(2).fixedSize(horizontal: false, vertical: true)
                }
            case .connected:
                if let counts = status.counts {
                    TileStatusLines(name: status.site,
                                    detail: status.growing ? String(localized: "\(counts) · 더 받는 중") : counts,
                                    growing: status.growing)
                } else {
                    // paused for Atlassian's limit, nothing to fetch, or not reached: the line needs its whole width
                    VStack(alignment: .leading, spacing: 3) {
                        TileStatusLines(name: status.site)
                        Text(status.line).uiFont(11).foregroundStyle(Theme.ink3)
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

/// 설정 → 연결's Jira · Confluence card, under Google Drive's.
struct AtlassianCardBody: View {
    let status: AtlassianStatus
    var actions = AtlassianActions()
    /// Just disconnected here: the card points at Atlassian's connected apps, where Tacie's access really ends.
    var disconnected = false
    @State private var justConnected = false

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            LogoTile(logo: "ServiceJira")
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: "Jira · Confluence").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                switch status.state {
                case .off:
                    Text("연결되지 않았어요").uiFont(12.5).foregroundStyle(Theme.ink2)
                    if disconnected {
                        Button { NSWorkspace.shared.open(AtlassianStatus.accountApps) } label: {
                            Text("Atlassian 쪽 허용도 끊으려면 Atlassian 계정 설정에서 Tacie를 지워 주세요")
                                .uiFont(12).foregroundStyle(Theme.ink3)
                                .underline(color: Theme.ink.opacity(0.25))
                                .multilineTextAlignment(.leading)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .buttonStyle(.plain)
                        .padding(.top, 2)
                    }
                case .waiting:
                    Text("브라우저에서 사이트를 고르고 허용해 주세요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .failed(let why):
                    Text(why).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                case .lost, .signedOut, .unreadable:
                    Text(status.line).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                case .syncing(let line):
                    Text("동기화 중 · \(line)").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .connected:
                    Text(status.line).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
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
                        if status.state == .connected { Button("프로젝트·스페이스 고르기", action: actions.pickScopes) }
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

/// 프로젝트·스페이스 고르기: Jira projects and Confluence spaces with their counts. Only what is turned off is
/// remembered, so new projects and spaces come in on their own (docs/31 5절 6).
struct AtlassianScopesBody: View {
    /// nil while the engine is asked.
    let scopes: AtlassianScopes?
    @Binding var onProjects: Set<String>
    @Binding var onSpaces: Set<String>
    /// The projects and spaces could not be asked for (Atlassian or the engine did not answer).
    var unreachable = false
    var cancel: () -> Void = {}
    var save: () -> Void = {}
    /// The list scrolls past 280 points; the render tests draw it flat (ImageRenderer leaves a ScrollView blank).
    var scrolls = true

    /// Everything turned off would leave nothing to search: saving waits for one pick.
    private var noneOn: Bool {
        guard let scopes, !(scopes.projects.isEmpty && scopes.spaces.isEmpty) else { return false }
        return onProjects.isDisjoint(with: scopes.projects.map(\.id)) && onSpaces.isDisjoint(with: scopes.spaces.map(\.id))
    }

    private var empty: Bool { scopes.map { $0.projects.isEmpty && $0.spaces.isEmpty } ?? false }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("프로젝트·스페이스 고르기").uiFont(16, weight: .semibold).foregroundStyle(Theme.ink)
            Text("끈 프로젝트와 스페이스의 자료는 찾지 않아요. 새로 생긴 건 저절로 들어와요.")
                .uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            Group {
                if let scopes {
                    if unreachable {
                        Text("Jira · Confluence에 닿지 못했어요. 잠시 뒤에 다시 열어 주세요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, minHeight: 60, alignment: .leading)
                    } else if empty {
                        Text("고를 수 있는 프로젝트나 스페이스가 없어요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                            .frame(maxWidth: .infinity, minHeight: 60, alignment: .leading)
                    } else {
                        let list = VStack(alignment: .leading, spacing: 2) {
                            if !scopes.projects.isEmpty {
                                header(String(localized: "Jira 프로젝트"))
                                ForEach(scopes.projects) { p in
                                    row(p, count: String(localized: "이슈 \(p.count)개"), on: $onProjects)
                                }
                            }
                            if !scopes.spaces.isEmpty {
                                header(String(localized: "Confluence 스페이스"))
                                    .padding(.top, scopes.projects.isEmpty ? 0 : 10)
                                ForEach(scopes.spaces) { s in
                                    row(s, count: String(localized: "페이지 \(s.count)개"), on: $onSpaces)
                                }
                            }
                        }
                        if scrolls {
                            ScrollView { list }
                                .scrollIndicators(.never)
                                .frame(maxHeight: 280)
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
                    Text("프로젝트나 스페이스를 하나 이상 골라요").uiFont(12).foregroundStyle(Theme.ink3)
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
        .frame(width: 400)
        .background(Theme.paper)
    }

    private func header(_ title: String) -> some View {
        Text(title).uiFont(12, weight: .semibold).foregroundStyle(Theme.ink3)
            .padding(.horizontal, 10)
            .padding(.bottom, 2)
    }

    /// One project or space: its name, its key, how many items, and an ink check when it is searched. The whole row
    /// toggles.
    private func row(_ s: AtlassianScopes.Scope, count: String, on: Binding<Set<String>>) -> some View {
        let isOn = on.wrappedValue.contains(s.id)
        return Button {
            if isOn { on.wrappedValue.remove(s.id) } else { on.wrappedValue.insert(s.id) }
        } label: {
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
                Text(s.name.isEmpty ? s.key : s.name).uiFont(13.5, weight: .medium)
                    .foregroundStyle(isOn ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3)).lineLimit(1)
                if !s.key.isEmpty, !s.name.isEmpty {
                    Text(verbatim: s.key).uiFont(12).foregroundStyle(Theme.ink3).lineLimit(1).fixedSize()
                }
                Spacer(minLength: 8)
                Text(count).uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize()
            }
            .padding(.horizontal, 10)
            .frame(height: 36)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}
