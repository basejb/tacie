import SwiftUI

/// The main window's content: the meeting list (300 pt) and the chosen meeting, or a first-run page with none.
struct MainView: View {
    let library: MeetingLibrary
    let meeting: MeetingModel
    let onboarding: OnboardingModel
    @Bindable var selection: MainSelection
    let actions: MainActions
    let settings: SettingsContext
    /// The panel on the right (docs/36): ⌘J, and the entrances on the pages. Watched only by `PanelSlot`, so a step
    /// streaming in does not redraw the whole window.
    let panel: PanelStore
    @Environment(\.reducesMotion) private var reduceMotion
    @Environment(\.displayScale) private var displayScale
    /// The window's one player of meeting recordings (MainWindowController).
    @Environment(RecordingPlayer.self) private var player: RecordingPlayer?

    /// Weeks start on Monday: "이번 주" means the working week.
    static let calendar: Calendar = {
        var c = Calendar.current
        c.firstWeekday = 2
        return c
    }()

    @AppStorage("sidebarWidth") private var sidebarWidth = MainView.sidebarStandard
    /// Dragged shut past its narrowest; the button beside 회의 시작 opens it again.
    @AppStorage("sidebarHidden") private var sidebarHidden = false
    @AppStorage(TextScale.key) private var textScale = TextScale.standard
    /// The sidebar width when the drag began; nil while not dragging.
    @State private var dragFrom: Double?
    /// Compact window: the meeting list dropped down under the top bar.
    @State private var listOpen = false
    /// The text-size badge in the corner; each change restarts its timer.
    @State private var badge = false
    @State private var badgeTurn = 0
    /// The feedback card (FeedbackSurvey): what is remembered, whether it is up, and the pointer its face follows.
    @State private var survey = FeedbackSurvey.load()
    @State private var surveyUp = false
    @State private var pointer = PointerTracker()
    /// The connect card (ConnectPrompt): whether it was answered, and whether it is up. It shares the corner with the
    /// feedback card and goes first.
    @State private var connectDone = ConnectPrompt.load()
    @State private var connectUp = false
    /// 새로 생긴 기능 after an update (WhatsNew), while it is up.
    @State private var whatsNew: WhatsNewEntry?

    static let sidebarStandard = 300.0
    static let sidebarRange = 220.0...440.0
    /// Narrower than this the sidebar folds into a bar along the top.
    static let compactBelow = 760.0
    /// The page keeps at least this much beside the sidebar; a wider sidebar gives way when the window narrows.
    static let pageMin = 460.0
    /// How far past the narrowest width the edge has to go before the sidebar folds away.
    static let hideBeyond = 70.0

