import SwiftUI

/// The answer under the island: one sentence big, the files it came from as quiet numbered chips.
/// Why it appeared (who asked what) sits in the pill above, so the card starts with the answer.
struct AnswerCard: View {
    let block: MeetingModel.Block
    let mode: MeetingMode?
    let large: Bool
    var onClose: () -> Void = {}
    var onLarge: () -> Void = {}
    var onConnect: () -> Void = {}
    /// 메인 창에서 이어서 물어보기: nil where the card has no such line (대화 기록 on the island).
    var onContinue: (() -> Void)? = nil
    /// Under its question in 대화 기록 on the island: no close button, no width of its own, a smaller answer line.
    var inline = false

    @State private var copied = false
    @Environment(\.reducesMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: inline ? 10 : large ? 22 : 16) {
            answer
            if block.phase == .found, !block.cards.isEmpty {
                SourceChips(cards: block.cards, real: FirstSteps.realAnswer(practice: block.practice, sample: block.sample, onboarding: block.onboarding), near: !block.shown || block.fallback || block.saidNoAnswer, large: large,
                            quiet: block.isQuiet, compact: inline)
            }
            if block.firstCardHint, block.phase == .found, !block.cards.isEmpty {
                Text("눌러서 근거를 열어 보세요")
                    .font(.system(size: large ? 14 : 12)).foregroundStyle(Theme.ink3)
            }
            if block.connectHint, block.phase == .found {
                VStack(alignment: .leading, spacing: large ? 14 : 10) {
                    Text("아직 지난 회의에서만 찾아요. 팀 자료를 연결하면 이 질문도 다시 찾아요.")
                        .font(.system(size: large ? 14 : 12)).foregroundStyle(Theme.ink3)
                        .fixedSize(horizontal: false, vertical: true)
                    Button("팀 자료 연결하기", action: onConnect).buttonStyle(InkButtonStyle())
                }
            }
            if block.sample, block.phase == .found {
                Text("예시 자료에서 찾은 답이에요. 서비스를 연결하면 팀 코드와 문서에서 찾아요.")
                    .font(.system(size: large ? 14 : 12)).foregroundStyle(Theme.ink3)
            }
            if footerKind != nil || showsContinue {
                HStack {
                    if showsContinue, let onContinue {
                        ContinueLink(large: large, action: onContinue)
                    }
                    Spacer()
                    if let footer = footerKind { footerButton(footer) }
                }
            }
        }
        .padding(.horizontal, inline ? 14 : large ? 28 : 20)
        .padding(.top, inline ? 12 : large ? 26 : 18)
        .padding(.bottom, inline ? 12 : footerKind == nil && !showsContinue ? (large ? 26 : 18) : 14)
        .frame(width: inline ? nil : large ? Theme.cardWidthLarge : Theme.cardWidth, alignment: .leading)
        .frame(maxWidth: inline ? .infinity : nil, alignment: .leading)
        .overlay(alignment: .topTrailing) {
            if !inline {
                Button(action: onClose) {
                    Image(systemName: "xmark").font(.system(size: 9.5, weight: .bold)).foregroundStyle(Theme.ink3)
                        .frame(width: 22, height: 22).background(Theme.fill, in: Circle())
                }
                .buttonStyle(.plain)
                .padding(12)
                .help("닫기 (esc)")
            }
        }
    }

    @ViewBuilder private var answer: some View {
        switch block.phase {
        case .finding:
            Thinking(large: large)
        case .failed(let msg):
            Note(text: String(localized: "찾다가 멈췄어요. \(msg)"), large: large)
        case .found:
            if let sync = block.waitingSync {
                Note(text: String(localized: "아직 처음 동기화 중이에요(\(sync)). 질문은 적어 두고, 끝나면 찾아 드릴게요."), large: large)
            } else if block.noSource {
                Note(text: String(localized: "연결한 서비스가 없어서 찾지 않았어요. 질문은 적어 두고, 연결하면 다시 찾아 드릴게요."), large: large)
                if block.firstNoSource {
                    HStack(spacing: 5) {
                        Text("서비스는 메인 창")
                        Keycap(text: "⌃⌥⌘T")
                        Text("의 설정 → 연결에서 연결해요.")
                    }
                    .font(.system(size: large ? 15 : 12.5)).foregroundStyle(Theme.ink3)
                }
            } else if block.cards.isEmpty {
                Note(text: String(localized: "찾은 곳이 없어요. 회의 정리에 문서로 남길 질문으로 적어 둘게요."), large: large)
            } else if block.fallback {
                Note(text: (block.trouble.map { $0 + ". " } ?? "") + String(localized: "순위를 다시 매기지 못해서 비슷한 순서대로 보여 드려요."), large: large)
            } else if !block.shown {
                Note(text: String(localized: "확실한 곳은 못 찾았어요. 가장 가까운 곳이에요."), large: large)
            } else if block.saidNoAnswer {
                Note(text: String(localized: "카드에서 답을 찾지는 못했어요. 가장 가까운 곳이에요."), large: large)
            } else if let a = block.answer {
                Text(Citations.styled(a, size: inline ? 14 : large ? 26 : 16))
                    .font(inline ? .system(size: 14, weight: .medium) : large ? Theme.answerLarge : Theme.answer)
                    .foregroundStyle(Theme.ink)
                    .lineSpacing(large ? 6 : 3)
                    .tracking(-0.2)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                    .padding(.trailing, inline ? 0 : 24)
                    .transition(reduceMotion ? .opacity : .blurIn)
            } else if !block.answerDone {
                Thinking(large: large, lines: 1)
            } else {
                Note(text: String(localized: "가장 관련 있는 파일이에요."), large: large)
            }
        }
    }

    private var showsContinue: Bool {
        guard !inline, onContinue != nil, block.phase == .found, block.answerDone, !block.noSource, block.waitingSync == nil else { return false }
        return PanelEntries.canContinue(answer: block.answer, cards: block.cards)
    }

    private enum Footer { case large, slack }

    /// In a room the laptop gets turned around, so answers can grow; Slack answers go back to Slack.
    private var footerKind: Footer? {
        guard !inline, block.phase == .found, !block.cards.isEmpty else { return nil }
        switch block.origin {
        case .voice where mode == .inPerson: return .large
        case .slack: return .slack
        default: return nil
        }
    }

    @ViewBuilder private func footerButton(_ f: Footer) -> some View {
        switch f {
        case .large:
            Button(action: onLarge) {
                Label(large ? "작게 보기" : "크게 보기", systemImage: large ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right")
            }
            .buttonStyle(GhostButtonStyle())
        case .slack:
            Button {
                CardActions.copyForSlack(block)
                copied = true
            } label: {
                Label(copied ? "복사했어요" : "슬랙 답으로 복사", systemImage: copied ? "checkmark" : "doc.on.doc")
            }
            .buttonStyle(InkButtonStyle())
            .copiedFlash($copied)
        }
    }
}

