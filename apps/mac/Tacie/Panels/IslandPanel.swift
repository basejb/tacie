import AppKit
import SwiftUI

/// Panel that takes typing without activating the app, so the meeting app keeps focus.
final class TaciePanel: NSPanel {
    init(contentRect: NSRect) {
        super.init(contentRect: contentRect, styleMask: [.nonactivatingPanel, .borderless, .fullSizeContentView],
                   backing: .buffered, defer: false)
        level = .statusBar
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        hidesOnDeactivate = false
        backgroundColor = .clear
        isOpaque = false
        hasShadow = false  // the island draws its own soft shadow
        appearance = AppearanceMode.shared.nsAppearance
        sharingType = Capture.sharing
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    enum DragStep { case began(NSPoint), moved(NSPoint), ended }
    /// Whether a press at this point (window coordinates) may drag the island.
    var dragsFrom: (NSPoint) -> Bool = { _ in false }
    var onDrag: (DragStep) -> Void = { _ in }
    private var press: NSPoint?
    private var dragging = false

    /// Dragging by hand, not AppKit's window drag: that one could not hold the island at a screen's edge. A press
    /// stays a click until the pointer travels a few points; from then on the drag events are ours.
    override func sendEvent(_ event: NSEvent) {
        switch event.type {
        case .leftMouseDown:
            dragging = false
            press = startsDrag(event) ? NSEvent.mouseLocation : nil
        case .leftMouseDragged:
            guard let p = press else { break }
            let m = NSEvent.mouseLocation
            if !dragging, hypot(m.x - p.x, m.y - p.y) >= 3 {
                dragging = true
                onDrag(.began(p))
            }
            if dragging {
                onDrag(.moved(m))
                return
            }
        case .leftMouseUp:
            press = nil
            if dragging {
                dragging = false
                onDrag(.ended)
                // the view under the pointer saw the press: letting go far outside it ends it without a click
                if let up = NSEvent.mouseEvent(with: .leftMouseUp, location: NSPoint(x: -10_000, y: -10_000),
                                               modifierFlags: event.modifierFlags, timestamp: event.timestamp,
                                               windowNumber: event.windowNumber, context: nil,
                                               eventNumber: event.eventNumber, clickCount: event.clickCount, pressure: 0) {
                    super.sendEvent(up)
                }
                return
            }
        default:
            break
        }
        super.sendEvent(event)
    }

    private func startsDrag(_ event: NSEvent) -> Bool {
        guard let content = contentView else { return false }
        // a drag in the ask field selects text
        var view = content.hitTest(content.superview?.convert(event.locationInWindow, from: nil) ?? event.locationInWindow)
        while let v = view {
            if v is NSText || v is NSTextField { return false }
            view = v.superview
        }
        return dragsFrom(event.locationInWindow)
    }

    /// The ask field's editor. Over dark windows the glass turns dark and the system colours follow it: a dark-mode
    /// selection under ink text, and a light caret. Both keep their light look, so they read on the white glass.
    /// SwiftUI's own editor, only recoloured: SwiftUI casts the editor to its class and crashed on a plain NSTextView.
    override func fieldEditor(_ createFlag: Bool, for object: Any?) -> NSText? {
        let editor = super.fieldEditor(createFlag, for: object)
        guard let editor = editor as? NSTextView else { return editor }
        var selection = NSColor.selectedTextBackgroundColor
        var caret = NSColor.textInsertionPointColor
        AppearanceMode.shared.nsAppearance?.performAsCurrentDrawingAppearance {
            // fixed colours, resolved here: a dynamic one resolves again under the glass's dark look when drawn
            selection = NSColor.selectedTextBackgroundColor.usingColorSpace(.sRGB) ?? selection
            caret = NSColor.textInsertionPointColor.usingColorSpace(.sRGB) ?? caret
        }
        editor.selectedTextAttributes = [.backgroundColor: selection]
        editor.insertionPointColor = caret
        return editor
    }
}

/// Takes the first click. The island never activates the app, so with another app in front a click on the pill only
/// made the panel key, and the chooser needed a second click.
private final class FirstClickHostingView<Content: View>: NSHostingView<Content> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

/// Company code stays out of screen shares and recordings unless 설정 → 개인정보 "화면 공유·녹화에 보이기" is on
/// (off by default); `--allow-capture` is for our own screenshots.
enum Capture {
    static let key = "showInCapture"
    private static let flag = LaunchArgs.has("--allow-capture")
    static var allowed: Bool { flag || UserDefaults.standard.bool(forKey: key) }
    static var sharing: NSWindow.SharingType { allowed ? .readOnly : .none }