    var body: some View {
        GeometryReader { g in
            let compact = g.size.width < Self.compactBelow
            TimelineView(.periodic(from: .now, by: 30)) { context in
                if compact {
                    VStack(spacing: 0) {
                        CompactBar(title: currentTitle, open: listOpen, running: meeting.isRunning,
                                   attention: onboarding.keeper.step != nil || !LeftoverList.actionable(leftovers).isEmpty,
                                   toggle: { listOpen.toggle() }, actions: actions, selection: selection)
                        pageAndPanel
                    }
                    .overlay(alignment: .topLeading) { dropdown(context.date, size: g.size) }
                } else {
                    HStack(spacing: 0) {
                        if !sidebarHidden {
                            list(context.date, width: sidebarFits(g.size.width), compact: false)
                                .background(Theme.ink.opacity(0.03))
                                .transition(.move(edge: .leading).combined(with: .opacity))
                        }
                        // its own slot, not an overlay on the list: the drag must outlive the list it folds away
                        Color.clear.frame(width: 0)
                            .overlay { if !sidebarHidden || dragFrom != nil { resizeHandle(window: g.size.width) } }
                            .zIndex(2)
                        pageAndPanel
                    }
                    .overlay(alignment: .topLeading) { if sidebarHidden { hiddenBar } }
                }
            }
            .onChange(of: compact) { if !compact { listOpen = false } }
        }
        .background(Theme.paper)
        .modifier(SurveyHooks(card: feedbackCard, pointer: pointer, chance: surveyChance, running: meeting.isRunning,
                              offer: offerSurvey, hide: { withAnimation(Theme.anim(Theme.enter, reduce: reduceMotion)) { surveyUp = false } }))
        .modifier(SurveyHooks(card: connectCard, pointer: pointer, chance: connectChance, running: meeting.isRunning,
                              offer: offerConnect, hide: { withAnimation(Theme.anim(Theme.enter, reduce: reduceMotion)) { connectUp = false } }))
        .overlay { settingsModal }
        .overlay { signInModal }
        .overlay(alignment: .bottomTrailing) { textSizeBadge }
        .modifier(PanelHooks(panel: panel, page: panelPage))
        .modifier(WhatsNewHooks(modal: whatsNewModal, chance: whatsNewChance, offer: offerWhatsNew,
                                animation: modalMotion(open: whatsNew != nil), shown: whatsNew))
        .background { shortcuts }
        .animation(modalMotion(open: selection.settings != nil), value: selection.settings)
        .animation(modalMotion(open: selection.signIn != nil), value: selection.signIn)
        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: listOpen)
        .onChange(of: selection.id) {
            listOpen = false
            meeting.forgetNamingUndo()
        }
        .onChange(of: library.meetings.count) { TranscriptViewCache.keepOnly(Set(library.meetings.map(\.id))) }
        // settings over the page: its sound stops, as when leaving it
        .onChange(of: selection.settings) { if selection.settings != nil { player?.stop() } }
        .onChange(of: selection.view) { listOpen = false }
        .onChange(of: textScale) {
            Analytics.capture(.textSizeChanged(percent: Int((TextScale.clamp(textScale) * 100).rounded())))
            badge = true
            badgeTurn += 1
        }
        .ignoresSafeArea()
        .foregroundStyle(Theme.ink)
        .appearanceScheme()
        // back from System Settings: a permission may be on now
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            onboarding.refreshPermissions()
        }
        // a meeting starting: the page goes to it, from any other meeting or collected page (a search that would hide it
        // is cleared); settings open over the window stay where they are
        .onChange(of: meeting.liveID) { _, id in
            guard let id else { return }
            selection.query = ""
            selection.id = id
        }
    }

    /// The page with the panel beside it, or over it in a narrow window (PanelLayout).
    private var pageAndPanel: some View {
        PanelSlot(panel: panel, meeting: { id in library.meetings.first { $0.id == id } }, actions: panelActions) { page }
            .environment(\.textScale, TextScale.clamp(textScale))
    }

    /// The meeting the page shows, nil on a page about no meeting (결정 사항, 할 일) or none at all.
    private var panelPage: String? {
        guard selection.view == nil else { return nil }
        let rows = rows
        return (rows.first(where: { $0.id == selection.id }) ?? rows.first)?.id
    }

    /// A press on a source in the panel: a meeting opens in the middle (at its transcript lines when given) without
    /// moving the panel's conversation; anything else opens where it lives.
    private var panelActions: PanelActions {
        var a = PanelActions()
        let shown = panelPage
        let all = Set(allRows.map(\.id)), listed = Set(rows.map(\.id))
        a.openMeeting = { [selection] t in
            let move = selection.jump.go(t, shown: shown, exists: all.contains(t.meetingID), listed: listed.contains(t.meetingID))
            if move != .none { Analytics.capture(.panelSourceOpened(kind: .meeting)) }
            guard case .select(let clearQuery) = move else { return }
            // a search in the list that hides the meeting would keep the page where it is (as a meeting starting does)
            if clearQuery { selection.query = "" }
            selection.id = t.meetingID
        }
        let openCard = a.openCard
        a.openCard = { Analytics.capture(.panelSourceOpened(kind: .source)); openCard($0) }
        a.say = MeetingModel.say
        return a
    }

    /// ⌘J: the panel on the page's meeting, or on every meeting; again, closed.
    private func togglePanel() {
        if panel.isOpen { panel.isOpen = false } else { panel.open(scope: panel.pageScope(for: panelPage), via: .shortcut) }
    }

    /// The chosen meeting's page, or the first-run page; text follows the window's text size.
    private var page: some View {
        let rows = rows
        // a ZStack, not a Group: during the crossfade both pages share the slot instead of splitting it
        return ZStack {
            if let v = selection.view {
                Group {
                    switch v {
                    case .home where firstRun:
                        EmptyMain(actions: actions, leftovers: LeftoverList.actionable(leftovers), fix: fix,
                                  dismiss: { onboarding.dismiss($0) }, firstSteps: FirstStepsStore.shared.state,
                                  firstStepsInfo: firstStepsInfo, firstStepsActions: firstStepsActions)
                    case .home:
                        HomePage(meetings: allRows, live: meeting.liveID, now: Date(), selection: selection, library: library,
                                 people: meeting.vault.people, panel: panel, sources: sourceNames, synced: syncedLine,
                                 openConnections: { selection.settings = .connections })
                    case .meetings:
                        MeetingListPage(meetings: allRows, live: meeting.liveID, now: Date(), selection: selection,
                                        library: library, panel: panel)
                    default:
                        CollectedPage(view: v, meetings: library.meetings, library: library, selection: selection,
                                      people: meeting.vault.people, panel: panel)
                    }
                }
                    .id(v.rawValue)
                    .transition(.asymmetric(insertion: Theme.arrive(y: 4, reduce: reduceMotion, scale: displayScale),
                                            removal: .opacity.animation(Theme.swap)))
            } else if let m = rows.first(where: { $0.id == selection.id }) ?? rows.first {
                MeetingPage(meeting: m, live: m.id == meeting.liveID, warning: warning,
                            liveLine: m.id == meeting.liveID ? meeting.liveLine : nil, library: library,
                            selection: selection, ids: rows.map(\.id), actions: actions, people: meeting.vault.people,
                            editing: editing(m.id), running: meeting.isRunning && m.id != meeting.liveID,
                            sources: firstStepsInfo, syncLine: onboarding.keeper.firstSync == nil ? nil : onboarding.keeper.stepLine,
                            connect: { _, _ in selection.settings = .connections }, panel: panel,
                            back: { selection.view = .meetings })
                    .id(m.id)
                    // the old page is gone in a couple of frames, the new one rises 4pt into place (fades only on 1x, Theme.arrive)
                    .transition(.asymmetric(insertion: Theme.arrive(y: 4, reduce: reduceMotion, scale: displayScale),
                                            removal: .opacity.animation(Theme.swap)))
            } else if firstRun {
                EmptyMain(actions: actions, leftovers: LeftoverList.actionable(leftovers), fix: fix,
                          dismiss: { onboarding.dismiss($0) }, firstSteps: FirstStepsStore.shared.state,
                          firstStepsInfo: firstStepsInfo, firstStepsActions: firstStepsActions)
            } else {
                Color.clear
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: selection.id)
        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: selection.view)
        .environment(\.textScale, TextScale.clamp(textScale))
    }

    private func list(_ now: Date, width: CGFloat?, compact: Bool) -> some View {
        let all = allRows
        return MeetingList(rows: all.filter { $0.matches(selection.query) }, all: all, live: meeting.liveID, now: now, running: meeting.isRunning,
                    stopped: stopped, warning: warning,
                    // the sign-in has its own card at the sidebar's foot
                    leftovers: firstRun ? [] : LeftoverList.actionable(leftovers).filter { $0 != .signIn || !MainView.needsSignIn },
                    fix: fix, dismiss: { onboarding.dismiss($0) }, sync: onboarding.keeper.step, syncFraction: onboarding.keeper.syncFraction, syncRows: onboarding.keeper.syncRows, syncNote: onboarding.keeper.syncNote, firstSync: onboarding.keeper.firstSync != nil,
                    lost: onboarding.keeper.lostNames,
                    firstSteps: firstRun ? nil : FirstStepsStore.shared.state, firstStepsInfo: firstStepsInfo,
                    firstStepsActions: firstStepsActions,
                    download: onboarding.download, stt: settings.localSTT, selection: selection, actions: actions,
                    pin: { id, on in library.update(id) { $0.pinned = on ? true : nil } },
                    remove: { library.remove($0) },
                    ask: { PanelEntries.searchAsk($0, in: panel) },
                    width: width, compact: compact)
            .environment(\.textScale, TextScale.clamp(textScale))
    }

    /// The stored width, or less when the window is too narrow to give the page its room.
    private func sidebarFits(_ window: CGFloat) -> CGFloat {
        let stored = min(max(sidebarWidth, Self.sidebarRange.lowerBound), Self.sidebarRange.upperBound)
        return max(Self.sidebarRange.lowerBound, min(stored, window - Self.pageMin))
    }

    /// The sidebar's right edge: a hairline, and a strip a few points wide that drags it (double-click: back to 300).
    private func resizeHandle(window: CGFloat) -> some View {
        let shown = sidebarFits(window)
        // the hairline sits on the edge; the strip that takes the drag reaches a few points into the page
        return Color.clear
        .frame(width: 9)
        .contentShape(Rectangle())
        .overlay(alignment: .leading) {
            Rectangle().fill(dragFrom == nil ? Theme.hairline : Theme.ink.opacity(0.2)).frame(width: 1)
        }
        .offset(x: 4)
        .onHover { inside in
            if inside { NSCursor.resizeLeftRight.push() } else { NSCursor.pop() }
        }
        .gesture(
            DragGesture(minimumDistance: 1, coordinateSpace: .global)
                .onChanged { v in
                    let from = dragFrom ?? (sidebarHidden ? 0 : shown)
                    dragFrom = from
                    let w = from + v.translation.width
                    // past the narrowest width by hideBeyond it folds away; back over that line it comes out again
                    let hide = w < Self.sidebarRange.lowerBound - Self.hideBeyond
                    if hide != sidebarHidden {
                        withAnimation(Theme.anim(Theme.settle, reduce: reduceMotion)) { sidebarHidden = hide }
                    }
                    if !hide {
                        sidebarWidth = min(max(w, Self.sidebarRange.lowerBound), min(Self.sidebarRange.upperBound, window - Self.pageMin))
                    }
                }
                .onEnded { _ in dragFrom = nil }
        )
        .simultaneousGesture(TapGesture(count: 2).onEnded {
            withAnimation(Theme.anim(Theme.settle, reduce: reduceMotion)) { sidebarWidth = Self.sidebarStandard }
        })
        .help("끌어서 너비를 바꿔요. 두 번 클릭하면 원래 너비로 돌아가요.")
    }

    /// The sidebar dragged shut: 회의 시작 and the button that opens the sidebar again, beside the traffic lights.
    private var hiddenBar: some View {
        HStack(spacing: 6) {
            StartMeetingButton(start: actions.startMode, disabled: meeting.isRunning)
            Button {
                withAnimation(Theme.anim(Theme.settle, reduce: reduceMotion)) { sidebarHidden = false }
            } label: { IconButton(symbol: "sidebar.left") }
                .buttonStyle(.plain)
                .help("회의 목록 펼치기")
        }
        .padding(.leading, CompactBar.besideLights)
        .frame(height: CompactBar.height)
        .transition(.opacity)
    }

    /// Compact window: the list under the top bar, over the page. A click outside or Esc folds it back.
    @ViewBuilder private func dropdown(_ now: Date, size: CGSize) -> some View {
        if listOpen {
            ZStack(alignment: .topLeading) {
                Theme.scrimSoft
                    .contentShape(Rectangle())
                    .onTapGesture { listOpen = false }
                    .transition(.opacity)
                list(now, width: nil, compact: true)
                    .frame(width: min(360, size.width - 24), height: max(240, min(600, size.height - CompactBar.height - 24)))
                    .background(Theme.raised)
                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.ink.opacity(0.08)))
                    .shadow(color: Theme.modalShadow, radius: 24, y: 10)
                    .padding(.leading, 12)
                    .padding(.top, 6)
                    .transition(reduceMotion ? .opacity
                                : .asymmetric(insertion: .opacity.combined(with: .offset(y: -6)),
                                              removal: .opacity.animation(Theme.exit)))
                if selection.settings == nil {
                    Button("") { listOpen = false }
                        .keyboardShortcut(.cancelAction)
                        .frame(width: 0, height: 0)
                        .opacity(0)
                }
            }
            .padding(.top, CompactBar.height)
        }
    }

    /// The compact bar's title: the chosen meeting, or the list's first.
    private var currentTitle: String {
        if let v = selection.view { return v.title }
        let rows = rows
        guard let m = rows.first(where: { $0.id == selection.id }) ?? rows.first else { return String(localized: "mainView.meetings", defaultValue: "회의") }
        return m.displayTitle(calendar: MainView.calendar)
    }

    /// ⌘+ (and ⌘=, the same key without Shift), ⌘-, ⌘0: the main window's text size.
    private var shortcuts: some View {
        ZStack {
            Button("") { textScale = TextScale.larger(textScale) }.keyboardShortcut("=", modifiers: .command)
            Button("") { textScale = TextScale.larger(textScale) }.keyboardShortcut("+", modifiers: .command)
            Button("") { textScale = TextScale.smaller(textScale) }.keyboardShortcut("-", modifiers: .command)
            Button("") { textScale = TextScale.standard }.keyboardShortcut("0", modifiers: .command)
            Button("") { togglePanel() }.keyboardShortcut("j", modifiers: .command)
            // ⌘Z / ⌘⇧Z: the last naming of speakers (MeetingModel.namingUndo); in a field being typed in, its text
            Button("") { undo(redo: false) }.keyboardShortcut("z", modifiers: .command)
            Button("") { undo(redo: true) }.keyboardShortcut("z", modifiers: [.command, .shift])
        }
        .frame(width: 0, height: 0)
        .opacity(0)
        .accessibilityHidden(true)
    }

    private func undo(redo: Bool) {
        if let field = NSApp.keyWindow?.firstResponder as? NSTextView, field.isEditable, let u = field.undoManager {
            redo ? u.redo() : u.undo()
        } else {
            redo ? meeting.namingUndo.redo() : meeting.namingUndo.undo()
        }
    }

    /// Bottom right, for a moment after the text size changes: the new size. Fades away on its own.
    // MARK: - feedback card

    /// The card over the window's lower right, the pointer its face follows, and when it may come up (kept out of `body`,
    /// whose chain is long enough already for the type checker).
    private struct SurveyHooks<Card: View>: ViewModifier {
        let card: Card
        let pointer: PointerTracker
        let chance: String
        let running: Bool
        let offer: () async -> Void
        let hide: () -> Void

        func body(content: Content) -> some View {
            content
                .overlay(alignment: .bottomTrailing) { card }
                .onContinuousHover(coordinateSpace: .global) { phase in
                    switch phase {
                    case .active(let p): pointer.location = p
                    case .ended: pointer.location = nil
                    }
                }
                .task(id: chance) { await offer() }
                .onChange(of: running) { if running { hide() } }
        }
    }

    /// The page and the panel: the one view that watches the panel's store.
    private struct PanelSlot<Page: View>: View {
        @ObservedObject var panel: PanelStore
        let meeting: (String) -> SavedMeeting?
        let actions: PanelActions
        @ViewBuilder let page: Page

        var body: some View {
            PanelLayout(open: panel.isOpen, close: { panel.isOpen = false }) {
                page
            } panel: {
                PanelView(store: panel, meeting: meeting, actions: actions)
            }
        }
    }

    /// The page under the panel changed: where a new conversation starts follows it, also after a source in the panel
    /// moved it; the conversation on screen stays (docs/36 0-1절).
    private struct PanelHooks: ViewModifier {
        let panel: PanelStore
        let page: String?

        func body(content: Content) -> some View {
            content.onChange(of: page) { _, id in
                panel.pageChanged(to: id)
            }
        }
    }

    /// Something over the window that the corner cards (feedback, connect) wait for: settings, the sign-in, 새로 생긴 기능.
    private var cardCovered: Bool { selection.settings != nil || selection.signIn != nil || whatsNew != nil }

    /// The page the card would come up over: the meeting shown, when no collected view is.
    private var surveyPage: SavedMeeting? {
        guard selection.view == nil else { return nil }
        let rows = rows
        return rows.first(where: { $0.id == selection.id }) ?? rows.first
    }

    private var surveyAllowed: Bool {
        FeedbackSurvey.offered(signedIn: TacieAccount.shared.signedIn) && !connectUp && !connectAllowed && FeedbackSurvey.shouldShow(record: survey, notes: FeedbackSurvey.notesCount(library.meetings), page: surveyPage,
                                  running: meeting.isRunning, covered: cardCovered)
    }

    /// Changes whenever the card's chance changes: a new page, a meeting starting or ending, a modal over the window.
    private var surveyChance: String {
        "\(surveyPage?.id ?? "-")|\(surveyPage?.notes.rawValue ?? "-")|\(meeting.isRunning)|\(cardCovered)|\(survey.shows)|\(survey.finished)"
    }

    /// The page has been open `FeedbackSurvey.delay` seconds and the card may come up: it does, once per launch.
    private func offerSurvey() async {
        guard !surveyUp, surveyAllowed else { return }
        try? await Task.sleep(for: .seconds(FeedbackSurvey.demo ? 1 : FeedbackSurvey.delay))
        guard !Task.isCancelled, !surveyUp, surveyAllowed else { return }
        survey.shows += 1
        FeedbackSurvey.save(survey)
        Analytics.capture(.feedbackShown(shown: survey.shows))
        withAnimation(Theme.anim(Theme.grow, reduce: reduceMotion)) { surveyUp = true }
    }

    // MARK: - connect card

    /// What is connected, once the engine has said (nil before: the card waits rather than guess).
    private var teamSource: Bool? {
        onboarding.keeper.info == nil ? nil : onboarding.facts.hasSource
    }

    private var connectCounts: ConnectPrompt.Counts { ConnectPrompt.counts(library.meetings) }

    private var connectAllowed: Bool {
        ConnectPrompt.shouldShow(done: connectDone, counts: connectCounts, teamSource: teamSource,
                                 recordOnly: MeetingModel.recordOnlyDefault, page: surveyPage, running: meeting.isRunning,
                                 covered: cardCovered)
    }

    /// Changes whenever the connect card's chance changes (the feedback card's, plus what is connected).
    private var connectChance: String { "\(surveyChance)|\(teamSource.map(String.init) ?? "-")|\(connectDone)" }

    /// The page has been open `ConnectPrompt.delay` seconds and the card may come up: it does, once.
    private func offerConnect() async {
        guard !connectUp, !surveyUp, connectAllowed else { return }
        try? await Task.sleep(for: .seconds(ConnectPrompt.demo ? 1 : ConnectPrompt.delay))
        guard !Task.isCancelled, !connectUp, !surveyUp, connectAllowed else { return }
        let counts = connectCounts
        Analytics.capture(.connectPromptShown(meetings: counts.meetings, unanswered: counts.unanswered))
        withAnimation(Theme.anim(Theme.grow, reduce: reduceMotion)) { connectUp = true }
    }

    @ViewBuilder private var connectCard: some View {
        if connectUp {
            ConnectPromptCard(pointer: pointer, counts: connectCounts) { choice in
                connectDone = true
                ConnectPrompt.save(true)
                Analytics.capture(.connectPromptAnswered(choice: choice))
                switch choice {
                case .connect: selection.settings = .connections
                case .recordOnly:
                    UserDefaults.standard.set(true, forKey: MeetingModel.recordOnlyKey)
                    Analytics.capture(.settingChanged(setting: .recordOnly, value: .on))
                case .close: break
                }
                withAnimation(Theme.anim(Theme.enter, reduce: reduceMotion)) { connectUp = false }
            }
            .padding(22)
            .transition(reduceMotion ? .opacity
                        : .asymmetric(insertion: .offset(y: 24).combined(with: .opacity).combined(with: .scale(scale: 0.96, anchor: .bottomTrailing)),
                                      removal: .offset(y: 30).combined(with: .opacity)))
        }
    }

    @ViewBuilder private var feedbackCard: some View {
        if surveyUp {
            FeedbackCard(pointer: pointer, notes: FeedbackSurvey.notesCount(library.meetings)) { outcome in
                survey.finished = true
                FeedbackSurvey.save(survey)
                switch outcome {
                case let .sent(wishes, text):
                    Analytics.capture(.feedbackSent(wishes: Set(wishes), wrote: !text.isEmpty))
                    Task { _ = await TacieAccount.shared.sendFeedback(wishes: wishes, text: text) }
                case .skipped: Analytics.capture(.feedbackClosed(how: .skip))
                case .closed: Analytics.capture(.feedbackClosed(how: .close))
                }
                withAnimation(Theme.anim(Theme.enter, reduce: reduceMotion)) { surveyUp = false }
            }
            .padding(22)
            .transition(reduceMotion ? .opacity
                        : .asymmetric(insertion: .offset(y: 24).combined(with: .opacity).combined(with: .scale(scale: 0.96, anchor: .bottomTrailing)),
                                      removal: .offset(y: 30).combined(with: .opacity)))
        }
    }

    @ViewBuilder private var textSizeBadge: some View {
        if badge {
            HStack(spacing: 7) {
                Image(systemName: "textformat.size").font(.system(size: 12, weight: .semibold))
                Text("글자 크기 \(TextScale.percent(textScale))").font(.system(size: 13, weight: .semibold).monospacedDigit())
                    .contentTransition(.numericText())
            }
            .foregroundStyle(Theme.onStrong)
            .padding(.horizontal, 14)
            .frame(height: 34)
            .background(Theme.ink.opacity(0.86), in: Capsule())
            .shadow(color: Theme.modalShadow, radius: 12, y: 4)
            .padding(20)
            .allowsHitTesting(false)
            .animation(Theme.anim(Theme.snappy, reduce: reduceMotion), value: textScale)
            .transition(reduceMotion ? .opacity
                        : .asymmetric(insertion: .opacity.combined(with: .scale(scale: 0.96)).animation(Theme.enter),
                                      removal: .opacity.animation(.easeOut(duration: 0.4))))
            .task(id: badgeTurn) {
                try? await Task.sleep(for: .milliseconds(1300))
                guard !Task.isCancelled else { return }
                withAnimation(.easeOut(duration: 0.4)) { badge = false }
            }
        }
    }

    /// Naming speakers on a meeting's page: the meeting model names them, the vault keeps the remembered people.
    private func editing(_ id: String) -> SpeakerEditing {
        SpeakerEditing(near: { meeting.near($0, in: id) },
                       apply: { meeting.applyName($0, key: $1, lines: $2, in: id) },
                       renamePerson: { meeting.vault.rename(person: $0, to: $1) },
                       forgetPerson: { meeting.forget(person: $0) },
                       toast: meeting.namingToast,
                       undo: { meeting.namingUndo.undo() },
                       dropToast: { meeting.dropToast($0) },
                       suggestions: meeting.suggestions(in: id),
                       acceptSuggestion: { meeting.acceptSuggestion(key: $0, person: $1, in: id) },
                       dismissSuggestion: { meeting.dismissSuggestion(key: $0, person: $1, in: id) })
    }

    /// No meeting yet: the page itself lists what is left, so the list's footer row stays away.
    private var firstRun: Bool { library.meetings.isEmpty && meeting.liveID == nil }

    /// 처음 써 보기 (docs/11 14절): what is connected now, what could be, and the unanswered questions.
    private var firstStepsInfo: FirstStepsInfo {
        let k = onboarding.keeper
        var info = FirstStepsInfo()
        if let repo = k.info?.repo, k.connected { info.connected.append(.init(tool: .github, label: "GitHub · \(repo)")) }
        else if ConnectionsScene.Source.available.contains(.github) { info.unconnected.append(.github) }
        let on: [ATool: Bool] = [.notion: k.notion.connected, .linear: k.linear.connected, .gdrive: k.gdrive.connected,
                                 .atlassian: k.atlassian.connected, .asana: k.asana.connected, .dropbox: k.dropbox.connected,
                                 .notes: k.notesOn]
        // only what this edition can connect (the open edition has no Tacie server or connection apps)
        for s in ConnectionsScene.Source.firstStepsTools(open: Build.isOpen, dev: Build.isDev) {
            guard let tool = ATool(rawValue: s.rawValue) else { continue }
            if on[tool] == true { info.connected.append(.init(tool: tool, label: FirstStepsInfo.name(tool))) } else { info.unconnected.append(tool) }
        }
        info.unanswered = library.meetings.reduce(0) { n, m in n + m.questions.filter { $0.unanswered || $0.noSource == true }.count }
        return info
    }

    private var firstStepsActions: FirstStepsActions {
        FirstStepsActions(start: actions.start, skip: { FirstStepsStore.shared.skip($0) },
                          connect: { tool, connected in
                              Analytics.capture(.firstStepToolClicked(tool: tool, connected: connected))
                              selection.settings = .connections
                          },
                          hide: { FirstStepsStore.shared.hide() })
    }

    /// What the onboarding left for later (docs/11 원칙 7).
    private var leftovers: [OnboardingLeftover] { OnboardingFlow.leftovers(onboarding.progress, onboarding.facts) }

    /// Connecting a service is a settings page right here; the rest goes back to the onboarding's step.
    private func fix(_ item: OnboardingLeftover) {
        if item == .connectSource || item == .connectNotion || item == .connectLinear || item == .connectDrive
            || item == .connectAtlassian { selection.settings = .connections }
        else if item == .signIn { selection.signIn = .card }
        else { actions.fixLeftover(item) }
    }

    /// Tacie AI waits for a sign-in on this build (docs/14): the sidebar shows its card, and a meeting asks first.
    static var needsSignIn: Bool { OnboardingModel.accountStep && !TacieAccount.shared.signedIn }

    /// The sign-in sheet over the window (SignInSheet): a click on the scrim or Esc is 나중에.
    private var signInModal: some View {
        // settings opened over it (⌘,) take its place; the sheet comes back when they close
        ModalLayer(item: selection.settings == nil ? selection.signIn : nil) { ask in
            let close = { (signedIn: Bool) in
                guard selection.signIn != nil else { return }  // Esc again while it fades
                selection.signIn = nil
                actions.afterSignIn(signedIn)
            }
            ZStack {
                Theme.scrim
                    .contentShape(Rectangle())
                    .onTapGesture { if TacieAccount.shared.phase != .signingIn { close(false) } }
                    .transition(.asymmetric(insertion: .opacity.animation(Theme.grow), removal: .identity))
                SignInSheet(ask: ask, done: close)
                    .transition(reduceMotion ? .opacity
                                : .asymmetric(insertion: .scale(scale: 0.98).combined(with: .opacity).animation(Theme.grow),
                                              removal: .identity))
                Button("") { if TacieAccount.shared.phase == .signingIn { TacieAccount.shared.cancelSignIn() }; close(false) }
                    .keyboardShortcut(.cancelAction)
                    .frame(width: 0, height: 0)
                    .opacity(0)
            }
        }
    }

    // MARK: - 새로 생긴 기능

    /// The sheet over the window and when it may come up (kept out of `body`, like SurveyHooks).
    private struct WhatsNewHooks<Modal: View>: ViewModifier {
        let modal: Modal
        let chance: String
        let offer: () async -> Void
        let animation: Animation
        let shown: WhatsNewEntry?

        func body(content: Content) -> some View {
            content
                .overlay { modal }
                .animation(animation, value: shown)
                .task(id: chance) { await offer() }
        }
    }

    /// Changes whenever the sheet's chance changes: a meeting starting or ending, a modal over the window.
    private var whatsNewChance: String {
        "\(meeting.isRunning)|\(selection.settings != nil || selection.signIn != nil || surveyUp || connectUp)"
    }

    /// After an update with something worth trying, once the window has been open a moment and nothing else is over
    /// it: never during a meeting (it waits for the meeting to end) and never over settings, the sign-in, the
    /// feedback card or the connect card; those cards in turn wait for it (`cardCovered`). A meeting starting takes it down without marking it seen.
    private func offerWhatsNew() async {
        if meeting.isRunning, whatsNew != nil { whatsNew = nil }
        guard whatsNew == nil, !meeting.isRunning, selection.settings == nil, selection.signIn == nil, !surveyUp,
              !connectUp, WhatsNew.due != nil else { return }
        try? await Task.sleep(for: .milliseconds(600))
        guard !Task.isCancelled, !meeting.isRunning, let entry = WhatsNew.due else { return }
        Log.info("whats new shown: \(entry.version)")
        whatsNew = entry
    }

    private func closeWhatsNew() {
        WhatsNew.markSeen()
        whatsNew = nil
    }

    private var whatsNewModal: some View {
        ModalLayer(item: selection.settings == nil && selection.signIn == nil ? whatsNew : nil) { entry in
            ZStack {
                Theme.scrim
                    .contentShape(Rectangle())
                    .onTapGesture { closeWhatsNew() }
                    .transition(.asymmetric(insertion: .opacity.animation(Theme.grow), removal: .identity))
                WhatsNewSheet(entry: entry,
                              notes: {
                                  if let url = WhatsNew.notesURL { NSWorkspace.shared.open(url) }
                                  closeWhatsNew()
                              },
                              done: closeWhatsNew)
                    .transition(reduceMotion ? .opacity
                                : .asymmetric(insertion: .scale(scale: 0.98).combined(with: .opacity).animation(Theme.grow),
                                              removal: .identity))
                Button("") { closeWhatsNew() }
                    .keyboardShortcut(.cancelAction)
                    .frame(width: 0, height: 0)
                    .opacity(0)
            }
        }
    }

    /// A modal over the window comes in on `grow` and leaves on `exit`. Leaving on `grow` kept the closed modal in the
    /// view tree until the spring settled (about 0.75 s, measured 2026-11-10) while it showed gone after 0.18 s: its
    /// see-through scrim took every click on the window in between.
    private func modalMotion(open: Bool) -> Animation { Theme.anim(open ? Theme.grow : Theme.exit, reduce: reduceMotion) }

    /// Settings over the window: a scrim that closes on a click, and the modal (Esc closes too, even from a text field).
    private var settingsModal: some View {
        ModalLayer(item: selection.settings) { pane in
            GeometryReader { g in
                ZStack {
                    Theme.scrim
                        .contentShape(Rectangle())
                        .onTapGesture { selection.settings = nil }
                        .transition(.asymmetric(insertion: .opacity.animation(Theme.grow), removal: .identity))
                    SettingsView(pane: SettingsPane.resolve(pane), context: settings, selection: selection)
                        .environment(\.textScale, TextScale.clamp(textScale))
                        .frame(width: min(900, g.size.width - 80), height: min(660, g.size.height - 110))
                        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.modalEdge))
                        .shadow(color: Theme.modalShadow, radius: 30, y: 12)
                        .padding(.top, 20)
                        // comes in with a little scale; ModalLayer fades it out
                        .transition(reduceMotion ? .opacity
                                    : .asymmetric(insertion: .scale(scale: 0.98).combined(with: .opacity).animation(Theme.grow),
                                                  removal: .identity))
                    Button("") { selection.settings = nil }
                        .keyboardShortcut(.cancelAction)
                        .frame(width: 0, height: 0)
                        .opacity(0)
                }
            }
        }
    }

    /// 홈's foot: the connected sources by name (설정 → 연결's order), and when the search data was last synced.
    private var sourceNames: [String] {
        let on = ConnectionsPage.linked(settings.keeper)
        return ConnectionsScene.Source.allCases.filter { on[$0] == true }.map(\.name)
    }

    private var syncedLine: String? {
        if let line = settings.keeper.syncLine { return line }
        return settings.keeper.builtAt.map { SettingsText.synced($0, now: Date(), calendar: MainView.calendar) }
    }

    /// The meeting going on first, then the kept ones, filtered by the search field.
    private var rows: [SavedMeeting] { allRows.filter { $0.matches(selection.query) } }

    /// Why the meeting did not start or stopped, the island's 멈췄어요, while no meeting goes on.
    private var stopped: String? {
        guard case .failed(let reason) = meeting.state, meeting.liveID == nil else { return nil }
        return reason
    }

    /// What the island's pill says in place of 대면·원격 while a meeting goes on: a service failing, no microphone.
    private var warning: String? { meeting.liveID == nil ? nil : meeting.trouble ?? meeting.callNote ?? meeting.micNote ?? meeting.sttNote ?? meeting.quietNote }

    /// The same list before the search filter: the list keeps every row mounted and folds the ones that do not
    /// match, so the survivors slide instead of jumping.
    private var allRows: [SavedMeeting] {
        let live = meeting.live
        let kept = library.meetings.filter { $0.id != live?.id }
        return (live.map { [$0] } ?? []) + kept
    }
}

