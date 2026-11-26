import AppKit
import SwiftUI

/// Text that selects across paragraphs. SwiftUI's `.textSelection(.enabled)` stops at the edge of each `Text`, so a
/// drag over the transcript broke at every turn. Here every paragraph of one `TextSelectionGroup` takes part in one
/// selection: a drag runs from any of them into the others (scrolling the page at its edge), ⌘C and the right-click
/// menu copy it as one piece, ⌘A takes all of them.

/// One selection over many paragraphs, kept apart from the views: a paragraph scrolled away (the transcript is lazy)
/// keeps its place and its text, and takes its part of the selection again when it comes back.
@MainActor
final class TextSelectionGroup {
    struct Point: Equatable {
        var id: String
        var offset: Int
    }

    private final class Member {
        let id: String
        var order: Int
        var text: String
        weak var view: SelectableTextView?

        init(id: String, order: Int, text: String) {
            self.id = id
            self.order = order
            self.text = text
        }
    }

    private var members: [String: Member] = [:]
    /// The paragraphs each part of the page gave, in its order: the notes first, then the transcript.
    private var sections: [Int: [(id: String, text: String)]] = [:]
    private var anchor: Point?
    private var focus: Point?
    private var monitor: Any?

    /// One part of the page's paragraphs in reading order, with their texts: also the ones never drawn yet, so ⌘A and
    /// a copy reach them. Parts follow one another by `section` (a meeting page: 0 the notes, 1 the transcript).
    func set(_ paragraphs: [(id: String, text: String)], section: Int = 0) {
        let now = sections[section] ?? []
        if now.count == paragraphs.count, zip(now, paragraphs).allSatisfy({ $0.id == $1.id && $0.text == $1.text }) { return }
        sections[section] = paragraphs
        let all = sections.keys.sorted().flatMap { sections[$0]! }
        var next: [String: Member] = [:]
        for (i, p) in all.enumerated() {
            let m = members[p.id] ?? Member(id: p.id, order: i, text: p.text)
            m.order = i
            m.text = p.text
            next[p.id] = m
        }
        members = next
        apply()
    }

    fileprivate func attach(_ view: SelectableTextView, id: String, text: String) {
        let m = members[id] ?? {
            let m = Member(id: id, order: (members.values.map(\.order).max() ?? -1) + 1, text: text)
            members[id] = m
            return m
        }()
        m.text = text
        m.view = view
        view.showSelection(range(for: id))
    }

    /// A paragraph that holds the keys (⌘C) is leaving the window: the lazy transcript drops turns scrolled far
    /// away, also during a drag. Another paragraph on screen takes the keys over, or the window would hand them to
    /// the search field and ⌘C would copy nothing.
    fileprivate func handOff(from leaving: SelectableTextView) {
        guard hasSelection else { return }
        // the window is not kept here: it may be going away itself (its views leave it as it closes), and a reference
        // to a window being freed stops the app. The paragraph that takes over says which window it is.
        DispatchQueue.main.async { [weak self] in
            guard let self,
                  let next = self.members.values.compactMap({ $0.view }).first(where: { $0 !== leaving && $0.window != nil }),
                  let window = next.window, window.firstResponder !== leaving || leaving.window == nil else { return }
            window.makeFirstResponder(next)
        }
    }

    /// A paragraph still in the window, to scroll the page with once the one a drag began on was dropped.
    func shown(in window: NSWindow) -> SelectableTextView? {
        members.values.compactMap { $0.view }.first { $0.window === window }
    }

    var hasSelection: Bool { !Self.ranges(order: ordered(), anchor: anchor, focus: focus).isEmpty }

    fileprivate func begin(_ p: Point) {
        listenForClicksElsewhere()
        anchor = p
        focus = p
        apply()
    }

    fileprivate func extend(to p: Point) {
        guard anchor != nil else { return begin(p) }
        focus = p
        apply()
    }

    fileprivate func select(from a: Point, to b: Point) {
        listenForClicksElsewhere()
        anchor = a
        focus = b
        apply()
    }

