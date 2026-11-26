import SwiftUI

struct ConnectionsPage: View {
    let context: SettingsContext
    let open: (SettingsDetail) -> Void
    /// The row under the pointer, for the scene above the page (SettingsView draws it).
    @Binding var focus: ConnectionsScene.Source?

    private var keeper: IndexKeeper { context.keeper }
    private var ready: Bool { context.engine.state == .ready && context.engine.client != nil }
    private var syncing: Bool { keeper.step != nil }
    private var blocker: String? { context.syncBlocker }
    private var clearBlocker: String? { context.clearBlocker }

    @AppStorage("autoSync") private var autoSync = true
    @State private var askDisconnect = false
    @State private var askRebuild = false
    @State private var disconnectFailed = false  // the keychain kept the token; it stays here until the page closes
    /// Which section each source sits in, fixed the first time its state is known: a source connected while the page
    /// is open stays where it is (and shows its 연결했어요) instead of jumping to 연결됨; it moves the next time.
    @State private var placed: [ConnectionsScene.Source: Bool] = [:]

    private var headline: String {
        if let line = keeper.syncLine { return line }
        if keeper.cleared { return String(localized: "아직 동기화하지 않았어요") }
        if keeper.problem != nil { return String(localized: "동기화하지 못했어요 · 지난 데이터로 찾아요") }
        if let at = keeper.builtAt { return SettingsText.synced(at, now: Date(), calendar: MainView.calendar) }
        if keeper.info == nil { return String(localized: "검색 엔진이 준비되면 보여요") }
        return String(localized: "아직 동기화하지 않았어요")
    }

    private var detail: String? {
        if keeper.problem != nil {
            return SettingsText.githubProblem(keeper.problemKind)
        }
        if syncing { return nil }
        if keeper.cleared { return String(localized: "지금 동기화를 누르면 검색 데이터를 다시 만들어요.") }
        return keeper.info?.bytes.map { String(localized: "검색 데이터 \(SettingsText.size($0))는 이 Mac에 저장돼 있어요.") }
    }

