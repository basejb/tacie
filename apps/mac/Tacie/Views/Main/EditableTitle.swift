import AppKit
import SwiftUI

/// The meeting's title, edited where it stands: the pointer on it shows a light fill and a pen, a click turns it into a field in the same type and place, with no box. Return or a click elsewhere keeps the change, Esc drops it.
struct EditableTitle: View {
    let text: String
    var grey = false
    var editable = true
    @Binding var editing: Bool
    let commit: (String) -> Void

    @State private var draft = ""
    @State private var hover = false
    /// The field's frame in the window (top-left origin), to tell a click outside it.
    @State private var fieldFrame = CGRect.zero
    @State private var clickWatch: Any?
    @FocusState private var focused: Bool
    @Environment(\.reducesMotion) private var reduceMotion
    /// A copy made only for measuring (the 대화 기록 header): the still title, never a second field or click monitor.
    @Environment(\.layoutProbe) private var probe

    var body: some View {
        Group {
            if editing && !probe {
                TextField("", text: $draft, prompt: Text("회의 제목").foregroundStyle(Theme.ink4))
                    .textFieldStyle(.plain)
                    .uiFont(30, weight: .bold).retinaTracking(-0.6)
                    .foregroundStyle(Theme.ink)
                    .focused($focused)
                    .onSubmit { finish(keep: true) }
                    .onExitCommand { finish(keep: false) }
                    .onChange(of: focused) { if !focused && editing { finish(keep: true) } }
                    .onAppear {
                        draft = text
                        focused = true
                        watchClicks()
                    }
                    .onDisappear(perform: stopWatching)
                    .background(GeometryReader { g in
                        Color.clear.onAppear { fieldFrame = g.frame(in: .global) }
                            .onChange(of: g.frame(in: .global)) { _, f in fieldFrame = f }
                    })
                    // no box: the caret and the selection say it is being edited; same padding as the title, so nothing moves
                    .padding(.horizontal, 8).padding(.vertical, 2)
            } else {
                HStack(alignment: .center, spacing: 10) {
                    Text(text)
                        .uiFont(30, weight: .bold).retinaTracking(-0.6)
                        .foregroundStyle(grey ? Theme.ink.opacity(0.35) : Theme.ink)
                        .fixedSize(horizontal: false, vertical: true)
                    if editable {
                        Image(systemName: "pencil").uiFont(15, weight: .semibold).foregroundStyle(Theme.ink3)
                            .opacity(hover ? 1 : 0)
                            .offset(x: hover || reduceMotion ? 0 : -3)
                    }
                }
                .padding(.horizontal, 8).padding(.vertical, 2)
                .background(Theme.ink.opacity(editable && hover ? 0.04 : 0), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .contentShape(Rectangle())
                .onHover { inside in
                    withAnimation(Theme.anim(.easeOut(duration: inside ? 0.14 : 0.2), reduce: reduceMotion)) { hover = inside }
                }
                .onTapGesture { if editable { editing = true } }
                .help(editable ? String(localized: "눌러서 제목을 바꿔요") : "")
            }
        }
        // the fill and field reach 8 pt past the words, so the words stay in the page's column
        .padding(.horizontal, -8).padding(.vertical, -2)
    }

    /// A click on the page outside the field keeps the change: SwiftUI leaves the focus in the field when the click
    /// lands on something that takes no focus (the page's plain background), so it would never end on its own.
    private func watchClicks() {
        stopWatching()
        clickWatch = NSEvent.addLocalMonitorForEvents(matching: .leftMouseDown) { e in
            if let h = e.window?.contentView?.bounds.height {
                let p = CGPoint(x: e.locationInWindow.x, y: h - e.locationInWindow.y)
                if !fieldFrame.insetBy(dx: -8, dy: -8).contains(p) {
                    DispatchQueue.main.async { if editing { finish(keep: true) } }
                }
            }
            return e
        }
    }

    private func stopWatching() {
        if let w = clickWatch { NSEvent.removeMonitor(w) }
        clickWatch = nil
    }

    private func finish(keep: Bool) {
        if keep, draft.trimmingCharacters(in: .whitespacesAndNewlines) != text { commit(draft) }
        editing = false
        hover = false
    }
}