    /// After the setting changes: every window of the app (island, main window, onboarding, sheets) at once.
    @MainActor static func applyToAll() {
        for w in NSApp.windows { w.sharingType = sharing }
    }
}

/// `--reduce-motion` forces the 움직임 줄이기 branch on the island, for checking it without touching System Settings.
enum DebugFlags {
    static let reduceMotion = LaunchArgs.has("--reduce-motion")
}

/// Keeps the island at the top center of the screen (or where it was dragged). The window follows what the
/// island shows: it grows at once and shrinks after the fold animation, so nothing gets clipped. What it draws stays
/// on one screen: growing past an edge, or the open button coming in there, moves it inward, and back afterwards.
@MainActor
final class IslandController: NSObject {
    private let panel: TaciePanel
    private let model: MeetingModel
    private var host: NSView?
    private var size = NSSize(width: 200, height: 60)
    private var target = NSSize(width: 200, height: 60)  // the size asked for last; `size` is the frame's
    private var generation = 0
    /// What the island draws (glass, not the shadow or the room kept for hovering) and the pill, in its own space.
    private var drawn = CGRect.null
    private var pill = CGRect.null
    /// While dragging: the pointer and the anchor at the press, and whether the pointer was on the pill.
    private var drag: (pointer: NSPoint, anchor: NSPoint, hovered: Bool)?
    private var dragAnchor: NSPoint?
    private let glide = WindowGlide()
    /// Where the island was dragged to; none while it sits in the middle (the settings page reads it too).
    nonisolated static let anchorKey = "islandAnchor"
    /// Wider and taller than the island ever gets (크게 보기 is 760 pt wide plus shadow room).
    private static let canvas = NSSize(width: 1200, height: 1200)

    init(model: MeetingModel, onStart: @escaping (MeetingMode) -> Void, onOpenMain: @escaping (String?) -> Void,
         onHide: @escaping () -> Void) {
        self.model = model
        panel = TaciePanel(contentRect: NSRect(x: 0, y: 0, width: 200, height: 60))
        super.init()
        let root = IslandView(model: model, onStart: onStart, onOpenMain: onOpenMain, onHide: onHide,
                              onSize: { [weak self] s in self?.fit(s) },
                              onDrawn: { [weak self] all, pill in self?.drew(all, pill: pill) })
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .environment(\.forcedReduceMotion, DebugFlags.reduceMotion)
        let host = FirstClickHostingView(rootView: root)
        host.sizingOptions = []
        // Not the content view itself: as content view it also resized the window on its own (animated window
        // size, corner insets), and quick size changes made AppKit throw (more constraint passes than views).
        // A fixed canvas pinned to the box's top center, bigger than anything the island shows: the window grows and
        // shrinks around it without SwiftUI laying it out again. When the host followed the window, a card opening
        // got its new bounds one frame into the open animation, and the pill dropped about 60 pt and climbed back.
        let box = NSView(frame: NSRect(x: 0, y: 0, width: 200, height: 60))
        box.addSubview(host)
        self.host = host
        pinCanvas(to: box.bounds.size)
        panel.contentView = box
        glide.panel = panel
        panel.dragsFrom = { [weak self] p in self?.onPill(p) ?? false }
        panel.onDrag = { [weak self] step in self?.dragged(step) }
        watchTyping()
        watchOutsideClicks()
        watchEscape()
        watchBlur()
    }

