import SwiftUI

/// What the onboarding can ask the app to do.
struct OnboardingActions {
    let openMain: () -> Void
    let copyNotice: () -> Void
    let close: () -> Void
    /// The practice steps move the window out of the island's way.
    let stepAside: (Bool) -> Void
}

/// The onboarding window's content, 540 × 620 (docs/11 4절, the 2026-10-26 renders).
struct OnboardingView: View {
    @Bindable var model: OnboardingModel
    let settings: SettingsContext
    let actions: OnboardingActions

    var body: some View {
        VStack(spacing: 0) {
            if model.step != .welcome { bar }
            Group {
                switch model.step {
                case .welcome: WelcomeStep(next: model.next)
                case .account: if Build.isOpen { AIChoiceStep(model: model, choice: AIChoiceModel(context: settings)) } else { AccountStep(model: model) }
                case .sources: SourcesStep(model: model, settings: settings)
                case .listen: ListenStep(model: model, stt: settings.localSTT, copyNotice: actions.copyNotice)
                case .ask: PracticeStep(model: model, step: .ask)
                case .meeting: PracticeStep(model: model, step: .meeting)
                case .done: DoneStep(model: model, openMain: { actions.openMain(); actions.close() })
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            if model.step != .welcome { SyncLine(keeper: model.keeper, download: model.download, done: model.step == .done) }
        }
        .frame(width: 540, height: 620)
        .background(Theme.paper)
        .animation(Theme.move, value: model.step)
        .onChange(of: model.step, initial: true) { _, step in actions.stepAside(step == .ask || step == .meeting) }
    }

    /// The row under the traffic lights: back at the content's left edge, later at its right, the dots in the centre.
    private var bar: some View {
        ZStack {
            HStack(spacing: 6) {
                ForEach(model.dots, id: \.self) { s in
                    Capsule().fill(s == model.step ? Theme.ink : Theme.ink.opacity(0.14)).frame(width: s == model.step ? 16 : 6, height: 6)
                }
            }
            .accessibilityLabel(String(localized: "\(model.dots.count)단계 중 \((model.dots.firstIndex(of: model.step) ?? 0) + 1)단계"))
            HStack {
                Button("뒤로") { model.back() }.buttonStyle(.plain).font(Theme.button).foregroundStyle(Theme.ink3)
                Spacer()
                if model.step != .done {
                    Button("나중에") { model.later() }.buttonStyle(.plain).font(Theme.button).foregroundStyle(Theme.ink3)
                }
            }
            .padding(.horizontal, 32)  // the steps' own side margin
        }
        .frame(height: 52)
    }
}

/// A heading with the face beside it, the same on every step.
private struct StepTitle: View {
    let mood: EyeMood
    let title: String
    let sub: String
    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            LitHead(channels: EyeRig.target(for: mood, remote: false), height: 30)
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.system(size: 22, weight: .bold)).tracking(-0.4).foregroundStyle(Theme.ink)
                Text(sub).font(.system(size: 13.5)).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

// MARK: - 0 welcome

private struct WelcomeStep: View {
    let next: () -> Void
    @State private var shown = 0
    @Environment(\.reducesMotion) private var reduce

    var body: some View {
        VStack(spacing: 0) {
            Spacer()
            GazeHead(mood: shown == 0 ? .asleep : .listening, height: 78).padding(.bottom, 30)
            Group {
                Text("회의 중에 답하는 회의 어시스턴트").font(.system(size: 26, weight: .bold)).tracking(-0.5)
                    .opacity(shown >= 1 ? 1 : 0)
                Text("회의에서 나온 질문에, 팀 코드와 문서로 답해요.")
                    .font(.system(size: 15)).foregroundStyle(Theme.ink2).multilineTextAlignment(.center)
                    .padding(.top, 10).opacity(shown >= 2 ? 1 : 0)
            }
            .foregroundStyle(Theme.ink)
            Spacer()
            Button("시작하기", action: next).buttonStyle(InkButtonStyle()).keyboardShortcut(.defaultAction)
                .opacity(shown >= 3 ? 1 : 0)
        }
        .padding(.bottom, 30)
        .task {
            // the face wakes, then the lines come in one after another (no-rush)
            if reduce { shown = 3; return }
            for i in 1...3 {
                try? await Task.sleep(for: .milliseconds(i == 1 ? 700 : 420))
                withAnimation(Theme.move) { shown = i }
            }
        }
    }
}

// MARK: - 0-1 account

/// 로그인 (docs/14): Tacie AI works once the account is in. Waiting for approval still moves on; the island says so.
/// 오픈소스판 2-D: 로그인 자리의 "어떻게 쓸까요?". 고르지 않고 넘어가도 돼요(남은 일에 "AI 고르기"가 남아요).
private struct AIChoiceStep: View {
    @Bindable var model: OnboardingModel
    @State var choice: AIChoiceModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            StepTitle(mood: .listening, title: String(localized: "어떻게 쓸까요?"),
                      sub: String(localized: "나중에 설정에서 언제든 바꿀 수 있어요."))
                .padding(.bottom, 16)
            // the same scene as 설정 → AI: where the meeting's sound and text go with the card in view
            AIPathScene(local: choice.open == .local, keyOn: choice.keyOn, short: true).padding(.bottom, 12)
            AIChoiceCards(model: choice)
            Spacer(minLength: 0)
            HStack {
                if choice.mode == nil {
                    Button("나중에 할게요") { model.next() }.buttonStyle(.plain).font(Theme.button).foregroundStyle(Theme.ink3)
                }
                Spacer()
                Button("계속") { model.next() }.buttonStyle(InkButtonStyle())
            }
            .padding(.bottom, 22)
        }
        .padding(.horizontal, 32)
        .padding(.top, 14)
    }
}