struct MeetingList: View {
    let rows: [SavedMeeting]      // what the search leaves
    let all: [SavedMeeting]       // every meeting; rows the search hides fold to zero height
    let live: String?
    let now: Date
    let running: Bool
    let stopped: String?
    let warning: String?
    let leftovers: [OnboardingLeftover]
    let fix: (OnboardingLeftover) -> Void
    var dismiss: (OnboardingLeftover) -> Void = { _ in }
    /// A running sync's progress ("노션 페이지 받는 중 40%"); a click opens 설정 → 연결.
    var sync: String? = nil
    /// How far the whole sync is, 0–1 (IndexKeeper.syncFraction).
    var syncFraction: Double? = nil
    /// Each source of the running sync on its own line (IndexKeeper.syncRows); nil shows the step in one line.
    var syncRows: [SyncRules.Row]? = nil
    /// What the running sync cannot answer from yet (IndexKeeper.syncNote).
    var syncNote: String? = nil
    /// The running sync is the first one: nothing is answered until it ends.
    var firstSync = false
    /// Connections that were refused (GitHub, 노션, Linear, Google Drive); a click opens 설정 → 연결, where 다시 연결 is.
    var lost: [String] = []
    /// 처음 써 보기 in the footer (nil on the empty page, which shows it in full).
    var firstSteps: FirstStepsState? = nil
    var firstStepsInfo = FirstStepsInfo()
    var firstStepsActions = FirstStepsActions()
    /// The model download: its voice-only top-up shows as a row above the sync block.
    var download: ModelDownload? = nil
    /// On-device speech recognition: its download shows as a row under the voice model's.
    var stt: LocalSTT? = nil
    @Bindable var selection: MainSelection
    let actions: MainActions
    /// Pins a meeting to the top of the list (true) or lets it go (false).
    var pin: (String, Bool) -> Void = { _, _ in }
    /// Deletes a meeting chosen from a row's menu, after 이 회의를 지울까요?.
    var remove: (String) -> Void = { _ in }
    /// The top row of a search: the typed text goes to the panel on every meeting and source (docs/36 1-1절).
    var ask: (String) -> Void = { _ in }
    /// The sidebar's width; nil fills the space it gets (the compact window's drop-down).
    var width: CGFloat? = 300
    /// In the compact window's drop-down: the top bar already holds 회의 시작 and the account.
    var compact = false
    @Namespace private var ns
    @State private var showLeftovers = false
    /// The meeting a row's 이 회의 지우기… asks about.
    @State private var deleting: SavedMeeting?
    /// Older groups (months, years) opened by a click; the rest stay folded.
    @State private var opened: Set<String> = []
    @AppStorage("sidebarPinsOpen") private var pinsOpen = true
    @FocusState private var searching: Bool
    @Environment(\.reducesMotion) private var reduceMotion

