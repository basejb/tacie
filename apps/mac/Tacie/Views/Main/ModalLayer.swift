import SwiftUI

/// A modal over the main window (settings, the sign-in sheet, What's new) that stays in the view tree while it fades
/// out, and takes no click from the moment it closes.
///
/// A view removed with a transition stays on screen until the transition ends, and it keeps taking clicks the whole
/// time: hit testing set on it, or on anything above it, no longer reaches it once it is removed. Its see-through scrim
/// then swallowed every click on the window, now and then for seconds after settings closed with Esc (2026-11-12, after
/// a language change). Here the layer itself is never removed: it fades by its own opacity with hits off, and the
/// content goes once it is invisible.
struct ModalLayer<Item: Equatable, Content: View>: View {
    /// The modal to show; nil closes it.
    let item: Item?
    @ViewBuilder let content: (Item) -> Content
    /// The last item, drawn while the layer fades out.
    @State private var kept: Item?

    var body: some View {
        let open = item != nil
        ZStack {
            if let shown = item ?? kept { content(shown) }
        }
        .opacity(open ? 1 : 0)
        .allowsHitTesting(open)
        .task(id: open) {
            guard !open else { return }
            // the fade is `Theme.exit` (or the shorter reduced one); the content goes after it, without a transition
            try? await Task.sleep(for: .seconds(Theme.exitSeconds + 0.1))
            guard !Task.isCancelled else { return }
            kept = nil
        }
        .onChange(of: item, initial: true) { _, new in
            if let new { kept = new }
        }
    }
}