    func clear() {
        stopListening()
        guard anchor != nil else { return }
        anchor = nil
        focus = nil
        apply()
    }

    func selectAll() {
        let o = ordered()
        guard let first = o.first, let last = o.last else { return }
        select(from: Point(id: first.id, offset: 0), to: Point(id: last.id, offset: (last.text as NSString).length))
    }

    /// The selected text, the paragraphs a line apart.
    var selectedText: String {
        let o = ordered()
        let r = Self.ranges(order: o, anchor: anchor, focus: focus)
        return o.compactMap { p in r[p.id].map { (p.text as NSString).substring(with: $0) } }.joined(separator: "\n")
    }

    func copy() {
        let s = selectedText
        guard !s.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }

    /// Where a drag is now, in the window: on a paragraph, the letter there; between or beside them, the nearest
    /// place in reading order (the end of the paragraph above, the start of the first one).
    fileprivate func drag(to p: NSPoint, in window: NSWindow) {
        if let at = point(at: p, in: window) { extend(to: at) }
    }

    /// The place in the text nearest a point of the window, as `drag(to:in:)` takes it; none with no paragraph shown.
    fileprivate func point(at p: NSPoint, in window: NSWindow) -> Point? {
        let shown = members.values
            .compactMap { m -> (Member, SelectableTextView)? in
                guard let v = m.view, v.window === window else { return nil }
                return (m, v)
            }
            .sorted { $0.0.order < $1.0.order }
        guard !shown.isEmpty else { return nil }
        for (m, v) in shown {
            let q = v.convert(p, from: nil)
            if v.bounds.contains(q) { return Point(id: m.id, offset: v.offset(at: q)) }
        }
        var above: (Member, SelectableTextView)?
        for (m, v) in shown where v.convert(p, from: nil).y >= 0 { above = (m, v) }
        guard let (m, v) = above else { return Point(id: shown[0].0.id, offset: 0) }
        let q = v.convert(p, from: nil)
        let offset = q.y > v.bounds.height
            ? (m.text as NSString).length
            : v.offset(at: NSPoint(x: min(max(q.x, 0), v.bounds.width), y: q.y))
        return Point(id: m.id, offset: offset)
    }

    /// A drag near the top or bottom of the page scrolls it, faster the closer it is to the edge or the further past
    /// it. `autoscroll(with:)` alone scrolled only once the pointer left the whole window: the page's scroll view
    /// fills the window (the top row sits over it), so a window as tall as the screen never scrolled at all.
    func scrollAtEdge(_ p: NSPoint, in window: NSWindow) {
        guard let scroll = shown(in: window)?.enclosingScrollView else { return }
        let clip = scroll.contentView
        guard let doc = clip.documentView else { return }
        let f = clip.convert(clip.bounds, to: nil)
        let delta = Self.edgeStep(y: p.y, top: f.maxY - scroll.contentInsets.top, bottom: f.minY + scroll.contentInsets.bottom)
        guard delta != 0 else { return }
        var origin = clip.bounds.origin
        // `delta` is up the screen; a flipped page grows downward
        origin.y += clip.isFlipped ? -delta : delta
        let lowest = -scroll.contentInsets.top
        let highest = max(doc.frame.height - clip.bounds.height + scroll.contentInsets.bottom, lowest)
        origin.y = min(max(origin.y, lowest), highest)
        guard origin.y != clip.bounds.origin.y else { return }
        clip.scroll(to: origin)
        scroll.reflectScrolledClipView(clip)
    }

    /// How far to scroll for one step of a drag at `y` (window points, up positive) when the page shows from `bottom`
    /// to `top`: up when within `zone` of the top or above it, down near or below the bottom, nothing in between.
    nonisolated static func edgeStep(y: CGFloat, top: CGFloat, bottom: CGFloat, zone: CGFloat = 48) -> CGFloat {
        let zone = min(zone, max((top - bottom) / 4, 1))
        func speed(_ depth: CGFloat) -> CGFloat { min(max(depth * 0.4, 2), 48) }
        if y > top - zone { return speed(y - (top - zone)) }
        if y < bottom + zone { return -speed(bottom + zone - y) }
        return 0
    }