    private var account: Account { Account.current }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !compact { HStack {
                Spacer()
                StartMeetingButton(start: actions.startMode, disabled: running)
            }
            .padding(.horizontal, 12)
            .frame(height: 52)
            }

            HStack(spacing: 7) {
                Image(systemName: "magnifyingglass").uiFont(12, weight: .medium).foregroundStyle(Theme.ink3)
                TextField("회의 검색", text: $selection.query)
                    .textFieldStyle(.plain)
                    .uiFont(13)
                    .focused($searching)
                    .onSubmit { if let q = PanelEntries.searchReturn(query) { ask(q) } }
                    // the results are the 회의 page (2026-11-16)
                    .onChange(of: selection.query) { if !query.isEmpty, selection.view != .meetings { selection.view = .meetings } }
                if selection.query.isEmpty && !searching {
                    Text("⌘K").uiFont(11.5).foregroundStyle(Theme.ink4)
                }
            }
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background(Theme.ink.opacity(0.045), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
            .padding(.top, compact ? 12 : 0)
            .background {
                Button("") { searching = true }.keyboardShortcut("k", modifiers: .command)
                    .frame(width: 0, height: 0).opacity(0).accessibilityHidden(true)
            }

            ScrollView {
                // not Lazy: a LazyVStack kept showing a row as live after its meeting ended (same id, new group)
                VStack(alignment: .leading, spacing: 2) {
                    if PanelEntries.showsSearchAsk, !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        SearchAskRow(query: query) { ask(query) }
                    }
                    views
                    // 2026-11-16: the meetings are a page (회의); the sidebar keeps the one going on, the last few opened,
                    // and the pinned ones. A search shows its results on the 회의 page.
                    if let live, let m = all.first(where: { $0.id == live }) {
                        group(String(localized: "지금"), full: true, [m])
                    } else if let stopped {
                        Text("지금").uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink3)
                            .padding(.horizontal, 10).padding(.top, 14).padding(.bottom, 4)
                        StoppedRow(reason: stopped, actions: actions)
                    }
                    recentlyOpened
                    pinned
                }
                .padding(.horizontal, 8)
                .padding(.bottom, 12)
            }
            .scrollIndicators(.never)
            // one highlight glides to the chosen meeting; rows a search hides fold shut so the rest slide up.
            // Rows stay mounted (see `all`): a removed row would free its space at once and the survivors would
            // slide over its fading text.
            .animation(Theme.anim(Theme.glide, reduce: reduceMotion), value: selection.id)
            .animation(Theme.anim(Theme.glide, reduce: reduceMotion), value: selection.view)
            .animation(Theme.anim(Theme.settle, reduce: reduceMotion), value: opened)
            .animation(Theme.anim(Theme.settle, reduce: reduceMotion), value: pinsOpen)
            .onAppear { openGroup(of: selection.id) }
            .onChange(of: selection.id) { openGroup(of: selection.id) }
            .animation(Theme.anim(Theme.settle, reduce: reduceMotion), value: rows.map(\.id))

            if let download, download.onlyVoice,
               let row = ModelDownload.voiceRow(state: download.state, done: download.voiceDone, total: download.voiceTotal) {
                ModelRow(row: row, retry: { download.start(force: true) })
            }
            if let stt {
                STTSidebarRow(stt: stt, open: { selection.settings = .privacy })
            }
            if let sync {
                Button { selection.settings = .connections } label: {
                    VStack(alignment: .leading, spacing: 5) {
                        HStack(spacing: 6) {
                            Text(firstSync ? String(localized: "처음 동기화 중") : String(localized: "동기화 중"))
                                .uiFont(12, weight: .semibold).foregroundStyle(Theme.ink)
                            Spacer(minLength: 4)
                            if let syncFraction {
                                Text(verbatim: "\(Int(syncFraction * 100))%").uiFont(11.5, weight: .medium, digits: true)
                                    .foregroundStyle(Theme.ink3)
                            }
                        }
                        SyncBar(fraction: syncFraction ?? 0)
                        if let syncRows {
                            VStack(alignment: .leading, spacing: 3) {
                                ForEach(syncRows, id: \.name) { SyncRowLine(row: $0) }
                            }
                        } else {
                            Text(sync).uiFont(11.5).foregroundStyle(Theme.ink2).lineLimit(1)
                        }
                        if let syncNote {
                            Text(syncNote).uiFont(11).foregroundStyle(Theme.ink3).lineLimit(2)
                        }
                    }
                    .padding(.horizontal, 14).padding(.vertical, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("설정 → 연결에서 자세히 봐요")
            }
            if !lost.isEmpty && sync == nil {
                Button { selection.settings = .connections } label: {
                    HStack(spacing: 8) {
                        Circle().fill(Theme.warn).frame(width: 6, height: 6)
                        Text(SettingsText.lostLine(lost)).uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink2).lineLimit(1)
                        Spacer()
                        Text("다시 연결").uiFont(12, weight: .semibold).foregroundStyle(Theme.ink)
                    }
                    .padding(.horizontal, 14).frame(height: 36).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("설정 → 연결에서 다시 연결해요")
            }
            switch AppUpdate.shared.state {
            case .ready(let version): UpdateRow(version: version, ready: true)
            case .available(let version): UpdateRow(version: version, ready: false)
            case .none: EmptyView()
            }
            if let fs = firstSteps, FirstSteps.visible(fs) || FirstSteps.showsFinish(fs) {
                FirstStepsFooterRow(state: fs, info: firstStepsInfo, actions: firstStepsActions,
                                    finished: { FirstStepsStore.shared.finishShown() })
            }
            if !leftovers.isEmpty {
                Button { showLeftovers.toggle() } label: {
                    HStack(spacing: 8) {
                        Circle().fill(Theme.warn).frame(width: 6, height: 6)
                        Text("남은 일 \(leftovers.count)개").uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink2)
                        Spacer()
                        Image(systemName: "chevron.up").uiFont(10, weight: .semibold).foregroundStyle(Theme.ink3)
                    }
                    .padding(.horizontal, 14).frame(height: 36).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .popover(isPresented: $showLeftovers, arrowEdge: .top) {
                    VStack(alignment: .leading, spacing: 12) {
                        Text(String(localized: "mainView.finishSetup", defaultValue: "남은 일")).uiFont(13, weight: .semibold).foregroundStyle(Theme.ink)
                        LeftoverList(items: leftovers, dismiss: dismiss) { item in
                            showLeftovers = false
                            fix(item)
                        }
                    }
                    .padding(16)
                    .frame(width: 340)
                    .appearanceScheme()
                }
            }
            if !compact && MainView.needsSignIn {
            Rectangle().fill(Theme.hairline).frame(height: 1)
            SignInCard(signIn: { selection.signIn = .card }, settings: { selection.settings = .account })
            } else if !compact {
            Rectangle().fill(Theme.hairline).frame(height: 1)
            HStack(spacing: 9) {
                Avatar(size: 26)
                VStack(alignment: .leading, spacing: 0) {
                    Text(account.name).uiFont(13, weight: .semibold).lineLimit(1)
                    if TacieAccount.offered {  // the open edition has no Tacie usage to show
                        Text(account.monthUsageLine).uiFont(11).foregroundStyle(Theme.ink3).lineLimit(1)
                    }
                }
                Spacer()
                Button { selection.settings = .account } label: { IconButton(symbol: "gearshape") }
                    .buttonStyle(.plain)
                    .help("설정 (⌘,)")
            }
            .padding(.horizontal, 14)
            .frame(height: 56)
            }
        }
        .frame(width: width)
        .confirmationDialog("이 회의를 지울까요?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
                            presenting: deleting) { m in
            Button("지우기", role: .destructive) { delete(m) }
            Button("취소", role: .cancel) {}
        } message: { m in
            Text(IPhoneMeetingUI.deleteMessage(for: m))
        }
    }

    /// As the page's 이 회의 지우기…: the meeting shown moves to its neighbour when it is the one going.
    private func delete(_ m: SavedMeeting) {
        Analytics.capture(.meetingDeleted)
        if selection.view == nil && m.id == (selection.id ?? rows.first?.id) {
            selection.id = MeetingGroups.selection(afterDeleting: m.id, in: rows.map(\.id))
        }
        remove(m.id)
    }

    private var visible: Set<String> { Set(rows.map(\.id)) }
    private var query: String { selection.query }

    /// The kept meetings by day, week, month and year (the one going on is 지금, above them).
    private var groups: [(title: String, kind: MeetingGroups.Kind, meetings: [SavedMeeting])] {
        MeetingGroups.group(all.filter { $0.id != live }, now: now, calendar: MainView.calendar)
    }

    /// Older groups fold behind their title; a search opens everything so every match shows.
    private func folds(_ kind: MeetingGroups.Kind) -> Bool { query.isEmpty && !MeetingGroups.open.contains(kind) }

    /// A meeting chosen from elsewhere (the island, a collected page) shows its row: its folded group opens.
    private func openGroup(of id: String?) {
        guard let id, let g = groups.first(where: { $0.meetings.contains { $0.id == id } }), folds(g.kind) else { return }
        opened.insert(g.title)
    }

    private func isSelected(_ m: SavedMeeting) -> Bool {
        selection.view == nil && m.id == (selection.id ?? rows.first?.id)
    }

    /// 할 일 · 결정 사항 · 문서로 남길 질문: pages that gather from every meeting.
    private var views: some View {
        VStack(spacing: 2) {
            ForEach(SideView.allCases, id: \.self) { v in
                Button {
                    if selection.view != v, SideView.collected.contains(v) {
                        let n = Collected.count(v, in: all) ?? Collected.decisions(all).reduce(0) { $0 + $1.indices.count }
                        Analytics.capture(.collectedOpened(view: ASideView(rawValue: v.rawValue) ?? .todos, count: n))
                    }
                    selection.view = v
                } label: {
                    SideViewRow(view: v, count: Collected.count(v, in: all), selected: selection.view == v, ns: ns)
                }
                .buttonStyle(PressStyle())
            }
        }
        .padding(.top, 2)
        .padding(.bottom, 4)
    }

    /// Meetings pinned from a row's menu or the page's •••, newest first; they stay in their day's group too.
    @ViewBuilder private var pinned: some View {
        let pins = all.filter { $0.pinned == true }.sorted { $0.start > $1.start }
        if !pins.isEmpty {
            fold(String(localized: "고정한 회의"), count: nil, open: pinsOpen) { pinsOpen.toggle() }
                .padding(.top, 8)
            if pinsOpen {
                ForEach(pins) { m in
                    ListedMeeting(full: false, open: { open(m) }) { hovering in
                        MeetingRow(meeting: m, live: m.id == live, now: now, selected: isSelected(m), ns: ns,
                                   full: false, glide: false, hovering: hovering)
                    } items: { menuItems(m) }
                }
            }
        }
    }

    /// A group title that opens and shuts what is under it, with how many meetings it holds.
    private func fold(_ title: String, count: Int?, open: Bool, toggle: @escaping () -> Void) -> some View {
        FoldHeader(title: title, count: count, open: open, toggle: toggle)
    }

    /// The row's ••• and its right-click menu: the same items. A meeting going on is ended, not deleted.
    @ViewBuilder private func menuItems(_ m: SavedMeeting) -> some View {
        Button(m.pinned == true ? String(localized: "고정 풀기") : String(localized: "목록 위에 고정")) {
            Analytics.capture(.meetingPinned(on: m.pinned != true))
            pin(m.id, m.pinned != true)
        }
        if m.id != live {
            Button("이 회의 지우기…", role: .destructive) { deleting = m }
        }
    }

    private func open(_ m: SavedMeeting) {
        if m.id != live { Analytics.capture(.meetingNotesOpened(from: .list, hasLedger: !m.ledger.isEmpty)) }
        selection.id = m.id
    }

    @ViewBuilder
    private func group(_ title: String, full: Bool, _ meetings: [SavedMeeting]) -> some View {
        Text(title).uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink3)
            .padding(.horizontal, 10).padding(.top, 14).padding(.bottom, 4)
        rowList(meetings, full: full)
    }

    /// Recent rows show the time and the summary's first line; older ones only the title and the date.
    private func rowList(_ meetings: [SavedMeeting], full: Bool) -> some View {
        ForEach(meetings) { m in
            ListedMeeting(full: full, open: { open(m) }) { hovering in
                MeetingRow(meeting: m, live: m.id == live, warning: m.id == live ? warning : nil, now: now,
                           selected: isSelected(m), ns: ns, full: full, hovering: hovering)
            } items: { menuItems(m) }
        }
    }

    /// The last three meetings opened (not the one going on): one line each.
    @ViewBuilder private var recentlyOpened: some View {
        let byID = Dictionary(all.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        let recent = selection.recent.compactMap { byID[$0] }.filter { $0.id != live }.prefix(3)
        if !recent.isEmpty {
            Text("최근에 연 회의").uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink3)
                .padding(.horizontal, 10).padding(.top, 14).padding(.bottom, 4)
            ForEach(Array(recent)) { m in
                ListedMeeting(full: false, open: { open(m) }) { hovering in
                    MeetingRow(meeting: m, live: false, now: now, selected: isSelected(m), ns: ns,
                               full: false, glide: false, hovering: hovering)
                } items: { menuItems(m) }
            }
        }
    }
}