private struct AccountStep: View {
    @Bindable var model: OnboardingModel
    private var account: TacieAccount { TacieAccount.shared }

    var body: some View {
        let signedIn = account.signedIn
        VStack(alignment: .leading, spacing: 0) {
            StepTitle(mood: signedIn ? .happy : .listening, title: String(localized: "Tacie에 로그인해요"),
                      sub: String(localized: "로그인하면 바로 회의를 듣고 답할 수 있어요."))
                .padding(.bottom, 22)
            VStack(alignment: .leading, spacing: 14) {
                state
                VStack(alignment: .leading, spacing: 2) {
                    Text("어느 계정이든 이름과 이메일만 받아요. 회의 내용은 서버에 남기지 않아요.")
                    Text("같은 이메일이면 어느 쪽으로 들어와도 같은 계정이에요.")
                    Text("기능을 쓴 횟수만 통계로 보내요. 자세한 건 설정 → 개인정보에 있어요.")
                }
                .font(.system(size: 11.5)).foregroundStyle(Theme.ink3)
            }
            Spacer(minLength: 0)
            HStack {
                Spacer()
                Button("계속") { model.next() }.buttonStyle(InkButtonStyle()).disabled(!signedIn).opacity(signedIn ? 1 : 0.35)
            }
            .padding(.bottom, 22)
        }
        .padding(.horizontal, 32)
        .padding(.top, 14)
        .task { await account.refresh() }
    }

    @ViewBuilder private var state: some View {
        switch (account.signedIn, account.phase) {
        case (false, .idle):
            SignInButtons()
        case (false, .failed(let why)):
            Text(why).font(.system(size: 11.5)).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            SignInButtons()
        default:
            VStack(alignment: .leading, spacing: 8) { status }
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.paper))
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .strokeBorder(account.signedIn ? Theme.ink.opacity(0.35) : Theme.ink.opacity(0.1)))
        }
    }

    @ViewBuilder private var status: some View {
        if account.signedIn {
            let waiting = account.info?.status == "waiting"
            HStack(spacing: 6) {
                Image(systemName: waiting ? "clock" : "checkmark.circle.fill").font(.system(size: 11))
                    .foregroundStyle(waiting ? AnyShapeStyle(Theme.ink3) : AnyShapeStyle(Theme.live))
                Text(waiting ? String(localized: "로그인했어요 · 승인을 기다리고 있어요")
                     : account.info?.email.map { String(localized: "로그인했어요 · \($0)") } ?? String(localized: "로그인했어요"))
                    .font(.system(size: 11.5, weight: .medium)).foregroundStyle(Theme.ink2).lineLimit(1)
                Spacer(minLength: 0)
                if waiting {
                    Button("다시 확인") { Task { await account.refresh() } }.buttonStyle(.plain)
                        .font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.ink)
                }
            }
            if waiting {
                Text("지금은 승인받은 분만 쓸 수 있어요. 승인되면 앱을 다시 열지 않아도 바로 써요.")
                    .font(.system(size: 11)).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
            }
        } else {
            switch account.phase {
            case .signingIn:
                HStack(spacing: 6) {
                    ProgressView().controlSize(.mini)
                    Text("\(account.tried.label) 창에서 계정을 골라 주세요").font(.system(size: 11.5, weight: .medium)).foregroundStyle(Theme.ink2)
                    Spacer(minLength: 0)
                    Button("취소") { account.cancelSignIn() }.buttonStyle(.plain)
                        .font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.ink3)
                }
            case .failed, .idle:
                EmptyView()
            }
        }
    }
}

// MARK: - 1 sources

private struct Tool: Identifiable {
    let id: String
    let name: String
    let what: String
    let symbol: String
}

private let soonTools = [
    Tool(id: "slack", name: String(localized: "슬랙"), what: String(localized: "채널에서 나눈 대화"), symbol: "number"),
]

private struct SourcesStep: View {
    @Environment(\.colorScheme) private var scheme
    @Bindable var model: OnboardingModel
    let settings: SettingsContext