    private func ordered() -> [(id: String, text: String)] {
        members.sorted { $0.value.order < $1.value.order }.map { ($0.key, $0.value.text) }
    }

    private func range(for id: String) -> NSRange? {
        Self.ranges(order: ordered(), anchor: anchor, focus: focus)[id]
    }

    private func apply() {
        let r = Self.ranges(order: ordered(), anchor: anchor, focus: focus)
        for (id, m) in members { m.view?.showSelection(r[id]) }
    }

    /// A click on anything but a paragraph of this group lets the selection go, as a click outside text does.
    private func listenForClicksElsewhere() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] e in
            guard let self else { return e }
            let hit = e.window?.contentView?.hitTest(e.locationInWindow)
            let onMine = (hit as? SelectableTextView).map { v in self.members.values.contains { $0.view === v } } ?? false
            if !onMine && e.type == .leftMouseDown { self.clear() }
            return e
        }
    }

    private func stopListening() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
    }

    /// Each paragraph's part of the selection from `anchor` to `focus` (either way round), in the paragraphs'
    /// characters; none for an empty selection.
    nonisolated static func ranges(order: [(id: String, text: String)], anchor: Point?, focus: Point?) -> [String: NSRange] {
        guard let anchor, let focus,
              let ia = order.firstIndex(where: { $0.id == anchor.id }),
              let iff = order.firstIndex(where: { $0.id == focus.id }) else { return [:] }
        let (start, si, end, ei) = (ia, anchor.offset) <= (iff, focus.offset)
            ? (anchor, ia, focus, iff) : (focus, iff, anchor, ia)
        var out: [String: NSRange] = [:]
        for i in si...ei {
            let n = (order[i].text as NSString).length
            let from = i == si ? min(max(start.offset, 0), n) : 0
            let to = i == ei ? min(max(end.offset, 0), n) : n
            if to > from { out[order[i].id] = NSRange(location: from, length: to - from) }
        }
        return out
    }
}

/// Lets a drag over the page's text column start anywhere on it, not only on the letters: beside a short line, between
/// turns, in the margin. Put behind the column. A paragraph is only as wide as its longest line, so the letters alone
/// left little to press on. A press there still reaches what it lands on (a name's menu, a to-do's circle, a button);
/// only once it moves on does it become a selection from the letter nearest where it began.
struct SelectionPad: NSViewRepresentable {
    let group: TextSelectionGroup

    func makeNSView(context: Context) -> SelectionPadView { SelectionPadView() }

    func updateNSView(_ v: SelectionPadView, context: Context) { v.group = group }
}

final class SelectionPadView: NSView {
    weak var group: TextSelectionGroup?
    private var monitor: Any?
    /// Where a press began that may turn into a drag, in the window.
    private var down: NSPoint?
    private var dragging = false
    private var ticker: Timer?