    /// The ask is being shown on purpose from elsewhere (the onboarding's 3단계): leaving the island keeps it.
    var holdsAsk: () -> Bool = { false }

    var isVisible: Bool { panel.isVisible }
    var windowNumber: Int { panel.windowNumber }
    /// Where the island is on screen now (nil while hidden): other windows keep clear of it.
    var frame: NSRect? { panel.isVisible ? panel.frame : nil }

    func show() {
        place(size)
        panel.orderFrontRegardless()
    }

    func hide() { panel.orderOut(nil) }
    func toggle() { isVisible ? hide() : show() }

    /// Forgets where the island was dragged: back to the top center under the menu bar.
    func resetPosition() {
        UserDefaults.standard.removeObject(forKey: Self.anchorKey)
        if isVisible { place(size) }
    }

    /// Debug control: as if the island had been dragged so its top center sits here.
    func move(toTopCenter p: NSPoint) {
        UserDefaults.standard.set(NSStringFromPoint(p), forKey: Self.anchorKey)
        place(size)
    }

    /// Called from SwiftUI's layout pass, so the frame changes on the next turn of the run loop: resizing the
    /// window inside that pass made AppKit throw (more constraint passes than views) when questions came fast.
    private func fit(_ s: CGSize) {
        let new = NSSize(width: ceil(s.width), height: ceil(s.height))
        guard new != target else { return }
        target = new
        generation += 1
        let g = generation
        // let a fold finish inside the old frame first
        let steps = IslandSizing.steps(from: size, to: new)
        for (step, delay) in [(steps.now, 0), (steps.later, IslandSizing.shrinkDelay)] {
            guard let step else { continue }
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
                guard let self, self.generation == g, step != self.size else { return }
                self.size = step
                self.place(step)
            }
        }
    }

