import AppKit
import SwiftUI

/// The panel's words (docs/36 1절). Keys with a Korean default; the English is in Localizable.xcstrings.
extension PanelText {
    static var newChat: String { String(localized: "panel.newChat", defaultValue: "새 대화") }
    static var list: String { String(localized: "panel.list", defaultValue: "대화 목록") }
    static var close: String { String(localized: "panel.close", defaultValue: "패널 닫기") }
    static var emptyBody: String {
        String(localized: "panel.empty.body", defaultValue: "회의 기록과 연결한 서비스에서 찾아 근거와 함께 답해요.")
    }
    static var hint: String { String(localized: "panel.hint", defaultValue: "궁금한 걸 물어보세요") }
    static var hintMore: String { String(localized: "panel.hintMore", defaultValue: "이어서 물어보세요") }
    static var prepOpen: String { String(localized: "panel.prep.open", defaultValue: "아직 안 정한 것") }
    static var prepTodos: String { String(localized: "panel.prep.todos", defaultValue: "안 끝난 할 일") }
    static var prepNone: String { String(localized: "panel.prep.none", defaultValue: "아직 정하지 않은 것은 없어요.") }
    static func prepAlso(_ dates: String) -> String { String(localized: "panel.prep.also", defaultValue: "\(dates)에도 나왔어요") }
    static func prepMore(_ n: Int) -> String { String(localized: "나머지 항목 \(n)개 더") }
    static var whyLines: String { String(localized: "panel.why.lines", defaultValue: "대화 기록에서") }
    static var copy: String { String(localized: "panel.copy", defaultValue: "복사") }
    static var copied: String { String(localized: "복사했어요") }
    static var overdue: String { String(localized: "기한 지남") }
    static var allMeetings: String { String(localized: "panel.scope.allMeetings", defaultValue: "모든 회의") }
    static var noHistory: String { String(localized: "panel.history.empty", defaultValue: "아직 물어본 게 없어요.") }
    static var failed: String { String(localized: "panel.failed", defaultValue: "답을 가져오지 못했어요.") }
    static var deletedMeeting: String { String(localized: "panel.deletedMeeting", defaultValue: "지운 회의") }
    static var send: String { String(localized: "panel.send", defaultValue: "보내기") }

    /// The head's title: the conversation's, "새 대화" before anything is asked.
    static func headTitle(_ thread: PanelThread?) -> String { thread?.title ?? newChat }

    /// Where a conversation started: the meeting's title, "모든 회의", or "지운 회의" for one that is gone.
    static func place(_ scope: PanelScope, meeting: (String) -> SavedMeeting?, calendar: Calendar) -> String {
        switch scope {
        case .all: return allMeetings
        case .meeting(let id): return meeting(id)?.displayTitle(calendar: calendar) ?? deletedMeeting
        }
    }

    /// The empty screen's title (docs/36 0-1절).
    static func emptyTitle(_ scope: PanelScope, meeting: (String) -> SavedMeeting?, calendar: Calendar) -> String {
        switch scope {
        case .all: return String(localized: "panel.empty.all", defaultValue: "회의와 연결한 서비스에 물어보세요")
        case .meeting:
            let name = place(scope, meeting: meeting, calendar: calendar)
            return String(localized: "panel.empty.meeting", defaultValue: "\(name)에 대해 물어보세요")
        }
    }

    static func notFound(_ scope: String) -> String {
        String(localized: "panel.notFound", defaultValue: "\(scope)에서는 이야기가 나오지 않았어요")
    }

    /// after 10 s, after the step line
    static var slow: String { String(localized: "panel.slow", defaultValue: "조금 더 걸려요") }

    /// The engine's failure kinds (errors.rs `ServiceError`) and "engine" (no engine to ask); anything else gets the
    /// generic line.
    static let knownKinds: Set<String> = ["key", "credit", "network", "busy", "down", "model", "engine"]

