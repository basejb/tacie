import SwiftUI

/// A page under the main window's title bar: a row at the top (52 pt, beside the traffic lights), then content that
/// scrolls. On macOS 26 the scroll view's frame reaches up under that row (it spans the whole window height), and a
/// later sibling wins the click where the two overlap. So the row sits in front; without that, End, copy and the menu
/// took no clicks.
/// `focus` scrolls the view with that id to the top once, then clears (a question opened from the island).
/// `jumps` puts the Scroll to top / Scroll to bottom buttons at the bottom right of a long page, that far up from the
/// bottom edge.
struct TitleRowPage<Row: View, Content: View>: View {
    private let row: Row
    private let content: Content
    @Binding private var focus: String?
    private let jumps: CGFloat?

    @State private var spot = PageSpot()

    init(focus: Binding<String?> = .constant(nil), jumps: CGFloat? = nil,
         @ViewBuilder row: () -> Row, @ViewBuilder content: () -> Content) {
        _focus = focus
        self.jumps = jumps
        self.row = row()
        self.content = content()
    }

    var body: some View {
        if #available(macOS 26.0, *) {
            scroller
        } else {
            VStack(spacing: 0) {
                row.zIndex(1)
                scroller
            }
        }
    }

    private var scroller: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(spacing: 0) {
                    Color.clear.frame(height: 0).id(PageSpot.top)
                    content
                    Color.clear.frame(height: 0).id(PageSpot.bottom)
                }
                .background {
                    // SwiftUI geometry here came once, with the page still 0 tall, and never again: the buttons never
                    // showed (0.1.21). The AppKit scroll view says where it is every time it moves or grows.
                    if jumps != nil {
                        ScrollWatcher { f, viewport in
                            let next = PageSpot.Shown(content: f, viewport: viewport)
                            if next != spot.shown { spot.shown = next }
                        }
                    }
                }
            }
            .scrollIndicators(.never)
            .modifier(SoftTopBar(bar: row))
            .overlay(alignment: .bottomTrailing) {
                if let jumps, spot.shown.long {
                    JumpButtons(up: spot.shown.up, down: spot.shown.down) { to in
                        jump(proxy, to: to)
                    }
                    .padding(.trailing, 16).padding(.bottom, jumps)
                    .transition(.opacity)
                }
            }
            .animation(.easeOut(duration: 0.18), value: spot.shown)
            .task(id: focus) {
                guard let id = focus else { return }
                // a window or page that just came up is not laid out yet; scrolling before that does nothing
                try? await Task.sleep(for: .milliseconds(120))
                proxy.scrollTo(id, anchor: .top)
                focus = nil
            }
        }
    }

    /// Straight there, without sliding: sliding past a long lazy transcript draws every turn on the way. The lazy
    /// turns guess their heights until drawn, so the bottom is asked for again once the last ones are in.
    private func jump(_ proxy: ScrollViewProxy, to id: String) {
        proxy.scrollTo(id, anchor: id == PageSpot.top ? .top : .bottom)
        guard id == PageSpot.bottom else { return }
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(60))
            proxy.scrollTo(id, anchor: .bottom)
        }
    }
}

/// The row over the page on macOS 26, as on the iPhone (`EdgeBar`): no line and no paper of its own; the page runs on
/// under it and fades out toward the top. The system's soft edge alone was too faint to see on the Mac, so the page
/// is also faded by hand: gone at the window's top, whole again at the row's lower edge, so a page at rest (it starts
/// below the row) is never dimmed. On the scroll view itself, so the soft edge knows its bar. Before 26 the row
/// stands above the page.
private struct SoftTopBar<Bar: View>: ViewModifier {
    let bar: Bar
    /// the row's height (`MeetingPage.toolbar`)
    static var rowHeight: CGFloat { 52 }

    func body(content: Content) -> some View {
        if #available(macOS 26.0, *) {
            content
                .mask {
                    VStack(spacing: 0) {
                        LinearGradient(stops: [.init(color: .clear, location: 0),
                                               .init(color: .clear, location: 0.3),
                                               .init(color: .black, location: 1)],
                                       startPoint: .top, endPoint: .bottom)
                            .frame(height: Self.rowHeight)
                        Rectangle()
                    }
                    .ignoresSafeArea()
                }
                .safeAreaBar(edge: .top, spacing: 0) { bar }
                .scrollEdgeEffectStyle(.soft, for: .top)
        } else {
            content
        }
    }
}