    /// The pointer coming onto the pill brings the open button in: near an edge the island makes room for it.
    private func drew(_ all: CGRect, pill: CGRect) {
        let moved = all != drawn
        drawn = all
        self.pill = pill
        // a new size on its way places the island with this; else the next turn, after this layout pass
        guard moved, isVisible, target == size else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.target == self.size else { return }
            self.place(self.size)
        }
    }

    /// Top center under the menu bar, or the remembered spot if it is still on a screen.
    private var anchor: NSPoint {
        if let saved = UserDefaults.standard.string(forKey: Self.anchorKey) {
            let p = NSPointFromString(saved)
            if NSScreen.screens.contains(where: { $0.frame.insetBy(dx: 0, dy: -1).contains(NSPoint(x: p.x, y: p.y - 1)) }) { return p }
        }
        let f = (NSScreen.main ?? NSScreen.screens.first)?.visibleFrame ?? .zero
        return NSPoint(x: f.midX, y: f.maxY - 6)
    }

    private static let edgeMargin: CGFloat = 8
    /// How far past an edge a drag may pull the island.
    private static let stretch: CGFloat = 24

    /// While dragging the window follows the pointer. Otherwise, when the island has to move (growing or shrinking
    /// at an edge, the open button making room, letting go past an edge), the window first takes its new size where
    /// the island already is, and then glides over at that size. Moving and resizing at once showed the island
    /// jump aside and then grow; AppKit's window animation, mixed with resizes, showed it in two places by turns.
    private func place(_ s: NSSize) {
        let a = dragAnchor ?? anchor
        var frame = NSRect(x: (a.x - s.width / 2).rounded(), y: a.y - s.height, width: s.width, height: s.height)
        // the screen the island sits on: while dragging the pointer's, so it crosses whole once the pointer has;
        // else the anchor's, so growing never crosses onto the next
        let point = drag == nil ? NSPoint(x: a.x, y: a.y - 1) : NSEvent.mouseLocation
        if let screen = Self.screen(at: point)?.visibleFrame, let shown = onScreen(a) {
            let d = drag == nil
                ? IslandSizing.push(shown, into: screen, margin: Self.edgeMargin)
                : IslandSizing.stretch(shown, into: screen, margin: Self.edgeMargin, limit: Self.stretch)
            frame.origin.x += d.dx
            frame.origin.y += d.dy
        }
        guard frame != (glide.target ?? panel.frame) else { return }
        let now = panel.frame
        glide.stop()
        pinCanvas(to: s)
        guard drag == nil, panel.isVisible else { return panel.setFrame(frame, display: true) }
        // the island is centred in the window: same center and top, it stays where it is
        let here = NSRect(x: (now.midX - s.width / 2).rounded(), y: now.maxY - s.height, width: s.width, height: s.height)
        if here != now { panel.setFrame(here, display: true) }
        glide.start(to: frame)
    }

    /// Where the drawn island lands on screen with its top center at `a`. The island is centred under the anchor
    /// whatever the window's size, so the window's lag behind a shrink does not matter.
    private func onScreen(_ a: NSPoint) -> CGRect? {
        guard !drawn.isNull else { return nil }
        return CGRect(x: a.x - target.width / 2 + drawn.minX, y: a.y - drawn.maxY, width: drawn.width, height: drawn.height)
    }

    private static func screen(at p: NSPoint) -> NSScreen? {
        NSScreen.screens.first { $0.frame.contains(p) } ?? NSScreen.main ?? NSScreen.screens.first
    }

    /// A window point on the pill: the pill is the handle (the card's text selects, the ask field types).
    private func onPill(_ p: NSPoint) -> Bool {
        let f = panel.frame
        return pill.contains(CGPoint(x: p.x - (f.width - target.width) / 2, y: f.height - p.y))
    }

    private func dragged(_ step: TaciePanel.DragStep) {
        switch step {
        case .began(let p):
            drag = (p, anchor, model.pillHovered)
            // pause, stop and the open button fold while it moves
            model.setPillHover(false)
        case .moved(let m):
            guard let d = drag else { return }
            dragAnchor = NSPoint(x: d.anchor.x + m.x - d.pointer.x, y: d.anchor.y + m.y - d.pointer.y)
            place(size)
        case .ended:
            guard let d = drag, var a = dragAnchor else { drag = nil; return }
            // kept inside the screen the pointer let go on; it springs back there from past the edge
            if let screen = Self.screen(at: NSEvent.mouseLocation)?.visibleFrame, let shown = onScreen(a) {
                let p = IslandSizing.push(shown, into: screen, margin: Self.edgeMargin)
                a = NSPoint(x: a.x + p.dx, y: a.y + p.dy)
            }
            drag = nil
            dragAnchor = nil
            UserDefaults.standard.set(NSStringFromPoint(a), forKey: Self.anchorKey)
            place(size)
            if d.hovered { model.setPillHover(true) }
        }
    }

    /// The canvas's top center on the window's top center (the box is not flipped: y counts from the bottom).
    private func pinCanvas(to s: NSSize) {
        host?.frame = NSRect(x: ((s.width - Self.canvas.width) / 2).rounded(), y: s.height - Self.canvas.height,
                             width: Self.canvas.width, height: Self.canvas.height)
    }

    /// A card opened from the pill (the chooser, this meeting's questions) folds when a click lands anywhere else: in
    /// another app (a global monitor, which needs no permission for clicks) or in the main window. Answers keep their
    /// own timers: a click in the meeting app must not take one away while it is being read.
    private func watchOutsideClicks() {
        let clicks: NSEvent.EventTypeMask = [.leftMouseDown, .rightMouseDown, .otherMouseDown]
        NSEvent.addGlobalMonitorForEvents(matching: clicks) { [weak self] _ in
            Task { @MainActor in self?.foldOpenedCard() }
        }
        NSEvent.addLocalMonitorForEvents(matching: clicks) { [weak self] event in
            if event.window !== self?.panel { Task { @MainActor in self?.foldOpenedCard() } }
            return event
        }
    }

    /// Esc while the island holds the keys (the ask field, the chooser, this meeting's questions): the field empties
    /// and everything opened from the pill closes. SwiftUI's onExitCommand did not always get it from the
    /// non-activating panel (the field editor kept it, or nothing in the view was focused).
    private func watchEscape() {
        NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, event.keyCode == 53, event.window === self.panel,
                  self.model.asking || self.model.shown != .nothing else { return event }
            self.model.escape()
            return nil
        }
    }

    /// The ask box's answer closes when the focus goes elsewhere (AskBlur). The panel never activates the app, so the
    /// app's own resign-active does not come when the meeting app or the browser was in front all along; the panel
    /// losing key does (a click in another app, or in another Tacie window). Another app coming forward (⌘Tab, a card's
    /// link opening in the browser) is watched too, for when the panel was not key. A card's link is already on its
    /// way by then: both notifications come after `CardActions.open` has handed the URL over.
    private func watchBlur() {
        NotificationCenter.default.addObserver(forName: NSWindow.didResignKeyNotification, object: panel, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.blurred() }
        }
        NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil,
                                                          queue: .main) { [weak self] note in
            let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
            guard app?.processIdentifier != ProcessInfo.processInfo.processIdentifier else { return }
            MainActor.assumeIsolated { self?.blurred() }
        }
    }

    private func blurred() {
        let typed = model.current.map { $0.origin == .typed } ?? false
        guard AskBlur.closes(asking: model.asking, typedAnswer: typed, inMeeting: model.isRunning, held: holdsAsk()) else { return }
        model.close()
    }

    private func foldOpenedCard() {
        if model.shown == .chooser || model.shown == .history { model.close() }
    }

    /// The panel has to be key for the ask field to take typing; it stays non-activating.
    private func watchTyping() {
        withObservationTracking {
            _ = model.asking
        } onChange: { [weak self] in
            Task { @MainActor in
                guard let self else { return }
                if self.model.asking { self.panel.makeKey() }
                self.watchTyping()
            }
        }
    }
}

