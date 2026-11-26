import SwiftUI

/// A page inside the modal, with a back row.
enum SettingsDetail { case licenses, deleteMeetings, forgetPeople, githubConnect, githubRepo, clearSearchData, sentInfo, aiCustom }

struct SettingsActions {
    let sync: () -> Void
    let resetIsland: () -> Void
    let islandAlwaysChanged: (Bool) -> Void
    let copyNotice: () -> Void
    let restartEngine: () -> Void
    /// 처음 안내 다시 보기: the onboarding from its welcome (it was in the menu bar until 2026-10-27).
    let restartOnboarding: () -> Void
    /// Quits and opens Tacie again (a new app language), coming back to this settings page.
    let relaunch: (SettingsPane) -> Void
    /// The MCP switch changed: the socket opens or closes now (`MCPBridge.refresh`, docs/38 §5).
    let mcpChanged: () -> Void
}

/// Everything a settings page reads; built once in AppDelegate.
struct SettingsContext {
    let library: MeetingLibrary
    let vault: VoiceVault
    let meeting: MeetingModel
    let keeper: IndexKeeper
    let engine: EngineProcess
    let localSTT: LocalSTT
    let actions: SettingsActions

    /// Why a sync, a repo change or deleting the search data must wait (nil: it may).
    @MainActor var syncBlocker: String? {
        if keeper.step != nil { return String(localized: "동기화가 끝난 뒤에 할 수 있어요.") }
        if meeting.isRunning { return String(localized: "회의가 끝난 뒤에 할 수 있어요.") }
        if !keeper.anySource { return String(localized: "GitHub나 노션을 연결하면 할 수 있어요.") }
        return nil
    }

    /// Why the search data must not be deleted right now (nil: it may). Deleting needs no GitHub connection.
    @MainActor var clearBlocker: String? {
        if keeper.step != nil { return String(localized: "동기화가 끝난 뒤에 할 수 있어요.") }
        if meeting.isRunning { return String(localized: "회의가 끝난 뒤에 할 수 있어요.") }
        return nil
    }

    /// Why the engine must not be restarted right now (nil: it may). The one place for the button and the app delegate.
    @MainActor var restartBlocker: String? {
        if meeting.isRunning { return String(localized: "회의가 끝난 뒤에 다시 띄울 수 있어요.") }
        if keeper.step != nil { return String(localized: "동기화가 끝난 뒤에 다시 띄울 수 있어요.") }
        if library.meetings.contains(where: { $0.notes == .waiting }) { return String(localized: "요약을 다 쓴 뒤에 다시 띄울 수 있어요.") }
        if engine.state == .starting { return String(localized: "검색 엔진이 뜨는 중이에요.") }
        return nil
    }
}

/// The settings modal's content: panes on the left (214 pt), the chosen one on the right.
struct SettingsView: View {
    let pane: SettingsPane
    let context: SettingsContext
    @Bindable var selection: MainSelection
    @State private var detail: SettingsDetail?
    @State private var returning = false  // the pane comes back from a detail page: slide, not crossfade
    @State private var pickAfterConnect = false  // the reconnect started from the repo picker: back to it
    @State private var query = ""  // 설정 검색
    @State private var current = 0  // the result Return opens
    @State private var jump: SettingsSearchEntry?  // the picked result, waiting for its page to scroll
    @State private var target: String?  // the row marked for a moment
    @State private var meetingFocus: MeetingSettingsScene.Focus?  // the 회의 row under the pointer, for its scene
    @State private var connectionsFocus: ConnectionsScene.Source?  // the 연결 source under the pointer, for its scene
    @FocusState private var searching: Bool
    @Namespace private var ns
    @Environment(\.reducesMotion) private var reduceMotion
    @Environment(\.displayScale) private var displayScale