/// "메인 창에서 이어서 물어보기": grey words under the card, darker on hover. Theme inks, since glass flips system colours.
private struct ContinueLink: View {
    let large: Bool
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            Text(PanelEntries.continueLabel).font(.system(size: large ? 14 : 12.5, weight: .medium))
                .foregroundStyle(hover ? Theme.ink : Theme.ink2)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }
}

private struct Note: View {
    let text: String
    let large: Bool
    var body: some View {
        Text(verbatim: text).font(.system(size: large ? 20 : 14.5, weight: .medium)).foregroundStyle(Theme.ink2)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.trailing, 24)
    }
}

/// While searching: soft bars where the sentence will be.
private struct Thinking: View {
    let large: Bool
    var lines = 2
    @State private var on = false
    @Environment(\.reducesMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: large ? 12 : 9) {
            ForEach(0..<lines, id: \.self) { i in
                Capsule().fill(Theme.ink.opacity(on ? 0.09 : 0.045))
                    .frame(width: (large ? 600 : 400) * (i == lines - 1 && lines > 1 ? 0.55 : 0.92), height: large ? 16 : 11)
            }
        }
        .padding(.vertical, 3)
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) { on = true }
        }
    }
}

/// Numbered sources. Hover shows a few lines, click opens the line on GitHub, right-click for more.
/// Chips the answer line does not cite read quieter; numbers stay as they are so [2] still points at chip 2.
private struct SourceChips: View {
    let cards: [Card]
    /// Not the onboarding's practice or sample data: opening one counts toward 처음 써 보기.
    let real: Bool
    let near: Bool
    let large: Bool
    var quiet: (Card) -> Bool = { _ in false }
    /// In 대화 기록: narrower chips so a few share a row.
    var compact = false
    @State private var hovered: String?
    @State private var chipsHeight: CGFloat = 0
    @Environment(\.reducesMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Wrap(spacing: 6) {
                if near {
                    Text("가까운 곳").font(Theme.chip).foregroundStyle(Theme.ink3).frame(height: 28)
                }
                ForEach(Array(cards.enumerated()), id: \.element.id) { i, card in
                    Chip(n: i + 1, card: card, on: hovered == card.id, real: real, maxWidth: compact ? 196 : 300)
                        .opacity(quiet(card) && hovered != card.id ? 0.5 : 1)
                        // the files arrive one after another, 0.035s apart (all at once in 대화 기록: nothing may move it)
                        .transition(compact ? .identity : reduceMotion ? .opacity
                                    : .asymmetric(insertion: .opacity.combined(with: .scale(scale: 0.96)).animation(Theme.grow.delay(Theme.stagger(i))),
                                                  removal: .opacity.animation(Theme.exit)))
                        .onHover { inside in
                            let next = inside ? card.id : (hovered == card.id ? nil : hovered)
                            if compact { hovered = next } else { withAnimation(Theme.fast) { hovered = next } }
                        }
                }
            }
            if !compact, let card = cards.first(where: { $0.id == hovered }) ?? (large ? cards.first(where: { !quiet($0) }) ?? cards.first : nil) {
                // Not under the pointer: a preview sliding in from the top edge covered the chip being hovered,
                // the chip lost its hover, the preview went away, and the card jittered in a loop.
                Preview(card: card, large: large)
                    .allowsHitTesting(false)
                    .transition(reduceMotion ? .opacity : .opacity.combined(with: .offset(y: -6)))
            }
        }
        .onGeometryChange(for: CGFloat.self, of: { $0.size.height }) { chipsHeight = $0 }
        .overlay(alignment: .topLeading) {
            if compact, let card = cards.first(where: { $0.id == hovered }) {
                // in 대화 기록 it floats under the chips, over the lines after it: opening it moves nothing
                Preview(card: card, large: false, floating: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .offset(y: chipsHeight + 6)
                    .allowsHitTesting(false)
                    .transition(reduceMotion ? .opacity : .opacity.combined(with: .offset(y: -4)))
            }
        }
    }
}