    private func setAutoSync(_ on: Bool) {
        guard on != autoSync else { return }
        autoSync = on
        Analytics.capture(.settingChanged(setting: .autoSync, value: ASettingValue(on)))
    }

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                // while syncing, a band of light passes over the line (docs/03 13번); the page's one loop
                if syncing {
                    ShimmerText(text: headline, size: 13.5, weight: .medium, ink: AnyShapeStyle(Theme.ink2))
                } else {
                    Text(headline).uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink)
                }
                if let detail {
                    Text(detail).uiFont(12.5).foregroundStyle(Theme.ink3)
                }
            }
            Spacer()
            // GitHub refused the token (revoked, or the repo is out of reach): connect again from here
            if keeper.problemKind == "key", Build.usesTacieLogin, !GitHubDevice.clientID.isEmpty || PastedTokens.offered {
                Button { open(.githubConnect) } label: { OutlineButton(label: String(localized: "다시 연결")) }.buttonStyle(PressStyle())
            }
            let blocked = blocker != nil || !ready
            Button { context.actions.sync() } label: { StrongButton(label: String(localized: "지금 동기화")) }
                .buttonStyle(PressStyle())
                .disabled(blocked)
                .opacity(blocked ? 0.4 : 1)
        }
        .padding(.bottom, 30)
        .task { await keeper.check(auto: false) }

        let live = Self.linked(keeper)
        let joined = ConnectionsScene.Source.available.filter { (placed[$0] ?? live[$0]) == true }
        let rest = ConnectionsScene.Source.available.filter { (placed[$0] ?? live[$0]) != true }
        Group {
            if !joined.isEmpty {
                SettingsSection(title: String(localized: "연결됨")) { grid(joined) }
            }
            if !rest.isEmpty {
                SettingsSection(title: String(localized: "더 연결하기")) { grid(rest) }
            }
            if disconnectFailed {
                Text("이 Mac에서 연결 정보를 지우지 못했어요. 연결은 끊었어요.")
                    .uiFont(12.5).foregroundStyle(Theme.warn).padding(.top, 10)
            }
        }
        .onChange(of: live, initial: true) {
            for (s, on) in live where placed[s] == nil { if let on { placed[s] = on } }
        }

        SettingsSection(title: String(localized: "동기화")) {
            SettingsRow(title: String(localized: "자동 동기화"), desc: String(localized: "30분마다 바뀐 게 있는지 보고, 바뀌었으면 동기화해요. 회의 중이면 끝난 뒤에 해요.")) {
                Menu {
                    Button("켜짐") { setAutoSync(true) }
                    Button("꺼짐") { setAutoSync(false) }
                } label: {
                    MenuText(label: autoSync ? String(localized: "켜짐") : String(localized: "꺼짐"))
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
            }
        }
        if MCPAccess.offered {
            MCPSection(refresh: context.actions.mcpChanged)
        }
        // for when search goes wrong: rarely needed, so a line of links under the page instead of rows
        VStack(alignment: .leading, spacing: 8) {
            FooterLinks {
                Text("검색 결과가 이상하면").uiFont(12.5).foregroundStyle(Theme.ink3)
                FooterLink(label: String(localized: "검색 데이터 다시 만들기")) { askRebuild = true }
                    .disabled(blocker != nil)
                FooterLink(label: String(localized: "검색 데이터 지우기"), tint: Theme.warn) { open(.clearSearchData) }
                    .disabled(clearBlocker != nil)
            }
            if let why = blocker ?? clearBlocker {
                Text(why).uiFont(12.5).foregroundStyle(Theme.ink3)
            }
        }
        .alert("검색 데이터를 다시 만들까요?", isPresented: $askRebuild) {
            Button("다시 만들기") { Task { await keeper.rebuild(repo: nil, branch: nil, fresh: true) } }
            Button("취소", role: .cancel) {}
        } message: {
            Text("처음부터 다시 만들어요. 오래 걸릴 수 있고, 그동안은 지난 데이터로 찾아요.")
        }
        .alert("GitHub 연결을 해제할까요?", isPresented: $askDisconnect) {
            Button("연결 해제", role: .destructive) { disconnect() }
            Button("취소", role: .cancel) {}
        } message: {
            Text("검색 데이터는 그대로 남고 동기화만 멈춰요. GitHub에 남은 승인은 GitHub 설정의 Applications에서 Tacie 승인을 취소할 수 있어요.")
        }
    }

    /// Whether each source is connected right now; nil while it cannot be told yet (GitHub and the note folders wait for
    /// the engine's first answer).
    static func linked(_ keeper: IndexKeeper) -> [ConnectionsScene.Source: Bool?] {
        [
            .github: Build.usesTacieLogin ? (keeper.info == nil && !keeper.connected ? nil : keeper.connected) : true,
            .notion: keeper.notion.connected,
            .linear: keeper.linear.connected,
            .gdrive: keeper.gdrive.connected,
            .atlassian: keeper.atlassian.connected,
            .asana: keeper.asana.connected,
            .dropbox: keeper.dropbox.connected,
            .notes: keeper.notesInfo == nil ? nil : keeper.notesOn,
        ]
    }

    /// Two columns of sources; the one under the pointer lights its line in the scene.
    private func grid(_ list: [ConnectionsScene.Source]) -> some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 14, alignment: .top), GridItem(.flexible(), spacing: 14, alignment: .top)],
                  alignment: .leading, spacing: 6) {
            ForEach(list) { s in
                card(s)
                    .padding(.horizontal, 10).padding(.vertical, 9)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(focus == s ? Theme.ink.opacity(0.035) : .clear))
                    .contentShape(Rectangle())
                    .sceneFocus(s, $focus)
            }
        }
        .padding(.horizontal, -10)
        .padding(.top, 6)
    }

    @ViewBuilder private func card(_ s: ConnectionsScene.Source) -> some View {
        switch s {
        case .github:
            ConnectedCard(info: keeper.info, connected: keeper.connected, switching: keeper.switchingTo, blocker: blocker, connect: { open(.githubConnect) },
                          changeRepo: { open(.githubRepo) }, disconnect: { askDisconnect = true })
        case .notion: NotionSettingsCard(keeper: keeper)
        case .linear: LinearSettingsCard(keeper: keeper)
        case .gdrive: GDriveSettingsCard(keeper: keeper)
        case .atlassian: AtlassianSettingsCard(keeper: keeper)
        case .asana: AsanaSettingsCard(keeper: keeper)
        case .dropbox: DropboxSettingsCard(keeper: keeper)
        case .notes: NotesSettingsCard(keeper: keeper)
        }
    }

    /// Disconnects even when the keychain will not delete the token: the flag set first keeps it from the engine,
    /// so the push below hands over ("none", nil) either way (GitHubCredentials.resolve).
    private func disconnect() {
        UserDefaults.standard.set(true, forKey: GitHubCredentials.disconnectedKeyInUse)
        Log.info("github disconnected from settings")
        Analytics.capture(.sourceDisconnected(tool: .github))
        do { try KeychainTokenStore.github.delete() } catch { disconnectFailed = true }
        Task { await keeper.pushCredentials() }
    }
}

