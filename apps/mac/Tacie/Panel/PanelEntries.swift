import SwiftUI

/// The ways into the main window's panel from the pages (docs/36 1-1절): what each one opens and sends.
@MainActor
enum PanelEntries {
    static var prepLabel: String { String(localized: "meeting.prep", defaultValue: "다음 회의 준비") }
    static var whyLabel: String { String(localized: "ledger.why", defaultValue: "이유 보기") }
    /// The pages' entry next to Tacie's face: the face is the "Tacie", the word only "AI" (2026-11-16).
    static var askLabel: String { String(localized: "collected.ask", defaultValue: "AI") }
    static var searchSub: String { String(localized: "search.askSub", defaultValue: "Tacie가 회의와 연결한 서비스에서 찾아 답해요") }

    static var continueLabel: String { String(localized: "island.continue", defaultValue: "메인 창에서 이어서 물어보기") }

    /// The island card's line: only for a finished answer that has a sentence or sources to carry over.
    static func canContinue(answer: String?, cards: [Card]) -> Bool { !(answer ?? "").isEmpty || !cards.isEmpty }

    /// 메인 창에서 이어서 물어보기: the panel opens on every meeting and connected source (docs/36 0-1절: the island
    /// is outside the pages), with the card's question and answer as its first turn, so follow-ups can still reach the
    /// card's own sources. The engine is not asked again.
    static func continueIsland(question: String, answer: String?, cards: [Card], in store: PanelStore) {
        guard canContinue(answer: answer, cards: cards) else { return }
        store.open(scope: .all, via: .island)
        store.seed(question: question, ask: PanelAsk(answer: answer, cited: [], cards: cards, errorKind: nil))
    }

    /// "다음 회의 준비" only for a meeting that has ended: one still going on has nothing to prepare from.
    static func showsPrep(_ meeting: SavedMeeting) -> Bool { PanelFeatures.current.prepButton && meeting.end != nil }

    static func prep(_ meeting: SavedMeeting, in store: PanelStore) {
        guard showsPrep(meeting) else { return }
        store.open(scope: .meeting(meeting.id), via: .prep)
        store.suggest(.prep)
    }

    /// "이유 보기" only on decisions.
    static func showsWhy(_ entry: LedgerEntry) -> Bool { PanelFeatures.current.whyLink && entry.kind == "decision" }

    static func why(_ meeting: SavedMeeting, index: Int, in store: PanelStore) {
        guard meeting.ledger.indices.contains(index), showsWhy(meeting.ledger[index]) else { return }
        store.open(scope: .meeting(meeting.id), via: .why)
        store.suggest(.why(meetingID: meeting.id, ledgerIndex: index))
    }

    /// The search field's "Tacie에게 묻기" row; off, search only filters the meeting list.
    static var showsSearchAsk: Bool { PanelFeatures.current.searchAsk }

    /// The search field's top row: what was typed goes to every meeting and connected source.
    static func searchAsk(_ query: String, in store: PanelStore) {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty else { return }
        store.open(scope: .all, via: .search)
        store.newThread()
        store.send(q, kind: .ask)
    }

    /// Return in the sidebar search: what to ask, or nil for a blank field. Return did nothing there before the panel,
    /// so the first press asks.
    static func searchReturn(_ query: String) -> String? {
        guard showsSearchAsk else { return nil }
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        return q.isEmpty ? nil : q
    }

    /// "Tacie AI" on a page: the panel on the page's meeting (every meeting on the collected pages and on a meeting
    /// still going on, `PanelStore.pageScope`), nothing sent; a conversation that started there stays.
    static func ask(meetingID: String?, in store: PanelStore) {
        store.open(scope: store.pageScope(for: meetingID), via: meetingID == nil ? .collected : .page, startHere: true)
    }
}

/// The meeting page's "다음 회의 준비": the page's one black button, grey words while the panel is open.
struct PrepEntryButton: View {
    @ObservedObject var store: PanelStore
    let meeting: SavedMeeting
    @State private var hover = false

    var body: some View {
        let strong = PanelInk.pageButtonStrong(panelOpen: store.isOpen)
        Button { PanelEntries.prep(meeting, in: store) } label: {
            if strong {
                StrongButton(label: PanelEntries.prepLabel)
            } else {
                Text(PanelEntries.prepLabel).uiFont(12.5, weight: .medium)
                    .foregroundStyle(hover ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3))
                    .padding(.horizontal, 10).frame(height: 28)
                    .contentShape(Rectangle())
            }
        }
        .buttonStyle(PressStyle())
        .onHover { hover = $0 }
    }
}