    // SwiftUI's views over it take every click; it only watches the window's
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        stop()
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        guard window != nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .leftMouseDragged, .leftMouseUp]) { [weak self] e in
            guard let self else { return e }
            return self.handle(e)
        }
    }

    func handle(_ e: NSEvent) -> NSEvent? {
        guard let window, e.window === window, let group else { return e }
        switch e.type {
        case .leftMouseDown:
            stop()
            if e.clickCount == 1, starts(at: e.locationInWindow) { down = e.locationInWindow }
            return e
        case .leftMouseDragged:
            guard let start = down else { return e }
            if !dragging {
                guard hypot(e.locationInWindow.x - start.x, e.locationInWindow.y - start.y) > 3 else { return e }
                guard let from = group.point(at: start, in: window) else { stop(); return e }
                dragging = true
                group.begin(from)
                if let holder = group.shown(in: window) { window.makeFirstResponder(holder) }
                // the page scrolls at its edge on these ticks, also while the pointer rests there
                let t = Timer(timeInterval: 0.05, repeats: true) { [weak self] _ in
                    MainActor.assumeIsolated { self?.tick() }
                }
                RunLoop.main.add(t, forMode: .common)
                ticker = t
            }
            group.drag(to: e.locationInWindow, in: window)
            // a selection now: the views under the pointer do not take the drag for theirs
            return nil
        default:
            let was = dragging
            stop()
            if was {
                // as after a drag on a paragraph: SwiftUI may hand the keys to the search field after the click
                DispatchQueue.main.async { [weak self] in
                    guard let self, let window = self.window, let group = self.group, group.hasSelection,
                          !(window.firstResponder is SelectableTextView), let holder = group.shown(in: window) else { return }
                    window.makeFirstResponder(holder)
                }
            }
            return e
        }
    }

    private func tick() {
        guard dragging, let window, let group else { return }
        let p = window.mouseLocationOutsideOfEventStream
        group.scrollAtEdge(p, in: window)
        group.drag(to: p, in: window)
    }

    private func stop() {
        ticker?.invalidate()
        ticker = nil
        down = nil
        dragging = false
    }

    /// A press on the column itself, where the page shows: not on a paragraph (it starts its own drag), a text field
    /// or a control, nor on what floats over the page (the top row, the player, the jump buttons).
    private func starts(at p: NSPoint) -> Bool {
        guard let window, let scroll = enclosingScrollView, convert(bounds, to: nil).contains(p) else { return false }
        let clip = scroll.contentView
        let shown = clip.convert(clip.bounds, to: nil)
        guard p.y < shown.maxY - scroll.contentInsets.top, p.y > shown.minY + scroll.contentInsets.bottom,
              let hit = window.contentView?.hitTest(p),
              !(hit is NSText), !(hit is NSControl) else { return false }
        // in the page's scroll view: its empty places are the clip view's own (SwiftUI's views there take no click)
        var v: NSView? = hit
        while let view = v {
            if view === clip { return true }
            v = view.superview
        }
        return false
    }
}

/// A spoken word inside a paragraph: its characters and where it plays from.
struct PlayableWord: Equatable {
    let range: NSRange
    let ms: Int?
}

/// A paragraph of a `TextSelectionGroup`. With `words` and `play`, a click on a word (a click, not a drag) plays the
/// recording from there: the pointer on a word lights it and shows where it plays from (▶ 6:12), the word being heard
/// sits in a light blue box.
struct SelectableText: View {
    let id: String
    let text: String
    let group: TextSelectionGroup
    var size: CGFloat = 14.5
    var weight: NSFont.Weight = .regular
    var lineSpacing: CGFloat = 3.5
    var color: Color = Theme.ink.opacity(0.88)
    /// The text with its own looks (an answer's bold, code and citation marks), made for `size` at the text scale;
    /// its string must be `text`. The font, colour and line spacing above fill whatever it leaves out.
    var styled: ((CGFloat) -> NSAttributedString)? = nil
    var words: [PlayableWord] = []
    var current: Int? = nil
    var play: ((Int) -> Void)? = nil
    /// A click (no drag) on a citation mark of `styled` (`Citations.citeKey`) opens that source.
    var cite: ((Int) -> Void)? = nil

    @Environment(\.textScale) private var scale
    @State private var hover: (rect: CGRect, ms: Int)?

    var body: some View {
        // measured here, not in the alignment guide: it lays out a probe view, which must not happen inside a layout
        let baseline = SelectableTextView.swiftUILine(size * scale).baseline
        return SelectableTextRepresentable(id: id, text: text, group: group,
                                    font: NSFont.systemFont(ofSize: size * scale, weight: weight), lineSpacing: lineSpacing,
                                    color: NSColor(color), styled: styled?(size * scale), words: words, current: current, play: play,
                                    cite: cite, hover: { hover = $0 })
            // SwiftUI does not ask an AppKit view where its letters stand: a `.firstTextBaseline` row (a bullet, a
            // to-do's circle, a heading's count) lines up with the first line's baseline only when told
            .alignmentGuide(.firstTextBaseline) { _ in baseline }
            .overlay(alignment: .topLeading) {
                if let hover {
                    // above the word, its bottom 3 pt over the word's top: covers no word of the line, moves nothing
                    TimeChip(ms: hover.ms).fixedSize()
                        .offset(x: hover.rect.minX - 2, y: hover.rect.minY - (TimeChip.height * scale + 3))
                        .allowsHitTesting(false)
                }
            }
            .zIndex(hover == nil ? 0 : 1)
    }
}

