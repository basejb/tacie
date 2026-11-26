import SwiftUI

/// Connections → GitHub: shows the device code and waits (in a .task, so leaving the page stops it) for the approval.
struct GitHubConnectPage: View {
    let context: SettingsContext
    let back: () -> Void
    let connected: () -> Void

    private enum Phase: Equatable {
        case starting
        case pasting  // 오픈소스판: a pasted token instead of the device code (TokenConnectBody)
        case waiting(DeviceCode)
        case failed(String)
        case pushing  // token saved; the engine has not taken it yet
        case notPushed
        case done  // the engine holds a working login: a beat of 연결했어요 before the repo picker

        /// The case alone: a new code or a moving wait is not a new page.
        var stage: String {
            switch self {
            case .starting: "starting"
            case .pasting: "pasting"
            case .waiting: "waiting"
            case .failed: "failed"
            case .pushing: "pushing"
            case .notPushed: "notPushed"
            case .done: "done"
            }
        }
    }

    @State private var phase = Phase.starting
    @State private var attempt = 0
    @State private var retry = 0
    @State private var copied = false
    @State private var paste: TokenConnectModel?
    /// The page went away: a token checked after that is not handed over and the page does not move on.
    @State private var left = false
    @Environment(\.reducesMotion) private var reduceMotion
    @Environment(\.displayScale) private var displayScale

    var body: some View {
        DetailHeader(back: String(localized: "settings.connections", defaultValue: "연결"), action: back)
        PageHeader(title: String(localized: "GitHub 연결"),
                   desc: PastedTokens.offered ? TokenCopy.desc(.github) : String(localized: "브라우저에서 GitHub에 로그인하고 아래 코드를 넣어 주세요."))
        VStack(alignment: .leading, spacing: 0) {
            // each step comes in like a page (docs/03 10번): a short rise in, a quick fade out
            content
                .id(phase.stage)
                .transition(.asymmetric(insertion: Theme.arrive(y: 4, reduce: reduceMotion, scale: displayScale),
                                        removal: .opacity.animation(Theme.swap)))
        }
            .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: phase.stage)
            .frame(maxWidth: .infinity, alignment: .leading)
            .task(id: attempt) { await run() }
            .task(id: retry) { if retry > 0 { await push() } }
            .onAppear { left = false }
            .onDisappear {
                left = true
                paste?.cancel()
            }
    }

    @ViewBuilder private var content: some View {
        switch phase {
        case .starting:
            ProgressView().controlSize(.small)
        case .pasting:
            if let paste { TokenConnectBody(model: paste, showsDesc: false) }
        case .waiting(let code):
            VStack(alignment: .leading, spacing: 18) {
                Text(code.userCode)
                    .uiFont(28, weight: .semibold, design: .monospaced)
                    .tracking(2)
                    .foregroundStyle(Theme.ink)
                    .textSelection(.enabled)
                HStack(spacing: 10) {
                    Button { copy(code.userCode) } label: {
                        OutlineButton(label: copied ? String(localized: "복사했어요") : String(localized: "코드 복사"), icon: copied ? "checkmark" : "doc.on.doc")
                    }
                    .buttonStyle(PressStyle())
                    .copiedFlash($copied)
                    Button {
                        copy(code.userCode)
                        NSWorkspace.shared.open(code.verificationURI)
                    } label: { StrongButton(label: String(localized: "GitHub 열기"), icon: "arrow.up.right") }
                        .buttonStyle(PressStyle())
                }
                HStack(alignment: .top, spacing: 8) {
                    ProgressView().controlSize(.small)
                    VStack(alignment: .leading, spacing: 3) {
                        Text("GitHub에서 승인을 기다리고 있어요.").uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink)
                        Text("코드는 15분 동안 쓸 수 있어요.")
                            .uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
        case .failed(let reason):
            VStack(alignment: .leading, spacing: 14) {
                Text(reason).uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink)
                Button { attempt += 1 } label: { OutlineButton(label: String(localized: "새 코드 받기")) }.buttonStyle(PressStyle())
            }
        case .pushing:
            ProgressView().controlSize(.small)
        case .notPushed:
            VStack(alignment: .leading, spacing: 14) {
                Text("연결을 아직 마치지 못했어요. 잠시 뒤에 다시 해 주세요.")
                    .uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink)
                Button { retry += 1 } label: { OutlineButton(label: String(localized: "다시 해 보기")) }.buttonStyle(PressStyle())
            }
        case .done:
            DoneMark(text: String(localized: "GitHub에 연결했어요"))
        }
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        copied = true
    }

    private func run() async {
        if PastedTokens.offered {
            // the token is checked and saved by the body; this page hands it to the engine and moves on as before
            let m = TokenConnectModel(source: .github,
                                      check: { s, t, e, site in await PastedTokens.check(s, token: t, email: e, site: site) },
                                      connect: TokenCopy.connector(for: .github, keeper: context.keeper),
                                      done: { if !left { Task { await push() } } })
            m.closeAfter = .zero
            paste = m
            phase = .pasting
            return
        }
        phase = .starting
        copied = false
        let device = GitHubDevice()
        do {
            let code = try await device.start()
            phase = .waiting(code)
            let token = try await device.token(for: code)
            do { try KeychainTokenStore.github.save(token) } catch {
                phase = .failed(String(localized: "이 Mac에 연결 정보를 저장하지 못했어요."))
                return
            }
            UserDefaults.standard.set(false, forKey: GitHubCredentials.disconnectedKeyInUse)  // connected again
            await push()
        } catch is CancellationError {
            return  // left the page
        } catch let e as DeviceFlowError {
            phase = .failed(Self.reason(e))
        } catch {
            phase = .failed(String(localized: "지금은 GitHub에 연결할 수 없어요."))
        }
    }

    /// Hands the saved token to the engine; moves on only when it holds a working login.
    private func push() async {
        phase = .pushing
        await context.keeper.pushCredentials()
        if Task.isCancelled { return }  // left the page; the token stays saved and pushed
        if context.keeper.connected {
            Analytics.capture(.githubConnected)
            phase = .done
            try? await Task.sleep(for: .seconds(1.6))
            if Task.isCancelled { return }  // left the page meanwhile; the connection stands
            connected()
        } else {
            phase = .notPushed
        }
    }

    private static func reason(_ e: DeviceFlowError) -> String {
        switch e {
        case .expired: String(localized: "코드를 쓸 수 있는 시간이 지났어요.")
        case .denied: String(localized: "GitHub에서 연결을 취소했어요.")
        case .network: String(localized: "인터넷 연결을 확인해 주세요.")
        case .unavailable: String(localized: "지금은 GitHub에 연결할 수 없어요.")
        }
    }
}