/// Moves the island's window at a fixed size, a step per display frame, eased out. Stepped here rather than by AppKit's
/// window animation, which a resize partway through did not stop: the window went on moving under the new size.
@MainActor
private final class WindowGlide: NSObject {
    weak var panel: NSPanel?
    private(set) var target: NSRect?
    private var from = NSPoint.zero
    private var start: CFTimeInterval?
    private var link: CADisplayLink?
    private static let duration = 0.3

    func start(to frame: NSRect) {
        guard let panel, let view = panel.contentView else { return }
        let reduce = DebugFlags.reduceMotion || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        guard !reduce, panel.frame.origin != frame.origin else { return panel.setFrame(frame, display: true) }
        from = panel.frame.origin
        target = frame
        start = nil
        let link = view.displayLink(target: self, selector: #selector(step))
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    func stop() {
        link?.invalidate()
        link = nil
        target = nil
    }

    @objc private func step(_ link: CADisplayLink) {
        guard let panel, let target else { return stop() }
        let t0 = start ?? link.timestamp
        start = t0
        let t = min((link.targetTimestamp - t0) / Self.duration, 1)
        let k = 1 - pow(1 - t, 3)
        let x = from.x + (target.minX - from.x) * k, y = from.y + (target.minY - from.y) * k
        // whole backing pixels, so the glass does not shimmer between frames
        let scale = panel.backingScaleFactor
        panel.setFrameOrigin(NSPoint(x: (x * scale).rounded() / scale, y: (y * scale).rounded() / scale))
        if t >= 1 {
            panel.setFrame(target, display: true)
            stop()
        }
    }
}

