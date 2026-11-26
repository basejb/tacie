import AppKit
import SwiftUI

/// The transcript under the pill during a meeting (docs/04 3-1, 2026-11-13): the meeting's lines as bubbles by speaker,
/// the sentence being said growing at the bottom, and each heard question's answer right under it. Answers come here
/// instead of a card under the pill while it is open. At the bottom it follows new lines; scrolled up by the reader it
/// stays exactly where they are and shows a button that goes back down (with the count of answers that came meanwhile).
/// The panel keeps one height while open, so lines arriving never resize the island.
struct IslandTranscript: View {
    let model: MeetingModel
    var onOpenMain: () -> Void

    /// Following the newest line. Only the reader scrolling up turns it off; reaching the bottom turns it back on.
    @State private var following = true
    /// How far the bottom is below what the reader sees (macOS 15+), for when the down button shows.
    @State private var below: CGFloat = 0
    @State private var unseen: Set<String> = []
    /// When the reader last turned the wheel or dragged on the trackpad here: only that stops following.
    @State private var wheel = Wheel()
    @Environment(\.reducesMotion) private var reduceMotion

    private static let width: CGFloat = 460
    private static let height: CGFloat = 400

    private enum Item: Identifiable {
        case line(TranscriptLine)
        case typed(MeetingModel.Block)

        var id: String {
            switch self {
            case .line(let l): "l" + l.key
            case .typed(let b): "t" + b.id
            }
        }
        var at: Date {
            switch self {
            case .line(let l): l.at
            case .typed(let b): b.at
            }
        }
    }

    /// The lines and the questions typed during the meeting, in the order they came.
    private var items: [Item] {
        let typed = model.blocks.filter { $0.origin == .typed && model.meeting(of: $0.id) == model.liveID }
        var lines = model.liveTranscript
        // once speakers are known, a line without one is a backchannel ("네.") nobody could be put on: left out here
        if lines.contains(where: { $0.voice != nil }) {
            lines.removeAll { $0.voice == nil && $0.name == nil && !(model.mode == .remote && $0.speaker == "me") }
        }
        return (lines.map(Item.line) + typed.map(Item.typed)).sorted { $0.at < $1.at }
    }

    /// The heard question each line holds, by its line.
    private var asked: [String: MeetingModel.Block] {
        var out: [String: MeetingModel.Block] = [:]
        for b in model.blocks { if let k = b.lineKey, out[k] == nil { out[k] = b } }
        return out
    }

    /// Answers that have arrived, for counting the new ones while scrolled up.
    private var answered: Set<String> {
        Set(model.blocks.filter { $0.phase != .finding && model.meeting(of: $0.id) == model.liveID }.map(\.id))
    }