/// 연결 → 저장소: pick a repo and a branch, then sync it.
struct RepoPickerPage: View {
    let context: SettingsContext
    let back: () -> Void
    /// Opens the GitHub code page again. Offered when GitHub refuses the saved login (revoked or expired): the engine
    /// took the token, so 연결됨 shows, and only listing the repositories finds out.
    var reconnect: (() -> Void)? = nil
    /// The list's tallest height. The onboarding window is short: there the branch and the sync button must still show
    /// under the list without scrolling the page, or the button goes unseen.
    var listHeight: CGFloat = 280

    private enum Load {
        case loading
        case failed(String, login: Bool)
        case loaded(GitHubRepos)
    }

    @State private var load = Load.loading
    @State private var query = ""
    @State private var chosen: GitHubRepo?
    @State private var branches: [String] = []
    @State private var branch = ""
    @State private var reload = 0
    /// The org access page was opened: coming back to Tacie loads the list again.
    @State private var awaitingGrant = false

    private var keeper: IndexKeeper { context.keeper }

    var body: some View {
        DetailHeader(back: String(localized: "settings.connections", defaultValue: "연결"), action: back)
        PageHeader(title: String(localized: "어느 저장소에서 찾을까요?"),
                   desc: String(localized: "내 GitHub 계정으로 볼 수 있는 저장소예요. 고른 저장소의 코드, 문서, 이슈와 PR을 이 Mac에 받아서 찾아요."))
        VStack(alignment: .leading, spacing: 0) {
            switch load {
            case .loading:
                Text("저장소를 불러오는 중이에요").uiFont(12.5).foregroundStyle(Theme.ink3)
            case .failed(let why, let login):
                VStack(alignment: .leading, spacing: 14) {
                    Text(why).uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink)
                    if login, let reconnect {
                        Button(action: reconnect) { StrongButton(label: String(localized: "GitHub 다시 연결"), icon: "arrow.clockwise") }.buttonStyle(PressStyle())
                    } else {
                        Button { reload += 1 } label: { OutlineButton(label: String(localized: "다시 불러오기")) }.buttonStyle(PressStyle())
                    }
                }
            case .loaded(let r):
                loaded(r)
                    .onAppear { pickCurrent(in: r) }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .task(id: reload) { await loadRepos() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            // back from GitHub's org access page: the org's repositories may be there now
            if awaitingGrant { awaitingGrant = false; reload += 1 }
        }
    }

    @ViewBuilder private func loaded(_ r: GitHubRepos) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            // a pasted token (오픈소스판) has no Tacie app for an admin to approve
            ForEach(PastedTokens.offered ? [] : r.needsApproval, id: \.self) { org in
                SettingsRow(title: String(localized: "\(org) 저장소는 관리자 승인이 필요해요"),
                            desc: String(localized: "회사 GitHub 관리자가 Tacie를 승인해야 비공개 저장소를 받을 수 있어요.")) {
                    if let url = GitHubDevice.approvalPage {
                        Button { NSWorkspace.shared.open(url) } label: { OutlineButton(label: String(localized: "승인 요청하기")) }.buttonStyle(PressStyle())
                    }
                }
            }
            ForEach(r.needsSso, id: \.self) { org in
                SettingsRow(title: String(localized: "\(org) 저장소는 회사 SSO 로그인이 필요해요"),
                            desc: String(localized: "브라우저에서 회사 SSO로 로그인한 뒤 다시 불러와 주세요.")) { EmptyView() }
            }
            orgAccess.padding(.bottom, 6)
            if r.repos.isEmpty {
                Text("볼 수 있는 저장소가 없어요.").uiFont(12.5).foregroundStyle(Theme.ink3).padding(.vertical, 11)
            } else {
                search.padding(.vertical, 10)
                list(currentFirst(SettingsText.filter(r.repos, query: query)))
                if let chosen { choice(chosen) }
            }
        }
    }

    /// 조직 저장소가 안 보일 때: GitHub lists an org's repositories to Tacie only once Tacie may see that org. Its page
    /// for Tacie has a Grant (an org owner) or Request (anyone else, then the owner approves) button per org.
    @ViewBuilder private var orgAccess: some View {
        // TACIE_DEBUG_ORG_ACCESS=1 shows it in a development build on gh (captures)
        if Build.usesTacieLogin || (Build.isDev && ProcessInfo.processInfo.environment["TACIE_DEBUG_ORG_ACCESS"] == "1"),
           let page = GitHubDevice.approvalPage {
            HStack(alignment: .center, spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("조직 저장소가 안 보이나요?").uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink)
                    Text("GitHub에서 조직 옆의 Grant를 눌러요. 조직 관리자가 아니면 Request를 누르고, 관리자가 승인하면 보여요. 돌아오면 목록을 다시 불러와요.")
                        .uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 8)
                Button {
                    awaitingGrant = true
                    NSWorkspace.shared.open(page)
                } label: { OutlineButton(label: String(localized: "조직 권한 열기"), icon: "arrow.up.right") }
                    .buttonStyle(PressStyle()).fixedSize()
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.fill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        } else if PastedTokens.offered {
            // a fine-grained token sees one owner's repositories: the one picked when it was made
            Text("조직 저장소가 보이지 않으면 GitHub에서 새 토큰을 만들 때 Resource owner를 그 조직으로 골라 다시 연결해요. 조직이 토큰 승인을 요구하면 조직 관리자가 승인한 뒤에 보여요.")
                .uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
        } else {
            Text(SettingsText.orgHint).uiFont(12.5).foregroundStyle(Theme.ink3)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var search: some View {
        HStack(spacing: 7) {
            Image(systemName: "magnifyingglass").uiFont(12, weight: .medium).foregroundStyle(Theme.ink3)
            TextField("저장소 검색", text: $query).textFieldStyle(.plain).uiFont(13)
        }
        .padding(.horizontal, 10)
        .frame(height: 30)
        .background(Theme.ink.opacity(0.045), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private func list(_ repos: [GitHubRepo]) -> some View {
        ScrollView {
            VStack(spacing: 0) {
                ForEach(repos) { repo in
                    Button { pick(repo) } label: { row(repo) }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(isCurrent(repo) || repo.fullName == chosen?.fullName ? .isSelected : [])
                }
            }
        }
        .scrollIndicators(.never)
        .frame(maxHeight: listHeight)
    }

    /// The repo in use sits on top of the list, so the one already checked is the first thing seen.
    private func currentFirst(_ repos: [GitHubRepo]) -> [GitHubRepo] {
        repos.filter(isCurrent) + repos.filter { !isCurrent($0) }
    }

    /// Opens on the repo in use, already chosen: nothing to press when it stays as it is.
    private func pickCurrent(in r: GitHubRepos) {
        guard chosen == nil, let repo = r.repos.first(where: isCurrent) else { return }
        pick(repo)
    }

    /// The repo the search data is from. GitHub names ignore case (O/Old is o/old).
    private func isCurrent(_ repo: GitHubRepo) -> Bool {
        keeper.info?.repo.map { $0.caseInsensitiveCompare(repo.fullName) == .orderedSame } ?? false
    }

    private func row(_ repo: GitHubRepo) -> some View {
        let current = isCurrent(repo)
        let picked = repo.fullName == chosen?.fullName
        return HStack(spacing: 8) {
            Text(repo.fullName).uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink).lineLimit(1)
            if repo.private {
                Image(systemName: "lock").uiFont(10.5).foregroundStyle(Theme.ink3).accessibilityLabel("비공개")
            }
            Spacer(minLength: 8)
            if let d = SettingsText.pushed(repo.pushedAt, calendar: MainView.calendar) {
                Text(d).uiFont(12.5).foregroundStyle(Theme.ink3)
            }
            Image(systemName: "checkmark").uiFont(11, weight: .semibold).foregroundStyle(Theme.ink)
                .opacity(current ? 1 : 0)
                .accessibilityHidden(!current)
        }
        .padding(.horizontal, 10)
        .frame(height: 36)
        .background(picked ? SettingsStyle.selected : .clear, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .contentShape(Rectangle())
    }

    private func choice(_ repo: GitHubRepo) -> some View {
        let blocker = context.syncBlocker
        return VStack(alignment: .leading, spacing: 0) {
            SettingsRow(title: String(localized: "브랜치")) {
                Menu {
                    ForEach(branches, id: \.self) { b in
                        Button(Self.label(b, default: repo.defaultBranch)) { branch = b }
                    }
                } label: {
                    MenuText(label: Self.label(branch, default: repo.defaultBranch))
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
            }
            .padding(.top, 8)
            VStack(alignment: .leading, spacing: 6) {
                if let label = SettingsText.repoAction(current: isCurrent(repo), hasRepo: keeper.info?.repo != nil,
                                           sameBranch: branch == keeper.info?.branch) {
                    Button { sync(repo) } label: { StrongButton(label: label) }
                        .buttonStyle(PressStyle())
                        .disabled(blocker != nil || branch.isEmpty)
                        .opacity(blocker != nil || branch.isEmpty ? 0.4 : 1)
                    if let blocker { Text(blocker).uiFont(12.5).foregroundStyle(Theme.ink3) }
                } else {
                    Text("지금 이 저장소에서 찾고 있어요.")
                        .uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.top, 10)
        }
    }

    private static func label(_ b: String, default d: String) -> String { b == d ? String(localized: "\(b) (기본)") : b }

    private func loadRepos() async {
        guard let client = context.engine.client else {
            load = .failed(SettingsText.githubProblem(nil), login: false)
            return
        }
        load = .loading
        do {
            load = .loaded(try await client.githubRepos())
        } catch {
            // a cancelled request (URLError.cancelled) must not leave the page on "loading"
            if Task.isCancelled, (error is CancellationError || (error as? URLError)?.code == .cancelled) {
                if case .loading = load { load = .failed(SettingsText.githubProblem(nil), login: false) }
                return
            }
            let kind = (error as? EngineError)?.kind
            load = .failed(SettingsText.githubProblem(kind), login: kind == "key")
        }
    }

    private func pick(_ repo: GitHubRepo) {
        chosen = repo
        // the current repo starts on the branch it is synced on, any other on its default
        let synced = isCurrent(repo) ? keeper.info?.branch : nil
        branches = [repo.defaultBranch] + [synced].compactMap { $0 }.filter { $0 != repo.defaultBranch }
        branch = synced ?? repo.defaultBranch
        Task {
            guard let client = context.engine.client, let r = try? await client.githubBranches(repo.fullName),
                  chosen?.fullName == repo.fullName else { return }
            // the default first, so it is the top of the menu
            branches = [r.default] + r.branches.filter { $0 != r.default }
            branch = synced.flatMap { branches.contains($0) ? $0 : nil } ?? r.default
        }
    }

    private func sync(_ repo: GitHubRepo) {
        guard context.syncBlocker == nil, !branch.isEmpty else { return }
        let b = branch
        let same = isCurrent(repo) && b == keeper.info?.branch
        back()
        if same { return }
        Analytics.capture(.syncStarted(first: keeper.builtAt == nil || (keeper.info?.chunks ?? 0) == 0, privateRepo: repo.private))
        Task { await keeper.rebuild(repo: repo.fullName, branch: b, fresh: false) }
    }
}