    var body: some View {
        switch model.sourcesPage {
        case .github:
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    GitHubConnectPage(context: settings, back: { model.sourcesPage = .tools },
                                      connected: {
                        // connected again with a repo already chosen: nothing to pick, sync it and move on
                        if settings.keeper.info?.repo != nil {
                            model.sourcesPage = .tools
                            settings.actions.sync()
                        } else {
                            model.sourcesPage = .repos
                        }
                    })
                }
                .padding(.horizontal, 32)
            }
        case .repos:
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    RepoPickerPage(context: settings, back: { model.sourcesPage = .tools },
                                   reconnect: { model.sourcesPage = .github }, listHeight: 200)
                }
                .padding(.horizontal, 32)
            }
        case .tools:
            tools
        }
    }

    private var tools: some View {
        // a repository picked and syncing counts: the user did their part, the rest runs in the background
        let connected = model.facts.hasSource || model.keeper.firstSync != nil
        return VStack(alignment: .leading, spacing: 0) {
            StepTitle(mood: connected ? .happy : .listening, title: String(localized: "어디에서 답을 찾을까요?"),
                      sub: String(localized: "팀 코드와 문서, 내 노트를 연결하면 거기서 답을 찾아요."))
                .padding(.bottom, 22)
            // nine tiles are taller than the 620-point window: the tiles scroll, the title and 계속 stay put, and the
            // bottom edge fades so a half-shown row reads as "more below" (docs/34 7-2절)
            ScrollView {
                // a Grid, not a LazyVGrid: the two tiles of a row take the taller one's height, top-aligned
                // only what this edition can connect (the open edition has no Tacie server or connection apps)
                let every: [ConnectionsScene.Source: AnyView] = [
                    .github: AnyView(github), .notion: AnyView(NotionTile(keeper: model.keeper)),
                    .linear: AnyView(LinearTile(keeper: model.keeper)), .gdrive: AnyView(GDriveTile(keeper: model.keeper)),
                    .atlassian: AnyView(AtlassianTile(keeper: model.keeper)), .asana: AnyView(AsanaTile(keeper: model.keeper)),
                    .dropbox: AnyView(DropboxTile(keeper: model.keeper)), .notes: AnyView(NotesTile(keeper: model.keeper))]
                let tiles: [AnyView] = ConnectionsScene.Source.available.compactMap { every[$0] }
                    + (Build.isOpen ? [] : soonTools.map { AnyView(soon($0)) })  // "알림 받기" is the official app's news
                Grid(horizontalSpacing: 10, verticalSpacing: 10) {
                    ForEach(0..<(tiles.count + 1) / 2, id: \.self) { r in
                        GridRow(alignment: .top) {
                            tiles[r * 2]
                            if r * 2 + 1 < tiles.count { tiles[r * 2 + 1] } else { Color.clear.gridCellUnsizedAxes([.horizontal, .vertical]) }
                        }
                    }
                }
                .padding(.bottom, 18)
                .padding(.horizontal, 32)
            }
            // the scroll area runs to the window's edges so the scroller sits in the side margin, not on the tiles
            .padding(.horizontal, -32)
            .scrollIndicators(.automatic)
            .mask(VStack(spacing: 0) {
                Rectangle()
                LinearGradient(colors: [.black, .clear], startPoint: .top, endPoint: .bottom).frame(height: 24)
            })
            HStack {
                Spacer()
                Button("계속") { model.next() }.buttonStyle(InkButtonStyle()).disabled(!connected && model.progress.notify.isEmpty)
                    .opacity(!connected && model.progress.notify.isEmpty ? 0.35 : 1)
            }
            .padding(.top, 12)
            .padding(.bottom, 22)
        }
        .padding(.horizontal, 32)
        .padding(.top, 14)
    }

    private var github: some View {
        // the repository picked a moment ago shows at once, while its first sync runs
        let repo = model.keeper.info?.repo ?? model.keeper.firstSync
        let connected = model.keeper.connected
        let branch = model.keeper.info?.branch
        let repoLine = repo.map { r in branch.map { "\(r) · \($0)" } ?? r }
        // the same shape as the other tiles (2026-11-13): the name and what it holds, then two status lines; the
        // repository is changed from ⋯ like the other tiles' 폴더 고르기
        return VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 0) {
                label("GitHub", String(localized: "코드, 문서, 이슈와 PR"), symbol: "chevron.left.forwardslash.chevron.right", logo: "ServiceGitHub", live: true)
                if connected {
                    TileMenu {
                        Button(repo == nil ? String(localized: "저장소 고르기") : String(localized: "저장소 바꾸기")) { model.sourcesPage = .repos }
                        Button("다시 연결") { model.sourcesPage = .github }
                    }
                }
            }
            if connected, let repoLine, let line = model.keeper.stepLine {
                // syncing: which repository, and how far
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    ProgressView().controlSize(.mini).scaleEffect(0.8)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(verbatim: repoLine).uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2).lineLimit(1).help(repoLine)
                        Text("동기화 중 · \(line)").uiFont(11).foregroundStyle(Theme.ink3).lineLimit(1)
                    }
                }
            } else if connected, let repoLine {
                // the engine says connected while it holds any login; no login name means GitHub refused it (or could
                // not be asked), so say so and offer the code again instead of a plain 연결됨
                if let login = model.keeper.info?.githubLogin {
                    TileStatusLines(name: login, detail: repoLine)
                } else {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Image(systemName: "exclamationmark.circle").uiFont(11, weight: .semibold).foregroundStyle(Theme.ink3)
                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 6) {
                                Text("로그인을 확인하지 못했어요").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2).lineLimit(1)
                                Spacer(minLength: 6)
                                Button("다시 연결") { model.sourcesPage = .github }.buttonStyle(.plain)
                                    .uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink).fixedSize()
                            }
                            Text(verbatim: repoLine).uiFont(11).foregroundStyle(Theme.ink3).lineLimit(1)
                        }
                    }
                }
            } else {
                Button(connected ? String(localized: "저장소 고르기") : String(localized: "연결")) { model.sourcesPage = connected ? .repos : .github }
                    .buttonStyle(InkButtonStyle())
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 100, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.paper))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
            .strokeBorder(connected && repo != nil ? Theme.ink.opacity(0.35) : Theme.ink.opacity(0.1)))
    }

    private func soon(_ t: Tool) -> some View {
        let wanted = model.progress.notify.contains(t.id)
        return VStack(alignment: .leading, spacing: 10) {
            label(t.name, t.what, symbol: t.symbol, live: false)
            Button { model.toggleNotify(t.id) } label: {
                HStack(spacing: 5) {
                    Image(systemName: wanted ? "bell.fill" : "bell").font(.system(size: 10, weight: .semibold))
                    Text(wanted ? String(localized: "준비되면 알려 드릴게요") : String(localized: "준비 중 · 알림 받기")).font(.system(size: 11.5, weight: .medium))
                }
                .foregroundStyle(wanted ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3))
                .padding(.horizontal, 10).frame(height: 26)
                .background(wanted ? Theme.ink.opacity(0.08) : Color.clear, in: Capsule())
                .overlay(Capsule().strokeBorder(wanted ? Color.clear : Theme.ink.opacity(0.12)))
            }
            .buttonStyle(.plain)
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 100, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.paper))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.ink.opacity(0.1)))
    }

    /// `logo`: a service's own mark from the asset catalog (as in 설정 → 연결's LogoTile), drawn as is.
    private func label(_ name: String, _ what: String, symbol: String, logo: String? = nil, live: Bool) -> some View {
        HStack(spacing: 10) {
            Group {
                if let logo {
                    Image(ServiceLogo.name(logo, scheme)).resizable().scaledToFit().frame(width: 17, height: 17)
                } else {
                    Image(systemName: symbol).font(.system(size: 13, weight: .semibold)).foregroundStyle(live ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3))
                }
            }
            .frame(width: 20, height: 20)
            VStack(alignment: .leading, spacing: 1) {
                Text(name).font(.system(size: 13.5, weight: .semibold)).foregroundStyle(live ? Theme.ink : Theme.ink2)
                Text(what).font(.system(size: 11.5)).foregroundStyle(Theme.ink3)
            }
            Spacer(minLength: 0)
        }
    }
}