/// A meeting in the list: the row opens it; under the pointer a ••• at its right end holds the right-click menu.
private struct ListedMeeting<Row: View, Items: View>: View {
    let full: Bool
    let open: () -> Void
    @ViewBuilder let row: (Bool) -> Row
    @ViewBuilder let items: () -> Items
    @State private var hovered = false

    var body: some View {
        Button(action: open) { row(hovered) }
            .buttonStyle(PressStyle())
            .overlay(alignment: full ? .topTrailing : .trailing) {
                Menu { items() } label: {
                    Image(systemName: "ellipsis").uiFont(12, weight: .semibold).foregroundStyle(Theme.ink2)
                        .frame(width: 24, height: 24)
                        .contentShape(Rectangle())
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .padding(.trailing, 4)
                .padding(.top, full ? 5 : 0)
                // kept in place while hidden: a menu that is open stays open when the pointer leaves the row
                .opacity(hovered ? 1 : 0)
                .allowsHitTesting(hovered)
                .help("회의 메뉴")
            }
            .onHover { inside in withAnimation(.easeOut(duration: inside ? 0.12 : 0.2)) { hovered = inside } }
            .contextMenu { items() }
    }
}

/// A row at the top of the list that opens a collected page: its symbol, its name, and a number when there is
/// something left to do.
private struct SideViewRow: View {
    let view: SideView
    let count: Int?
    let selected: Bool
    let ns: Namespace.ID
    @State private var hovered = false

