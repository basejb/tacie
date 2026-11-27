import SwiftUI

/// What a Linear card says (docs/18 7절, 7-1절), the same in the onboarding and in Settings → Connections.
@MainActor
struct LinearStatus {
    enum State: Equatable {
        case off  // not connected: 연결
        case waiting  // the approval page is open
        case failed(String)  // 다시 연결
        case lost  // Linear refused the refresh token: 다시 연결 (found issues stay for seven days)
        case syncing(String)  // "Linear 이슈 받는 중 40%"
        case connected
    }

    let keeper: IndexKeeper
    private var link: LinearLink { keeper.linear }

    var state: State {
        switch link.phase {
        case .waiting: return .waiting
        case .failed(let why) where !link.connected: return .failed(why)
        default: break
        }
        guard link.connected else { return .off }
        // the engine's lost_since: the connection is lost even if this app has not seen it yet
        if link.lost || keeper.linearInfo?.lostSince != nil { return .lost }
        // a sync with nothing from Linear yet is Linear's first one
        if let line = keeper.stepLine, keeper.linearInfo?.issues == nil { return .syncing(line) }
        return .connected
    }

    /// "연결됨 · 팀 · 이슈 1,240개", or why not (SettingsText.linearLine).
    var line: String { SettingsText.linearLine(keeper.linearInfo, lost: link.lost, now: Date()) }

    /// "마지막으로 받은 때: 10월 31일 오후 3:20" when Linear could not be reached on the last sync.
    var lastFetched: String? {
        guard let info = keeper.linearInfo, ["network", "busy", "down"].contains(info.errorKind ?? ""),
              let at = info.fetchedAt.flatMap(Self.iso) else { return nil }
        return String(localized: "마지막으로 받은 때: \(at.formatted(.dateTime.month().day().hour().minute().locale(AppLocale.locale)))")
    }

    private static func iso(_ s: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.date(from: s) ?? ISO8601DateFormatter().date(from: s)
    }

    /// Nothing to show: no issues in the visible teams, or every team is off.
    var empty: Bool { keeper.linearInfo?.issues == 0 }
}

/// The onboarding's Linear tile (SourcesStep), next to Notion's.
struct LinearTile: View {
    @Environment(\.colorScheme) private var scheme
    let keeper: IndexKeeper
    @State private var pickTeams = false
    @State private var pasting = false

    var body: some View {
        let status = LinearStatus(keeper: keeper)
        let state = status.state
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image(ServiceLogo.name("ServiceLinear", scheme)).resizable().scaledToFit().frame(width: 17, height: 17)
                    .frame(width: 20, height: 20)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Linear").uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink)
                    Text("이슈와 프로젝트").uiFont(11.5).foregroundStyle(Theme.ink3)
                }
                Spacer(minLength: 0)
                if state == .connected {
                    TileMenu { Button("팀 고르기") { pickTeams = true } }
                }
            }
            switch state {
            case .off:
                Button("연결") { TokenCopy.start(.linear, keeper: keeper, paste: $pasting) }.buttonStyle(InkButtonStyle())
            case .waiting:
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.mini)
                        Text("브라우저에서 연결을 기다리고 있어요").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                        Spacer(minLength: 0)
                        Button("취소") { keeper.linear.cancel() }.buttonStyle(.plain)
                            .uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink3)
                    }
                    Text("워크스페이스를 고르고 허용해 주세요. Tacie는 읽기만 해요.")
                        .uiFont(11).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                }
            case .failed(let why):
                problem(why)
            case .lost:
                problem(status.line)
            case .syncing(let line):
                HStack(spacing: 6) {
                    ProgressView().controlSize(.mini)
                    Text("동기화 중 · \(line)").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                        .lineLimit(1)
                }
            case .connected:
                if let issues = keeper.linearInfo?.issues, issues > 0 {
                    TileStatusLines(name: keeper.linear.workspace ?? String(localized: "연결됨"),
                                    detail: String(localized: "이슈 \(issues)개"))
                } else {
                    // nothing to search (every team off, or none with issues): picking teams comes out of the menu
                    TileStatusLines(name: keeper.linear.workspace ?? String(localized: "연결됨"), detail: status.line,
                                    pick: status.empty ? (String(localized: "팀 고르기"), { pickTeams = true }) : nil)
                }
            }
        }
        .sheet(isPresented: $pickTeams) { LinearTeamsSheet(keeper: keeper) }
        .sheet(isPresented: $pasting) { TokenConnectSheet(source: .linear, keeper: keeper) }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 100, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.paper))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
            .strokeBorder(state == .connected ? Theme.ink.opacity(0.35) : Theme.ink.opacity(0.1)))
    }

    private func problem(_ why: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(why).uiFont(11.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            Button("다시 연결") { TokenCopy.start(.linear, keeper: keeper, paste: $pasting) }.buttonStyle(InkButtonStyle())
        }
    }
}

/// 설정 → 연결's Linear card, under Notion's.
struct LinearSettingsCard: View {
    let keeper: IndexKeeper
    @State private var askDisconnect = false
    @State private var justConnected = false
    @State private var pickTeams = false
    @State private var pasting = false