// MARK: - 2 listen

private struct ListenStep: View {
    @Bindable var model: OnboardingModel
    let stt: LocalSTT
    let copyNotice: () -> Void
    @State private var copied = false

    var body: some View {
        let micOK = model.mic == .authorized
        let stuck = model.callAudio == .silent
        return VStack(alignment: .leading, spacing: 0) {
            StepTitle(mood: stuck ? .unsure : micOK && model.level > 0.15 ? .heard : .listening,
                      title: String(localized: "회의를 들을 준비를 해요"), sub: String(localized: "녹음은 켤 때만 이 Mac에 남아요."))
                .padding(.bottom, 22)
            // one list, a row per voice: who it hears on the left, the state or the button on the right edge;
            // anything longer than that goes under the list, full width
            VStack(spacing: 0) {
                row(symbol: "mic", who: String(localized: "내 목소리"), source: String(localized: "마이크"), tag: nil, on: micOK && model.micConnected) { micAction }
                Divider().opacity(0.5)
                row(symbol: "speaker.wave.2", who: String(localized: "상대 목소리"), source: String(localized: "회의 앱 소리 · 화면은 보지 않아요"), tag: String(localized: "onboarding.remoteTag", defaultValue: "원격 회의"),
                    on: model.callAudio == .granted) { callAction }
            }
            .padding(.horizontal, 16)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.ink.opacity(0.1)))
            // the open edition chose how to transcribe one step earlier ("어떻게 쓸까요?", 2-D)
            if stt.phase != .unsupported && !Build.isOpen { OnboardingSTTRow(stt: stt) }
            micNote
            callNote
            if stuck { StuckBox(model: model).padding(.top, 12) }
            if micOK, model.callAudio == .granted || model.progress.callAudioLater {
                notice.padding(.top, 14)
            }
            Spacer(minLength: 0)
            HStack {
                if model.callAudio != .granted, !model.progress.callAudioLater {
                    Button("대면 회의만 쓸게요") { model.callAudioLater() }.buttonStyle(.plain).font(Theme.button).foregroundStyle(Theme.ink3)
                }
                Spacer()
                Button("계속") { model.next() }.buttonStyle(InkButtonStyle()).disabled(!OnboardingFlow.listenDone(model.facts))
                    .opacity(OnboardingFlow.listenDone(model.facts) ? 1 : 0.35)
            }
            .padding(.bottom, 22)
        }
        .padding(.horizontal, 32)
        .padding(.top, 14)
        .onAppear { model.refreshPermissions(); model.startMeter() }
        .onDisappear { model.stopMeter() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            // back from System Settings: the switch may be on now
            model.refreshPermissions()
            model.startMeter()
        }
    }

    @ViewBuilder private var micAction: some View {
        if model.mic == .authorized && !model.micConnected {
            status("exclamationmark.circle", String(localized: "마이크가 없어요"), ok: false)
        } else if model.mic == .authorized {
            MicMeter(level: model.level)
            if model.heardVoice { DoneMark(text: String(localized: "들려요")) } else { status("checkmark.circle.fill", String(localized: "말해 보세요"), ok: true) }
        } else if model.mic == .notDetermined {
            Button("켜기") { Task { await model.turnOnMic() } }.buttonStyle(GhostButtonStyle())
        } else {
            Button("시스템 설정에서 켜기") { Task { await model.turnOnMic() } }.buttonStyle(InkButtonStyle())
        }
    }

    @ViewBuilder private var callAction: some View {
        if model.probing {
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("허용을 기다려요").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.ink2)
            }
            .frame(height: 30)
        } else if model.callAudio == .granted {
            DoneMark(text: String(localized: "소리가 들어와요"))
        } else if model.progress.callAudioLater {
            Button("지금 켜기") { Task { await model.turnOnCallAudio() } }.buttonStyle(GhostButtonStyle())
        } else {
            Button(model.callAudio == .silent ? String(localized: "다시 확인") : String(localized: "켜기")) { Task { await model.turnOnCallAudio() } }
                .buttonStyle(GhostButtonStyle())
        }
    }

    /// What does not fit in a half-width tile: the reason and the second button, under both tiles.
    @ViewBuilder private var micNote: some View {
        if model.mic == .authorized && !model.micConnected {
            note(String(localized: "연결된 마이크가 없어요. 마이크나 이어폰을 연결해 주세요."))
        } else if model.mic == .denied || model.mic == .restricted {
            note(String(localized: "권한 창을 다시 띄우려면 Tacie를 다시 열어야 해요.")) {
                Button("Tacie 다시 열기") { model.askMicAgain() }.buttonStyle(GhostButtonStyle())
                    .help("다시 열면 지금 단계로 돌아와요.")
            }
        }
    }

    @ViewBuilder private var callNote: some View {
        if model.probing {
            TimelineView(.periodic(from: .now, by: 1)) { ctx in
                let long = ctx.date.timeIntervalSince(model.probeStarted ?? ctx.date) > 30
                note(long ? String(localized: "권한 창이 안 보이면 다른 창 뒤에 있을 수 있어요. 시스템 설정에서 직접 켜도 돼요.")
                          : String(localized: "권한 창이 뜨면 허용을 눌러 주세요. 작은 확인음이 나요.")) {
                    if long { Button("그만 기다리기") { model.stopWaiting() }.buttonStyle(GhostButtonStyle()) }
                }
            }
        } else if model.progress.callAudioLater && model.callAudio != .granted {
            note(String(localized: "원격 회의를 시작할 때 다시 알려 드릴게요."))
        }
    }

    private func status(_ symbol: String, _ text: String, ok: Bool) -> some View {
        HStack(spacing: 5) {
            Image(systemName: symbol).font(.system(size: 12, weight: ok ? .regular : .semibold)).foregroundStyle(ok ? AnyShapeStyle(Theme.live) : AnyShapeStyle(Theme.ink3))
            Text(text).font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.ink2)
        }
        .frame(height: 30)
    }

    private func note(_ text: String) -> some View { note(text) { EmptyView() } }

    private func note<B: View>(_ text: String, @ViewBuilder button: () -> B) -> some View {
        HStack(spacing: 10) {
            Text(text).font(.system(size: 12)).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            button()
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.fill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .padding(.top, 10)
    }

    private func row<Action: View>(symbol: String, who: String, source: String, tag: String?, on: Bool,
                                   @ViewBuilder action: () -> Action) -> some View {
        HStack(spacing: 12) {
            Image(systemName: symbol).font(.system(size: 14, weight: .semibold))
                .foregroundStyle(on ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3))
                .frame(width: 32, height: 32).background(Theme.fill, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(who).font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.ink)
                    if let tag {
                        Text(tag).font(.system(size: 10.5, weight: .medium)).foregroundStyle(Theme.ink3)
                            .padding(.horizontal, 6).frame(height: 17).background(Theme.fill, in: Capsule())
                    }
                }
                Text(source).font(.system(size: 12)).foregroundStyle(Theme.ink3)
            }
            Spacer(minLength: 8)
            HStack(spacing: 10) { action() }
        }
        .frame(minHeight: 64)
    }

    private var notice: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("회의 전에 참석자에게 알려 주세요").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.ink)
            Text("Tacie가 듣고 있다는 걸 모두가 알아야 해요.")
                .font(.system(size: 12.5)).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            Button { copyNotice(); copied = true } label: {
                HStack(spacing: 5) {
                    Image(systemName: copied ? "checkmark" : "doc.on.doc").font(.system(size: 10.5, weight: .semibold))
                        .contentTransition(.symbolEffect(.replace))
                    Text(copied ? String(localized: "복사했어요") : String(localized: "알릴 문구 복사")).font(.system(size: 12, weight: .semibold))
                }
                .foregroundStyle(Theme.ink).padding(.horizontal, 10).frame(height: 26).background(Theme.fill, in: Capsule())
            }
            .buttonStyle(.plain)
            .copiedFlash($copied)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.fill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