    var body: some View {
        HStack(spacing: 9) {
            Image(systemName: view.symbol).uiFont(13, weight: .medium).foregroundStyle(Theme.ink2).frame(width: 16)
            Text(view.title).uiFont(13.5, weight: .medium).lineLimit(1)
            Spacer(minLength: 6)
            if let count, count > 0 {
                Text(verbatim: "\(count)").uiFont(12, weight: .medium, digits: true).foregroundStyle(Theme.ink3)
                    .contentTransition(.numericText())
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 30)
        .background { RowHighlight(selected: selected, hovered: hovered, ns: ns) }
        .contentShape(Rectangle())
        .onHover { inside in withAnimation(.easeOut(duration: inside ? 0.12 : 0.2)) { hovered = inside } }
    }
}

/// 고정한 회의 and the older months and years: a chevron that turns, the title, and how many meetings are inside.
private struct FoldHeader: View {
    let title: String
    let count: Int?
    let open: Bool
    let toggle: () -> Void
    @State private var hovered = false

    var body: some View {
        Button(action: toggle) {
            HStack(spacing: 6) {
                Image(systemName: "chevron.right").uiFont(9, weight: .bold).foregroundStyle(Theme.ink4)
                    .rotationEffect(.degrees(open ? 90 : 0))
                Text(title).uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink3)
                Spacer(minLength: 6)
                if let count {
                    Text(verbatim: "\(count)").uiFont(11.5, weight: .medium, digits: true).foregroundStyle(Theme.ink4)
                }
            }
            .padding(.horizontal, 10)
            .frame(height: 28)
            .background {
                RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.ink.opacity(hovered ? 0.035 : 0))
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { inside in withAnimation(.easeOut(duration: inside ? 0.12 : 0.2)) { hovered = inside } }
        .help(open ? String(localized: "접기") : String(localized: "펼치기"))
    }
}

private extension View {
    /// Zero height and invisible, but still in the layout, so neighbours slide instead of jumping (Arc inbox-triage).
    func folded(_ on: Bool) -> some View {
        self.frame(height: on ? 0 : nil, alignment: .top)
            .clipped()
            .opacity(on ? 0 : 1)
            .allowsHitTesting(!on)
    }
}

/// Under 지금 when the meeting did not start or stopped: why, as the island's 멈췄어요 card says it, and a way to try again.
/// It stays until the next start or 닫기 (which brings the island back to before a meeting too).
struct StoppedRow: View {
    let reason: String
    let actions: MainActions

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                LiveDot(warn: true)
                Text("회의가 멈췄어요").uiFont(13.5, weight: .semibold)
            }
            Text(reason).uiFont(12.5).foregroundStyle(Theme.ink2).lineSpacing(1.5)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 6) {
                Button(action: actions.start) { OutlineButton(label: String(localized: "mainView.startAgain", defaultValue: "다시 시작")) }.buttonStyle(PressStyle())
                Button(action: actions.dismissFailure) { OutlineButton(label: String(localized: "닫기"), tint: Theme.ink2) }.buttonStyle(PressStyle())
            }
            .padding(.top, 4)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.warn.opacity(0.06), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}