    var body: some View {
        let status = LinearStatus(keeper: keeper)
        HStack(alignment: .top, spacing: 12) {
            LogoTile(logo: "ServiceLinear")
            VStack(alignment: .leading, spacing: 3) {
                Text("Linear").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                switch status.state {
                case .off:
                    Text("연결되지 않았어요").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .waiting:
                    Text("브라우저에서 워크스페이스를 고르고 허용해 주세요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .failed(let why):
                    Text(why).uiFont(12.5).foregroundStyle(Theme.ink2)
                case .lost:
                    Text(status.line).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                case .syncing(let line):
                    Text("동기화 중 · \(line)").uiFont(12.5).foregroundStyle(Theme.ink2)
                case .connected:
                    Text(status.line).uiFont(12.5).foregroundStyle(Theme.ink2)
                    if let last = status.lastFetched {
                        Text(last).uiFont(12).foregroundStyle(Theme.ink3)
                    }
                }
            }
            Spacer()
            if justConnected {
                DoneMark(text: String(localized: "연결했어요")).padding(.top, -5)
            } else {
                switch status.state {
                case .off:
                    Button { TokenCopy.start(.linear, keeper: keeper, paste: $pasting) } label: { OutlineButton(label: String(localized: "연결")) }.buttonStyle(PressStyle())
                case .waiting:
                    Button { keeper.linear.cancel() } label: { OutlineButton(label: String(localized: "취소")) }.buttonStyle(PressStyle())
                case .failed, .lost:
                    Button { TokenCopy.start(.linear, keeper: keeper, paste: $pasting) } label: { OutlineButton(label: String(localized: "다시 연결")) }.buttonStyle(PressStyle())
                case .syncing, .connected:
                    if status.empty {
                        Button { pickTeams = true } label: { OutlineButton(label: String(localized: "팀 고르기")) }.buttonStyle(PressStyle())
                    }
                    Menu {
                        Button("팀 고르기") { pickTeams = true }
                        Button("연결 해제", role: .destructive) { askDisconnect = true }
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
        .alert("Linear 연결을 해제할까요?", isPresented: $askDisconnect) {
            Button("연결 해제", role: .destructive) {
                Analytics.capture(.sourceDisconnected(tool: .linear))
                Task { await keeper.linear.disconnect() }
            }
            Button("취소", role: .cancel) {}
        } message: {
            // a pasted key is not revoked by Tacie: it lives on until the person deletes it in Linear
            Text(PastedTokens.pasted(.linear) ? String(localized: "받아 둔 Linear 자료를 이 Mac에서 지워요. 키는 Linear의 Settings → Security & access → Personal API keys에서 지우면 연결이 끊겨요.")
                 : String(localized: "받아 둔 Linear 자료를 이 Mac에서 지우고, Linear 쪽 연결도 끊어요."))
        }
        .sheet(isPresented: $pickTeams) { LinearTeamsSheet(keeper: keeper) }
        .sheet(isPresented: $pasting) { TokenConnectSheet(source: .linear, keeper: keeper) }
    }
}

/// 팀 고르기: the visible teams with their issue counts. Turned-off teams leave search on the next sync; new teams
/// come in on their own.
struct LinearTeamsSheet: View {
    let keeper: IndexKeeper
    @Environment(\.dismiss) private var dismiss
    @State private var teams: [LinearTeam] = []
    @State private var on: Set<String> = []
    @State private var loading = true

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("팀 고르기").uiFont(16, weight: .semibold).foregroundStyle(Theme.ink)
            Text("끈 팀의 이슈는 찾지 않아요. 새로 생긴 팀은 저절로 들어와요.")
                .uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            Group {
                if loading {
                    ProgressView().controlSize(.small).frame(maxWidth: .infinity, minHeight: 60)
                } else if teams.isEmpty {
                    Text("아직 받은 팀이 없어요. 동기화가 끝나면 보여요.").uiFont(12.5).foregroundStyle(Theme.ink2)
                        .frame(maxWidth: .infinity, minHeight: 60, alignment: .leading)
                } else {
                    ScrollView {
                        VStack(spacing: 2) { ForEach(teams) { row($0) } }
                    }
                    .scrollIndicators(.never)
                    .frame(maxHeight: 280)
                    .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.top, 14)
            Text("팀을 하나 이상 골라요").uiFont(12).foregroundStyle(Theme.warn)
                .padding(.top, 8)
                .opacity(!loading && !teams.isEmpty && on.isEmpty ? 1 : 0)
            HStack(spacing: 8) {
                Spacer()
                Button { dismiss() } label: { OutlineButton(label: String(localized: "취소")) }
                    .buttonStyle(PressStyle()).keyboardShortcut(.cancelAction)
                Button("저장") {
                    let muted = teams.map(\.id).filter { !on.contains($0) }
                    Task { await keeper.linear.setMuted(muted) }
                    dismiss()
                }
                .buttonStyle(InkButtonStyle())
                .keyboardShortcut(.defaultAction)
                .disabled(loading || teams.isEmpty || on.isEmpty)
                .opacity(loading || teams.isEmpty || on.isEmpty ? 0.35 : 1)
            }
            .padding(.top, 10)
        }
        .padding(22)
        .frame(width: 400)
        .background(Theme.paper)
        .task {
            teams = (try? await keeper.linearTeams()) ?? []
            let muted = Set(keeper.linear.saved?.muted ?? [])
            on = Set(teams.map(\.id)).subtracting(muted)
            loading = false
        }
    }

    /// One team: its name, how many issues, and an ink check when it is searched. The whole row toggles.
    private func row(_ t: LinearTeam) -> some View {
        let isOn = on.contains(t.id)
        return Button {
            if isOn { on.remove(t.id) } else { on.insert(t.id) }
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
                Text(t.name).uiFont(13.5, weight: .medium).foregroundStyle(isOn ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3)).lineLimit(1)
                Spacer(minLength: 8)
                Text(String(localized: "이슈 \(t.issues)개"))
                    .uiFont(12.5).foregroundStyle(Theme.ink3)
            }
            .padding(.horizontal, 10)
            .frame(height: 36)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

extension LinearStatus.State {
    var stage: SourceStage {
        switch self {
        case .off: .off
        case .waiting: .waiting
        case .failed: .failed
        case .lost: .lost
        case .syncing: .syncing
        case .connected: .connected
        }
    }
}