/// The level bars beside "들려요".
private struct MicMeter: View {
    let level: Float
    var body: some View {
        HStack(spacing: 3) {
            ForEach(0..<14, id: \.self) { i in
                Capsule().fill(Float(i) / 14 < level ? Theme.eyeGlow : Theme.fill).frame(width: 4, height: 6 + CGFloat(i % 5) * 2.5)
            }
        }
        .animation(.linear(duration: 0.08), value: level)
        .accessibilityHidden(true)
    }
}

/// The permission is on but the meeting app's sound is silence (docs/11 원칙 9).
private struct StuckBox: View {
    let model: OnboardingModel
    @State private var copied = false
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("확인음이 들리지 않았어요. 시스템 설정에서 Tacie의 시스템 오디오 녹음이 켜져 있는지 봐 주세요.")
                .font(.system(size: 12.5)).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            Text(model.diagnostics).font(.system(size: 11.5, design: .monospaced)).foregroundStyle(Theme.ink2)
                .padding(10).frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.fill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .textSelection(.enabled)
            HStack(spacing: 8) {
                Button("시스템 설정 열기") { OnboardingModel.openPrivacy("Privacy_ScreenCapture") }.buttonStyle(InkButtonStyle())
                Button("권한 다시 받기") { Task { await model.askCallAudioAgain() } }.buttonStyle(GhostButtonStyle())
                Button(copied ? String(localized: "복사했어요") : String(localized: "진단 정보 복사")) {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(model.diagnostics, forType: .string)
                    copied = true
                }
                .buttonStyle(GhostButtonStyle())
                .copiedFlash($copied)
            }
        }
    }
}

// MARK: - 3·4 practice on the real island

/// 직접 물어보기 (3) and 회의 연습 (4): the window has moved aside and the real island answers. Nothing to think up
/// or type: 3 is a question button that goes into the island, 4 plays a spoken question into a short practice meeting
/// (no microphone needed). The shortcuts are only shown for later (docs/11 원칙 1, 6; 2026-10-26 사용자: 직접 묻게 하는
/// 건 기대가 너무 커요).
private struct PracticeStep: View {
    let model: OnboardingModel
    let step: OnboardingStep