/// Where the page is scrolled, as far as the jump buttons care.
@Observable
@MainActor
final class PageSpot {
    static let top = "page top"
    static let bottom = "page bottom"

    struct Shown: Equatable {
        /// More than two screens of page: shorter ones need no buttons.
        var long = false
        /// Far enough from the top for 맨 위로 to mean something; the same for the bottom.
        var up = false
        var down = false

        init() {}

        init(content f: CGRect, viewport: CGFloat) {
            guard viewport > 0 else { return }
            let slack = viewport / 2
            long = f.height > viewport * 2
            up = f.minY < -slack
            down = f.maxY > viewport + slack
        }
    }

    var shown = Shown()
}

/// Reports the page's frame against the visible part of its scroll view (minY 0 at the top, negative once scrolled) and
/// that visible height, whenever the page scrolls, grows or the window resizes.
private struct ScrollWatcher: NSViewRepresentable {
    let report: (CGRect, CGFloat) -> Void

    func makeNSView(context: Context) -> WatchView { WatchView() }
    func updateNSView(_ view: WatchView, context: Context) { view.report = report }

    final class WatchView: NSView {
        var report: ((CGRect, CGFloat) -> Void)?
        private weak var watched: NSScrollView?

        override func hitTest(_ point: NSPoint) -> NSView? { nil }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            NotificationCenter.default.removeObserver(self)
            guard window != nil, let scroll = enclosingScrollView else { return }
            watched = scroll
            scroll.contentView.postsBoundsChangedNotifications = true
            scroll.contentView.postsFrameChangedNotifications = true
            for (name, object) in [(NSView.boundsDidChangeNotification, scroll.contentView as NSView),
                                   (NSView.frameDidChangeNotification, scroll.contentView as NSView)] {
                NotificationCenter.default.addObserver(self, selector: #selector(changed), name: name, object: object)
            }
            changed()
        }

        override func setFrameSize(_ size: NSSize) {
            super.setFrameSize(size)
            changed()
        }

        @objc private func changed() {
            guard let scroll = watched else { return }
            let clip = scroll.contentView
            let viewport = clip.bounds.height
            // this view fills the page, so its frame in the clip's visible rect is the page's frame against the viewport
            let inClip = convert(bounds, to: clip)
            let top = clip.isFlipped ? inClip.minY - clip.bounds.minY : clip.bounds.maxY - inClip.maxY
            let f = CGRect(x: inClip.minX, y: top, width: inClip.width, height: inClip.height)
            let send = report
            DispatchQueue.main.async { send?(f, viewport) }
        }
    }
}

/// 맨 위로 and 맨 아래로 in a small floating pill, only the ones with somewhere to go: at the top only 맨 아래로, at the
/// bottom only 맨 위로, both in between.
struct JumpButtons: View {
    let up: Bool
    let down: Bool
    let go: (String) -> Void

    var body: some View {
        VStack(spacing: 0) {
            if up { button("chevron.up", help: String(localized: "맨 위로")) { go(PageSpot.top) } }
            if up && down { Rectangle().fill(Theme.ink.opacity(0.08)).frame(width: 16, height: 1) }
            if down { button("chevron.down", help: String(localized: "맨 아래로")) { go(PageSpot.bottom) } }
        }
        .background { Capsule().fill(Theme.raised).shadow(color: Theme.ink.opacity(0.08), radius: 11, y: 6) }
        .overlay(Capsule().strokeBorder(Theme.ink.opacity(0.1)))
        .opacity(up || down ? 1 : 0)
    }

    private func button(_ symbol: String, help: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol).uiFont(12, weight: .semibold).foregroundStyle(Theme.ink2)
                .frame(width: 32, height: 32)
                .contentShape(Rectangle())
        }
        .buttonStyle(PressStyle())
        .help(help)
    }
}