private struct Chip: View {
    let n: Int
    let card: Card
    let on: Bool
    let real: Bool
    var maxWidth: CGFloat = 300

    var body: some View {
        HStack(spacing: 6) {
            Text("\(n)").font(.system(size: 10.5, weight: .semibold)).foregroundStyle(Theme.ink2)
                .frame(width: 17, height: 17).background(Theme.paper.opacity(0.9), in: Circle())
            SourceMark(card: card, size: 12)
            Text(card.name).font(Theme.chip).foregroundStyle(Theme.ink).lineLimit(1)
        }
        .padding(.leading, 5).padding(.trailing, 11)
        .frame(height: 28)
        .frame(maxWidth: maxWidth, alignment: .leading)
        .background(on ? Theme.fillHover : Theme.fill, in: Capsule())
        .contentShape(Capsule())
        .opacity(MeetingCards.gone(card) ? 0.5 : 1)
        .onTapGesture { CardActions.open(card, real: real) }
        .contextMenu {
            if !MeetingCards.gone(card) {
                Button("\(card.opensIn)에서 열기") { CardActions.open(card, real: real) }
            }
            if card.inRepo {
                Button("에디터에서 열기") { CardActions.openInEditor(card, real: real) }
                Button("경로 복사") { CardActions.copyPath(card) }
            }
        }
        .help(MeetingCards.gone(card) ? String(localized: "지운 회의라 열 수 없어요")
              : String(localized: "\(card.byline ?? card.path) · 누르면 \(card.opensIn)에서 열어요"))
    }
}

/// A few lines of the file under the chips; code stays monospaced, docs read as text.
private struct Preview: View {
    let card: Card
    let large: Bool
    var floating = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(card.byline ?? "\(card.path):\(card.lineStart)").font(.system(size: 11.5)).foregroundStyle(Theme.ink3)
                .lineLimit(1).truncationMode(.head)
            Text(Snippet.text(for: card, lines: card.isDoc ? 3 : (large ? 8 : 5)))
                .font(card.isDoc ? .system(size: large ? 15 : 13) : .system(size: large ? 13.5 : 11.5, design: .monospaced))
                .foregroundStyle(Theme.ink2)
                .lineSpacing(2)
                .lineLimit(card.isDoc ? 3 : (large ? 8 : 5))
                .frame(maxWidth: .infinity, alignment: .leading)
                .mask(LinearGradient(stops: [.init(color: .black, location: 0.6), .init(color: .black.opacity(0), location: 1)],
                                     startPoint: .top, endPoint: .bottom))
        }
        .padding(12)
        .background(floating ? Theme.paper : Theme.paper.opacity(0.6), in: RoundedRectangle(cornerRadius: Theme.radiusInner))
        .overlay { if floating { RoundedRectangle(cornerRadius: Theme.radiusInner).strokeBorder(Theme.hairline) } }
        .modifier(FloatShadow(on: floating))
    }
}

private struct FloatShadow: ViewModifier {
    let on: Bool
    func body(content: Content) -> some View {
        if on { content.softShadow() } else { content }
    }
}

/// Snippet text for a card: code as is; docs without headings, tables as "a · b", inline markdown rendered.
enum Snippet {
    static func text(for card: Card, lines n: Int) -> AttributedString {
        var lines = card.snippet.components(separatedBy: "\n")
        if card.isDoc {
            let body = lines.filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("#") }
            lines = body.contains { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
                ? body : lines.map { $0.replacingOccurrences(of: #"^\s*#+\s*"#, with: "", options: .regularExpression) }
            lines = lines.compactMap { l in
                let t = l.trimmingCharacters(in: .whitespaces)
                guard t.hasPrefix("|") else { return l }
                if t.allSatisfy({ "|-: ".contains($0) }) { return nil }
                return t.split(separator: "|").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }.joined(separator: " · ")
            }
            lines = lines.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
        }
        while let first = lines.first, first.trimmingCharacters(in: .whitespaces).isEmpty { lines.removeFirst() }
        let text = lines.prefix(n).joined(separator: "\n")
        if card.isDoc, let md = try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)) {
            return md
        }
        return AttributedString(text)
    }
}