    private var ask: Bool { step == .ask }
    private var state: OnboardingModel.Practice { model.practice(step) }
    private var meetingOn: Bool { model.meeting?.isRunning ?? false }
    /// No AI key: the practice cannot run, so the key comes first and 계속 is not held back.
    private var needsKey: Bool { model.aiKeyMissing == true }
    /// 계속 waits for a card, unless the practice cannot give one (no key, or it failed).
    private var canGoOn: Bool { state.done || needsKey || state == .missed }
    @State private var keyDraft = ""
    @State private var keyNote: String?
    @State private var savingKey = false
    /// The question last pressed: its row shows the search and then the result in place of the arrow.
    @State private var picked: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            StepTitle(mood: mood, title: ask ? String(localized: "직접 물어보세요") : String(localized: "누가 물어도 답해요"),
                      sub: ask ? String(localized: "하나를 눌러 보세요. 화면 위 Tacie에 답이 떠요.")
                               : String(localized: "들려주기를 누르면 Tacie가 듣고 답해요."))
                .padding(.bottom, model.usesSample ? 10 : 22)
            if model.usesSample {
                Text(model.keeper.firstSync != nil
                     ? String(localized: "준비하는 동안 예시 결제 서비스로 보여 드려요.")
                     : String(localized: "연결한 곳이 없어서 예시 결제 서비스로 보여 드려요."))
                    .font(.system(size: 12.5)).foregroundStyle(Theme.ink3)
                    .padding(.bottom, 16)
            }
            if needsKey {
                keyPrompt
            } else if let examples = model.examples {
                if ask { questions(examples) } else { meetingPlay }
            } else {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(model.keeper.step != nil ? String(localized: "검색 준비가 끝나면 질문을 보여 드릴게요") : String(localized: "물어볼 질문을 고르고 있어요"))
                        .font(.system(size: 12.5)).foregroundStyle(Theme.ink3)
                }
                .padding(.vertical, 8)
            }
            if !needsKey { status.padding(.top, 18) }
            Spacer(minLength: 0)
            HStack(spacing: 6) {
                Text(ask ? String(localized: "직접 묻기") : String(localized: "회의 시작 · 끝내기")).foregroundStyle(Theme.ink3)
                Keycap(text: ask ? "⌃⌥⌘A" : "⌃⌥⌘M")
                Spacer()
                Button("계속") { model.leavePractice() }.buttonStyle(InkButtonStyle()).keyboardShortcut(.defaultAction)
                    .disabled(!canGoOn).opacity(canGoOn ? 1 : 0.35)
            }
            .font(.system(size: 12))
            .padding(.bottom, 22)
        }
        .padding(.horizontal, 32)
        .padding(.top, 14)
        // per step: going from 3 to 4 keeps the same view, so onAppear would not run again
        .task(id: step) { model.beginPractice() }
        // the meeting practice's voice and answer are made from 3단계 on, so 들려주기 answers as soon as the voice stops
        .task(id: "\(model.meetingQuestion ?? "")|\(model.aiKeyMissing == true)") { model.prepareMeeting() }
        // back from 설정 > AI (or anywhere): a key may be there now
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { await model.checkAIKey() }
        }
        .onChange(of: state) { _, now in if now.done { model.markPracticed(step, now) } }
    }

    /// The AI key, pasted right here (설정 > AI's 받아쓰기·답 키): without it the practice can neither hear nor answer.
    @ViewBuilder private var keyPrompt: some View {
        if Relay.on {
            // Tacie AI: what is missing is the sign-in (or its approval), not a key
            signInPrompt
        } else {
            ownKeyPrompt
        }
    }

    private var signInPrompt: some View {
        let account = TacieAccount.shared
        let waiting = account.info?.status == "waiting"
        return VStack(alignment: .leading, spacing: 10) {
            Text(waiting ? String(localized: "승인을 기다리고 있어요") : String(localized: "로그인하면 바로 해 볼 수 있어요"))
                .font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.ink)
            Text(waiting ? String(localized: "승인되면 앱을 다시 켜지 않아도 바로 쓸 수 있어요. 지금은 계속을 눌러 넘어가도 돼요.")
                         : String(localized: "나중에 하려면 계속을 눌러 넘어가요."))
                .font(.system(size: 12.5)).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            if waiting {
                Button("다시 확인") { Task { await account.refresh(); await model.checkAIKey() } }.buttonStyle(InkButtonStyle())
            } else {
                SignInButtons()
            }
        }
        .padding(16).frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.fill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private var ownKeyPrompt: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("AI 키를 넣으면 바로 해 볼 수 있어요").font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.ink)
            Text("회의 받아쓰기와 답에 쓰는 OpenRouter 키예요. 이 Mac의 키체인에만 저장하고, 설정 → AI에서 언제든 바꿀 수 있어요. 지금 없으면 계속을 눌러 넘어가도 돼요.")
                .font(.system(size: 12.5)).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                SecureField("", text: $keyDraft, prompt: Text("키 붙여 넣기"))
                    .textFieldStyle(.plain)
                    .font(.system(size: 12.5, design: keyDraft.isEmpty ? .default : .monospaced))
                    .textContentType(.oneTimeCode)  // an API key, not a website password
                    .padding(.horizontal, 9).frame(height: 30)
                    .background(Theme.paper, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).stroke(Theme.ink.opacity(0.14)))
                    .onSubmit { saveKey() }
                Button(savingKey ? String(localized: "저장 중") : String(localized: "저장")) { saveKey() }.buttonStyle(InkButtonStyle())
                    .disabled(savingKey || keyDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            if let keyNote { Text(keyNote).font(.system(size: 12)).foregroundStyle(Theme.warn) }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.fill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private func saveKey() {
        let paste = keyDraft
        savingKey = true
        Task {
            keyNote = await model.saveAIKey(paste)
            if keyNote == nil { keyDraft = "" }
            savingKey = false
        }
    }

    private func questions(_ examples: [String]) -> some View {
        VStack(spacing: 8) {
            ForEach(examples, id: \.self) { q in
                Button { picked = q; model.askOnIsland(q) } label: {
                    HStack {
                        Text(q).font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.ink)
                        Spacer()
                        mark(for: q)
                    }
                    .padding(.horizontal, 16).frame(height: 48)
                    .contentShape(Rectangle())
                    .background(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.ink.opacity(0.12)))
                }
                .buttonStyle(.plain)
                .disabled(state == .finding)
            }
        }
    }

    /// The row's right edge: an arrow, the search while it runs, then the result (it settles to one check).
    @ViewBuilder private func mark(for q: String) -> some View {
        if q == picked, state == .finding {
            ProgressView().controlSize(.small).frame(height: 30)
        } else if q == picked, state.done {
            DoneMark(text: String(localized: "답이 떴어요")).id(q)
        } else if q == picked, state == .missed {
            Image(systemName: "questionmark.circle").font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.ink3)
        } else {
            Image(systemName: "arrow.up.right").font(.system(size: 11, weight: .bold)).foregroundStyle(Theme.ink3)
        }
    }

    @ViewBuilder private var meetingPlay: some View {
        if let line = model.meetingQuestion {
            HStack(alignment: .center, spacing: 14) {
                Image(systemName: "person.wave.2").font(.system(size: 16, weight: .semibold)).foregroundStyle(Theme.ink2)
                    .frame(width: 38, height: 38).background(Theme.fill, in: Circle())
                VStack(alignment: .leading, spacing: 3) {
                    Text("회의 중 누군가").font(.system(size: 12)).foregroundStyle(Theme.ink3)
                    Text("“\(line)”").font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.ink)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
                Button(meetingOn || model.speaking ? String(localized: "듣는 중") : state.done ? String(localized: "다시 듣기") : String(localized: "들려주기")) {
                    Task { await model.playMeeting() }
                }
                .buttonStyle(InkButtonStyle())
                .disabled(meetingOn || model.speaking)
            }
            .padding(16)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.ink.opacity(0.1)))
        }
    }

    private var mood: EyeMood {
        switch state {
        case .waiting: .listening
        case .finding: .thinking
        case .answered: .happy
        case .near, .missed: .unsure
        }
    }

    @ViewBuilder private var status: some View {
        HStack(alignment: .top, spacing: 8) {
            switch state {
            case .waiting:
                if meetingOn {
                    ProgressView().controlSize(.mini)
                    Text("Tacie가 회의를 듣고 있어요")
                }
            case .finding:
                ProgressView().controlSize(.mini)
                Text(ask ? String(localized: "답을 찾고 있어요") : String(localized: "질문을 들었어요. 답을 찾고 있어요"))
            case .answered:
                // asking, the row already shows the check
                if !ask { Image(systemName: "checkmark.circle.fill").font(.system(size: 12)).foregroundStyle(Theme.live) }
                Text(ask ? String(localized: "카드를 누르면 답이 적힌 곳이 열려요.") : String(localized: "회의를 멈추지 않아도 답이 떠요."))
            case .near:
                Image(systemName: "checkmark.circle.fill").font(.system(size: 12)).foregroundStyle(Theme.live)
                Text("확실한 답을 못 찾아서 가장 가까운 곳을 띄웠어요.")
            case .missed:
                Image(systemName: "questionmark.circle").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.ink3)
                Text(ask ? String(localized: "답을 못 찾았어요. 다른 질문을 눌러 보세요.") : String(localized: "답을 못 찾았어요. 다시 들어 볼까요?"))
            }
        }
        .font(.system(size: 12.5))
        .foregroundStyle(Theme.ink2)
        .fixedSize(horizontal: false, vertical: true)
    }
}