    /// Why an answer did not come, from the engine's `error_kind`: the island's words for the kinds the engine knows
    /// (`say`, the app passes `MeetingModel.say`), a plain line for any other ("none": nothing to ask about).
    static func reason(_ kind: String, say: (String) -> String = FailureWords.say) -> String {
        knownKinds.contains(kind) ? say(kind) : failed
    }

}

/// One black button per screen (docs/29): while the panel is open the page's own black button turns to grey words,
/// and the panel's one strong control is its send button, once something is typed.
enum PanelInk {
    static func pageButtonStrong(panelOpen: Bool) -> Bool { !panelOpen }
    static func sendStrong(draft: String) -> Bool { !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
}

private struct PanelOpenKey: EnvironmentKey { static let defaultValue = false }

extension EnvironmentValues {
    /// The main window's panel is open: the page's black button steps back (PanelInk).
    var panelOpen: Bool {
        get { self[PanelOpenKey.self] }
        set { self[PanelOpenKey.self] = newValue }
    }
}

/// Where the panel sits beside the page: on its right, pushing it narrower, or over it when the page would get narrower
/// than `pageMin` (docs/36 1-4절). Over the page, a click outside closes it.
struct PanelLayout<Page: View, Side: View>: View {
    let open: Bool
    var close: () -> Void
    @ViewBuilder let page: Page
    @ViewBuilder let panel: Side
    @Environment(\.reducesMotion) private var reduceMotion

    static var width: CGFloat { 372 }
    /// A little narrower over the page, as in the approved look (panel v7), so some of the page still shows beside it.
    static var overlayWidth: CGFloat { 360 }
    /// 480 (docs/36 said 560; ruled 2026-11-14): the default 1200pt window with the 300pt list keeps the panel beside the page.
    static var pageMin: CGFloat { 480 }

    /// `width` is what the page and the panel share (the window less the sidebar).
    static func overlays(width: CGFloat) -> Bool { width - Self.width < pageMin }

    var body: some View {
        GeometryReader { g in
            let over = Self.overlays(width: g.size.width)
            HStack(spacing: 0) {
                page.frame(maxWidth: .infinity, maxHeight: .infinity)
                if open && !over {
                    panel
                        .frame(width: Self.width)
                        .background(Theme.paper)
                        .overlay(alignment: .leading) { Rectangle().fill(Theme.hairline).frame(width: 1) }
                        .transition(reduceMotion ? .opacity : .move(edge: .trailing).combined(with: .opacity))
                }
            }
            .overlay(alignment: .trailing) {
                if open && over {
                    ZStack(alignment: .trailing) {
                        Color.clear
                            .contentShape(Rectangle())
                            .onTapGesture(perform: close)
                        panel
                            .frame(width: Self.overlayWidth)
                            .frame(maxHeight: .infinity)
                            .background(Theme.paper)
                            .overlay(alignment: .leading) { Rectangle().fill(Theme.hairline).frame(width: 1) }
                            .shadow(color: Theme.modalShadow, radius: 20, x: -8)
                    }
                    .transition(reduceMotion ? .opacity : .move(edge: .trailing).combined(with: .opacity))
                }
            }
            .environment(\.panelOpen, open)
        }
        .animation(Theme.anim(Theme.settle, reduce: reduceMotion), value: open)
    }
}

/// What a press in the panel does in the window around it.
struct PanelActions {
    /// A meeting source: show the meeting, its ledger row or transcript lines lit when the target names them.
    var openMeeting: (PanelTarget) -> Void = { _ in }
    /// Any other source: its original (Linear, Slack, GitHub, …).
    var openCard: (Card) -> Void = { card in if let url = card.link { NSWorkspace.shared.open(url) } }
    var copy: (String) -> Void = { s in
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }
    /// Words for an engine failure kind (the app passes `MeetingModel.say`).
    var say: (String) -> String = FailureWords.say
}


/// The main window's panel (docs/36 0-1절, approved look: panel v9, layout B): the head with the conversation's title
/// and its list, the conversation (or the empty screen with example questions), and the input.
struct PanelView: View {
    @ObservedObject var store: PanelStore
    let meeting: (String) -> SavedMeeting?
    let actions: PanelActions
    let scrolls: Bool
    /// Renders only: how far the conversation is drawn scrolled up, since a render cannot scroll.
    let renderScroll: CGFloat
    @State private var showingList: Bool
    /// the conversation list's own height, for its card to fit it
    @State private var listHeight: CGFloat = 0
    @State private var draft = ""
    @FocusState private var typing: Bool
    @Environment(\.reducesMotion) private var reduceMotion

