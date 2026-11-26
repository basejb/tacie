import AppKit
import SwiftUI

/// Tacie has a Dock icon while one of its ordinary windows (the main window, the onboarding) is open.
enum AppWindows {
    /// Tacie starts as an agent app (no Dock icon), and macOS does not always make such an app active when it opens
    /// one of its windows: the window then opened behind the app in front (seen 2026-10-27 with the onboarding behind
    /// a terminal). Order it to the front regardless, ask to be active, and again once the policy change has landed.
    @MainActor static func bringToFront(_ w: NSWindow) {
        NSApp.activate(ignoringOtherApps: true)
        w.makeKeyAndOrderFront(nil)
        w.orderFrontRegardless()
        DispatchQueue.main.async {
            NSApp.activate(ignoringOtherApps: true)
            w.makeKeyAndOrderFront(nil)
        }
    }

    @MainActor static func dropDockIcon(closing: NSWindow?) {
        let others = NSApp.windows.contains { $0 !== closing && $0.isVisible && $0.styleMask.contains(.titled) && !($0 is NSPanel) }
        if !others { NSApp.setActivationPolicy(.accessory) }
    }
}

/// What the main window can ask the app to do.
struct MainActions {
    let start: () -> Void
    /// 회의 시작 after picking 대면 or 원격 in its popover.
    let startMode: (MeetingMode) -> Void
    let stop: () -> Void
    let ask: () -> Void
    let copyNotice: () -> Void
    let rewriteNotes: (String) -> Void
    /// 이어서 듣기 on an ended meeting's page (docs/35 5절).
    let resume: (String) -> Void
    /// The meeting page's kind button: the notes again as another kind (docs/23).
    let changeKind: (String, NotesKind) -> Void
    let dismissFailure: () -> Void
    /// A 남은 일 row's button (connecting a service opens settings in the window itself).
    let fixLeftover: (OnboardingLeftover) -> Void
    /// The sign-in sheet closed: signed in (a meeting or a question that waited for it goes on) or not (it is dropped).
    let afterSignIn: (Bool) -> Void
}

/// Which meeting the main window shows, and what the list is filtered by.
@Observable
@MainActor
final class MainSelection {
    /// The chosen meeting. Choosing one (even the same again) leaves a collected page for that meeting.
    var id: String? {
        didSet {
            view = nil
            if let id { remember(id) }
        }
    }
    /// The page shown in place of a meeting: 홈 (where the window opens), 회의, or one that collects across meetings
    /// (할 일, 결정 사항, 문서로 남길 질문).
    var view: SideView? = .home
    /// The meetings opened last, newest first: the sidebar's 최근에 연 회의.
    var recent: [String] = UserDefaults.standard.stringArray(forKey: MainSelection.recentKey) ?? []

    static let recentKey = "recentMeetings"

    private func remember(_ id: String) {
        guard recent.first != id else { return }
        recent = Array(([id] + recent.filter { $0 != id }).prefix(10))
        UserDefaults.standard.set(recent, forKey: Self.recentKey)
    }
    /// A question on the chosen meeting's page to scroll to; the page clears it once it has scrolled.
    var question: String?
    var query = ""
    /// The settings page shown over the window, if any.
    var settings: SettingsPane?
    /// A settings row to open as a search result would (the debug command `settings-row <title>`, for captures).
    var settingsRow: String?
    /// A source pressed in the main window's panel: the move it makes and what the meeting page lights (docs/36 1-2절).
    let jump = PanelJump()
    /// The sign-in sheet over the window, and why it is up.
    var signIn: SignInAsk?
}

/// The main window: meetings on the left, the chosen one's notes on the right. Tacie lives in the menu bar;
/// while this window is open it also has a Dock icon and a place in ⌘Tab.
@MainActor
final class MainWindowController: NSObject, NSWindowDelegate {
    private let library: MeetingLibrary
    private let meeting: MeetingModel
    private let onboarding: OnboardingModel
    private let actions: MainActions
    private let settings: SettingsContext
    private let panel: PanelStore
    let selection = MainSelection()
    /// Plays meeting recordings from the meeting page; one for the window, stopped when it closes.
    private let player = RecordingPlayer()
    private var window: NSWindow?

    /// Where the island is now, so the window opens and resizes clear of it (IslandClearance).
    var islandFrame: () -> NSRect? = { nil }