// MARK: - 5 done

private struct DoneStep: View {
    let model: OnboardingModel
    let openMain: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            GazeHead(mood: .happy, height: 78).padding(.top, 10)
            Text("준비됐어요").font(.system(size: 24, weight: .bold)).tracking(-0.4).foregroundStyle(Theme.ink).padding(.top, 18)
            Text("Tacie는 화면 위 가운데에 있어요. 화면 공유와 화면 녹화에는 안 보여요.")
                .font(.system(size: 13.5)).foregroundStyle(Theme.ink2).padding(.top, 6)
            VStack(spacing: 0) {
                hotkey(String(localized: "회의 시작 · 끝내기"), "⌃⌥⌘M")
                Divider().opacity(0.5)
                hotkey(String(localized: "직접 묻기"), "⌃⌥⌘A")
                Divider().opacity(0.5)
                hotkey(String(localized: "메인 창 열기 · 회의 정리"), "⌃⌥⌘T")
            }
            .padding(.horizontal, 16)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.ink.opacity(0.1)))
            .padding(.top, 22)
            let left = OnboardingFlow.leftovers(model.progress, model.facts)
            if !left.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    Text("남은 일").font(.system(size: 12.5, weight: .semibold)).foregroundStyle(Theme.ink3)
                    LeftoverList(items: left, dismiss: { model.dismiss($0) }) { item in
                        switch item {
                        case .mic: Task { await model.turnOnMic() }
                        case .connectSource, .connectNotion, .connectLinear, .connectDrive, .connectAtlassian: model.go(.sources)
                        case .signIn, .chooseAI: model.go(.account)
                        case .callAudio: model.go(.listen)
                        case .connectMic, .notify: break
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 20)
            }
            let f = model.facts
            if !(f.hasSource && f.notionConnected && f.linearConnected && f.gdriveConnected && f.atlassianConnected && f.asanaConnected
                && f.dropboxConnected && f.notesOn) {
                Text("연결은 메인 창의 처음 써 보기에서 이어서 해요")
                    .font(.system(size: 12)).foregroundStyle(Theme.ink3)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.top, 14)
            }
            Spacer(minLength: 0)
            HStack {
                Text("처음 안내는 설정 → 일반에서 다시 볼 수 있어요").font(.system(size: 12)).foregroundStyle(Theme.ink3)
                Spacer()
                Button("메인 창 열기", action: openMain).buttonStyle(InkButtonStyle()).keyboardShortcut(.defaultAction)
            }
            .padding(.bottom, 22)
        }
        .padding(.horizontal, 32)
    }

    private func hotkey(_ what: String, _ keys: String) -> some View {
        HStack {
            Text(what).font(.system(size: 13.5, weight: .medium)).foregroundStyle(Theme.ink)
            Spacer()
            Keycap(text: keys)
        }
        .frame(height: 44)
    }
}