/// "(얼굴) Tacie AI" (⌘J under the pointer) at the top right of a meeting page (on that meeting) and of the collected pages (on
/// every meeting): grey words with the panel's small face (docs/36 0-1절).
struct AskEntryButton: View {
    let store: PanelStore
    var meetingID: String? = nil
    @State private var hover = false
    @State private var tip = false
    @Environment(\.reducesMotion) private var reduce

    var body: some View {
        Button { PanelEntries.ask(meetingID: meetingID, in: store) } label: {
            HStack(spacing: 6) {
                // under the pointer the eyes light up a step and the face grows a little, as if it looked up (2026-11-16)
                PanelFace(size: .mini, glow: hover ? 3 : 0)
                    .scaleEffect(hover && !reduce ? 1.2 : 1)
                    .animation(reduce ? Theme.reduced : .spring(response: 0.32, dampingFraction: 0.62), value: hover)
                Text(PanelEntries.askLabel).uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink)
            }
            .animation(Theme.fast, value: hover)
            // a white pill (2026-11-16): a button, not loose words in the top row
            .padding(.leading, 8).padding(.trailing, 12).frame(height: 28)
            .background(Capsule().fill(Theme.raised).shadow(color: .black.opacity(hover ? 0.12 : 0.07), radius: hover ? 4 : 2, y: 1))
            .overlay(Capsule().strokeBorder(Theme.hairline))
            .contentShape(Capsule())
        }
        .buttonStyle(PressStyle())
        .onHover { hover = $0 }
        // the shortcut in a small tip under the button after a short rest of the pointer: no empty room beside the label
        .overlay(alignment: .top) {
            if tip {
                ShortcutTip(keys: "⌘J")
                    .fixedSize()
                    .offset(y: 34)
                    .transition(reduce ? .opacity
                                : .asymmetric(insertion: .opacity.combined(with: .scale(scale: 0.94, anchor: .top)).combined(with: .offset(y: -3)),
                                              removal: .opacity.animation(Theme.exit)))
                    .allowsHitTesting(false)
            }
        }
        .zIndex(tip ? 1 : 0)
        .task(id: hover) {
            guard hover else { withAnimation(Theme.exit) { tip = false }; return }
            try? await Task.sleep(for: .milliseconds(450))
            guard !Task.isCancelled, hover else { return }
            withAnimation(Theme.anim(.spring(response: 0.28, dampingFraction: 0.8), reduce: reduce)) { tip = true }
        }
        // the face says "Tacie" on screen, not to VoiceOver
        .accessibilityLabel(Text(verbatim: "Tacie AI"))
        .accessibilityHint(Text(verbatim: "⌘J"))
    }
}

/// A key or two in a small raised capsule, the app's own tooltip for a shortcut.
struct ShortcutTip: View {
    let keys: String
    var body: some View {
        Text(verbatim: keys).uiFont(11.5, weight: .medium, digits: true).foregroundStyle(Theme.ink2)
            .padding(.horizontal, 8).frame(height: 22)
            .background(Capsule().fill(Theme.raised))
            .overlay(Capsule().strokeBorder(Theme.hairline))
            .shadow(color: .black.opacity(0.12), radius: 8, y: 3)
    }
}

/// "이유 보기" at the end of a decision row: faint, always shown.
struct WhyEntryLink: View {
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            Text(PanelEntries.whyLabel).uiFont(12, weight: .medium)
                .foregroundStyle(hover ? AnyShapeStyle(Theme.ink2) : AnyShapeStyle(Theme.ink3))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }
}

/// The sidebar search's top row while something is typed: the text as typed and what the panel will do with it.
struct SearchAskRow: View {
    let query: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 9) {
                LitHead(channels: EyeRig.target(for: .listening, remote: false), height: 17)
                VStack(alignment: .leading, spacing: 1) {
                    Text(query.trimmingCharacters(in: .whitespacesAndNewlines)).uiFont(13, weight: .semibold)
                        .foregroundStyle(Theme.ink).lineLimit(1)
                    Text(PanelEntries.searchSub).uiFont(11.5).foregroundStyle(Theme.ink3).lineLimit(1)
                }
                Spacer(minLength: 0)
                // Return in the field sends this row
                Image(systemName: "return").uiFont(10.5, weight: .medium).foregroundStyle(Theme.ink3)
            }
            .padding(.horizontal, 10).padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.fill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.bottom, 4)
    }
}
