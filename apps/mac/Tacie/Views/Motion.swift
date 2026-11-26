import SwiftUI

/// "Copied" turns back into the button's own label after 1.5 s, the button's width following with `snappy` (docs/03 Arc
/// no. 6: the same 1.5 s in every place). The words roll over letter by letter and the icon is replaced in place
/// (`OutlineButton`); with Reduce Motion the words only crossfade.
struct CopiedFlash: ViewModifier {
    @Binding var copied: Bool
    @Environment(\.reducesMotion) private var reduce

    func body(content: Content) -> some View {
        content
            .contentTransition(reduce ? .opacity : .numericText())
            .animation(Theme.anim(Theme.snappy, reduce: reduce), value: copied)
            .task(id: copied) {
                guard copied else { return }
                try? await Task.sleep(for: .seconds(1.5))
                copied = false
            }
    }
}

extension View {
    func copiedFlash(_ copied: Binding<Bool>) -> some View { modifier(CopiedFlash(copied: copied)) }
}

/// A source card's state by case only. Syncing stays one stage while its percent moves, so the card moves when the
/// stage changes and not on every percent.
enum SourceStage: Equatable { case off, waiting, failed, lost, syncing, connected }

/// A 설정 → 연결 card: the stage changes crossfade and the card's height follows with `settle`; when a wait in the
/// browser or the first sync ends connected, `justConnected` holds for a beat so the card can show
/// `DoneMark("연결했어요")` in its menu's place (the settle-to-mark rule).
struct SourceCardMotion: ViewModifier {
    let stage: SourceStage
    @Binding var justConnected: Bool
    @Environment(\.reducesMotion) private var reduce

    func body(content: Content) -> some View {
        content
            .animation(Theme.anim(Theme.settle, reduce: reduce), value: stage)
            .animation(Theme.anim(Theme.settle, reduce: reduce), value: justConnected)
            .onChange(of: stage) { old, new in
                if new == .connected, old == .waiting || old == .syncing { justConnected = true }
            }
            .task(id: justConnected) {
                guard justConnected else { return }
                try? await Task.sleep(for: .seconds(3.2))
                justConnected = false
            }
    }
}

extension View {
    func sourceCardMotion(_ stage: SourceStage, justConnected: Binding<Bool>) -> some View {
        modifier(SourceCardMotion(stage: stage, justConnected: justConnected))
    }
}