    /// `scrolls: false` and `showingList` are for renders: ImageRenderer leaves a scroll view empty and cannot draw
    /// the text field.
    init(store: PanelStore, meeting: @escaping (String) -> SavedMeeting?, actions: PanelActions = PanelActions(),
         scrolls: Bool = true, showingList: Bool = false, renderScroll: CGFloat = 0) {
        self.store = store
        self.meeting = meeting
        self.actions = actions
        self.scrolls = scrolls
        self.renderScroll = renderScroll
        _showingList = State(initialValue: showingList)
    }

    static let calendar: Calendar = {
        var c = Calendar.current
        c.firstWeekday = 2
        return c
    }()

    private var inConversation: Bool { !(store.current?.turns.isEmpty ?? true) }

    /// On macOS 26 the head and the input float over the conversation, as the meeting page's row does (and the
    /// iPhone's bars): no lines and no paper of their own; the conversation runs on under both and fades out toward
    /// the panel's top and bottom. Renders, and older macOS, keep them stacked with a line between.
    private var floats: Bool {
        if #available(macOS 26.0, *) { return scrolls }
        return false
    }

    var body: some View {
        Group {
            if #available(macOS 26.0, *), floats {
                middle
                    .mask {
                        VStack(spacing: 0) {
                            LinearGradient(stops: [.init(color: .clear, location: 0), .init(color: .clear, location: 0.3),
                                                   .init(color: .black, location: 1)],
                                           startPoint: .top, endPoint: .bottom)
                                .frame(height: Self.headHeight)
                            Rectangle()
                            LinearGradient(stops: [.init(color: .black, location: 0), .init(color: .clear, location: 0.75),
                                                   .init(color: .clear, location: 1)],
                                           startPoint: .top, endPoint: .bottom)
                                .frame(height: Self.footHeight)
                        }
                        .ignoresSafeArea()
                    }
                    .safeAreaBar(edge: .top, spacing: 0) { header }
                    .safeAreaBar(edge: .bottom, spacing: 0) { footer }
                    .scrollEdgeEffectStyle(.soft, for: .all)
            } else {
                VStack(spacing: 0) {
                    // in front, as in TitleRowPage: on macOS 26 the scroll view's top edge reaches up under this row
                    // and, as the later sibling, took the clicks on its buttons (E2E 2026-11-14)
                    header.zIndex(1)
                    middle.clipped()  // a scrolled answer stops at the head row's line, never draws over it
                    footer
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.paper)
        .background {
            // Esc closes the list (only while it is open, so Esc keeps its other meanings otherwise)
            if showingList {
                Button("") { showingList = false }.keyboardShortcut(.cancelAction)
                    .opacity(0).frame(width: 0, height: 0).accessibilityHidden(true)
            }
        }
        .onChange(of: store.isOpen) { if store.isOpen { showingList = false; typing = true } }
    }

    private static let bottomID = "panel bottom"
    /// the head row's height, and the input's row (the pill and the room around it)
    static let headHeight: CGFloat = 52
    static let footHeight: CGFloat = 68

    private var middle: some View {
        Group {
            if inConversation {
                scrolled
            } else if scrolls {
                // a short panel scrolls the empty screen rather than clipping its title. Not a ViewThatFits: its
                // picked view stayed put while the panel slid in, instead of coming in with it
                GeometryReader { g in
                    ScrollView { empty.frame(minHeight: g.size.height) }
                        .scrollBounceBehavior(.basedOnSize)
                }
            } else {
                empty
            }
        }
        .frame(minHeight: 0, maxHeight: .infinity)
        .overlay(alignment: .top) { listDrop }
    }

    /// The conversations, from the title's ▾: a small card that comes up just under the head over the conversation, as
    /// Notion AI's and Cursor's chat panels do. It grows a little and fades in from its top (transform and opacity
    /// only, nothing laid out again on the way), fits its list up to most of the panel's height and scrolls past that.
    /// A click outside or Esc closes it.
    private var listDrop: some View {
        GeometryReader { g in
            let maxHeight = max(g.size.height * 0.7, 160)
            ZStack(alignment: .top) {
                // measured all the time, open or not, at the card's width: a card that only learned its list's height
                // once open came up empty and then jumped, its scroll bar flashing as it grew
                listContent(open: false)
                    .frame(width: max(g.size.width - 20, 0))
                    .fixedSize(horizontal: false, vertical: true)
                    .background(GeometryReader { p in Color.clear.preference(key: ListHeightKey.self, value: p.size.height) })
                    .hidden()
                    .accessibilityHidden(true)
                if showingList {
                    Color.clear.contentShape(Rectangle()).onTapGesture { showingList = false }
                    listCard(maxHeight: maxHeight)
                        .transition(reduceMotion ? .opacity
                                    : .scale(scale: 0.96, anchor: .top).combined(with: .opacity))
                }
            }
            .frame(width: g.size.width, height: g.size.height, alignment: .top)
        }
        .onPreferenceChange(ListHeightKey.self) { listHeight = $0 }
        .animation(Theme.anim(PanelMotion.listCard, reduce: reduceMotion), value: showingList)
    }

    private func listContent(open: Bool) -> some View {
        PanelConversationList(history: store.history, currentID: store.current?.id, meeting: meeting,
                              calendar: Self.calendar) { id in
            store.reopen(id)
            showingList = false
        }
        .padding(.horizontal, 12).padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .allowsHitTesting(open)
    }

    private func listCard(maxHeight: CGFloat) -> some View {
        let fits = listHeight <= maxHeight
        return Group {
            if scrolls {
                // a scroll bar only when the list runs past the card: a list that fits never shows or flashes one
                ScrollView { listContent(open: true) }
                    .scrollIndicators(fits ? .never : .automatic)
                    .scrollBounceBehavior(.basedOnSize)
                    .scrollDisabled(fits)
                    .frame(height: min(listHeight, maxHeight))
            } else {
                listContent(open: true)
            }
        }
        // the list is clipped to the card, the card's shadow is not
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous))
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.raised)
            // white on the white panel: the shadow is what shows its edge (no outline, as on the iPhone)
            .shadow(color: .black.opacity(0.12), radius: 18, y: 8)
            .shadow(color: .black.opacity(0.08), radius: 1.5, y: 0.5))
        .padding(.horizontal, 10).padding(.top, 2)
    }

    // MARK: Head

    private var header: some View {
        HStack(spacing: 9) {
            PanelHeadFace(turn: lastTurn, doneAt: lastDoneAt)
            Button { showingList.toggle() } label: {
                VStack(alignment: .leading, spacing: 1) {
                    // the row hugs the title, so a cut title keeps the ▾ right after its "…". Not a ViewThatFits:
                    // its picked line was laid out apart from the panel and showed ahead of it as the panel slid in
                    titleLine(Text(title).lineLimit(1).truncationMode(.tail))
                    Text(PanelText.place(store.current?.origin ?? store.scope, meeting: meeting, calendar: Self.calendar))
                        .uiFont(11.5).foregroundStyle(Theme.ink3).lineLimit(1)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(PanelText.list)
            .accessibilityLabel(title)
            .accessibilityHint(PanelText.list)
            Spacer(minLength: 8)
            HeadButton(label: PanelText.newChat) {
                showingList = false
                store.newThread()
                typing = true
            }
            Button { store.isOpen = false } label: {
                Image(systemName: "xmark").uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink3)
                    .frame(width: 24, height: 24).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(PanelText.close)
            .accessibilityLabel(PanelText.close)
        }
        .padding(.horizontal, 16)
        .frame(height: Self.headHeight)
        .frame(maxWidth: .infinity)
        .background(floats ? Color.clear : Theme.paper)
        .overlay(alignment: .bottom) { if !floats { Rectangle().fill(Theme.hairline).frame(height: 1) } }
        .contentShape(Rectangle())
        .animation(Theme.fast, value: showingList)
    }

    private var title: String { PanelText.headTitle(store.current) }

    /// The conversation's last turn, for the head face to show Tacie at work until its done line.
    private var lastTurn: PanelTurn? { store.current?.turns.last }

    private var lastDoneAt: Double {
        guard let t = lastTurn, let body = t.body, let thread = store.current else { return 0 }
        let prepared: String? = if case .meeting(let id) = thread.origin { id } else { nil }
        let c = PanelShown.revealCounts(body, preparedID: prepared, say: actions.say)
        return PanelReveal.of(t, at: .distantPast, reduce: reduceMotion).doneAt(chips: c.chips, chunks: c.chunks)
    }

    private func titleLine(_ text: some View) -> some View {
        HStack(spacing: 4) {
            text.uiFont(13.5, weight: .bold).foregroundStyle(Theme.ink)
            Image(systemName: "chevron.down").uiFont(9, weight: .bold).foregroundStyle(Theme.ink3)
                .rotationEffect(.degrees(showingList ? 180 : 0))
                .fixedSize()
        }
    }

    // MARK: Body

    @ViewBuilder private var scrolled: some View {
        if scrolls {
            ScrollViewReader { proxy in
                ScrollView { content.padding(.horizontal, 16).padding(.top, 16).padding(.bottom, 8) }
                    .onChange(of: store.current) {
                        withAnimation(Theme.fast) { proxy.scrollTo(Self.bottomID, anchor: .bottom) }
                    }
            }
        } else {
            content.padding(.horizontal, 16).padding(.top, 16).padding(.bottom, 8)
                .offset(y: -renderScroll)
                .frame(minHeight: 0, maxHeight: .infinity, alignment: .top)
                .clipped()
        }
    }

    @ViewBuilder private var content: some View {
        VStack(alignment: .leading, spacing: 13) {
            if let thread = store.current {
                ForEach(thread.turns) { turn in
                    PanelTurnView(turn: turn, scope: thread.origin, meeting: meeting, actions: actions, calendar: Self.calendar, store: store)
                }
            }
            Color.clear.frame(height: 1).id(Self.bottomID)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Nothing asked yet: what this conversation searches, and three questions to start from (mockup v9 `.empty`).
    private var empty: some View {
        VStack(alignment: .leading, spacing: 6) {
            Spacer(minLength: 0)
            Text(PanelText.emptyTitle(store.scope, meeting: meeting, calendar: Self.calendar))
                .uiFont(15, weight: .bold).foregroundStyle(Theme.ink)
                .fixedSize(horizontal: false, vertical: true)
            Text(PanelText.emptyBody)
                .uiFont(13).foregroundStyle(Theme.ink2).lineSpacing(3)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.bottom, 8)
            ForEach(Array(store.suggestions(for: store.scope).enumerated()), id: \.offset) { _, s in
                ExampleChip(text: store.text(of: s)) {
                    store.suggest(s)
                    typing = true
                }
            }
        }
        .padding(.horizontal, 18).padding(.bottom, 14)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
    }

    // MARK: Foot

    private var footer: some View {
        input
            .padding(.horizontal, 14).padding(.top, 10).padding(.bottom, 14)
            .overlay(alignment: .top) { if !floats { Rectangle().fill(Theme.hairline).frame(height: 1) } }
    }

    private var input: some View {
        HStack(spacing: 8) {
            ZStack(alignment: .leading) {
                if draft.isEmpty {
                    // drawn by hand: the field's own placeholder is AppKit and does not draw in renders
                    Text(inConversation ? PanelText.hintMore : PanelText.hint)
                        .uiFont(13.5).foregroundStyle(Theme.ink3).lineLimit(1)
                        .allowsHitTesting(false)
                }
                // a render draws only the hint: ImageRenderer cannot draw the AppKit field
                if scrolls {
                    TextField("", text: $draft)
                        .textFieldStyle(.plain)
                        .uiFont(13.5)
                        .focused($typing)
                        .onSubmit(send)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            let strong = PanelInk.sendStrong(draft: draft)
            Button(action: send) {
                Image(systemName: "arrow.up").uiFont(11.5, weight: .bold).foregroundStyle(Theme.onStrong)
                    .frame(width: 24, height: 24)
                    .background(Circle().fill(strong ? Theme.ink : Theme.ink4))
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .disabled(!strong)
            .help(PanelText.send)
            .accessibilityLabel(PanelText.send)
        }
        .padding(.leading, 16).padding(.trailing, 10)
        .frame(minHeight: 44)
        // the iPhone's field (`raised`): a white pill lifted by a soft shadow, no outline
        .background(Capsule().fill(Theme.raised)
            .shadow(color: .black.opacity(0.10), radius: 12, y: 4)
            .shadow(color: .black.opacity(0.05), radius: 2, y: 1))
        .contentShape(Capsule())
        .onTapGesture { typing = true }
    }

    private func send() {
        let t = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return }
        showingList = false
        store.ask(t)
        draft = ""
    }
}

/// Tacie's face in the panel: the same lit head as the logo (`LitHead`, sign-in and the new-features sheet), so the
/// panel reads as Tacie. Its lights glance toward the question and grow by a step while working; at rest they breathe
/// as the island's do with no meeting (`EyeRig` asleep). No blink: the lights are glows, and shrinking them read wrong.
struct PanelFace: View {
    enum Size {
        case head, mini
        /// the head's height in points (its width is `BrandGeometry.aspect` times this)
        var height: CGFloat {
            switch self {
            case .head: 22
            case .mini: 13
            }
        }
    }

    let size: Size
    /// glancing toward the question (docs/36 0-2절 3)
    var look = false
    /// glow steps above rest: one while working, two after 10 s
    var glow = 0
    /// breathing slowly, at rest only
    var breathes = false
    @Environment(\.reducesMotion) private var reduce
    /// `.key` only while the window is key and the app is active; otherwise the breath holds still (nobody watches it)
    @Environment(\.controlActiveState) private var active

    var body: some View {
        Group {
            if breathes && !reduce {
                TimelineView(.animation(minimumInterval: PanelMotion.breathFrame, paused: active != .key)) { ctx in
                    head(breath: PanelMotion.breath(at: ctx.date.timeIntervalSinceReferenceDate))
                }
            } else {
                head(breath: .still)
            }
        }
        .accessibilityHidden(true)
    }

    private func head(breath: PanelMotion.Breath) -> some View {
        PanelLitHead(height: size.height, look: look ? 1 : 0, glow: CGFloat(glow), breath: breath)
            .animation(PanelMotion.look, value: look)
            .animation(PanelMotion.glowAnimation(reduce: reduce), value: glow)
    }

    /// The lights for a glance (0...1), a glow step (0...2) and the breath at rest.
    static func channels(look: Double, glow: Double, breath: PanelMotion.Breath = .still) -> EyeChannels {
        func side(_ size: Double, _ light: Double) -> EyeSide {
            EyeSide(size: (1 + 0.14 * glow) * (1 + size), glow: min(1, 0.92 + light), x: -0.12 * look, y: 0.05 * look)
        }
        return EyeChannels(left: side(breath.size, breath.glow), right: side(-breath.size, -breath.glow + 2 * breath.both))
    }
}

/// `LitHead` with the glance and the glow animatable between their values.
private struct PanelLitHead: View, Animatable {
    let height: CGFloat
    var look: CGFloat
    var glow: CGFloat
    let breath: PanelMotion.Breath

    var animatableData: AnimatablePair<CGFloat, CGFloat> {
        get { AnimatablePair(look, glow) }
        set { look = newValue.first; glow = newValue.second }
    }

    var body: some View {
        LitHead(channels: PanelFace.channels(look: Double(look), glow: Double(glow), breath: breath), height: height)
    }
}

/// "+ 새 대화": grey words in the head row, a tint under the pointer.
private struct HeadButton: View {
    let label: String
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 3) {
                Image(systemName: "plus").uiFont(10, weight: .semibold)
                Text(label).uiFont(12)
            }
            .foregroundStyle(hover ? Theme.ink : Theme.ink2)
            .padding(.horizontal, 8).frame(height: 22)
            .background(Capsule().fill(hover ? Theme.fillHover : .clear))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }
}

/// An example question on the empty screen: a white chip with a hairline, one under another.
private struct ExampleChip: View {
    let text: String
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            Text(text).uiFont(12.5).foregroundStyle(Theme.ink).lineLimit(1)
                .padding(.horizontal, 11).frame(height: 28)
                .background(Capsule().fill(hover ? Theme.fill : Theme.paper))
                .overlay(Capsule().strokeBorder(Theme.hairline))
                .contentShape(Capsule())
        }
        .buttonStyle(PressStyle())
        .onHover { hover = $0 }
    }
}