/// No meeting yet: what will pile up here, and how to start.
struct EmptyMain: View {
    let actions: MainActions
    var leftovers: [OnboardingLeftover] = []
    var fix: (OnboardingLeftover) -> Void = { _ in }
    var dismiss: (OnboardingLeftover) -> Void = { _ in }
    var firstSteps = FirstStepsState()
    var firstStepsInfo = FirstStepsInfo()
    var firstStepsActions = FirstStepsActions()
    @State private var copied = false

    var body: some View {
        VStack(spacing: 14) {
            Spacer()
            BrandMark(style: .l1(dark: false), height: 50)
                .softShadow()
                .padding(.bottom, 10)
            Text("회의를 시작하면 여기에 정리돼요").uiFont(24, weight: .semibold).retinaTracking(-0.4)
            Text("회의가 끝나면 요약, 결정 사항과 할 일, Tacie의 답을\n회의마다 모아 둬요.")
                .uiFont(14).foregroundStyle(Theme.ink2).multilineTextAlignment(.center).lineSpacing(3)
                .fixedSize(horizontal: false, vertical: true)  // the second line was cut to "…" (오픈소스판 e2e, 2026-11-25)
            HStack(spacing: 8) {
                StartMeetingButton(start: actions.startMode)
                Button(action: actions.ask) { OutlineButton(label: String(localized: "직접 물어보기")) }.buttonStyle(PressStyle())
            }
            .padding(.top, 10)
            HStack(spacing: 14) {
                HStack(spacing: 6) { Text("회의 시작·끝내기").uiFont(12); Keycap(text: "⌃⌥⌘M") }
                HStack(spacing: 6) { Text("직접 묻기").uiFont(12); Keycap(text: "⌃⌥⌘A") }
                HStack(spacing: 6) { Text("메인 창 열기").uiFont(12); Keycap(text: "⌃⌥⌘T") }
            }
            .foregroundStyle(Theme.ink3)
            .padding(.top, 6)
            if !leftovers.isEmpty {
                VStack(alignment: .leading, spacing: 12) {
                    Text("시작하기 전에 남은 일").uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink3)
                    LeftoverList(items: leftovers, dismiss: dismiss, fix: fix)
                }
                .padding(16)
                .frame(width: 560, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.ink.opacity(0.1)))
                .padding(.top, 22)
            }
            if FirstSteps.visible(firstSteps) {
                FirstStepsCard(state: firstSteps, info: firstStepsInfo, actions: firstStepsActions)
                    .padding(.top, leftovers.isEmpty ? 22 : 2)
            }
            Spacer()
            HStack(spacing: 4) {
                Text("회의 전에는 참석자에게 Tacie가 듣고 있다고 알려 주세요.").uiFont(12).foregroundStyle(Theme.ink3)
                Button {
                    actions.copyNotice()
                    copied = true
                } label: {
                    Text(copied ? String(localized: "복사했어요") : String(localized: "mainView.copyNotice", defaultValue: "알릴 문구 복사")).uiFont(12, weight: .semibold).foregroundStyle(Theme.ink2).underline()
                }
                .buttonStyle(.plain)
                .copiedFlash($copied)
            }
            .padding(.bottom, 30)
        }
        .frame(maxWidth: .infinity)
        .padding(.bottom, 20)
    }
}