    init(library: MeetingLibrary, meeting: MeetingModel, onboarding: OnboardingModel, actions: MainActions, settings: SettingsContext,
         panel: PanelStore) {
        self.panel = panel
        self.onboarding = onboarding
        self.library = library
        self.meeting = meeting
        self.actions = actions
        self.settings = settings
    }

    var windowNumber: Int? { window?.windowNumber }
    var isVisible: Bool { window?.isVisible ?? false }
    /// The window is the one the person is looking at (key, Tacie in front).
    var isKey: Bool { window?.isKeyWindow ?? false }

    /// `focus: false` brings it up without taking the keyboard (the end of a meeting: the call goes on).
    /// `question` scrolls that meeting's page to it.
    func show(meeting id: String? = nil, question: String? = nil, focus: Bool) {
        if let id {
            selection.id = id
            selection.question = question
            selection.settings = nil  // the meeting that just ended must not sit under the modal
        }
        let first = window == nil
        let w = window ?? makeWindow()
        if first { w.center() }
        if first { clearIsland(w) }  // also a frame restored from last time that sits under the island
        window = w
        NSApp.setActivationPolicy(.regular)
        if focus {
            AppWindows.bringToFront(w)
        } else {
            w.orderFrontRegardless()
        }
        if first { w.makeFirstResponder(nil) }  // AppKit would put the caret in the search field
    }

    func close() { window?.close() }

    /// Brings the window up and puts the settings modal over it, on `pane`.
    func openSettings(_ pane: SettingsPane = .account) {
        show(focus: true)
        selection.settings = pane
        window?.makeFirstResponder(nil)  // Tab must not reach the list under the modal
    }