    var body: some View {
        VStack(spacing: 0) {
            feed
            Rectangle().fill(Theme.hairline).frame(height: 1)
            HStack {
                Spacer()
                Button("메인 창에서 보기", action: onOpenMain)
                    .buttonStyle(.plain)
                    .font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.ink2)
            }
            .padding(.horizontal, 18).padding(.vertical, 10)
        }
        .frame(width: Self.width)
        .onChange(of: answered) { old, new in
            guard !following else { return }
            withAnimation(Theme.anim(Theme.settle, reduce: reduceMotion)) { unseen.formUnion(new.subtracting(old)) }
        }
        .onChange(of: following) { _, on in
            if on { withAnimation(Theme.anim(Theme.fast, reduce: reduceMotion)) { unseen = [] } }
        }
    }

    private var feed: some View {
        ScrollViewReader { proxy in
          ZStack(alignment: .bottom) {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    let asked = asked
                    let list = items
                    if list.isEmpty && model.liveLine == nil {
                        Text("아직 들은 말이 없어요.").font(.system(size: 12.5)).foregroundStyle(Theme.ink3)
                            .frame(maxWidth: .infinity, alignment: .center).padding(.vertical, 14)
                    }
                    let names = list.map(name(of:))
                    ForEach(Array(list.enumerated()), id: \.element.id) { i, item in
                        // the name once per turn: the same speaker again right after keeps the bubble only
                        let again = i > 0 && names[i - 1] == names[i] && !names[i].isEmpty
                        switch item {
                        case .line(let l):
                            let who = model.speaker(of: l)
                            Bubble(name: again ? "" : who.name, color: who.color, me: who.me, text: l.text)
                                .padding(.top, again ? -6 : 0)
                            if let b = asked[l.key] { answer(b) }
                        case .typed(let b):
                            Bubble(name: again ? "" : String(localized: "나"), color: nil, me: true, text: b.question)
                                .padding(.top, again ? -6 : 0)
                            answer(b)
                        }
                    }
                    // always there while the meeting runs, empty between sentences: a slot that came and went with
                    // each sentence moved the whole chat down and back up (2026-11-13)
                    LiveBubble(line: model.liveLine ?? LiveLine(said: "", partial: ""))
                    // macOS 14 has no scroll geometry: there the bottom being on screen is "following"
                    Color.clear.frame(height: 1).id("end")
                        .onAppear { if Self.legacy { following = true } }
                        .onDisappear { if Self.legacy { following = false } }
                }
                .padding(.horizontal, 16).padding(.top, 16).padding(.bottom, 12)
                // a short meeting sits at the bottom, next to where new lines come in
                .frame(minHeight: Self.height, alignment: .bottom)
                // lines come in as they are: a fade on each new line, while the sentence it was made from left, blinked
                .transaction { $0.animation = nil }
            }
            .scrollIndicators(.never)
            .frame(height: Self.height)
            .mask(alignment: .top) {
                VStack(spacing: 0) {
                    LinearGradient(colors: [.clear, .black], startPoint: .top, endPoint: .bottom).frame(height: 32)
                    Rectangle()
                }
            }
            .modifier(Follow(following: $following, below: $below, wheel: wheel))
            .onAppear { wheel.start() }
            .onDisappear { wheel.stop() }
            .onAppear { proxy.scrollTo("end", anchor: .bottom) }
            // macOS 15+ keeps the bottom pinned while following (Follow); a change it did not pin (or macOS 14) scrolls down
            .onChange(of: items.count) { if following, Self.legacy || below > 2 { follow(proxy) } }
            .onChange(of: model.liveLine) { if following, Self.legacy || below > 2 { follow(proxy) } }
            .onChange(of: answered) { if following, Self.legacy || below > 2 { follow(proxy) } }
            // about a line and a half below the view is enough to show the way back down (2026-11-13)
            if !following, Self.legacy || below > 64 || !unseen.isEmpty {
                DownButton(count: unseen.count) {
                    following = true
                    follow(proxy, animated: true)
                }
                .padding(.bottom, 10)
                .transition(reduceMotion ? .opacity : .opacity.combined(with: .offset(y: 6)))
            }
          }
        }
    }

    private static var legacy: Bool {
        if #available(macOS 15.0, *) { false } else { true }
    }

    private func name(of item: Item) -> String {
        switch item {
        case .line(let l): model.speaker(of: l).name
        case .typed: String(localized: "나")
        }
    }

    private func follow(_ proxy: ScrollViewProxy, animated: Bool = false) {
        if animated, !reduceMotion {
            withAnimation(Theme.settle) { proxy.scrollTo("end", anchor: .bottom) }
        } else {
            proxy.scrollTo("end", anchor: .bottom)
        }
    }

    /// The answer under its question. Until it is whole (found, its line written, its sources in) it is one quiet
    /// "찾는 중" line; then it comes in at once and nothing inside it animates. Growing in steps (searching lines, the
    /// line streaming in, the sources one by one) moved the whole chat each time (2026-11-13).
    @ViewBuilder private func answer(_ b: MeetingModel.Block) -> some View {
        if Self.whole(b) {
            AnswerCard(block: b, mode: model.mode, large: false, onConnect: { model.connectFromHint() }, inline: true)
                // the outline under the content: a source's preview floats over the box edge, not under it
                .background {
                    RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill)
                    RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline)
                }
                .transaction { $0.animation = nil }
                .padding(.top, -4)
                // a source's preview floats down over the next lines: the answer draws above them
                .zIndex(1)
        } else {
            Text("찾는 중").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.ink3)
                .frame(height: 18)
                .padding(.top, -6)
        }
    }

    private static func whole(_ b: MeetingModel.Block) -> Bool {
        switch b.phase {
        case .finding: false
        case .failed: true
        case .found: b.answerDone || b.cards.isEmpty || b.noSource || b.waitingSync != nil || b.fallback
        }
    }

}

/// Floating at the bottom while the reader is scrolled up: a round "⌄", or "새 답 2개 ⌄" when answers came meanwhile.
/// Pressing it goes back down and follows again.
private struct DownButton: View {
    let count: Int
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                if count > 0 { Text("새 답 \(count)개").font(.system(size: 12, weight: .semibold)) }
                Image(systemName: "chevron.down").font(.system(size: 10, weight: .bold))
            }
            .foregroundStyle(Theme.onStrong)
            .padding(.horizontal, count > 0 ? 12 : 0)
            .frame(minWidth: 28, minHeight: 28)
            .background(Theme.ink, in: Capsule())
            .softShadow()
        }
        .buttonStyle(.plain)
        .help("맨 아래로")
    }
}

