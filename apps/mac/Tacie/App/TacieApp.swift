import AppKit
import Carbon.HIToolbox
import SwiftUI

@main
struct TacieApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        Settings { EmptyView() }
            .commands {
                CommandGroup(replacing: .appSettings) {
                    Button("설정…") { delegate.openSettings() }.keyboardShortcut(",")
                }
            }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    /// The account's space opens before anything reads a meeting, a person or a connection (AccountSpace). A move
    /// into it that stopped half way (the app quit during it) finishes first.
    /// Only with a session: a kept space without one (the keychain lost the session) stays closed.
    override init() {
        AccountSpace.inUse = AccountSpace(id: TacieAccount.shared.spaceID)
        // a deleted account's space left by a quit goes first (nothing is open yet), even if that account is back
        SpaceAdoption().eraseRequested(keeping: .signedOut)
        if let id = AccountSpace.inUse.id { SpaceAdoption().adopt(into: id) }
        super.init()
    }

    private let engine = EngineProcess()
    /// `--meetings-dir <folder>` keeps meetings elsewhere (fixtures for screenshots, checks that must not touch real ones).
    /// The remembered people then live in a `people` folder next to it, unless `--people-dir <folder>` says where.
    private lazy var vault = VoiceVault(root: Self.argument("--people-dir").map { URL(fileURLWithPath: $0) }
        ?? Self.argument("--meetings-dir").map { URL(fileURLWithPath: $0).deletingLastPathComponent().appendingPathComponent("people") }
        ?? VoiceVault.standardRoot)
    private lazy var library = MeetingLibrary(store: Self.argument("--meetings-dir").map { MeetingStore(directory: URL(fileURLWithPath: $0)) }
        ?? .standard, vault: vault)
    private lazy var meeting: MeetingModel = {
        let m = MeetingModel(engine: engine, library: library, vault: vault)
        m.panel = panel
        return m
    }()
    #if !TACIE_OPEN  // iPhone sync is not in the open-source edition (docs/41 8절)
    /// The account's iPhone meetings, development builds only (IPhoneMeetings); made at launch so a removal is sent.
    private lazy var iphoneMeetings = IPhoneMeetings(
        library: library, client: { await IPhoneMeetings.liveClient() },
        decisions: { [weak self] id in await self?.meeting.markChangedDecisions(id) },
        // only when the session's account is the space in use (a switch may be waiting for a meeting to end), and
        // never into the signed-out space (a session from before spaces has no id until GET /me answers)
        ready: { TacieAccount.shared.spaceID != nil && AccountSpace(id: TacieAccount.shared.spaceID) == AccountSpace.inUse })
    #endif
    private lazy var island = IslandController(model: meeting, onStart: { [weak self] mode in self?.startMeeting(mode, trigger: .island) },
                                               onOpenMain: { [weak self] question in self?.openMain(at: question) },
                                               onHide: { [weak self] in self?.setIslandAlways(false) })
    private lazy var main = MainWindowController(library: library, meeting: meeting, onboarding: onboarding.model, actions: MainActions(
        start: { [weak self] in self.map { $0.startMeeting($0.lastMode, trigger: .main) } },
        startMode: { [weak self] mode in self?.startMeeting(mode, trigger: .main) },
        stop: { [weak self] in self?.meeting.stop(by: .main) },
        ask: { [weak self] in self?.askDirectly() },
        copyNotice: { Self.copyNotice() },
        rewriteNotes: { [weak self] id in self?.meeting.rewriteNotes(id) },
        resume: { [weak self] id in self?.resumeMeeting(id) },
        changeKind: { [weak self] id, kind in self?.meeting.changeKind(id, to: kind) },
        dismissFailure: { [weak self] in self?.meeting.dismissFailure() },
        fixLeftover: { [weak self] item in
            guard let self else { return }
            switch item {
            case .mic: Task { await self.onboarding.model.turnOnMic() }
            case .callAudio: self.onboarding.show(step: .listen)
            case .connectSource, .connectNotion, .connectLinear, .connectDrive, .connectAtlassian: self.onboarding.show(step: .sources)
            case .signIn: self.onboarding.show(step: .account)
            case .chooseAI: self.openSettings(.ai)
            case .connectMic, .notify: break
            }
        },
        afterSignIn: { [weak self] signedIn in
            let go = self?.afterSignIn
            self?.afterSignIn = nil
            if signedIn { go?() }
        }),
        settings: settingsContext, panel: panel)
    /// The main window's panel (docs/36): its conversation and the history of the account in use.
    private lazy var panel = PanelStore(engine: { [weak self] in self?.engine.client.map(PanelEngine.live) },
                                        meetings: { [weak self] id in self?.library.meetings.first { $0.id == id } },
                                        update: { [weak self] id, change in self?.library.update(id, change) },
                                        allMeetings: { [weak self] in self?.library.meetings ?? [] },
                                        rewrite: { [weak self] id in self?.meeting.rewriteNotes(id) },
                                        remember: { MeetingTerms.remember($0) },
                                        nameSpeaker: { [weak self] id, key, name in
                                            self?.meeting.applyName(.typed(name), key: key, lines: [], in: id)
                                        },
                                        mergeSpeaker: { [weak self] id, key, into in
                                            self?.meeting.applyName(.speaker(into), key: key, lines: [], in: id)
                                        },
                                        moveLines: { [weak self] id, lines, to, name in
                                            guard let self, let m = self.library.meetings.first(where: { $0.id == id }) else { return }
                                            // 이 줄만 goes by the first line's speaker, as the namer opened on that turn would
                                            let key = m.transcript.first { $0.key == lines[0] }?.voice.map(m.resolved) ?? name
                                            self.meeting.applyName(to.map(NamerPick.speaker) ?? .typed(name), key: key, lines: lines, in: id)
                                        },
                                        rewriteAsked: { [weak self] id, request in self?.meeting.rewriteNotes(id, request: request) },
                                        changeKind: { [weak self] id, kind in self?.meeting.changeKind(id, to: kind) })
    // the closures call back into this delegate through [weak self]; lazy vars keep the init order free
    private lazy var settingsContext = SettingsContext(library: library, vault: vault, meeting: meeting, keeper: keeper, engine: engine, localSTT: localSTT,
        actions: SettingsActions(
            sync: { [weak self] in Task { await self?.keeper.refresh() } },
            resetIsland: { [weak self] in self?.island.resetPosition() },
            islandAlwaysChanged: { [weak self] on in
                guard let self else { return }
                if on { self.island.show() } else if !self.islandHasWork { self.island.hide() }
            },
            copyNotice: { Self.copyNotice() },
            restartEngine: { [weak self] in self?.restartEngine() },
            restartOnboarding: { [weak self] in self?.showIntroAgain() },
            relaunch: { [weak self] pane in self?.relaunch(reopening: pane) },
            mcpChanged: { [weak self] in self?.mcp.refresh() }))
    private lazy var keeper = IndexKeeper(engine: engine)
    /// AI apps reading this Mac's meetings through `tacie-mcp` (docs/38): a socket only while it is switched on.
    private lazy var mcp = MCPBridge(host: LiveMCPHost(
        engine: engine,
        meetingIsRunning: { [weak self] in self?.meeting.isRunning ?? false },
        modelIsReady: { [weak self] in self.map { $0.keeper.info?.modelReady ?? $0.model.isReady } ?? false },
        store: { [weak self] in self?.library.folder ?? .standard }))
    /// A meeting app using the microphone: the pill asks to listen along, and its release ends the meeting listened to.
    private let meetingApps = MeetingStartWatcher()
    /// The macOS notification that goes with the pill's question.
    private let meetingNotice = MeetingNotice()
    /// The search model, downloaded in the background on the first run (docs/11 4-2절).
    private lazy var model = ModelDownload()
    /// On-device speech recognition (docs/22): its own model file, never restarts the engine.
    private lazy var localSTT = LocalSTT(offered: LocalSTT.offered, searchReady: { [weak self] in self?.model.isReady ?? false },
                                         engine: { [weak self] in self?.engine.client })
    /// The model arrived during a meeting: the engine starts again with it once the meeting ends.
    private var restartForModel = false
    private lazy var onboarding = OnboardingWindowController(keeper: keeper, meeting: meeting, download: model, settings: settingsContext,
                                                              openMain: { [weak self] in self?.openMain() },
                                                              showIsland: { [weak self] in self?.island.show() },
                                                              islandFrame: { [weak self] in self?.island.frame })

    /// The account changed. The engine's AI addresses are fixed when it starts, so a relay that should now be on (or
    /// off) needs a new engine; that waits for the meeting to end. Otherwise the keys alone change.
    private func accountChanged() async {
        // another account's space (or none): a new engine comes with it, so the relay is right then too
        if wantedSpace != AccountSpace.inUse {
            followSpace()
            return
        }
        if engine.launchedWithRelay != Relay.on {
            if meeting.isRunning { restartForModel = true } else { engine.restart() }
        } else {
            await keeper.pushAIKeys()
        }
    }

    /// The space of the account signed in now (AccountSpace); differs from `AccountSpace.inUse` until `switchSpace` ran.
    private var wantedSpace: AccountSpace { AccountSpace(id: TacieAccount.shared.spaceID) }
    private var spaceSwitch: Task<Void, Never>?

    /// Changes to the wanted space once nothing of the current one is in use: a meeting always ends first. Notes
    /// still being written for a meeting that just ended are waited for (two minutes at most), except when another
    /// account signed in meanwhile: its questions must never reach the last account's search data.
    private func followSpace() {
        guard spaceSwitch == nil else { return }
        spaceSwitch = Task { [weak self] in
            var waited = 0
            while let self, self.wantedSpace != AccountSpace.inUse {
                let target = self.wantedSpace
                let otherAccount = target.id != nil && AccountSpace.inUse.id != nil
                let notes = self.library.meetings.contains { $0.notes == .waiting }
                if self.meeting.isRunning || (notes && !otherAccount && waited < 120) {
                    waited += 1
                    try? await Task.sleep(for: .seconds(1))
                    continue
                }
                await self.switchSpace(to: target)
            }
            self?.spaceSwitch = nil
        }
    }

    /// Closes everything of the current space and opens `target`'s: the island's answers, the connections, the engine
    /// (stopped and waited for, so nothing writes into the folder), the meetings and the remembered people. The first
    /// sign-in of an account on this Mac takes what the signed-out space held (SpaceAdoption).
    private func switchSpace(to target: AccountSpace) async {
        let from = AccountSpace.inUse
        Log.info("account space: \(from.id == nil ? "signed out" : "account") -> \(target.id == nil ? "signed out" : "account")")
        meeting.forgetAccount()
        engine.held = true
        // no MCP request may read this account's engine or meetings past here, nor reach the next account's
        await mcp.suspend()
        await keeper.settle()
        library.flush()
        await engine.stopAndWait()
        // a deleted account's space goes now that nothing uses it (before opening: an account signed up again with the
        // same id starts empty)
        SpaceAdoption().eraseRequested(keeping: .signedOut)
        if let id = target.id { SpaceAdoption().adopt(into: id) }
        AccountSpace.inUse = target
        if !Self.fixedFolders {
            vault.reopen(root: VoiceVault.standardRoot)
            library.reopen(store: .standard)
            library.recoverUnfinished()
            library.prune(keeping: MeetingLibrary.retention)
        }
        keeper.reload()
        panel.accountChanged()  // the new account's history; answers still on their way for the last one are dropped
        FirstStepsStore.shared.reopen()
        CalendarStore.shared.reopen()
        lastBuilt = nil
        lastKeys = []
        engine.held = false
        engine.start()  // the keeper hands it the account's connections once it is ready
        mcp.resume()  // the new account's own switch
        syncIPhoneMeetings()  // the new account's
    }

    /// iPhone meetings on this Mac (development builds): at launch, whenever the app comes to the front, and every
    /// 30 minutes. Not with fixture folders (`--meetings-dir`).
    private func startIPhoneMeetings() {
        #if !TACIE_OPEN
        guard Build.isDev, TacieAccount.offered, !Self.fixedFolders else { return }
        _ = iphoneMeetings
        syncIPhoneMeetings()
        NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.syncIPhoneMeetings() }
        }
        Task { [weak self] in
            while true {
                try? await Task.sleep(for: .seconds(1800))
                self?.syncIPhoneMeetings()
            }
        }
        #endif
    }

    private func syncIPhoneMeetings() {
        #if !TACIE_OPEN
        guard Build.isDev, !Self.fixedFolders, TacieAccount.shared.signedIn else { return }
        Task { await iphoneMeetings.sync() }
        #endif
    }

    /// `--meetings-dir` or `--people-dir`: fixtures that stay the same whoever is signed in.
    private static var fixedFolders: Bool { argument("--meetings-dir") != nil || argument("--people-dir") != nil }

    /// 개발자 탭: not during a meeting, a sync or while notes are being written (the page greys the button; the same check runs here).
    private func restartEngine() {
        guard settingsContext.restartBlocker == nil else { return }
        engine.restart()  // the keeper pushes the GitHub login and reads the index once the engine is ready
    }
    /// The GitHub login the engine gets: the Tacie connection in a product build; a development build may use `gh`.
    /// After 연결 해제 nothing, even if the keychain still holds the token. Never logged.
    static func githubCredentials() -> (mode: String, token: String?) {
        let tacie = Build.usesTacieLogin
        return GitHubCredentials.resolve(tacie: tacie, token: tacie ? KeychainTokenStore.github.read() : nil,
                                         disconnected: UserDefaults.standard.bool(forKey: GitHubCredentials.disconnectedKeyInUse))
    }
    /// 일반 설정의 "화면 위 Tacie 보이기" (on unless turned off).
    static var islandAlways: Bool {
        get { UserDefaults.standard.object(forKey: "islandAlways") as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: "islandAlways") }
    }
    /// 화면 위 Tacie 보이기, from the pill's close button or the menu: the same switch as in 설정 > 일반.
    private func setIslandAlways(_ on: Bool) {
        Self.islandAlways = on
        Analytics.capture(.settingChanged(setting: .islandAlways, value: ASettingValue(on)))
        if on { island.show() } else if !islandHasWork { island.hide() }
    }
    /// The island has something to show: a meeting, a question, a card, or why a meeting did not start.
    private var islandHasWork: Bool {
        if meeting.isRunning || meeting.asking || meeting.shown != .nothing || meeting.prompt != nil { return true }
        if case .failed = meeting.state { return true }
        return false
    }
    private var islandHadWork = false
    /// The search data's build time last seen (followNewData).
    private var lastBuilt: Date?
    /// The sources fully searchable at the last look (docs/11 14절).
    private var lastKeys: Set<String> = []
    private var wasRunning = false
    private var statusItem: NSStatusItem?
    private var hotKeys: [HotKey] = []
    private var statusLine: NSMenuItem?
    private var refreshItem: NSMenuItem?
    private var updateItem: NSMenuItem?
    private var meetingItems: [NSMenuItem] = []
    private var islandItem: NSMenuItem?
    private var stopItem: NSMenuItem?

    /// The first launch without saved progress. Waits a few seconds for the engine's first GET /index; someone with a
    /// repository and the microphone already is marked finished and goes to the usual window, anyone else gets the
    /// onboarding.
    private func firstLaunch() async {
        WhatsNew.markSeen()  // a fresh install: the onboarding introduces the app, nothing new to announce
        for _ in 0..<30 where keeper.info == nil { try? await Task.sleep(for: .milliseconds(200)) }
        if OnboardingFlow.alreadySetUp(onboarding.model.facts) {
            var done = OnboardingProgress()
            done.step = .done
            done.finished = true
            onboarding.model.progress = done
            Log.info("onboarding skipped: already set up")
            if LaunchWindow.showsMain(arguments: LaunchArgs.all, loginItem: false) { main.show(focus: true) }
        } else {
            onboarding.show()
        }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        main.islandFrame = { [weak self] in self?.island.frame }
        EngineClient.addressSet = { AIMode.addressSet() }  // the app waits longer on the person's own AI server (2-A)
        let loginItem = Self.launchedAsLoginItem  // read first: the event is the one being handled right now
        Analytics.start()
        Analytics.launched(loginItem: loginItem, firstLaunch: OnboardingStore().load() == nil)
        library.recoverUnfinished()
        library.prune(keeping: MeetingLibrary.retention)
        Task { [weak self] in
            while true {
                try? await Task.sleep(for: .seconds(24 * 3600))
                self?.library.prune(keeping: MeetingLibrary.retention)
            }
        }
        // the AI keys reach every new engine before it says ready, so a meeting renewing its session after a restart
        // never sends audio to an engine without keys
        engine.prepare = { [weak self] client in
            guard let self else { return true }
            await keeper.pushAIKeys(to: client)
            // with the switch on, an engine that never took it would send meeting sound to the server
            return await localSTT.pushToEngine(to: client)
        }
        meeting.onHeard = { [weak self] h in self?.localSTT.heard(h) }
        meeting.jevAvailable = { [weak self] in self?.keeper.jevAvailable ?? true }
        meeting.localSttReady = { [weak self] in
            guard let self, case .ready = self.localSTT.phase else { return false }
            return true
        }
        meeting.openAccount = { [weak self] in self?.openSettings(.account) }
        meeting.openAISettings = { [weak self] in self?.openSettings(.ai) }
        meeting.pushAIKeys = { [weak self] in await self?.keeper.pushAIKeys() }
        meeting.openConnections = { [weak self] in self?.openSettings(.connections) }
        meeting.accountChanging = { [weak self] in self.map { $0.wantedSpace != AccountSpace.inUse } ?? false }
        meeting.sttState = { [weak self] in self?.localSTT.meetingState }
        meeting.sttPaused = { [weak self] in self?.localSTT.paused ?? false }
        // a sign-in or out: the engine gets the session (or loses it); its AI addresses change with the relay
        TacieAccount.shared.changed = { [weak self] in await self?.accountChanged() }
        TacieAccount.shared.analytics = .live
        TacieAccount.shared.mcpChanged = { [weak self] in self?.mcp.refresh() }
        Task { await TacieAccount.shared.refresh() }
        engine.start()
        mcp.refresh()
        startIPhoneMeetings()
        // updates (docs/15): never during a meeting
        AppUpdate.shared.busy = { [weak self] in self?.meeting.isRunning ?? false }
        AppUpdate.shared.holding = { [weak self] in self?.onboarding.isVisible ?? false }
        AppUpdate.shared.mainOpen = { [weak self] in self?.main.isVisible ?? false }
        AppUpdate.shared.start()
        setUpStatusItem()
        // the main window comes up over the call without the keyboard, so the meeting app keeps typing (spec §5-1)
        // the onboarding's 3단계 asks through the island while its own window is in front: clicking it keeps the answer
        island.holdsAsk = { [weak self] in self?.onboarding.isVisible ?? false }
        meeting.onEnded = { [weak self] m in
            AppUpdate.shared.meetingEndedAt = Date()
            guard Self.openAfterMeeting else { return }
            Analytics.capture(.meetingNotesOpened(from: .afterMeeting, hasLedger: !m.ledger.isEmpty))
            self?.main.show(meeting: m.id, focus: false)
        }
        // ⌃⌥⌘M starts or ends a meeting (the last kind), ⌃⌥⌘A asks directly, ⌃⌥⌘T opens the main window (Tacie).
        // None is taken by macOS (⌥⌘Space is Finder search, ⌃Space switches 한/영).
        hotKeys = [
            HotKey(keyCode: kVK_ANSI_M, modifiers: controlKey | optionKey | cmdKey) { [weak self] in self?.toggleMeeting() },
            HotKey(keyCode: kVK_ANSI_A, modifiers: controlKey | optionKey | cmdKey) { [weak self] in self?.askDirectly() },
            HotKey(keyCode: kVK_ANSI_T, modifiers: controlKey | optionKey | cmdKey) { [weak self] in self?.openMain() },
        ]
        keeper.busy = { [weak self] in self?.meeting.isRunning ?? false }
        // the engine runs without the search model until it is downloaded, then starts again with it
        model.onReady = { [weak self] in
            guard let self else { return }
            self.localSTT.searchModelReady()
            if self.meeting.isRunning { self.restartForModel = true } else { self.restartWithModel() }
        }
        model.start()
        _ = localSTT  // a switch left on with the file half-fetched carries on by itself
        // until the engine first answers it is unknown: search as usual rather than keep questions by mistake
        meeting.firstSync = { [weak self] in
            guard let self, self.keeper.firstSync != nil else { return nil }
            return self.keeper.stepLine
        }
        meeting.hasSource = { [weak self] in
            // `--no-source` acts as if nothing were connected (screenshots, the kept-question flow)
            if LaunchArgs.has("--no-source") { return false }
            // `--no-connections` searches the engine's own index (the capture root): that index is the source
            if LaunchArgs.has("--no-connections") { return true }
            guard let self, self.keeper.info != nil else { return true }
            // past meetings are searched too (docs/30); 팀 자료 and 처음 써 보기 still count connected tools only
            return SourcePresence.searchable(facts: self.onboarding.model.facts.hasSource, meetings: self.library.hasDigest)
        }
        meeting.hasTeamSource = { [weak self] in
            if LaunchArgs.has("--no-source") { return false }
            if LaunchArgs.has("--no-connections") { return true }
            guard let self, self.keeper.info != nil else { return true }
            return self.onboarding.model.facts.hasSource
        }
        meeting.sourceKeys = { [weak self] in
            guard let self else { return [] }
            return ResearchRules.stampKeys(current: ResearchRules.keys(self.keeper.info, building: self.keeper.step != nil,
                                                                             atlassianCloud: self.keeper.atlassian.saved?.cloud,
                                                                             asanaUser: self.keeper.asana.saved?.userGid,
                                                                             dropboxAccount: self.keeper.dropbox.saved?.accountID),
                                           last: self.lastKeys)
        }
        keeper.credentials = { Self.githubCredentials() }
        keeper.meetings = { [weak self] in self?.library.digests() ?? [] }
        library.onChange = { [weak self] in self?.keeper.meetingsChanged() }
        MeetingCards.openMeeting = { [weak self] id in self?.main.show(meeting: id, focus: true) }
        MeetingCards.meetingExists = { [weak self] id in self?.library.meetings.contains { $0.id == id } ?? false }
        FirstStepsStore.shared.onDone = { step in
            Analytics.capture(.firstStepDone(item: AFirstStep(rawValue: step.rawValue) ?? .source))
        }
        keeper.start()
        observe()
        startMeetingApps()
        if Self.islandAlways { island.show() }
        // the first run opens the onboarding instead of the main window, until it is finished (docs/11)
        if let reopen = AppUpdate.takeQuietRelaunch() {
            // back from an update installed while the Mac sat idle: nothing pops up that was not up before
            if reopen { main.show(focus: false) }
        } else if UserDefaults.standard.bool(forKey: OnboardingModel.askMicOnLaunchKey) {
            // reopened by the onboarding's 다시 묻기: macOS asks again only in a new process
            UserDefaults.standard.removeObject(forKey: OnboardingModel.askMicOnLaunchKey)
            onboarding.show(step: .listen)
            Task { await onboarding.model.turnOnMic() }
        } else if !loginItem, OnboardingStore().load() == nil {
            // no saved progress: a new user, or one from before the onboarding; the engine's answer tells them apart
            Task { [weak self] in await self?.firstLaunch() }
        } else if OnboardingFlow.opensAtLaunch(OnboardingStore().load(), loginItem: loginItem) {
            onboarding.show()
        } else if LaunchWindow.showsMain(arguments: LaunchArgs.all, loginItem: loginItem) {
            main.show(focus: true)
        }

        let args = LaunchArgs.all
        let value = LaunchArgs.value
        // `--mode inperson|remote` picks the kind of meeting for `--replay` and `--listen`
        if let m = value("--mode").flatMap(MeetingMode.init(rawValue:)) { lastMode = m }
        // `--replay file.wav` listens to a recording instead of live sound (demos, screenshots);
        // in a remote meeting it plays as the call's side
        if let path = value("--replay") {
            meeting.start(lastMode, source: FileSource(url: URL(fileURLWithPath: path), as: lastMode == .remote ? .them : nil))
        }
        // `--listen` starts a live meeting right away
        if args.contains("--listen") { startMeeting(lastMode) }
        refreshMenu()
        // `--ask "question"` asks through the island
        if let q = value("--ask") {
            meeting.beginAsking()
            meeting.query = q
            meeting.ask()
        }
        if args.contains("--debug-control") { listenForDebugCommands() }
        reopenAfterRelaunch()
        // `--export-eyes dir` writes the face at every state as PNGs and quits (for checking the drawing)
        if let dir = value("--export-eyes") {
            do { try BrandImages.exportEyes(to: URL(fileURLWithPath: dir)) } catch { Log.info("export-eyes failed: \(error)") }
            NSApp.terminate(nil)
        }
        // `--export-brand dir` writes the logo PNGs (app icon, Slack avatar, favicons) and quits
        if let dir = value("--export-brand") {
            do { try BrandImages.exportBrand(to: URL(fileURLWithPath: dir)) } catch { Log.info("export-brand failed: \(error)") }
            NSApp.terminate(nil)
        }
    }

    /// Started by the login items (일반 설정의 "로그인 때 열기"): the apple event that opened the app says so.
    private static var launchedAsLoginItem: Bool {
        guard let event = NSAppleEventManager.shared().currentAppleEvent, event.eventID == kAEOpenApplication else { return false }
        return event.paramDescriptor(forKeyword: keyAEPropData)?.enumCodeValue == keyAELaunchedAsLogInItem
    }

    private static func argument(_ flag: String) -> String? { LaunchArgs.value(flag) }

    /// 일반 설정의 "회의가 끝나면 정리 열기" (on unless turned off).
    static var openAfterMeeting: Bool { UserDefaults.standard.object(forKey: "openAfterMeeting") as? Bool ?? true }

    /// Opening Tacie again (Spotlight, Finder) while it runs brings up the onboarding while it is not finished, else
    /// the main window.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if OnboardingFlow.opensAtLaunch(OnboardingStore().load(), loginItem: false) {
            onboarding.show()
        } else {
            main.show(focus: true)
        }
        return false
    }

    func applicationWillTerminate(_ notification: Notification) {
        meeting.closeRecordingForQuit()  // before stop(): its join runs in the background and would not finish
        meeting.stop(by: .quit)
        mcp.stop()  // mcp.sock goes before the engine
        engine.stop()
        BundledLLM.shared.stop()
        library.flush()  // the meeting's last save is written in the background
    }

    private var lastMode: MeetingMode {
        get { UserDefaults.standard.string(forKey: "meetingMode").flatMap(MeetingMode.init(rawValue:)) ?? .remote }
        set { UserDefaults.standard.set(newValue.rawValue, forKey: "meetingMode") }
    }

    /// What a remote meeting listens to: "mix" (the call + your mic, with headphones) or "system" (only the call).
    private var callSound: String {
        get { UserDefaults.standard.string(forKey: "listenTo") ?? "mix" }
        set { UserDefaults.standard.set(newValue, forKey: "listenTo") }
    }

    /// What waits on the sign-in sheet (a meeting to start, a question to ask), run once the account is in.
    private var afterSignIn: (() -> Void)?

    /// Tacie AI needs the account and there is none: the main window asks for the sign-in first (docs/14), and `then`
    /// runs once it is in, in place of a meeting that would only stop with "로그인해 주세요".
    private func needsSignIn(_ ask: SignInAsk, then: @escaping () -> Void) -> Bool {
        guard OnboardingModel.accountStep, !TacieAccount.shared.signedIn else { return false }
        afterSignIn = then
        main.selection.settings = nil
        main.selection.signIn = ask
        main.show(focus: true)
        return true
    }

    /// 이어서 듣기: the ended meeting goes on in its own mode, with the sound source a new meeting of that mode gets.
    private func resumeMeeting(_ id: String) {
        guard !meeting.isRunning, let m = library.meetings.first(where: { $0.id == id }), m.resumable(now: Date()),
              let mode = MeetingMode(rawValue: m.mode) else { return }
        meetingNotice.clearEnded()
        startMeeting(mode, trigger: .main, resuming: m)
    }

    private func startMeeting(_ mode: MeetingMode, trigger: ATrigger = .menu, app: MeetingApp? = nil, resuming: SavedMeeting? = nil) {
        if needsSignIn(.meeting, then: { [weak self] in self?.startMeeting(mode, trigger: trigger, app: app, resuming: resuming) }) { return }
        // the account's space is changing (a second or two): a meeting now would go into the last account's folder
        if spaceSwitch != nil || wantedSpace != AccountSpace.inUse {
            Log.info("meeting not started: the account's space is changing")
            return
        }
        lastMode = mode
        let source: SoundSource
        switch mode {
        case .inPerson: source = MicSource()
        case .remote: source = callSound == "system" ? CallAudio.make() : MixSource()
        }
        meeting.start(mode, source: source, trigger: trigger, app: app, resuming: resuming)
        island.show()
    }

    /// 회의 시작 알아채기 (docs/04): a meeting app took the microphone, or let it go.
    private func startMeetingApps() {
        meetingApps.meeting = { [weak self] in
            guard let self else { return (false, false) }
            return (self.meeting.isRunning, self.meeting.isRunning && self.meeting.mode == .remote)
        }
        meetingApps.onAsk = { [weak self] app in
            // not over the onboarding: its own steps hold the island
            guard let self, !self.onboarding.isVisible else { return }
            self.meeting.offer(app)
        }
        meetingApps.onEnded = { [weak self] _ in self?.meeting.proposeEnd(reason: .callEnded) }
        meetingApps.onOver = { [weak self] app in self?.meeting.promptOver(app) }
        meeting.onAcceptPrompt = { [weak self] app in self?.startMeeting(.remote, trigger: .prompt, app: app) }
        meeting.onPromptChanged = { [weak self] app in self?.meetingNotice.show(app) }
        meetingNotice.onAccept = { [weak self] in self?.meeting.acceptPrompt() }
        meeting.copyRecordNotice = { Self.copyNotice(recording: true) }
        // Tacie ended the meeting by itself while its window was not in front: say so, with the way back (이어서 듣기)
        meeting.onAutoEnded = { [weak self] m, reason in
            guard let self, !(NSApp.isActive && self.main.isKey) else { return }
            self.meetingNotice.showEnded(m, reason: reason)
        }
        meetingNotice.onOpenMeeting = { [weak self] id in
            // the meeting may have been deleted since: then just the window
            guard let self else { return }
            self.main.show(meeting: self.library.meetings.contains { $0.id == id } ? id : nil, focus: true)
        }
        meetingNotice.start()
        meetingApps.start()
    }

    private func toggleMeeting() {
        meeting.isRunning ? meeting.stop(by: .hotkey) : startMeeting(lastMode, trigger: .hotkey)
    }

    private func askDirectly() {
        if !meeting.asking, needsSignIn(.ask, then: { [weak self] in self?.askDirectly() }) { return }
        island.show()
        if meeting.asking { meeting.close() } else { meeting.beginAsking() }
    }

    private func setUpStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = BrandImages.menuBarTemplate()   // the pill with the eyes punched out; macOS tints it
        item.button?.image?.accessibilityDescription = "Tacie"
        // the few things done from here (2026-10-27): start or end a meeting, ask, the windows, sync, and 화면 위 Tacie
        // 보이기 (the way back after its close button). The rest lives in 설정 (듣는 소리, 알릴 문구, 처음 안내 다시 보기).
        let menu = NSMenu()
        menu.autoenablesItems = false  // refreshMenu sets isEnabled itself (지금 동기화 greys out)
        for mode in MeetingMode.allCases {
            let m = NSMenuItem(title: mode == .inPerson ? String(localized: "대면 회의 시작") : String(localized: "원격 회의 시작"), action: #selector(meetingMenu(_:)), keyEquivalent: "")
            m.representedObject = mode.rawValue
            m.target = self
            menu.addItem(m)
            meetingItems.append(m)
        }
        let stop = NSMenuItem(title: String(localized: "회의 끝내기"), action: #selector(stopMenu), keyEquivalent: "m")
        stop.keyEquivalentModifierMask = [.control, .option, .command]
        stop.target = self
        menu.addItem(stop)
        stopItem = stop
        let ask = NSMenuItem(title: String(localized: "직접 묻기"), action: #selector(askMenu), keyEquivalent: "a")
        ask.keyEquivalentModifierMask = [.control, .option, .command]
        ask.target = self
        menu.addItem(ask)
        menu.addItem(.separator())
        let open = NSMenuItem(title: String(localized: "메인 창 열기"), action: #selector(mainMenu), keyEquivalent: "t")
        open.keyEquivalentModifierMask = [.control, .option, .command]
        open.target = self
        menu.addItem(open)
        let shown = NSMenuItem(title: String(localized: "화면 위 Tacie 보이기"), action: #selector(islandMenu), keyEquivalent: "")
        shown.target = self
        menu.addItem(shown)
        islandItem = shown
        let settings = NSMenuItem(title: String(localized: "설정…"), action: #selector(settingsMenu), keyEquivalent: ",")
        settings.target = self
        menu.addItem(settings)
        menu.addItem(.separator())
        // only when the engine is not ready (starting, stopped, the search model still downloading)
        let status = NSMenuItem(title: "", action: nil, keyEquivalent: "")
        status.isEnabled = false
        status.isHidden = true
        menu.addItem(status)
        statusLine = status
        let refresh = NSMenuItem(title: String(localized: "지금 동기화"), action: #selector(refreshMenuItem), keyEquivalent: "")
        refresh.target = self
        menu.addItem(refresh)
        refreshItem = refresh
        // a new version downloaded and waiting (docs/15)
        let update = NSMenuItem(title: "", action: #selector(updateMenuItem), keyEquivalent: "")
        update.target = self
        update.isHidden = true
        menu.addItem(update)
        updateItem = update
        menu.addItem(.separator())
        menu.addItem(NSMenuItem(title: String(localized: "Tacie 종료"), action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"))
        menu.delegate = self
        item.menu = menu
        statusItem = item
    }

    @objc private func meetingMenu(_ item: NSMenuItem) {
        guard let mode = (item.representedObject as? String).flatMap(MeetingMode.init(rawValue:)) else { return }
        startMeeting(mode)
    }

    @objc private func mainMenu() { openMain() }

    /// The main window from the island: the meeting going on, or the one the question was caught in, at that question.
    /// A click asked for it, so it takes the keyboard (unlike the window that comes up when a meeting ends).
    private func openMain(at question: String? = nil) {
        main.show(meeting: question.flatMap(meeting.meeting(of:)) ?? meeting.live?.id, question: question, focus: true)
    }
    /// 설정 → 일반 → 처음 안내 다시 보기 (a method: the settings context and the onboarding are lazy and refer to each other).
    private func showIntroAgain() {
        // 처음 안내 다시 보기 brings 처음 써 보기 back too, with what is already true checked again
        FirstStepsStore.shared.reset()
        followFirstSteps()
        onboarding.show(restart: true)
    }

    @objc private func settingsMenu() { openSettings() }

    func openSettings(_ pane: SettingsPane = .account) { main.openSettings(pane) }

    /// The settings page to open once the app is back from `relaunch`.
    static let reopenPaneKey = "relaunchSettingsPane"

    /// Quits and opens this same app again with the same arguments, back on `pane` (a new app language). A shell
    /// waits for this process to end, then `open`s the bundle; the engine goes with the app as on any quit.
    func relaunch(reopening pane: SettingsPane) {
        UserDefaults.standard.set(pane.rawValue, forKey: Self.reopenPaneKey)
        let wait = Process()
        wait.executableURL = URL(fileURLWithPath: "/bin/sh")
        wait.arguments = ["-c", "while kill -0 \(ProcessInfo.processInfo.processIdentifier) 2>/dev/null; do sleep 0.1; done; "
                          + "exec /usr/bin/open -n \"$0\" --args \"$@\"", Bundle.main.bundlePath]
            + ProcessInfo.processInfo.arguments.dropFirst()
        do { try wait.run() } catch { Log.info("relaunch failed: \(error)"); return }
        Log.info("relaunch for \(pane.rawValue)")
        NSApp.terminate(nil)
    }

    /// Back from `relaunch`: the settings page it was on.
    private func reopenAfterRelaunch() {
        guard let raw = UserDefaults.standard.string(forKey: Self.reopenPaneKey) else { return }
        UserDefaults.standard.removeObject(forKey: Self.reopenPaneKey)
        if let pane = SettingsPane(rawValue: raw) { openSettings(pane) }
    }

    /// A change made in settings (the call sound) shows in the menu the next time it opens.
    func menuWillOpen(_ menu: NSMenu) { refreshMenu() }
    @objc private func stopMenu() { meeting.stop(by: .menu) }
    @objc private func askMenu() { askDirectly() }
    @objc private func islandMenu() { setIslandAlways(!Self.islandAlways) }

    /// Hidden from screen shares so company code does not leak, not to hide that Tacie is on: say so to the room.
    static var participantNotice: String {
        String(localized: "Tacie가 이 회의를 듣고 있어요. 말소리를 받아써서, 질문이 나오면 사내 코드·문서에서 답이 적힌 곳을 찾아 제 화면에 띄워요.")
            + (Capture.allowed ? "" : String(localized: "notice.hidden", defaultValue: " 사내 코드가 새지 않게 화면 공유에는 안 보여요."))
    }

    static func copyNotice(recording: Bool = false) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(recording ? recordingNotice : participantNotice, forType: .string)
    }

    /// For a meeting being recorded (녹음 알림, docs/35 6-1): what is kept and what leaves this Mac.
    static var recordingNotice: String {
        String(localized: "Tacie가 이 회의를 녹음하고 있어요. 녹음 파일은 이 Mac에 저장하고, 받아쓴 대화는 회의 정리를 만들려고 AI 서비스로 보내요.")
    }
    @objc private func updateMenuItem() { AppUpdate.shared.restart(place: .menu) }

    @objc private func refreshMenuItem() {
        guard keeper.anySource, keeper.step == nil else { return }
        Task { await keeper.refresh() }
    }

    /// `--debug-control`: scripts/debug.swift posts commands, one per notification. With `--debug-channel <name>` only commands
    /// sent with TACIE_DEBUG_CHANNEL=<name> arrive, so two apps from different worktrees don't both act on one command:
    /// start inperson|remote, replay inperson|remote <wav>, pause, stop, ending (as if "오늘은 여기까지 하죠" was heard), ask <text>, asking,
    /// query <text> (typed into the ask field and selected, not asked), close, history,
    /// chooser, present, slack <#channel>|<author>|<text>, move <x> <y> (top center of the island),
    /// hover in|out (as if the pointer entered or left the island), pill in|out (the pill row: the open button, pause and stop), look <n> (hover the n-th trail tab, oldest = 0), state,
    /// refresh (the menu's 지금 동기화), sync-check (the automatic check: syncs only if the rules say so), notice (the menu's 참석자에게 알릴 문구 복사),
    /// main, main-close (the main window), settings <account|general|ai|connections|privacy|developer>, settings-close,
    /// size <w> <h> (the main window's content size), click <x> <y> (a click in the main window, top-left origin), select <meeting id>, search <text> (its list), rewrite <meeting id> (다시 쓰기), kind <meeting id> <meeting|interview|talk> (정리 형식 바꾸기),
    /// focus <meeting id> <question id> (the main window at that question, as a click on the island over its answer opens it)
    private func listenForDebugCommands() {
        DistributedNotificationCenter.default().addObserver(forName: Notification.Name("ai.tacie.debug" + (Self.argument("--debug-channel").map { ".\($0)" } ?? "")), object: nil, queue: .main) { [weak self] note in
            guard let cmd = note.object as? String else { return }
            Task { @MainActor in self?.debugCommand(cmd) }
        }
        Log.info("debug control on")
    }

    /// Fake Linear states for captures: nothing is written to the keychain and nothing goes to Linear or the engine.
    private func previewLinear(_ state: String) {
        typealias Info = IndexState.LinearInfo
        let link = keeper.linear
        let pair = LinearConnection(token: "preview", refreshToken: "preview", expiresAt: .distantFuture,
                                    workspace: "Acme", workspaceID: "preview", muted: state == "empty" ? ["t3"] : [])
        let iso = ISO8601DateFormatter().string(from: Date().addingTimeInterval(-3 * 3600))
        func info(issues: Int?, refused: Bool? = nil, kind: String? = nil, lost: String? = nil) -> Info {
            Info(connected: true, workspace: "Acme", refused: refused, issues: issues, teams: 3, fetchedAt: iso,
                 errorKind: kind, lostSince: lost)
        }
        keeper.linearTeamsPreview = [LinearTeam(id: "t1", name: "Core", issues: 812),
                                     LinearTeam(id: "t2", name: "Design", issues: 264),
                                     LinearTeam(id: "t3", name: "Mobile", issues: 164)]
        switch state {
        case "waiting": keeper.linearPreview = nil; link.preview(nil, phase: .waiting)
        case "failed": keeper.linearPreview = nil; link.preview(nil, phase: .failed("Linear에 연결하지 못했어요. 다시 시도해 주세요."))
        case "connected": keeper.linearPreview = info(issues: 1240); link.preview(pair)
        case "empty": keeper.linearPreview = info(issues: 0); link.preview(pair)
        case "lost": keeper.linearPreview = info(issues: 1240, refused: true, lost: iso); link.preview(pair, lost: true)
        case "admin": keeper.linearPreview = info(issues: nil, kind: "key"); link.preview(pair)
        case "offline": keeper.linearPreview = info(issues: 1240, kind: "network"); link.preview(pair)
        default: keeper.linearPreview = nil; keeper.linearTeamsPreview = nil; link.preview(nil)
        }
    }

    /// Fake Google Drive states for captures: nothing is written to the keychain and nothing goes to Google or the engine.
    private func previewGDrive(_ state: String) {
        typealias Info = IndexState.GDriveInfo
        let link = keeper.gdrive
        let pair = GDriveConnection(token: "preview", refreshToken: "preview", expiresAt: .distantFuture,
                                    account: "me@example.com", accountID: "preview", muted: ["d3"])
        let iso = ISO8601DateFormatter().string(from: Date().addingTimeInterval(-3 * 3600))
        func info(files: Int?, done: Bool = true, refused: Bool? = nil, kind: String? = nil, lost: String? = nil) -> Info {
            Info(connected: true, account: "me@example.com", refused: refused, files: files, drives: 3, fetchedAt: iso,
                 initialDone: done, errorKind: kind, lostSince: lost)
        }
        keeper.gdriveDrivesPreview = [GDriveDrive(id: "d1", name: "기획팀", files: 412),
                                      GDriveDrive(id: "d2", name: "디자인", files: 188),
                                      GDriveDrive(id: "d3", name: "아카이브", files: 2031)]
        switch state {
        case "waiting": keeper.gdrivePreview = nil; link.preview(nil, phase: .waiting)
        case "failed": keeper.gdrivePreview = nil; link.preview(nil, phase: .failed(GDriveLink.words(.scope)))
        case "connected": keeper.gdrivePreview = info(files: 1240); link.preview(pair)
        case "partial": keeper.gdrivePreview = info(files: 500, done: false); link.preview(pair)
        case "empty": keeper.gdrivePreview = info(files: 0); link.preview(pair)
        case "lost": keeper.gdrivePreview = info(files: 1240, refused: true, lost: iso); link.preview(pair, lost: true)
        case "offline": keeper.gdrivePreview = info(files: 1240, kind: "network"); link.preview(pair)
        default: keeper.gdrivePreview = nil; keeper.gdriveDrivesPreview = nil; link.preview(nil)
        }
    }

    /// Fake Jira · Confluence states for captures: nothing is written to the keychain and nothing goes to Atlassian or
    /// the engine.
    private func previewAtlassian(_ state: String) {
        typealias Info = IndexState.AtlassianInfo
        let link = keeper.atlassian
        let pair = AtlassianConnection(token: "preview", refreshToken: "preview", expiresAt: .distantFuture, cloud: "preview",
                                       site: "acme", siteURL: "https://acme.atlassian.net", mutedSpaces: ["s3"])
        let iso = ISO8601DateFormatter().string(from: Date().addingTimeInterval(-3 * 3600))
        let soon = ISO8601DateFormatter().string(from: Date().addingTimeInterval(40 * 60))
        func info(issues: Int?, pages: Int?, done: Bool = true, kind: String? = nil, lost: String? = nil,
                  paused: String? = nil) -> Info {
            Info(connected: true, site: "acme", refused: nil, issues: issues, pages: pages, fetchedAt: iso, initialDone: done,
                 errorKind: kind, lostSince: lost, pausedUntil: paused)
        }
        typealias Scope = AtlassianScopes.Scope
        keeper.atlassianScopesPreview = AtlassianScopes(
            projects: [Scope(id: "p1", key: "ENG", name: "Engineering", count: 912), Scope(id: "p2", key: "OPS", name: "Operations", count: 328)],
            spaces: [Scope(id: "s1", key: "ENG", name: "엔지니어링", count: 241), Scope(id: "s2", key: "PM", name: "제품", count: 117),
                     Scope(id: "s3", key: "ARC", name: "아카이브", count: 22)])
        switch state {
        case "waiting": keeper.atlassianPreview = nil; link.preview(nil, phase: .waiting)
        case "denied": keeper.atlassianPreview = nil; link.preview(nil, phase: .failed(AtlassianLink.words(.denied)))
        case "expired": keeper.atlassianPreview = nil; link.preview(nil, phase: .failed(AtlassianLink.words(.expired)))
        case "blocked": keeper.atlassianPreview = nil; link.preview(nil, phase: .failed(AtlassianLink.words(.blocked)))
        case "syncing": keeper.atlassianPreview = info(issues: 300, pages: 40, done: false); link.preview(pair)
        case "paused": keeper.atlassianPreview = info(issues: 300, pages: 40, done: false, paused: soon); link.preview(pair)
        case "connected": keeper.atlassianPreview = info(issues: 1240, pages: 380); link.preview(pair)
        case "empty": keeper.atlassianPreview = info(issues: 0, pages: 0); link.preview(pair)
        case "lost": keeper.atlassianPreview = info(issues: 1240, pages: 380, lost: iso); link.preview(pair, lost: true)
        case "signed-out": keeper.atlassianPreview = info(issues: 1240, pages: 380); link.preview(pair, signedOut: true)
        case "offline": keeper.atlassianPreview = info(issues: 1240, pages: 380, kind: "network"); link.preview(pair)
        default: keeper.atlassianPreview = nil; keeper.atlassianScopesPreview = nil; link.preview(nil)
        }
    }

    /// Fake Asana states for captures: nothing is written to the keychain and nothing goes to Asana or the engine.
    private func previewAsana(_ state: String) {
        typealias Info = IndexState.AsanaInfo
        let link = keeper.asana
        let pair = AsanaConnection(token: "preview", refreshToken: "preview", expiresAt: .distantFuture, userGid: "preview",
                                   userName: "김지윤", mutedProjects: ["p3"])
        let iso = ISO8601DateFormatter().string(from: Date().addingTimeInterval(-3 * 3600))
        func info(workspaces: Int? = 2, tasks: Int?, done: Bool = true, kind: String? = nil, lost: String? = nil,
                  blocked: [String] = []) -> Info {
            Info(connected: true, refused: nil, workspaces: workspaces, tasks: tasks, fetchedAt: iso, initialDone: done,
                 errorKind: kind, lostSince: lost, blockedWorkspaces: blocked)
        }
        typealias W = AsanaScopes.Workspace
        typealias P = AsanaScopes.Project
        keeper.asanaScopesPreview = AsanaScopes(
            workspaces: [W(gid: "w1", name: "Acme", tasks: 1180), W(gid: "w2", name: "개인 워크스페이스", tasks: 60)],
            projects: [P(gid: "p1", workspace: "w1", name: "결제 개편", tasks: 412), P(gid: "p2", workspace: "w1", name: "온보딩", tasks: 268),
                       P(gid: "p3", workspace: "w1", name: "채용", tasks: 77), P(gid: "p4", workspace: "w2", name: "할 일", tasks: 60)])
        switch state {
        case "waiting": keeper.asanaPreview = nil; link.preview(nil, phase: .waiting)
        case "denied": keeper.asanaPreview = nil; link.preview(nil, phase: .failed(AsanaLink.words(.denied)))
        case "expired": keeper.asanaPreview = nil; link.preview(nil, phase: .failed(AsanaLink.words(.expired)))
        case "blocked": keeper.asanaPreview = nil; link.preview(nil, phase: .failed(AsanaLink.words(.blocked)))
        case "syncing": keeper.asanaPreview = info(tasks: 300, done: false); link.preview(pair)
        case "connected": keeper.asanaPreview = info(tasks: 1240); link.preview(pair)
        case "blocked-workspace": keeper.asanaPreview = info(workspaces: 1, tasks: 1240, blocked: ["Acme Corp"]); link.preview(pair)
        case "empty": keeper.asanaPreview = info(tasks: 0); link.preview(pair)
        case "lost": keeper.asanaPreview = info(tasks: 1240, lost: iso); link.preview(pair, lost: true)
        case "signed-out": keeper.asanaPreview = info(tasks: 1240); link.preview(pair, signedOut: true)
        case "offline": keeper.asanaPreview = info(tasks: 1240, kind: "network"); link.preview(pair)
        default: keeper.asanaPreview = nil; keeper.asanaScopesPreview = nil; link.preview(nil)
        }
    }

    /// Fake Dropbox states for captures: nothing is written to the keychain and nothing goes to Dropbox or the engine.
    private func previewDropbox(_ state: String) {
        typealias Info = IndexState.DropboxInfo
        let link = keeper.dropbox
        let pair = DropboxConnection(token: "preview", refreshToken: "preview", expiresAt: .distantFuture,
                                     accountID: "preview", account: "jiyoon@example.com", mutedFolders: ["f2"])
        let iso = ISO8601DateFormatter().string(from: Date().addingTimeInterval(-3 * 3600))
        func info(files: Int?, done: Bool = true, kind: String? = nil, lost: String? = nil) -> Info {
            Info(connected: true, account: pair.account, files: files, fetchedAt: iso, initialDone: done, refused: nil,
                 errorKind: kind, lostSince: lost)
        }
        keeper.dropboxFoldersPreview = [DropboxFolder(id: "f1", name: "영업", path: "/영업", files: 412, teamFolder: false),
                                        DropboxFolder(id: "f2", name: "디자인", path: "/디자인", files: 188, teamFolder: false),
                                        DropboxFolder(id: "f3", name: "Acme 팀 폴더", path: "/Acme 팀 폴더", files: 2031,
                                                      teamFolder: true)]
        switch state {
        case "waiting": keeper.dropboxPreview = nil; link.preview(nil, phase: .waiting)
        case "denied": keeper.dropboxPreview = nil; link.preview(nil, phase: .failed(DropboxLink.words(.denied)))
        case "expired": keeper.dropboxPreview = nil; link.preview(nil, phase: .failed(DropboxLink.words(.expired)))
        case "scope": keeper.dropboxPreview = nil; link.preview(nil, phase: .failed(DropboxLink.words(.scope)))
        case "syncing": keeper.dropboxPreview = info(files: 300, done: false); link.preview(pair)
        case "partial": keeper.dropboxPreview = info(files: 640, done: false); link.preview(pair)
        case "connected": keeper.dropboxPreview = info(files: 1240); link.preview(pair)
        case "empty": keeper.dropboxPreview = info(files: 0); link.preview(pair)
        case "lost": keeper.dropboxPreview = info(files: 1240, lost: iso); link.preview(pair, lost: true)
        case "unreadable": keeper.dropboxPreview = nil; link.preview(nil, unreadable: true)
        case "offline": keeper.dropboxPreview = info(files: 1240, kind: "network"); link.preview(pair)
        default: keeper.dropboxPreview = nil; keeper.dropboxFoldersPreview = nil; link.preview(nil)
        }
    }

    /// `<tool>-connect <file>`'s wait: the `tacie://<tool>` address `<tool>-return` hands over.
    /// The id tells a cancel which wait it belongs to (connecting again cancels the last one).
    private var signInReturn: (id: Int, wait: CheckedContinuation<URL, Error>)?
    private var signInWaits = 0

    /// A sign-in page written to `file` instead of shown; the wait ends at `<tool>-return` (headless E2E). Once only:
    /// `restore` puts the real sign-in window back.
    private func debugSignIn(file: String, restore: @escaping () -> Void) -> (URL) async throws -> URL {
        { @MainActor [weak self] url in
            try? url.absoluteString.write(toFile: file, atomically: true, encoding: .utf8)
            restore()
            // cancel (the card's 취소, the 5-minute limit) or a gone app delegate ends the wait at once
            guard let self else { throw CancellationError() }
            self.signInWaits += 1
            let id = self.signInWaits
            return try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { c in
                    guard !Task.isCancelled else { return c.resume(throwing: CancellationError()) }
                    self.signInReturn?.wait.resume(throwing: CancellationError())
                    self.signInReturn = (id, c)
                }
            } onCancel: {
                Task { @MainActor [weak self] in
                    guard let self, let r = self.signInReturn, r.id == id else { return }
                    self.signInReturn = nil
                    r.wait.resume(throwing: CancellationError())
                }
            }
        }
    }

    private func debugCommand(_ cmd: String) {
        let parts = cmd.split(separator: " ", maxSplits: 1).map(String.init)
        Log.info("debug: \(parts.first ?? "")")
        let rest = parts.count > 1 ? parts[1] : ""
        switch parts.first {
        case "pause": meeting.togglePause()
        case "net-state": meeting.debugNet(NetHealth.Level(rawValue: rest))  // `net-state ok|slow|offline|auto`: the pill's bars (captures)
        case "stop": meeting.stop(by: .auto)
        case "calendar-connect":  // 연결 on the calendar card (E2E; macOS asks the first time)
            Task { await CalendarStore.shared.connect(); Log.info("calendar access: \(CalendarStore.shared.access)") }
        case "calendar-test-event":  // `calendar-test-event [minutes]`: a plain test event starting then (E2E)
            do { try CalendarStore.shared.debugMakeTestEvent(startingIn: Double(rest) ?? 1); Log.info("calendar test event made") }
            catch { Log.info("calendar test event failed: \(error)") }
        case "calendar-test-delete": Log.info("calendar test events removed: \(CalendarStore.shared.debugDeleteTestEvents())")
        case "calendar-probe":  // `calendar-probe <unix seconds>`: what a meeting starting then would link (counts only)
            let at = Double(rest).map { Date(timeIntervalSince1970: $0) } ?? Date()
            Task {
                let e = await CalendarStore.shared.eventNow(at: at, app: nil)
                Log.info("calendar probe: found \(e != nil), invited \(e?.invited.count ?? 0), named \(e?.invited.filter { $0.name != nil }.count ?? 0), meaningful \(e.map { CalendarLink.meaningful($0.title) } ?? false)")
            }
        case "ending": meeting.proposeEnd(from: Date())
        case "start": startMeeting(MeetingMode(rawValue: rest) ?? lastMode)
        case "replay":
            let r = rest.split(separator: " ", maxSplits: 1).map(String.init)
            guard r.count == 2, let mode = MeetingMode(rawValue: r[0]) else { break }
            lastMode = mode
            meeting.start(mode, source: FileSource(url: URL(fileURLWithPath: r[1]), as: mode == .remote ? .them : nil))
        case "replay-resume":  // `replay-resume <meeting id> <wav>`: 이어서 듣기 on that meeting, fed from a file (E2E)
            let r = rest.split(separator: " ", maxSplits: 1).map(String.init)
            guard r.count == 2, let m = library.meetings.first(where: { $0.id == r[0] }), let mode = MeetingMode(rawValue: m.mode) else { break }
            meeting.start(mode, source: FileSource(url: URL(fileURLWithPath: r[1]), as: mode == .remote ? .them : nil), resuming: m)
        case "transcript": meeting.toggleTranscript()  // 대화 기록 under the pill, open or closed (captures)
        case "ask" where !rest.isEmpty: meeting.beginAsking(); meeting.query = rest; meeting.ask()
        case "asking": meeting.beginAsking()
        case "typing": meeting.beginAsking(); meeting.query = rest  // `query` without the selection: typed-looking text (captures)
        case "query":
            meeting.beginAsking()
            meeting.query = rest
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                NSApp.windows.first { $0 is TaciePanel }?.firstResponder?.tryToPerform(#selector(NSText.selectAll(_:)), with: nil)
            }
        case "close": meeting.close()
        case "panel-ask" where !rest.isEmpty:  // `panel-ask <text>`: a new conversation on the page's meeting, asked (E2E)
            panel.open(scope: panel.pageScope(for: main.selection.id), via: .page, startHere: true)
            panel.newThread()
            panel.ask(rest)
        case "panel-ask-all" where !rest.isEmpty:  // `panel-ask-all <text>`: a new conversation over every meeting, asked (captures)
            panel.open(scope: .all, via: .shortcut, startHere: true)
            panel.newThread()
            panel.ask(rest)
        case "panel-apply", "panel-undo", "panel-dismiss":  // the newest proposal card of the conversation on screen (E2E)
            panel.debugCard(parts[0])
        case "panel":  // the main window's panel, open or shut, as ⌘J does (captures of its opening)
            if panel.isOpen { panel.isOpen = false } else { panel.open(scope: panel.scope, via: .shortcut) }
        case "onboarding":  // `onboarding [welcome|sources|listen|ask|meeting|done]`: open it, on that step (captures)
            onboarding.show(restart: rest.isEmpty, step: OnboardingStep.allCases.first { "\($0)" == rest })
        case "onboarding-repos":  // the sources step's repository list, as right after connecting GitHub (captures)
            onboarding.show(restart: false, step: .sources)
            onboarding.model.sourcesPage = .repos
        case "onboarding-close": onboarding.close()
        case "model-state":  // `model-state expensive|space|offline|later|failed|downloading`: the download's line (captures)
            let states: [String: ModelDownload.State] = [
                "expensive": .waiting(.expensive), "space": .waiting(.space(1_300_000_000)), "offline": .waiting(.offline),
                "later": .later, "failed": .failed("서버가 503로 답했어요"), "downloading": .downloading,
            ]
            if let s = states[rest] { model.show(s) }
        case "linear-connect":  // `linear-connect <file>`: 연결, with the approval page written to <file> (headless E2E, then linear-return)
            let link = keeper.linear, browser = link.authenticate
            link.authenticate = debugSignIn(file: rest) { link.authenticate = browser }
            link.connect()
        case "notion-connect":  // `notion-connect <file>`: 연결, with the approval page written to <file> (headless E2E, then notion-return)
            let link = keeper.notion, browser = link.authenticate
            link.authenticate = debugSignIn(file: rest) { link.authenticate = browser }
            link.connect()
        case "linear-preview":  // `linear-preview off|waiting|failed|connected|empty|lost|admin|offline`: the Linear card (captures)
            previewLinear(rest)
        case "gdrive-connect":  // `gdrive-connect <file>`: 연결, with the approval page written to <file> instead of opened (E2E)
            let link = keeper.gdrive, browser = link.open
            link.open = { url in
                try? url.absoluteString.write(toFile: rest, atomically: true, encoding: .utf8)
                link.open = browser
            }
            link.connect()
        case "gdrive-preview":  // `gdrive-preview off|waiting|failed|connected|partial|empty|lost|offline`: the Drive card (captures)
            previewGDrive(rest)
        case "atlassian-connect":  // `atlassian-connect [<file>]`: 연결; with <file>, the approval page goes there (E2E)
            let link = keeper.atlassian
            if !rest.isEmpty {
                let browser = link.authenticate
                link.authenticate = debugSignIn(file: rest) { link.authenticate = browser }
            }
            link.connect()
        case "atlassian-return", "asana-return", "linear-return", "notion-return":
            // `<tool>-return <tacie://<tool>?…>`: where the browser ended (after <tool>-connect <file>)
            let r = signInReturn
            signInReturn = nil
            if let url = URL(string: rest) { r?.wait.resume(returning: url) } else { r?.wait.resume(throwing: CancellationError()) }
        case "atlassian-preview":  // `atlassian-preview off|waiting|denied|expired|blocked|syncing|paused|connected|empty|lost|signed-out|offline`
            previewAtlassian(rest)
        case "asana-connect":  // `asana-connect [<file>]`: 연결; with <file>, the approval page goes there (E2E)
            let link = keeper.asana
            if !rest.isEmpty {
                let browser = link.authenticate
                link.authenticate = debugSignIn(file: rest) { link.authenticate = browser }
            }
            link.connect()
        case "asana-preview":  // `asana-preview off|waiting|denied|expired|blocked|syncing|connected|blocked-workspace|empty|lost|signed-out|offline`
            previewAsana(rest)
        case "dropbox-connect":  // `dropbox-connect <file>`: 연결, with the approval page written to <file> instead of opened (E2E)
            guard !rest.isEmpty else {
                Log.info("debug: dropbox-connect needs a file")
                return
            }
            let link = keeper.dropbox, browser = link.open
            link.open = { url in
                try? url.absoluteString.write(toFile: rest, atomically: true, encoding: .utf8)
                link.open = browser
            }
            link.connect()
        case "notes-choose":  // `notes-choose all|none`: vault 고르기 with every vault Obsidian knows, or none (docs/34)
            let vaults = rest == "all" ? ObsidianVaults.found() : []
            Log.info("debug: notes-choose \(vaults.count) vaults")
            Task { await keeper.setNoteFolders(vaults.map { NoteFolder(path: $0.path, vaultID: $0.id, name: $0.name) }) }
        case "notes-folder" where !rest.isEmpty:  // `notes-folder <path>`: that one Markdown folder as the note folders (captures)
            Task { await keeper.setNoteFolders([NoteFolder(path: rest, vaultID: nil, name: (rest as NSString).lastPathComponent)]) }
        case "decisions-mark" where !rest.isEmpty:  // `decisions-mark <meeting id>`: compare its decisions with earlier ones now (captures)
            Task { await meeting.markChangedDecisions(rest) }
        case "dropbox-preview":  // `dropbox-preview off|waiting|denied|expired|scope|syncing|partial|connected|empty|lost|unreadable|offline`
            previewDropbox(rest)
        case "probe-call": Task { await onboarding.model.turnOnCallAudio() }  // the listen step's 회의 앱 소리 켜기
        case "listen-preview":  // `listen-preview new|on|nomic|denied|probing|stuck`: the listen step's permission states (captures)
            let m = onboarding.model
            switch rest {
            case "new": m.preview(mic: .notDetermined, connected: true, call: .unknown, probing: false, level: 0)
            case "on": m.preview(mic: .authorized, connected: true, call: .granted, probing: false, level: 0.55)
            case "nomic": m.preview(mic: .authorized, connected: false, call: .unknown, probing: false, level: 0)
            case "denied": m.preview(mic: .denied, connected: true, call: .unknown, probing: false, level: 0)
            case "probing": m.preview(mic: .authorized, connected: true, call: .unknown, probing: true, level: 0.2)
            case "stuck": m.preview(mic: .authorized, connected: true, call: .silent, probing: false, level: 0)
            default: break
            }
        case "history": meeting.toggle(.history)
        case "chooser": meeting.toggle(.chooser)
        case "limit-preview":  // `limit-preview stopped|start`: the card after the month's Tacie AI ran out (captures)
            meeting.debugFailed(Account.limitLine(kept: rest == "stopped"), limit: true)
        case "present": meeting.presenting.toggle()
        case "main": main.show(focus: true)
        case "main-below": main.debugBelowIsland(Double(rest) ?? 0)  // `main-below <points>`: under the island, centred (captures)
        case "main-at":  // `main-at <x> <y> [w h]`: the window's top-left at these screen points (top-left origin of the main screen), e.g. on a capture display
            let n = rest.split(separator: " ").compactMap { Double($0) }
            if n.count >= 2 { main.debugPlace(x: n[0], y: n[1], width: n.count >= 4 ? n[2] : nil, height: n.count >= 4 ? n[3] : nil) }
        case "main-peek": main.show(focus: false)  // up but not key: the island still shows answers over it (captures)
        case "search": main.selection.query = rest
        case "main-close": main.close()
        case "settings": openSettings(SettingsPane(rawValue: rest) ?? .account)
        case "settings-close": main.selection.settings = nil
        case "ai-mode":  // `ai-mode key|local|custom|none`: the open edition's AI choice, as the cards apply it (captures, 2-D)
            Task {
                if let mode = AIMode(rawValue: rest) {
                    let found = mode == .local ? await AIChecks.findLocalServer() : nil
                    await AIModeApply.apply(mode, server: found, context: settingsContext)
                } else {
                    AIMode.save(nil)
                    await keeper.pushAIKeys()
                }
            }
        case "settings-row": main.selection.settingsRow = rest  // `settings-row <row title>`: scroll to it as 설정 검색 does (captures)
        case "appearance": if let c = AppearanceMode.Choice(rawValue: rest) { AppearanceMode.shared.choice = c }  // system | light | dark
        case "update-check": AppUpdate.shared.updater?.checkForUpdatesInBackground()  // a scheduled check, now (captures)
        case "size":
            let wh = rest.split(separator: " ").compactMap { Double($0) }
            if wh.count == 2 { main.resize(width: wh[0], height: wh[1]) }
        case "text-size":  // `text-size up|down|reset`: ⌘+ / ⌘- / ⌘0 on the main window (captures)
            let d = UserDefaults.standard, v = d.object(forKey: TextScale.key) as? Double ?? TextScale.standard
            d.set(rest == "up" ? TextScale.larger(v) : rest == "down" ? TextScale.smaller(v) : TextScale.standard, forKey: TextScale.key)
        case "click":
            let xy = rest.split(separator: " ").compactMap { Double($0) }
            if xy.count == 2 { main.debugClick(x: xy[0], y: xy[1]) }
        case "scroll": main.debugScroll(rest)  // `scroll <points>|top`: the main window's page, without the wheel (captures)
        case "select": main.selection.id = rest
        case "ledger-bench": main.debugLedgerBench(rest)  // `ledger-bench <meeting id> [n]`: check timing (perf)
        case "view": main.selection.view = SideView(rawValue: rest)  // `view todos|decisions|questions`
        case "search": main.selection.query = rest
        case "focus":
            let f = rest.split(separator: " ").map(String.init)
            if f.count == 2 { main.show(meeting: f[0], question: f[1], focus: true) }
        case "rewrite": meeting.rewriteNotes(rest)
        case "kind":  // `kind <meeting id> <meeting|interview|talk>`: the notes again as that kind (renders)
            let f = rest.split(separator: " ").map(String.init)
            if f.count == 2, let k = NotesKind(rawValue: f[1]) { meeting.changeKind(f[0], to: k) }
        case "notice": Self.copyNotice()
        case "slack":
            let f = rest.split(separator: "|", maxSplits: 2).map(String.init)
            if f.count == 3 { meeting.fromSlack(channel: f[0], author: f[1], text: f[2]) }
        case "move":
            let xy = rest.split(separator: " ").compactMap { Double($0) }
            if xy.count == 2 { island.move(toTopCenter: NSPoint(x: xy[0], y: xy[1])) }
        case "hover": meeting.hover(rest == "in")
        case "pill": meeting.hoverPill(rest == "in")
        case "state": break  // just log the state line below
        case "meeting-app":  // `meeting-app zoom|browser|…`: pretend that app holds the microphone; `off` none; `real` the real list
            meetingApps.pretend = rest == "real" ? nil : rest == "off" ? [] : Set([MeetingApp(rawValue: rest)].compactMap { $0 })
        case "refresh": Task { await keeper.refresh() }
        case "update": AppUpdate.shared.show(version: rest.isEmpty ? nil : rest)  // `update 0.1.12` (captures)
        case "sign-in": TacieAccount.shared.signIn()  // 설정 → 계정의 로그인 (where its window comes up)
        case "sign-in-cancel": TacieAccount.shared.cancelSignIn()
        case "sign-in-sheet": main.selection.signIn = .card; main.show(focus: true)  // the main window's sign-in modal (captures)
        case "rebuild":  // `rebuild owner/name [branch]`: the repo picker's 동기화 (captures of a first sync)
            let r = rest.split(separator: " ").map(String.init)
            if let repo = r.first { Task { await keeper.rebuild(repo: repo, branch: r.count > 1 ? r[1] : nil, fresh: false) } }
        case "ai-key":  // `ai-key typesafe|openrouter <key>` or `ai-key typesafe -`: 설정 > AI's 저장 / 지우기 (checks)
            let r = rest.split(separator: " ", maxSplits: 1).map(String.init)
            if let name = r.first.flatMap(AIKeys.Name.init(rawValue:)), r.count == 2 {
                if r[1] == "-" { try? AIKeys.remove(name) } else { try? AIKeys.save(r[1], as: name) }
                Task { await keeper.pushAIKeys() }
            }
        case "sync-preview": keeper.debugSyncing(rest.isEmpty || rest == "off" ? nil : rest)  // `sync-preview <step>|off` (captures)
        case "sync-check": Task { await keeper.check(auto: true) }
        case "look":
            if let i = Int(rest), meeting.trail.indices.contains(i) { meeting.look(at: meeting.trail[i].id) }
        default: break
        }
        Log.info("state: \(meeting.state)\(meeting.trouble.map { " · trouble: \($0)" } ?? "")")
    }

    private func observe() {
        withObservationTracking {
            _ = engine.state
            _ = meeting.state
            _ = meeting.isRunning
            _ = meeting.asking
            _ = meeting.shown
            _ = meeting.prompt
            _ = meeting.mode
            _ = keeper.step
            _ = keeper.progress
            _ = keeper.connected
            _ = keeper.builtAt
            _ = keeper.info?.chunks
            _ = keeper.notion.connected
            _ = keeper.linear.connected
            _ = keeper.gdrive.connected
            _ = keeper.atlassian.connected
            _ = keeper.asana.connected
            _ = keeper.dropbox.connected
            _ = keeper.problem
            _ = keeper.problemKind
            _ = keeper.partial
            _ = AppUpdate.shared.state
        } onChange: { [weak self] in
            Task { @MainActor in
                self?.refreshMenu()
                self?.followIslandWork()
                self?.followMeetingEnd()
                self?.meetingApps.check()
                self?.followNewData()
                self?.followFirstSteps()
                self?.observe()
            }
        }
    }

    /// 처음 써 보기 (docs/11 14절): once the engine has answered, someone updating in gets what they already did, and a
    /// connection checks 팀 자료 연결하기 (it stays checked if the connection goes later).
    private func followFirstSteps() {
        guard keeper.info != nil else { return }
        let hasSource = onboarding.model.facts.hasSource
        FirstStepsStore.shared.backfill(hasMeeting: library.meetings.map(\.start).min(), hasSource: hasSource)
        if hasSource { FirstStepsStore.shared.mark(.source) }
    }

    /// Search data built anew (a first sync after connecting, or any later one): questions kept while nothing was
    /// connected are searched now (docs/11 4-1절 3번). A question with no kept ones costs nothing.
    private func followNewData() {
        // new search data, or a source whose first fetch just finished (its key appears without a rebuild)
        let built = keeper.builtAt
        let keys = ResearchRules.keys(keeper.info, building: keeper.step != nil, atlassianCloud: keeper.atlassian.saved?.cloud,
                                      asanaUser: keeper.asana.saved?.userGid, dropboxAccount: keeper.dropbox.saved?.accountID)
        defer { lastBuilt = built; if let keys { lastKeys = keys } }
        guard let built, let keys, built != lastBuilt || keys != lastKeys else { return }
        meeting.searchAgain(keys: keys)
    }

    private func restartWithModel() {
        restartForModel = false
        Log.info("engine restarts with the search model")
        engine.restart()
        Task {
            // the refused sync goes once the engine answers again
            for _ in 0..<60 where engine.client == nil || engine.state != .ready { try? await Task.sleep(for: .milliseconds(500)) }
            try? await Task.sleep(for: .seconds(1))
            await keeper.resumeAfterModel()
        }
    }

    /// A sync that waited for the meeting (the rules say no while one is on) starts when it ends.
    private func followMeetingEnd() {
        let running = meeting.isRunning
        defer { wasRunning = running }
        if wasRunning && !running && restartForModel { restartWithModel() }
        if wasRunning && !running { Task { await keeper.check(auto: true) } }
    }

    /// The island comes up when it gets work (also for starts that skip startMeeting, like the debug replay) and,
    /// with "화면 위 Tacie 보이기" off, goes away only when the work ends: hiding on every change would undo the menu's show.
    private func followIslandWork() {
        let has = islandHasWork
        defer { islandHadWork = has }
        if has && !islandHadWork {
            island.show()
        } else if !has && islandHadWork && !Self.islandAlways {
            island.hide()
        }
    }

    private func refreshMenu() {
        for item in meetingItems { item.isHidden = meeting.isRunning }
        stopItem?.isHidden = !meeting.isRunning
        islandItem?.state = Self.islandAlways ? .on : .off
        switch engine.state {
        case .starting: statusLine?.title = String(localized: "검색 엔진 준비 중…")
        case .ready: statusLine?.title = keeper.info?.modelReady == false ? String(localized: "menu.modelDownloading", defaultValue: "검색 모델을 받고 있어요") : ""
        case .failed(let msg): statusLine?.title = msg
        }
        statusLine?.isHidden = statusLine?.title.isEmpty ?? true
        refreshItem?.title = SettingsText.menuSync(fraction: keeper.syncFraction, syncing: keeper.step != nil,
                                                   failed: keeper.problem != nil, lost: keeper.lostNames, builtAt: keeper.builtAt, now: Date(),
                                                   calendar: MainView.calendar)
        refreshItem?.isEnabled = keeper.step == nil && engine.state == .ready && keeper.anySource
        switch AppUpdate.shared.state {
        case .ready(let v):
            updateItem?.title = String(localized: "다시 시작해서 업데이트 (\(v))")
            updateItem?.isHidden = false
            updateItem?.isEnabled = !meeting.isRunning
        case .available(let v):
            updateItem?.title = String(localized: "새 버전 받기 (\(v))")
            updateItem?.isHidden = false
            updateItem?.isEnabled = true
        case .none:
            updateItem?.isHidden = true
        }
    }
}