private struct ConnectedCard: View {
    let info: IndexState?
    let connected: Bool
    let switching: String?  // the repo a running sync moves to
    let blocker: String?
    let connect: () -> Void
    let changeRepo: () -> Void
    let disconnect: () -> Void

    /// Tacie's own login is the one in use and there is no connection yet.
    private var needsConnect: Bool { Build.usesTacieLogin && !connected }

    /// The engine sends the connection fields (an older one leaves them all out).
    private var known: Bool { info?.files != nil }

    private var second: String? {
        if let switching { return SettingsText.switching(to: switching) }
        guard known, let info else { return String(localized: "검색 엔진이 준비되면 보여요") }
        switch (info.repo, info.branch) {
        case let (r?, b?): return String(localized: "\(r) · \(b) 브랜치")
        case let (r?, nil): return r
        case let (nil, b?): return String(localized: "\(b) 브랜치")
        default: return nil
        }
    }

    /// Connected, but no repository picked yet (the engine knows and says none).
    private var noRepo: Bool { known && switching == nil && info?.repo == nil }

    var body: some View {
        if needsConnect { notConnected } else { card }
    }

    private var notConnected: some View {
        HStack(alignment: .center, spacing: 12) {
            LogoTile(logo: "ServiceGitHub")
            VStack(alignment: .leading, spacing: 3) {
                Text("GitHub").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                Text("연결되지 않았어요").uiFont(12.5).foregroundStyle(Theme.ink2)
            }
            Spacer()
            if !GitHubConnectOffer.canConnect(clientID: GitHubDevice.clientID, pasted: PastedTokens.offered) {
                Text(Build.isDev ? "개발 빌드에 Client ID가 없어요" : String(localized: "아직 GitHub 연결을 쓸 수 없어요"))
                    .uiFont(12).foregroundStyle(Theme.ink3)
            } else {
                Button(action: connect) { OutlineButton(label: String(localized: "연결")) }.buttonStyle(PressStyle())
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)  // a cell of 설정 → 연결's two columns, which draw the row
    }

    @ViewBuilder private var card: some View {
        let login = known ? (info?.githubLogin ?? String(localized: "로그인 안 됨")) : nil
        HStack(alignment: .top, spacing: 12) {
            LogoTile(logo: "ServiceGitHub")
            VStack(alignment: .leading, spacing: 3) {
                // the name never wraps; a long login shortens instead (a narrow column broke "GitHub" in two)
                HStack(spacing: 6) {
                    Text("GitHub").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink).fixedSize()
                    if let login { Text(login).uiFont(12.5).foregroundStyle(Theme.ink3).lineLimit(1).truncationMode(.middle) }
                }
                if let second { Text(second).uiFont(12.5).foregroundStyle(Theme.ink2) }
                if noRepo {
                    Text("아직 저장소를 고르지 않았어요").uiFont(12.5).foregroundStyle(Theme.ink2)
                    // connected without a repository picked: nothing is searched yet, so picking one is the step left.
                    // Under the line, not beside the name, so the column keeps its width for the words.
                    Button(action: changeRepo) { OutlineButton(label: String(localized: "저장소 고르기")) }.buttonStyle(PressStyle())
                        .fixedSize()
                        .disabled(blocker != nil)
                        .padding(.top, 6)
                }
                if known, !noRepo, let h = SettingsText.holdings(files: info?.files, items: info?.githubItems) {
                    Text(h).uiFont(12.5).foregroundStyle(Theme.ink3)
                }
                if info?.githubLost == true {
                    Text(SettingsText.githubLost).uiFont(12.5).foregroundStyle(Theme.ink2)
                }
                if known, info?.githubLogin == nil, !Build.usesTacieLogin {
                    Text(verbatim: "개발 빌드는 터미널의 gh auth login으로 로그인해요.").uiFont(12).foregroundStyle(Theme.ink3)
                }
            }
            Spacer()
            Menu {
                Button("저장소 바꾸기", action: changeRepo).disabled(blocker != nil)
                if Build.usesTacieLogin { Button("연결 해제", role: .destructive, action: disconnect) }
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
        .frame(maxWidth: .infinity, alignment: .leading)  // a cell of 설정 → 연결's two columns, which draw the row
    }
}

private struct AddCard: View {
    let symbol: String
    let name: String
    let line: String
    var body: some View {
        HStack(spacing: 10) {
            LogoTile(symbol: symbol, size: 30)
            VStack(alignment: .leading, spacing: 2) {
                Text(name).uiFont(13, weight: .semibold).foregroundStyle(Theme.ink)
                Text(line).uiFont(11.5).foregroundStyle(Theme.ink3).lineLimit(1)
            }
            Spacer(minLength: 8)
            SoonTag()
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.paper, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Theme.ink.opacity(0.1)))
    }
}