/// The conversation list under the head's ▾: by day, each row the conversation's title and where it started with the
/// time; the one on screen tinted.
struct PanelConversationList: View {
    let history: [PanelThread]
    let currentID: UUID?
    let meeting: (String) -> SavedMeeting?
    let calendar: Calendar
    let open: (UUID) -> Void

    /// Conversations under 오늘, 어제 or their date, in the order kept (newest first); one without a turn is left out.
    static func days(_ history: [PanelThread], calendar: Calendar, now: Date = Date()) -> [(title: String, threads: [PanelThread])] {
        var out: [(title: String, threads: [PanelThread])] = []
        for t in history {
            guard let first = t.turns.first else { continue }
            let title = day(first.at, calendar: calendar, now: now)
            if out.last?.title == title { out[out.count - 1].threads.append(t) } else { out.append((title, [t])) }
        }
        return out
    }

    static func day(_ d: Date, calendar: Calendar, now: Date = Date()) -> String {
        if calendar.isDate(d, inSameDayAs: now) { return String(localized: "오늘") }
        if let y = calendar.date(byAdding: .day, value: -1, to: now), calendar.isDate(d, inSameDayAs: y) { return String(localized: "어제") }
        return SavedMeeting.format("M월 d일 EEEE", d, calendar)
    }