private struct SelectableTextRepresentable: NSViewRepresentable {
    let id: String
    let text: String
    let group: TextSelectionGroup
    let font: NSFont
    let lineSpacing: CGFloat
    let color: NSColor
    let styled: NSAttributedString?
    let words: [PlayableWord]
    let current: Int?
    let play: ((Int) -> Void)?
    let cite: ((Int) -> Void)?
    let hover: ((rect: CGRect, ms: Int)?) -> Void

    func makeNSView(context: Context) -> SelectableTextView {
        let v = SelectableTextView.make()
        update(v)
        return v
    }

    func updateNSView(_ v: SelectableTextView, context: Context) { update(v) }

    private func update(_ v: SelectableTextView) {
        v.setText(text, font: font, lineSpacing: lineSpacing, color: color, styled: styled)
        v.words = words
        v.current = current
        v.play = play
        v.cite = cite
        v.reportHover = hover
        if v.group !== group || v.memberID != id {
            v.group = group
            v.memberID = id
        }
        group.attach(v, id: id, text: text)
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView v: SelectableTextView, context: Context) -> CGSize? {
        // as wide as its longest line, like `Text`: a heading's count can sit right after it
        // a zero-wide container does not wrap at all: lay out in one point instead, so the narrowest it can be is its
        // widest letter, as with `Text`, and the row around it does not take it for endlessly flexible
        return v.fit(forWidth: min(max(proposal.width ?? 10_000, 1), 10_000))
    }
}

final class SelectableTextView: NSTextView {
    weak var group: TextSelectionGroup?
    var memberID = ""
    var words: [PlayableWord] = [] {
        didSet { if words != oldValue { hoverWord = nil } }
    }
    var current: Int? {
        didSet { if current != oldValue { needsDisplay = true } }
    }
    var play: ((Int) -> Void)?
    var cite: ((Int) -> Void)?
    var reportHover: (((rect: CGRect, ms: Int)?) -> Void)?

    private var hoverWord: Int? {
        didSet {
            guard hoverWord != oldValue else { return }
            needsDisplay = true
            let h = hoverWord.flatMap { i in words[i].ms.map { (rect: rect(of: words[i].range), ms: $0) } }
            reportHover?(h)
        }
    }
    private var shown: NSRange?
    /// The TextKit 1 stack (the layout manager draws the selection and finds letters); the view does not keep it.
    private var storage: NSTextStorage?
    /// Where the letters stand in each line, from its top: where SwiftUI puts them.
    private var baseline: CGFloat = 0
    private var look: (font: NSFont, spacing: CGFloat, color: NSColor, styled: NSAttributedString?)?
    /// The sizes asked for, by width: SwiftUI asks again on every layout pass of the page (each new line of a meeting
    /// going on, each to-do checked), and in one pass it asks more than one width (the ideal, then the column), so
    /// keeping only the last one laid every paragraph out again each time. Laying out again for a width already
    /// asked gives the same answer.
    private var measured: [CGFloat: CGSize] = [:]

    override init(frame: NSRect, textContainer: NSTextContainer?) {
        super.init(frame: frame, textContainer: textContainer)
        isEditable = false
        isSelectable = false
        isRichText = false
        drawsBackground = false
        textContainerInset = .zero
        textContainer?.lineFragmentPadding = 0
        textContainer?.widthTracksTextView = true
        isVerticallyResizable = false
        isHorizontallyResizable = false
    }

    required init?(coder: NSCoder) { fatalError() }