/// One source of the running sync: done, running or waiting, with what it is at.
private struct SyncRowLine: View {
    let row: SyncRules.Row

    var body: some View {
        HStack(spacing: 6) {
            Group {
                switch row.state {
                case .done:
                    Image(systemName: "checkmark").uiFont(9, weight: .bold).foregroundStyle(Theme.live)
                case .running:
                    ProgressView().controlSize(.mini).scaleEffect(0.8)
                case .waiting:
                    Circle().strokeBorder(Theme.ink.opacity(0.25), lineWidth: 1.2).frame(width: 8, height: 8)
                }
            }
            .frame(width: 12, height: 12)
            Text(row.name).uiFont(11.5, weight: row.state == .running ? .semibold : .regular)
                .foregroundStyle(row.state == .waiting ? AnyShapeStyle(Theme.ink3) : AnyShapeStyle(Theme.ink2))
            if let detail = row.detail {
                Text(detail).uiFont(11.5, digits: true).foregroundStyle(Theme.ink3).lineLimit(1)
            }
        }
    }
}

/// A new version, downloaded and waiting: one line above 남은 일, like a finished sync (docs/15).
private struct UpdateRow: View {
    let version: String
    /// downloaded (다시 시작), or found with automatic download off (받기 opens Sparkle's window)
    let ready: Bool
    private var update: AppUpdate { AppUpdate.shared }

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "arrow.down.circle.fill").uiFont(15).foregroundStyle(Theme.ink)
            VStack(alignment: .leading, spacing: 1) {
                Text("새 버전 \(version)").uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink)
                if let notes = update.notesURL {
                    Link("바뀐 점 보기", destination: notes).uiFont(11).foregroundStyle(Theme.ink3)
                } else {
                    Text(ready ? String(localized: "다시 시작하면 바뀌어요") : String(localized: "받아서 바꿀 수 있어요")).uiFont(11).foregroundStyle(Theme.ink3)
                }
            }
            Spacer(minLength: 6)
            Button { update.restart(place: .main) } label: {
                Text(ready ? String(localized: "mainView.restart", defaultValue: "다시 시작") : String(localized: "받기")).uiFont(11.5, weight: .semibold).foregroundStyle(Theme.paper)
                    .padding(.horizontal, 10).frame(height: 24)
                    .background(Theme.ink, in: Capsule())
            }
            .buttonStyle(PressStyle())
            .help("Tacie를 다시 시작해서 \(version)으로 바꿔요. 회의 기록과 연결은 그대로예요.")
        }
        .padding(.horizontal, 10).padding(.vertical, 9)
        .background(Theme.fill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .padding(.horizontal, 8).padding(.bottom, 6)
    }
}