    var body: some View {
        HStack(spacing: 0) {
            sidebar
            ScrollViewReader { proxy in
                VStack(spacing: 0) {
                if hasScene {
                    sceneTop
                        .padding(.horizontal, 48)
                        .padding(.top, 34)
                        .frame(maxWidth: 640, alignment: .leading)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .transition(crossfade)
                }
                ScrollView {
                    // a ZStack, not a VStack: while the old pane fades it shares the slot instead of pushing the new one down
                    ZStack(alignment: .topLeading) { page }
                        .padding(.horizontal, 48)
                        .padding(.top, hasScene ? 26 : 34)
                        .padding(.bottom, 30)
                        .frame(maxWidth: 640, alignment: .leading)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        // panes crossfade with a small rise; a detail page slides in from the right and back out
                        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: pane)
                        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: detail)
                        .id(Self.topID)
                }
                .scrollIndicators(.never)
                // rows pass under the scene: a short fade instead of a hard edge
                .overlay(alignment: .top) {
                    if hasScene {
                        LinearGradient(colors: [Theme.paper, Theme.paper.opacity(0)], startPoint: .top, endPoint: .bottom)
                            .frame(height: 14).allowsHitTesting(false)
                    }
                }
                .onHover { inside in if !inside { meetingFocus = nil; connectionsFocus = nil } }
                }
                .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: hasScene)
                .environment(\.settingsTarget, target)
                // `settings-row <title>` (debug, captures): the row as if picked in 설정 검색
                .onChange(of: selection.settingsRow, initial: true) {
                    guard let title = selection.settingsRow else { return }
                    selection.settingsRow = nil
                    if title == "업체 직접 선택", Build.isOpen { detail = .aiCustom; return }  // the open edition's AI detail (2-D)
                    if let e = SettingsSearch.entries.first(where: { $0.title == title }) { pick(e) }
                }
                // a picked result: once its page is in, scroll to the row and mark it
                .task(id: jump?.id) {
                    guard let e = jump else { return }
                    try? await Task.sleep(for: .milliseconds(120))
                    withAnimation(Theme.anim(Theme.glide, reduce: reduceMotion)) {
                        switch e.anchor {
                        case .row: proxy.scrollTo(e.title, anchor: UnitPoint(x: 0.5, y: 0.35))
                        case .footer: proxy.scrollTo(SettingsSearch.footerID, anchor: .bottom)
                        case .top: proxy.scrollTo(Self.topID, anchor: .top)
                        }
                    }
                    if e.anchor == .row { target = e.title }
                    jump = nil
                }
            }
        }
        .background(alignment: .leading) {
            Rectangle().fill(Theme.settingsSidebar).frame(width: 214)
                .overlay(alignment: .trailing) { Rectangle().fill(Theme.hairline).frame(width: 1) }
        }
        .background(Theme.paper)
        .onChange(of: pane) { detail = nil; returning = false; meetingFocus = nil; connectionsFocus = nil }
        .onChange(of: pane, initial: true) { Analytics.capture(.settingsOpened(page: ASettingsPage(rawValue: pane.rawValue) ?? .general)) }
    }

    private var sidebar: some View {
        let account = Account.current
        let results = SettingsSearch.matches(query, in: SettingsSearch.entries)
        return VStack(alignment: .leading, spacing: 2) {
            // 설정 검색 at the top, where the main window's list has 회의 검색 (2026-11-12)
            SettingsSearchField(query: $query, focused: $searching,
                                move: { current = max(0, min(results.count - 1, current + $0)) },
                                submit: { if results.indices.contains(current) { pick(results[current]) } })
                .padding(.top, 14)
                .padding(.bottom, 10)
            if query.trimmingCharacters(in: .whitespaces).isEmpty {
                ForEach(SettingsPane.main.filter { $0 != .account }) { item($0) }
            } else {
                SettingsSearchResults(results: results, current: current, pick: pick)
            }
            Spacer()
            if Build.isDev { item(.developer) }
            // 계정 is the name and usage at the bottom (2026-11-11), the same place and look as the main window's
            if TacieAccount.offered { accountItem(account) }
        }
        .padding(.horizontal, 10)
        .padding(.bottom, 12)
        .animation(Theme.anim(Theme.glide, reduce: reduceMotion), value: pane)
        .onChange(of: query) { current = 0 }
        .frame(width: 214)
        .frame(maxHeight: .infinity)
    }

    private static let topID = "settings-top"

    /// A page whose title and scene stay above the scroll (2026-11-15: 회의 first; the other pages follow).
    private var hasScene: Bool { detail == nil && (pane == .meeting || pane == .connections) }

    @ViewBuilder private var sceneTop: some View {
        VStack(alignment: .leading, spacing: 0) {
            if pane == .connections {
                PageHeader(title: String(localized: "settings.connections", defaultValue: "연결"),
                           desc: String(localized: "연결한 서비스에서 답을 찾아요. 내 계정으로 볼 수 있는 것만 가져와요."))
                    .padding(.bottom, -12)
                ConnectionsScene(on: ConnectionsPage.linked(context.keeper).compactMapValues { $0 }, focus: connectionsFocus,
                                 syncing: context.keeper.step != nil ? context.keeper.syncLine : nil)
            } else {
                PageHeader(title: SettingsPane.meeting.title, desc: String(localized: "회의를 시작할 때부터 끝난 뒤까지, 순서대로 정해요."))
                    .padding(.bottom, -12)
                MeetingSettingsScene(focus: meetingFocus)
            }
        }
    }

    /// A search result: its pane (back from any detail page), then the scroll to the row.
    private func pick(_ e: SettingsSearchEntry) {
        query = ""
        searching = false
        target = nil
        returning = false
        detail = nil
        selection.settings = e.pane
        jump = e
    }

    private func accountItem(_ account: Account) -> some View {
        AccountRow(account: account, on: pane == .account, ns: ns) { returning = false; selection.settings = .account }
    }

    private func item(_ p: SettingsPane) -> some View {
        Button { returning = false; selection.settings = p } label: {
            HStack(spacing: 9) {
                Image(systemName: p.icon).uiFont(13, weight: .regular).frame(width: 18)
                Text(p.title).uiFont(13.5, weight: p == pane ? .medium : .regular)
                Spacer()
            }
            .foregroundStyle(p == pane ? Theme.ink : Theme.ink2)
            .padding(.horizontal, 8)
            .frame(height: 30)
            .background {
                if p == pane {
                    RoundedRectangle(cornerRadius: 7, style: .continuous).fill(SettingsStyle.selected)
                        .matchedGeometryEffect(id: "pane-on", in: ns)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(PressStyle())
    }

    /// Placeholders until the pages arrive: the title and the mock's lead-in.
    @ViewBuilder private var page: some View {
        // each page is a VStack of its own: page bodies are bare lists of sections, and the ZStack around them
        // would otherwise pile those sections on top of each other
        if let detail {
            VStack(alignment: .leading, spacing: 0) {
                switch detail {
                case .licenses: LicensesPage(back: back)
                case .deleteMeetings: DeleteMeetingsPage(context: context, selection: selection, back: back)
                case .forgetPeople: ForgetPeoplePage(context: context, back: back)
                case .sentInfo: SentInfoPage(localSTT: context.localSTT, back: back)
                case .githubConnect: GitHubConnectPage(context: context, back: back, connected: afterGitHubConnect)
                case .githubRepo: RepoPickerPage(context: context, back: back, reconnect: {
                    pickAfterConnect = true
                    self.detail = .githubConnect
                })
                case .clearSearchData: ClearSearchDataPage(context: context, back: back)
                case .aiCustom: AICustomPage(context: context, back: back)
                }
            }
            .id(detail)
            .transition(slide(from: 8))
        } else {
            VStack(alignment: .leading, spacing: 0) { panePage }
                .id(pane)
                // leaving for a detail page or coming back from one: the pane moves 8pt the other way
                .transition(returning || detail != nil ? slide(from: -8) : crossfade)
        }
    }

    private func back() {
        returning = true
        detail = nil
    }

    /// Connected (again): with a repo already chosen there is nothing to pick, so it goes back to 연결 and syncs that
    /// repo right away. The picker comes only for a first repo, or when the reconnect started from the picker.
    private func afterGitHubConnect() {
        let pick = pickAfterConnect || context.keeper.info?.repo == nil
        pickAfterConnect = false
        if pick { detail = .githubRepo; return }
        back()
        context.actions.sync()
    }

    /// the old page is gone in a couple of frames (Theme.swap), so the two never overlap; the new one arrives
    private var crossfade: AnyTransition {
        .asymmetric(insertion: Theme.arrive(y: 4, reduce: reduceMotion, scale: displayScale),
                    removal: .opacity.animation(Theme.swap))
    }

    private func slide(from dx: CGFloat) -> AnyTransition {
        .asymmetric(insertion: Theme.arrive(x: dx, reduce: reduceMotion, scale: displayScale),
                    removal: .opacity.animation(Theme.swap))
    }

    @ViewBuilder private var panePage: some View {
        switch pane {
        case .account: AccountPage(context: context)
        case .general: GeneralPage(context: context) { detail = .licenses }
        case .ai: if Build.isOpen { OpenAIPage(context: context) { detail = .aiCustom } } else { AIPage(context: context) }
        case .connections: ConnectionsPage(context: context, open: { detail = $0 }, focus: $connectionsFocus)
        case .meeting: MeetingSettingsPage(context: context, focus: $meetingFocus)
        case .privacy: PrivacyPage(context: context, showDelete: { detail = .deleteMeetings }, showForgetPeople: { detail = .forgetPeople },
                                     showSent: { detail = .sentInfo })
        case .developer: DeveloperPage(context: context)
        }
    }
}

/// 계정 at the bottom of the settings list: the name and usage, a › saying it opens a page, and a light fill under the
/// pointer (2026-11-11), so it reads as a button and not as a label.
private struct AccountRow: View {
    let account: Account
    let on: Bool
    let ns: Namespace.ID
    let open: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: open) {
            HStack(spacing: 9) {
                Avatar(size: 24)
                VStack(alignment: .leading, spacing: 0) {
                    Text(account.name).uiFont(13, weight: .semibold).foregroundStyle(Theme.ink).lineLimit(1)
                    Text(account.usageLine).uiFont(11).foregroundStyle(Theme.ink3)
                }
                Spacer(minLength: 0)
                Chevron()
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 7)
            .background {
                if on {
                    RoundedRectangle(cornerRadius: 7, style: .continuous).fill(SettingsStyle.selected)
                        .matchedGeometryEffect(id: "pane-on", in: ns)
                } else if hover {
                    RoundedRectangle(cornerRadius: 7, style: .continuous).fill(Theme.ink.opacity(0.035))
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(PressStyle())
        .onHover { hover = $0 }
        .accessibilityLabel("\(SettingsPane.account.title), \(account.name)")
    }
}