    var body: some View {
        let days = Self.days(history, calendar: calendar)
        if days.isEmpty {
            Text(PanelText.noHistory).uiFont(13).foregroundStyle(Theme.ink3)
                .frame(maxWidth: .infinity).padding(.vertical, 18)
        } else {
            VStack(alignment: .leading, spacing: 2) {
                ForEach(Array(days.enumerated()), id: \.offset) { i, day in
                    Text(day.title).uiFont(11, weight: .semibold).foregroundStyle(Theme.ink3)
                        .padding(.horizontal, 10).padding(.top, i == 0 ? 0 : 10).padding(.bottom, 4)
                    ForEach(day.threads) { t in
                        ConversationRow(title: t.title, sub: sub(t), on: t.id == currentID) { open(t.id) }
                    }
                }
            }
            .padding(.horizontal, -6)
        }
    }

    private func sub(_ t: PanelThread) -> String {
        let place = PanelText.place(t.origin, meeting: meeting, calendar: calendar)
        guard let at = t.turns.first?.at else { return place }
        return "\(place) · \(SavedMeeting.format("a h:mm", at, calendar))"
    }
}

private struct ConversationRow: View {
    let title: String
    let sub: String
    let on: Bool
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 1) {
                Text(title).uiFont(13).foregroundStyle(Theme.ink).lineLimit(1)
                Text(sub).uiFont(11.5).foregroundStyle(Theme.ink3).lineLimit(1)
            }
            .padding(.horizontal, 10).padding(.vertical, 7)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 7, style: .continuous).fill(on || hover ? Theme.fill : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }
}

private struct ListHeightKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = max(value, nextValue()) }
}