    static func make() -> SelectableTextView {
        let storage = NSTextStorage()
        let lm = NSLayoutManager()
        storage.addLayoutManager(lm)
        let tc = NSTextContainer(size: NSSize(width: 0, height: CGFloat.greatestFiniteMagnitude))
        lm.addTextContainer(tc)
        let v = SelectableTextView(frame: .zero, textContainer: tc)
        v.storage = storage
        lm.delegate = v
        return v
    }

    func setText(_ text: String, font: NSFont, lineSpacing: CGFloat, color: NSColor, styled: NSAttributedString? = nil) {
        let same = look.map { $0.font == font && $0.spacing == lineSpacing && $0.color == color && $0.styled == styled } ?? false
        guard !same || string != text, let storage = textStorage else { return }
        look = (font, lineSpacing, color, styled)
        let style = NSMutableParagraphStyle()
        style.lineSpacing = lineSpacing
        style.lineBreakStrategy = .hangulWordPriority  // wrap Korean at spaces, as SwiftUI Text does
        // SwiftUI's lines are about a point taller than TextKit's for the same font: take its height, so the page
        // looks as it did with `Text`
        let line = Self.swiftUILine(font.pointSize)
        style.minimumLineHeight = line.height
        style.maximumLineHeight = line.height
        baseline = line.baseline
        let base: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color, .paragraphStyle: style]
        if let styled, styled.string == text {
            let a = NSMutableAttributedString(string: text, attributes: base)
            styled.enumerateAttributes(in: NSRange(location: 0, length: styled.length)) { attrs, r, _ in
                a.addAttributes(attrs.filter { $0.key != .paragraphStyle }, range: r)
            }
            storage.setAttributedString(a)
        } else {
            storage.setAttributedString(NSAttributedString(string: text, attributes: base))
        }
        shown = nil
        hoverWord = nil
        measured = [:]
    }

    private static var lines: [CGFloat: (height: CGFloat, baseline: CGFloat)] = [:]

    /// One line of `Text` in `.system(size:)`, Korean and Latin together: its height and where its letters stand,
    /// measured once per size. SwiftUI rounds both to whole points, TextKit does not.
    static func swiftUILine(_ size: CGFloat) -> (height: CGFloat, baseline: CGFloat) {
        if let l = lines[size] { return l }
        final class Seen { var baseline: CGFloat = 0 }
        let seen = Seen()
        let probe = HStack(alignment: .firstTextBaseline, spacing: 0) {
            Text("가A").font(.system(size: size))
                .alignmentGuide(.firstTextBaseline) { d in
                    seen.baseline = d[.firstTextBaseline]
                    return d[.firstTextBaseline]
                }
            Color.clear.frame(width: 1, height: 1)
        }
        let h = NSHostingView(rootView: probe.fixedSize())
        h.frame.size = h.fittingSize
        h.layoutSubtreeIfNeeded()
        let l = (height: h.fittingSize.height, baseline: seen.baseline)
        lines[size] = l
        return l
    }

    /// The paragraph laid out in `w`: its longest line's width and its height.
    func fit(forWidth w: CGFloat) -> CGSize {
        guard let lm = layoutManager, let tc = textContainer else { return .zero }
        if let m = measured[w] { return m }
        tc.containerSize = NSSize(width: w, height: .greatestFiniteMagnitude)
        lm.ensureLayout(for: tc)
        let used = lm.usedRect(for: tc)
        // as wide as its longest line (each line's used rect), a point to spare so rounding never pushes the last word
        // to a new line when it is laid out again in that width
        var longest: CGFloat = 0
        lm.enumerateLineFragments(forGlyphRange: lm.glyphRange(for: tc)) { _, used, _, _, _ in longest = max(longest, used.maxX) }
        let need = ceil(longest) + 1
        let size = CGSize(width: w <= 1 ? need : min(w, need), height: ceil(used.height))
        if measured.count >= 8 { measured = [:] }
        measured[w] = size
        // drawn in the view's own width: a measure in another width must not leave the letters laid out in it
        // (the container follows the frame only when the frame changes)
        if bounds.width > 0, bounds.width != w { tc.containerSize = NSSize(width: bounds.width, height: .greatestFiniteMagnitude) }
        return size
    }

    /// The group's part for this paragraph, drawn the way selected text is.
    func showSelection(_ r: NSRange?) {
        guard r != shown, let lm = layoutManager else { return }
        let all = NSRange(location: 0, length: (string as NSString).length)
        lm.removeTemporaryAttribute(.backgroundColor, forCharacterRange: all)
        if let r, NSMaxRange(r) <= all.length {
            lm.addTemporaryAttribute(.backgroundColor, value: NSColor.selectedTextBackgroundColor, forCharacterRange: r)
        }
        shown = r
    }

    /// The place between letters nearest the point.
    /// The source number of the citation mark under `p`, if any.
    private func citation(at p: NSPoint) -> Int? {
        guard let lm = layoutManager, let tc = textContainer, let storage = textStorage, storage.length > 0 else { return nil }
        let g = lm.glyphIndex(for: p, in: tc)
        guard lm.boundingRect(forGlyphRange: NSRange(location: g, length: 1), in: tc).insetBy(dx: -3, dy: -3).contains(p) else { return nil }
        let i = lm.characterIndexForGlyph(at: g)
        guard i < storage.length else { return nil }
        return storage.attribute(Citations.citeKey, at: i, effectiveRange: nil) as? Int
    }

    func offset(at p: NSPoint) -> Int {
        guard let lm = layoutManager, let tc = textContainer else { return 0 }
        var fraction: CGFloat = 0
        let i = lm.characterIndex(for: p, in: tc, fractionOfDistanceBetweenInsertionPoints: &fraction)
        let n = (string as NSString).length
        if i >= n { return n }
        return fraction > 0.5 ? i + 1 : i
    }

    private func rect(of r: NSRange) -> CGRect {
        guard let lm = layoutManager, let tc = textContainer else { return .zero }
        let g = lm.glyphRange(forCharacterRange: r, actualCharacterRange: nil)
        return lm.boundingRect(forGlyphRange: g, in: tc)
    }

    private func word(at p: NSPoint) -> Int? {
        guard play != nil, !words.isEmpty, let lm = layoutManager, let tc = textContainer,
              lm.usedRect(for: tc).contains(p) else { return nil }
        let i = lm.characterIndex(for: p, in: tc, fractionOfDistanceBetweenInsertionPoints: nil)
        return words.firstIndex { NSLocationInRange(i, $0.range) && rect(of: $0.range).insetBy(dx: -2, dy: -1).contains(p) }
    }

    // MARK: drawing

    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        func box(_ i: Int?, _ color: Color) {
            guard let i, words.indices.contains(i) else { return }
            NSColor(color).setFill()
            NSBezierPath(roundedRect: self.rect(of: words[i].range).insetBy(dx: -2, dy: -1), xRadius: 4, yRadius: 4).fill()
        }
        box(current, Theme.speakers[0].opacity(0.16))
        if hoverWord != current, words.indices.contains(hoverWord ?? -1), words[hoverWord!].ms != nil {
            box(hoverWord, Theme.ink.opacity(0.07))
        }
    }

    // MARK: the pointer

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for a in trackingAreas where a.owner === self && a.userInfo?["word"] != nil { removeTrackingArea(a) }
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInActiveApp, .inVisibleRect],
                                       owner: self, userInfo: ["word": true]))
    }

    override func resetCursorRects() { addCursorRect(bounds, cursor: .iBeam) }

    override func mouseMoved(with event: NSEvent) {
        NSCursor.iBeam.set()
        hoverWord = word(at: convert(event.locationInWindow, from: nil))
    }

    override func mouseExited(with event: NSEvent) { hoverWord = nil }

    override func mouseDown(with event: NSEvent) {
        guard let group, let window else { return }
        window.makeFirstResponder(self)
        let p = convert(event.locationInWindow, from: nil)
        let at = offset(at: p)
        if event.clickCount >= 2 {
            let n = (string as NSString).length
            let r = event.clickCount == 2
                ? selectionRange(forProposedRange: NSRange(location: min(at, max(n - 1, 0)), length: 0), granularity: .selectByWord)
                : NSRange(location: 0, length: n)
            group.select(from: .init(id: memberID, offset: r.location), to: .init(id: memberID, offset: NSMaxRange(r)))
            return
        }
        if event.modifierFlags.contains(.shift) {
            group.extend(to: .init(id: memberID, offset: at))
        } else {
            group.begin(.init(id: memberID, offset: at))
        }
        let clicked = word(at: p)
        var dragged = event.modifierFlags.contains(.shift)
        NSEvent.startPeriodicEvents(afterDelay: 0.1, withPeriod: 0.05)
        defer { NSEvent.stopPeriodicEvents() }
        // the wheel too: a loop that waits only for the mouse leaves the page still under a scroll until the button
        // goes up, so a drag could not scroll on to what it wanted
        tracking: while let e = window.nextEvent(matching: [.leftMouseDragged, .leftMouseUp, .periodic, .scrollWheel]) {
            switch e.type {
            case .leftMouseUp:
                break tracking
            case .scrollWheel:
                (window === self.window ? self : group.shown(in: window))?.enclosingScrollView?.scrollWheel(with: e)
                // scrolling with the button down goes on with the selection, as in any text
                dragged = true
                hoverWord = nil
                group.drag(to: window.mouseLocationOutsideOfEventStream, in: window)
            case .periodic:
                guard dragged else { continue }
                let at = window.mouseLocationOutsideOfEventStream
                group.scrollAtEdge(at, in: window)
                group.drag(to: at, in: window)
            default:
                let d = hypot(e.locationInWindow.x - event.locationInWindow.x, e.locationInWindow.y - event.locationInWindow.y)
                if d > 3 { dragged = true }
                guard dragged else { continue }
                hoverWord = nil
                // the page scrolls on the ticks only, so its speed is the same whether the pointer moves or rests
                group.drag(to: e.locationInWindow, in: window)
            }
        }
        // a first click into the window from another app makes it key, and SwiftUI then hands the keys to the
        // search field (after this click): take them back so ⌘C copies what was dragged over
        DispatchQueue.main.async { [weak self] in
            guard let self, let group = self.group, group.hasSelection else { return }
            let holder = self.window === window ? self : group.shown(in: window)
            if let holder, !(window.firstResponder is SelectableTextView) { window.makeFirstResponder(holder) }
        }
        guard !dragged else { return }
        group.clear()
        if let cite, let n = citation(at: p) {
            cite(n)
            return
        }
        if let clicked, let ms = words[clicked].ms { play?(ms) }
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard group?.hasSelection == true else { return nil }
        let menu = NSMenu()
        menu.addItem(withTitle: String(localized: "복사"), action: #selector(copy(_:)), keyEquivalent: "").target = self
        return menu
    }

    override var acceptsFirstResponder: Bool { true }

    override func viewWillMove(toWindow next: NSWindow?) {
        if next == nil, window?.firstResponder === self { group?.handOff(from: self) }
        super.viewWillMove(toWindow: next)
    }

    override func copy(_ sender: Any?) { group?.copy() }

    override func selectAll(_ sender: Any?) { group?.selectAll() }

    override func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        switch item.action {
        case #selector(copy(_:)): return group?.hasSelection == true
        case #selector(selectAll(_:)): return true
        default: return super.validateUserInterfaceItem(item)
        }
    }
}

extension SelectableTextView: NSLayoutManagerDelegate {
    func layoutManager(_ lm: NSLayoutManager, shouldSetLineFragmentRect rect: UnsafeMutablePointer<NSRect>,
                       lineFragmentUsedRect used: UnsafeMutablePointer<NSRect>, baselineOffset: UnsafeMutablePointer<CGFloat>,
                       in tc: NSTextContainer, forGlyphRange glyphs: NSRange) -> Bool {
        // TextKit rounds a baseline on a whole or half pixel down the page where SwiftUI rounds it up: a little
        // under half a pixel higher lands every line on the pixel SwiftUI picks
        baselineOffset.pointee = baseline - 0.45 / (window?.backingScaleFactor ?? 1)
        return true
    }
}
