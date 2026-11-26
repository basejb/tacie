import AppKit
import SwiftUI

/// The onboarding window (540 × 620). Like the main window it gives Tacie a Dock icon while it is open.
@MainActor
final class OnboardingWindowController: NSObject, NSWindowDelegate {
    static let title = String(localized: "Tacie 시작하기")
    let model: OnboardingModel
    private let settings: SettingsContext
    private let openMain: () -> Void
    private let showIsland: () -> Void
    /// The island's frame on screen (nil while hidden): the window opens below it, never under it.
    private let islandFrame: () -> NSRect?
    private var window: NSWindow?
    /// The user dragged the window: from then on it stays where they put it on every step.
    private var userPlaced = false
    /// Moves until then are ours (centring, stepping aside; an animated move reports several).
    private var ownMoveUntil = Date.distantPast
    /// Where the window opened (centred): the steps outside the practice all sit exactly here.
    private var home: NSPoint?

    init(keeper: IndexKeeper, meeting: MeetingModel, download: ModelDownload, settings: SettingsContext, openMain: @escaping () -> Void,
         showIsland: @escaping () -> Void, islandFrame: @escaping () -> NSRect? = { nil }) {
        model = OnboardingModel(keeper: keeper)
        model.meeting = meeting
        model.download = download
        self.settings = settings
        self.openMain = openMain
        self.showIsland = showIsland
        self.islandFrame = islandFrame
    }

    var isVisible: Bool { window?.isVisible ?? false }
    var windowNumber: Int? { window?.windowNumber }

    /// `restart`: from the welcome again (설정 → 일반 → "처음 안내 다시 보기"). `step` jumps to one (debug captures).
    func show(restart: Bool = false, step: OnboardingStep? = nil) {
        if restart { model.restart() }
        if let step { model.go(step) }
        let w = window ?? makeWindow()
        if window == nil {
            ownMove {
                w.center()
                w.setFrameOrigin(IslandClearance.origin(clearOf: islandFrame(), window: w.frame, screen: (w.screen ?? NSScreen.main)?.visibleFrame))
            }
            home = w.frame.origin
        }
        window = w
        // a practice step shown before the window existed (a relaunch, a debug jump) still moves it aside
        stepAside(model.step == .ask || model.step == .meeting)
        NSApp.setActivationPolicy(.regular)
        AppWindows.bringToFront(w)
    }

    func close() { window?.close() }

    /// The practice steps: the window moves down to the bottom of the screen, out of the way of the island and its cards
    /// at the top middle, and the island shows (docs/11 원칙 6). Leaving them brings the window back to the middle.
    /// Once the user has dragged the window it stays put on every step (2026-10-26 사용자).
    func stepAside(_ aside: Bool) {
        if aside { showIsland() }
        guard !userPlaced, let w = window, let screen = w.screen ?? NSScreen.main else { return }
        let v = screen.visibleFrame
        var frame = w.frame
        if aside {
            // straight down from where it was, to the bottom: the island and its cards at the top middle stay clear
            frame.origin = NSPoint(x: (home?.x ?? v.midX - frame.width / 2), y: v.minY + 28)
        } else {
            frame.origin = home ?? NSPoint(x: v.midX - frame.width / 2, y: v.midY - frame.height / 2)
        }
        guard frame.origin != w.frame.origin else { return }
        ownMove { w.setFrame(frame, display: true, animate: !DebugFlags.reduceMotion) }
    }

    private func ownMove(_ move: () -> Void) {
        ownMoveUntil = Date().addingTimeInterval(0.8)
        move()
    }

    func windowDidMove(_ notification: Notification) {
        if Date() > ownMoveUntil { userPlaced = true }
    }

    func windowWillClose(_ notification: Notification) {
        model.stopMeter()
        AppWindows.dropDockIcon(closing: notification.object as? NSWindow)
    }

    private func makeWindow() -> NSWindow {
        let actions = OnboardingActions(openMain: { [weak self] in self?.openMain() },
                                        copyNotice: { AppDelegate.copyNotice() },
                                        close: { [weak self] in self?.close() },
                                        stepAside: { [weak self] in self?.stepAside($0) })
        let root = OnboardingView(model: model, settings: settings, actions: actions)
            .environment(\.forcedReduceMotion, DebugFlags.reduceMotion)
        let host = NSHostingController(rootView: root)
        host.sizingOptions = []
        let w = NSWindow(contentViewController: host)
        w.styleMask = [.titled, .closable, .miniaturizable, .fullSizeContentView]
        w.titleVisibility = .hidden
        w.titlebarAppearsTransparent = true
        w.isMovableByWindowBackground = true
        w.title = Self.title
        w.isReleasedWhenClosed = false
        w.appearance = AppearanceMode.shared.nsAppearance
        w.backgroundColor = NSColor(Theme.paper)
        w.sharingType = Capture.sharing
        w.setContentSize(NSSize(width: 540, height: 620))
        w.delegate = self
        return w
    }
}