    /// Debug: how long checking a to-do takes, from the change to the window drawn again, `n` times on the shown page
    /// (`ledger-bench <meeting id> [n]`, perf checks). Logs the median and the worst in ms.
    func debugLedgerBench(_ rest: String) {
        let parts = rest.split(separator: " ").map(String.init)
        guard let id = parts.first, let m = library.meetings.first(where: { $0.id == id }),
              let i = m.ledger.firstIndex(where: { $0.kind == "commitment" }) else { Log.info("bench: no to-do"); return }
        let n = parts.count > 1 ? Int(parts[1]) ?? 10 : 10
        // App Nap would hold the timers of a window in the back and add hundreds of ms that are not the app's
        let activity = ProcessInfo.processInfo.beginActivity(options: [.userInitiated, .latencyCritical], reason: "ledger bench")
        Task { @MainActor in
            defer { ProcessInfo.processInfo.endActivity(activity) }
            var total: [Double] = [], change: [Double] = [], cpu: [Double] = []
            func threadCPU() -> Double { Double(clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID)) / 1e6 }
            for _ in 0..<n {
                let c0 = threadCPU()
                let t0 = CACurrentMediaTime()
                library.update(m.id) { $0.ledger[i].done.toggle() }
                change.append((CACurrentMediaTime() - t0) * 1000)
                try? await Task.sleep(for: .milliseconds(1))
                total.append((CACurrentMediaTime() - t0) * 1000 - 1)
                cpu.append(threadCPU() - c0)
                try? await Task.sleep(for: .milliseconds(150))
            }
            func line(_ v: [Double]) -> String {
                let s = v.sorted()
                return String(format: "median %.1f max %.1f", s[s.count / 2], s.last ?? 0)
            }
            Log.info("bench: change \(line(change)) | change+draw \(line(total)) | cpu \(line(cpu)) | all \(total.map { String(format: "%.0f", $0) }.joined(separator: " "))")
        }
    }

    /// Debug: a click at (x, y) in the window, top-left origin in points, sent through the window like a real one.
    /// Logs the view that the click lands on first, so a dead spot shows which view took it.
    func debugClick(x: CGFloat, y: CGFloat) {
        guard let w = window, let frame = w.contentView?.superview else { return }
        let p = NSPoint(x: x, y: frame.bounds.height - y)
        let hit = frame.hitTest(p)
        let landed = hit.map { v -> String in
            let r = v.convert(v.bounds, to: nil)
            return "\(type(of: v)) at y \(Int(frame.bounds.height - r.maxY))–\(Int(frame.bounds.height - r.minY))"
        }
        Log.info("click \(Int(x)),\(Int(y)) lands on \(landed ?? "nothing")")
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            if let e = NSEvent.mouseEvent(with: type, location: p, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                          windowNumber: w.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1) {
                w.sendEvent(e)
            }
        }
    }

    /// Debug captures: scrolls the page (the biggest scroll view right of the sidebar, or anywhere in a compact window) by `dy` points, down when positive;
    /// `top` goes back to the start. No wheel events, so the pointer stays where it is and the window may sit behind others.
    func debugScroll(_ input: String) {
        // `<points>|top [minX]`: a minX past the page's left edge reaches the settings sheet's scroll view (captures)
        let parts = input.split(separator: " ").map(String.init)
        let arg = parts.first ?? ""
        let minX = parts.count > 1 ? Double(parts[1]) ?? 200 : 200
        guard let root = window?.contentView else { return }
        var found: [NSScrollView] = []
        func walk(_ v: NSView) {
            if let s = v as? NSScrollView, !s.isHidden, s.documentView != nil { found.append(s) }
            v.subviews.forEach(walk)
        }
        walk(root)
        let area = { (v: NSScrollView) in v.bounds.width * v.bounds.height }
        // right of the sidebar; a compact window has none, so then the biggest anywhere
        let right = found.filter { $0.convert($0.bounds, to: nil).minX > minX }
        let page = (right.isEmpty ? found : right).max { area($0) < area($1) }
        guard let page, let doc = page.documentView else { Log.info("scroll: no page scroll view"); return }
        let clip = page.contentView
        let maxY = max(0, doc.frame.height - clip.bounds.height)
        let now = doc.isFlipped ? clip.bounds.origin.y : maxY - clip.bounds.origin.y  // distance from the top
        let to = arg == "top" ? 0 : min(maxY, max(0, now + (Double(arg) ?? 0)))
        clip.scroll(to: NSPoint(x: clip.bounds.origin.x, y: doc.isFlipped ? to : maxY - to))
        page.reflectScrolledClipView(clip)
        Log.info("scroll \(arg): \(Int(now)) -> \(Int(to)) of \(Int(maxY))")
    }

    /// Debug captures: the window centred across the screen, its top `gap` points below the island's bottom, so the
    /// island's answer card drops into the space between them instead of over the window.
    func debugBelowIsland(_ gap: CGFloat) {
        guard let w = window, let island = islandFrame(), let screen = (w.screen ?? NSScreen.main)?.frame else { return }
        w.setFrameOrigin(NSPoint(x: (screen.midX - w.frame.width / 2).rounded(), y: island.minY - gap - w.frame.height))
    }

    /// Debug captures: the window's top-left at (x, y) in points from the main screen's top-left, optionally its content size.
    func debugPlace(x: CGFloat, y: CGFloat, width: CGFloat?, height: CGFloat?) {
        guard let w = window, let primary = NSScreen.screens.first else { return }
        if let width, let height { w.setContentSize(NSSize(width: width, height: height)) }
        w.setFrameTopLeftPoint(NSPoint(x: x, y: primary.frame.maxY - y))
    }

    /// Debug captures: the window's content size.
    func resize(width: CGFloat, height: CGFloat) {
        guard let w = window else { return }
        w.setContentSize(NSSize(width: width, height: height))
        clearIsland(w)
    }

    /// Moves the window down just below the island when its top would sit under it (the pill at the top middle covered
    /// the window's title row and the 회의 시작 button, 2026-11-05 사용자).
    private func clearIsland(_ w: NSWindow) {
        w.setFrameOrigin(IslandClearance.origin(clearOf: islandFrame(), window: w.frame, screen: (w.screen ?? NSScreen.main)?.visibleFrame))
    }

    func windowWillClose(_ notification: Notification) {
        selection.settings = nil
        player.stop()
        AppWindows.dropDockIcon(closing: notification.object as? NSWindow)
    }

    private func makeWindow() -> NSWindow {
        let root = MainView(library: library, meeting: meeting, onboarding: onboarding, selection: selection, actions: actions,
                            settings: settings, panel: panel)
            .environment(\.forcedReduceMotion, DebugFlags.reduceMotion)
            .environment(player)
        let host = NSHostingController(rootView: root)
        host.sizingOptions = []
        let w = NSWindow(contentViewController: host)
        MainWindowShape.apply(w)
        w.title = "Tacie"
        w.isReleasedWhenClosed = false
        w.appearance = AppearanceMode.shared.nsAppearance
        w.backgroundColor = NSColor(Theme.paper)
        w.sharingType = Capture.sharing
        w.setContentSize(NSSize(width: 1200, height: 780))
        w.minSize = NSSize(width: 560, height: 480)
        w.setFrameAutosaveName("TacieMain")
        w.delegate = self
        return w
    }
}