/// Who moved the scroll. Content growing never stops following; the reader moving up away from the bottom does, and
/// coming back near the bottom starts it again. While following, the bottom stays pinned as lines grow (no jumps from
/// scrolling once per word); while not, nothing moves under the reader.
/// Scroll-wheel and trackpad events on this app, kept as the time of the last one. Content moving never counts.
@MainActor final class Wheel {
    private(set) var last = Date.distantPast
    private var monitor: Any?

    var recent: Bool { Date().timeIntervalSince(last) < 0.6 }

    func start() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] e in
            if e.deltaY != 0 || e.scrollingDeltaY != 0 { self?.last = Date() }
            return e
        }
    }

    func stop() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
    }
}

private struct Follow: ViewModifier {
    @Binding var following: Bool
    @Binding var below: CGFloat
    let wheel: Wheel

    private struct Spot: Equatable {
        var offset: CGFloat
        var content: CGFloat
        var container: CGFloat
    }

    func body(content: Content) -> some View {
        if #available(macOS 15.0, *) {
            content
                .defaultScrollAnchor(following ? .bottom : nil, for: .sizeChanges)
                .onScrollGeometryChange(for: Spot.self) { g in
                    Spot(offset: g.contentOffset.y, content: g.contentSize.height, container: g.containerSize.height)
                } action: { old, new in
                    let was = max(0, old.content - (old.offset + old.container))
                    below = max(0, new.content - (new.offset + new.container))
                    if below > was + 0.5, below > 8, wheel.recent {
                        // the reader turned the wheel and the bottom went further away. Lines arriving never stop it:
                        // left alone it keeps following the newest
                        if following { following = false }
                    } else if below <= 24, !following, wheel.recent {
                        // the reader came back down
                        following = true
                    }
                }

        } else {
            content
        }
    }
}

/// One finished line: who said it on top (in the speaker's colour, "나" on the right), the words in a grey bubble.
private struct Bubble: View {
    let name: String
    let color: Int?
    let me: Bool
    let text: String

    var body: some View {
        VStack(alignment: me ? .trailing : .leading, spacing: 4) {
            if !name.isEmpty {
                Text(name).font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(color.map { AnyShapeStyle(Theme.speaker($0)) } ?? AnyShapeStyle(Theme.ink3))
            }
            Text(text).font(.system(size: 13.5, weight: .medium)).foregroundStyle(Theme.ink)
                .lineSpacing(2)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
                .padding(.horizontal, 12).padding(.vertical, 8)
                .background(me ? Theme.fillHover : Theme.fill, in: RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous))
        }
        .frame(maxWidth: 400, alignment: me ? .trailing : .leading)
        .frame(maxWidth: .infinity, alignment: me ? .trailing : .leading)
    }
}

/// The sentence being said: no name yet (it comes with the line), the final words as dark as a line and the words Soniox
/// may still change in the grey of small text (lighter read too faint over the glass, 2026-11-13). Words do not pop in
/// a few at a time: they are typed out quickly toward what was heard (`shown` catches up in about a quarter second);
/// a word Soniox takes back is dropped at once.
private struct LiveBubble: View {
    let line: LiveLine
    @State private var shown = ""
    @State private var typing: Task<Void, Never>?
    @Environment(\.reducesMotion) private var reduceMotion

    private var final: String { line.final }
    private var full: String {
        final + (final.isEmpty ? String(line.partial.drop { $0 == " " }) : line.partial)
    }

    var body: some View {
        let finalCount = min(final.count, shown.count)
        let empty = shown.isEmpty
        (empty ? Text(" ") : Text(String(shown.prefix(finalCount))).foregroundStyle(Theme.ink)
            + Text(String(shown.dropFirst(finalCount))).foregroundStyle(Theme.ink3))
            .font(.system(size: 13.5, weight: .medium))
            .lineSpacing(2)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(Theme.fill, in: RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous))
            .opacity(empty ? 0 : 1)
            .frame(maxWidth: 400, alignment: .leading)
            .onAppear { typeOut() }
            .onChange(of: full) { typeOut() }
            .onDisappear { typing?.cancel() }
    }

    private func typeOut() {
        let target = full
        typing?.cancel()
        // keep what still matches, drop a word that was taken back
        let same = zip(shown, target).prefix { $0 == $1 }.count
        if same < shown.count { shown = String(shown.prefix(same)) }
        guard !reduceMotion else { shown = target; return }
        let rest = Array(target.dropFirst(shown.count))
        guard !rest.isEmpty else { return }
        // the whole gap in about 0.25 s, a few characters per frame when far behind
        let steps = max(1, min(rest.count, 15))
        let per = Int((Double(rest.count) / Double(steps)).rounded(.up))
        typing = Task { @MainActor in
            var i = 0
            while i < rest.count, !Task.isCancelled {
                let next = min(rest.count, i + per)
                shown += String(rest[i..<next])
                i = next
                try? await Task.sleep(for: .milliseconds(16))
            }
        }
    }
}