/// The background work, under every step but the first and last: the first sync's step and percent.
private struct SyncLine: View {
    let keeper: IndexKeeper
    let download: ModelDownload?
    /// The last step shows only the model's line (the sync's has its own place there).
    var done = false

    var body: some View {
        Group {
            if let d = download, let line = modelLine(d) {
                line
            } else if !done, let step = keeper.step {
                bar(text: String(localized: "준비하고 있어요 · \(step)"), busy: true,
                    progress: keeper.progress.map { Double($0.done) / Double(max(1, $0.total)) })
            }
        }
        .padding(.horizontal, 32)
        .padding(.bottom, 16)
    }

    /// The search model's download (docs/11 4-2절): its progress, or the one question it waits on.
    private func modelLine(_ d: ModelDownload) -> AnyView? {
        // the 40MB voice top-up for someone who already has the search model: the sidebar row covers it
        if d.onlyVoice { return nil }
        let size = Self.gb(d.total)
        switch d.state {
        case .idle, .ready:
            return nil
        case .downloading:
            return AnyView(bar(text: String(localized: "검색 모델 받는 중 · \(Self.gb(d.done)) / \(size)GB"), busy: true,
                               progress: Double(d.done) / Double(max(1, d.total))))
        case .verifying:
            return AnyView(bar(text: String(localized: "검색 모델을 확인하고 있어요"), busy: true, progress: Double(d.done) / Double(max(1, d.total))))
        case .waiting(.expensive):
            return AnyView(ask(String(localized: "검색 모델 \(size)GB를 받아야 해요. 지금 쓰는 네트워크에서는 데이터 요금이 나올 수 있어요."),
                               primary: (String(localized: "지금 받기"), { d.start(force: true) }), secondary: (String(localized: "Wi-Fi에서 받기"), { d.later() })))
        case .waiting(.space(let need)):
            return AnyView(ask(String(localized: "검색 모델을 받으려면 공간이 \(Self.gb(need))GB 더 필요해요."), primary: (String(localized: "다시 확인"), { d.start() })))
        case .waiting(.offline):
            return AnyView(note(String(localized: "인터넷에 연결되면 검색 모델을 이어서 받아요.")))
        case .waiting(.go):
            return nil
        case .later:
            return AnyView(ask(String(localized: "Wi-Fi에 연결되면 검색 모델을 받아요."), primary: (String(localized: "지금 받기"), { d.start(force: true) })))
        case .failed(let why):
            return AnyView(ask(String(localized: "검색 모델을 받지 못했어요. \(why)."), primary: (String(localized: "다시 받기"), { d.start() })))
        }
    }

    private func bar(text: String, busy: Bool, progress: Double?) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                if busy { ProgressView().controlSize(.mini) }
                Text(text).font(.system(size: 11.5)).foregroundStyle(Theme.ink3)
                Spacer()
                if let progress {
                    Text("\(Int(progress * 100))%").font(.system(size: 11.5, weight: .medium)).monospacedDigit().foregroundStyle(Theme.ink3)
                }
            }
            if let progress {
                GeometryReader { g in
                    ZStack(alignment: .leading) {
                        Capsule().fill(Theme.fill)
                        Capsule().fill(Theme.ink.opacity(0.55)).frame(width: g.size.width * progress)
                    }
                }
                .frame(height: 3)
            }
        }
    }

    private func note(_ text: String) -> some View {
        Text(text).font(.system(size: 11.5)).foregroundStyle(Theme.ink3).frame(maxWidth: .infinity, alignment: .leading)
    }

    private func ask(_ text: String, primary: (String, () -> Void), secondary: (String, () -> Void)? = nil) -> some View {
        HStack(spacing: 8) {
            Text(text).font(.system(size: 12)).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            if let secondary { Button(secondary.0, action: secondary.1).buttonStyle(GhostButtonStyle()) }
            Button(primary.0, action: primary.1).buttonStyle(InkButtonStyle())
        }
        .padding(12)
        .background(Theme.fill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    }

    /// 1,133,410,304 bytes → "1.1".
    static func gb(_ bytes: Int64) -> String { String(format: "%.1f", Double(bytes) / 1_000_000_000) }
}
