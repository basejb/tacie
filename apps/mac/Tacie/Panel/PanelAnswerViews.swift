import SwiftUI

// MARK: - What an answer shows

/// What the panel shows for one answer, worked out apart from the views so tests can read it (docs/36 1-2, 1-3절).
enum PanelShown {
    /// "다음 회의 준비": the advice (or why it is missing), the groups that have items, and the read line.
    struct Prep: Equatable {
        /// The advice, "아직 정하지 않은 것은 없어요." when there is nothing open, nil when the model wrote none.
        var advice: String?
        /// Why the model's part did not come (`error_kind`); the ledger groups still show.
        var reason: String?
        var groups: [Group]
        /// "회의 n개와 연결한 서비스 n개를 읽고 답했어요", nil when it read nothing.
        var read: String?
        /// Items left out until the person asks for them (past the first `firstLimit` of a group).
        var hidden = 0
        /// Whether the answer has any item at all, shown or not.
        var hasItems = false
    }

    struct Group: Equatable {
        let title: String
        let items: [PanelItem]
    }

    /// How many items a group shows before "나머지 항목 n개 더".
    static let firstLimit = 5

    /// A group's items in the order they show: first up to `firstLimit` of those raised in more than one meeting, then
    /// the prepared meeting's (`recent`) own, each tier in its own order; then the rest, in their own order.
    /// Returns the ordered list and how many lead.
    static func ordered(_ items: [PanelItem], recent: String?) -> (items: [PanelItem], first: Int) {
        let repeated = items.indices.filter { !(items[$0].also ?? []).isEmpty }
        let own = items.indices.filter { items[$0].meetingId == recent && !repeated.contains($0) }
        let lead = Array((repeated + own).prefix(firstLimit))
        let rest = items.indices.filter { !lead.contains($0) }
        return ((lead + rest).map { items[$0] }, lead.count)
    }

    /// First up to five per group, repeated items before the prepared meeting's own; the rest is counted in `hidden`
    /// until `expanded`, when it follows them in place. Group order stays.
    static func prep(_ a: PrepAnswer, recent: String? = nil, expanded: Bool = true,
                     say: (String) -> String = FailureWords.say) -> Prep {
        var groups: [Group] = []
        var hidden = 0
        for (title, items) in [(PanelText.prepOpen, a.open), (PanelText.prepTodos, a.todos)] where !items.isEmpty {
            let (all, first) = ordered(items, recent: recent)
            let kept = expanded ? all : Array(all.prefix(first))
            hidden += items.count - kept.count
            if !kept.isEmpty { groups.append(Group(title: title, items: kept)) }
        }
        let hasItems = !a.open.isEmpty || !a.todos.isEmpty
        let reason = a.errorKind.map { PanelText.reason($0, say: say) }
        let advice: String?
        if !hasItems { advice = PanelText.prepNone }
        else if reason != nil { advice = nil }
        else { advice = a.advice.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : a.advice }
        return Prep(advice: advice, reason: hasItems ? reason : nil, groups: groups,
                    read: PanelText.read(meetings: a.meetingsRead, sources: a.sourcesRead), hidden: hidden, hasItems: hasItems)
    }

    /// The prepared items as Markdown, for Slack or Notion: the advice, then a heading and "- " lines per group.
    static func markdown(_ a: PrepAnswer, meeting: (String) -> SavedMeeting?, calendar: Calendar) -> String {
        let shown = prep(a)
        var out: [String] = []
        if let advice = shown.advice { out.append(Citations.unmarked(advice)) }
        for g in shown.groups {
            out += ["", "### \(g.title)"]
            for item in g.items {
                let sub = itemLine(item, meeting: meeting, calendar: calendar).map(\.text).joined()
                out.append(sub.isEmpty ? "- \(item.text)" : "- \(item.text) (\(sub))")
            }
        }
        return out.joined(separator: "\n")
    }

    /// The small line under an item, in pieces so the overdue part can be red: open questions say the day they came up
    /// (and the other meetings that raised them), to-dos who has them and when they are due.
    struct Piece: Equatable {
        let text: String
        var warn = false
    }

    static func itemLine(_ item: PanelItem, meeting: (String) -> SavedMeeting?, calendar: Calendar, now: Date = Date()) -> [Piece] {
        let m = meeting(item.meetingId)
        let day = { (m: SavedMeeting) in SavedMeeting.format("M월 d일", m.start, calendar) }
        var parts: [Piece] = []
        if item.kind == "commitment" {
            if let owner = item.owner, !owner.isEmpty {
                parts.append(Piece(text: m?.displayName(key: owner) ?? owner))
            }
            if let due = item.due, !due.isEmpty {
                if let m, let d = DueDate.day(due, from: m.start, calendar: calendar) {
                    let late = DueBucket(d, now: now, calendar: calendar) == .overdue
                    let date = SavedMeeting.format("M월 d일", d, calendar)
                    parts.append(Piece(text: late ? "\(date) · \(PanelText.overdue)" : date, warn: late))
                } else {
                    parts.append(Piece(text: due))
                }
            }
        } else {
            if let m { parts.append(Piece(text: day(m))) }
            let others = (item.also ?? []).compactMap(meeting).map(day)
            if !others.isEmpty { parts.append(Piece(text: PanelText.prepAlso(others.joined(separator: ", ")))) }
        }
        var out: [Piece] = []
        for (i, p) in parts.enumerated() {
            if i > 0 { out.append(Piece(text: " · ")) }
            out.append(p)
        }
        return out
    }

    /// A prepared item opens its meeting with its ledger row lit; the row's own chip offers the transcript lines.
    static func target(for item: PanelItem, meeting: (String) -> SavedMeeting?) -> PanelTarget {
        let refs = meeting(item.meetingId).flatMap { m in m.ledger.indices.contains(item.index) ? m.ledger[item.index].refs : nil }
        return PanelTarget(meetingID: item.meetingId, keys: refs ?? [], ledgerIndex: item.index)
    }

    /// A why answer's [n]: the n-th line it rests on.
    static func whyTarget(_ n: Int, answer: WhyAnswer, meetingID: String) -> PanelTarget? {
        answer.keys.indices.contains(n - 1) ? PanelTarget(meetingID: meetingID, keys: [answer.keys[n - 1]]) : nil
    }

    /// The meeting a prepared item came from, as a chip: "주간 결제 싱크 · 11/7".
    static func meetingChip(_ m: SavedMeeting, calendar: Calendar) -> String {
        "\(m.displayTitle(calendar: calendar)) · \(SavedMeeting.format("M/d", m.start, calendar))"
    }

    /// The sources and clauses an answer comes in by (docs/36 0-2절 5–6), for when its last clause is in. Prep and why
    /// count what follows their clauses as one more.
    static func revealCounts(_ body: PanelTurnBody, preparedID: String?, say: (String) -> String) -> (chips: Int, chunks: Int) {
        switch body {
        case .ask(let a) where a.errorKind == nil && found(a.answer, sources: a.cards.count):
            (a.cards.count, PanelBlocks.chunkCount(a.answer ?? ""))
        case .why(let a, _) where a.errorKind == nil && found(a.reason, sources: a.keys.count):
            (0, PanelMotion.chunks(a.reason).count + 1)
        case .prep(let a):
            (0, PanelBlocks.chunkCount(prep(a, recent: preparedID, expanded: false, say: say).advice ?? "") + 1)
        case .chat(let c) where c.errorKind == nil && c.prep != nil:
            (0, PanelBlocks.chunkCount(prep(chatPrep(c)!, recent: preparedID, expanded: false, say: say).advice ?? "") + 1)
        case .chat(let a) where a.errorKind == nil && !PanelText.isNoAnswer(a.answer):
            (a.cards.count, PanelBlocks.chunkCount(a.answer ?? ""))
        default:
            (0, 1)
        }
    }

    /// A chat answer's prep as the prep view shows it: the model's lead (the things to check first) in place of the
    /// prep's own advice, without its [n] (the prep view has no numbered sources).
    static func chatPrep(_ c: ChatAnswer) -> PrepAnswer? {
        guard let p = c.prep else { return nil }
        guard let lead = c.answer, !PanelText.isNoAnswer(lead) else { return p }
        return PrepAnswer(advice: Citations.unmarked(lead), open: p.open, todos: p.todos, sources: p.sources,
                          meetingsRead: p.meetingsRead, sourcesRead: p.sourcesRead, errorKind: p.errorKind)
    }

    /// A sentence answer is shown only with its sources; without any (or with the engine's no-answer sentence) the panel
    /// says the scope did not talk about it.
    static func found(_ answer: String?, sources: Int) -> Bool { !PanelText.isNoAnswer(answer) && sources > 0 }
}

// MARK: - One turn

/// The person's bubble, then what came back: the step line while searching, an answer, or why it did not come.
/// An answer that arrives while the turn is on screen comes in piece by piece (docs/36 0-2절 5–7): the step line
/// stays until the last clause is in, then folds and the done line shows.
struct PanelTurnView: View {
    let turn: PanelTurn
    let scope: PanelScope
    let meeting: (String) -> SavedMeeting?
    let actions: PanelActions
    let calendar: Calendar
    /// For the proposal cards under a chat answer (docs/38 12절); nil shows none.
    var store: PanelStore? = nil
    @State private var cardsIn = false
    @Environment(\.reducesMotion) private var reduce
    @Environment(\.displayScale) private var scale

    private var preparedID: String? { if case .meeting(let id) = scope { id } else { nil } }

    var body: some View {
        let counts = turn.body.map { PanelShown.revealCounts($0, preparedID: preparedID, say: actions.say) } ?? (chips: 0, chunks: 0)
        let end = PanelReveal.of(turn, at: .distantPast, reduce: reduce).endAt(chips: counts.chips, chunks: counts.chunks)
        VStack(alignment: .leading, spacing: 13) {
            Text(turn.question).uiFont(13.5).foregroundStyle(Theme.ink).lineSpacing(2)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 12).padding(.vertical, 8)
                .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.fill))
                .frame(maxWidth: 300, alignment: .trailing)
                .frame(maxWidth: .infinity, alignment: .trailing)
            // one timeline for the step line and the answer, so the line keeps its place (and shimmer) as the answer comes
            TimelineView(PanelTicks.turn(turn, reduce: reduce, endAt: end)) { _ in
                let now = Date()
                let reveal = PanelReveal.of(turn, at: now, reduce: reduce)
                let line = stepLine(at: now, reveal: reveal, counts: counts)
                VStack(alignment: .leading, spacing: 13) {
                    if let line {
                        PanelStepLine(text: line, brightness: reduce ? nil : PanelMotion.shimmer(at: now.timeIntervalSince(turn.at)))
                            .transition(Theme.arrive(y: 4, reduce: reduce, scale: scale))
                    }
                    if turn.body != nil {
                        answer.environment(\.panelReveal, (reveal.t ?? end) < end ? reveal : .shown)
                    }

                }
                .animation(Theme.anim(Theme.enter, reduce: reduce), value: line == nil)
            }
            // the cards come once the answer is all in: they act on what it says. Outside the timeline, whose last
            // tick falls just short of the end.
            // a turn reopened from the list (no answeredAt) shows them at once
            if let store, cardsIn || turn.answeredAt == nil {
                ForEach(turn.proposals) { p in
                    PanelProposalCard(proposal: p, turn: turn.id, state: turn.proposalStates?[p.id], store: store,
                                      meeting: meeting(p.meetingId))
                        .transition(Theme.arrive(y: 4, reduce: reduce, scale: scale))
                }
            }
        }
        .task(id: turn.body != nil) {
            guard turn.body != nil, !turn.proposals.isEmpty else { return }
            let left = turn.answeredAt.map { end - Date().timeIntervalSince($0) } ?? 0
            if left > 0 { try? await Task.sleep(for: .seconds(left)) }
            guard !Task.isCancelled else { return }
            withAnimation(Theme.anim(Theme.enter, reduce: reduce)) { cardsIn = true }
        }
    }

    /// The step line: while the question runs, from 3 s; after the answer arrives, the line it had then until the last
    /// clause is in (it folds as the done line shows).
    private func stepLine(at now: Date, reveal: PanelReveal, counts: (chips: Int, chunks: Int)) -> String? {
        guard turn.body != nil else {
            return PanelPhase.at(elapsed: now.timeIntervalSince(turn.at), steps: turn.steps, done: false).stepLine
        }
        guard let a = turn.answeredAt, let t = reveal.t, t < reveal.doneAt(chips: counts.chips, chunks: counts.chunks) else { return nil }
        return PanelPhase.at(elapsed: a.timeIntervalSince(turn.at), steps: turn.steps, done: false).stepLine
    }

    @ViewBuilder private var answer: some View {
        switch turn.body {
        case nil:
            EmptyView()
        case .prep(let a):
            PrepAnswerView(answer: a, steps: turn.steps, meeting: meeting, actions: actions, calendar: calendar,
                           preparedID: preparedID)
        case .why(let a, let id):
            if let kind = a.errorKind {
                PanelReason(text: PanelText.reason(kind, say: actions.say))
            } else if PanelShown.found(a.reason, sources: a.keys.count) {
                WhyAnswerView(answer: a, meetingID: id, meeting: meeting, actions: actions, calendar: calendar)
            } else {
                PanelNotFound(scope: scope, meeting: meeting, calendar: calendar)
            }
        case .ask(let a):
            if let kind = a.errorKind {
                PanelReason(text: PanelText.reason(kind, say: actions.say))
            } else if PanelShown.found(a.answer, sources: a.cards.count) {
                AskAnswerView(answer: a, meeting: meeting, actions: actions, calendar: calendar)
            } else {
                PanelNotFound(scope: scope, meeting: meeting, calendar: calendar)
            }
        case .chat(let a):
            if let kind = a.errorKind {
                PanelReason(text: PanelText.reason(kind, say: actions.say))
            } else if let p = PanelShown.chatPrep(a) {
                PrepAnswerView(answer: p, steps: turn.steps, meeting: meeting, actions: actions, calendar: calendar,
                               preparedID: preparedID)
            } else if !PanelText.isNoAnswer(a.answer) {
                AskAnswerView(answer: PanelAsk(answer: a.answer, cited: [], cards: a.cards, errorKind: nil),
                              meeting: meeting, actions: actions, calendar: calendar)
            } else {
                PanelNotFound(scope: scope, meeting: meeting, calendar: calendar)
            }
        case .failed(let kind):
            PanelReason(text: PanelText.reason(kind, say: actions.say))
        }
    }
}

/// Why the answer did not come (`error_kind`).
struct PanelReason: View {
    let text: String
    var body: some View {
        Text(text).uiFont(13.5).foregroundStyle(Theme.ink2).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
            .panelReveal(at: 0)
    }
}

/// "<범위>에서는 이야기가 나오지 않았어요".
struct PanelNotFound: View {
    let scope: PanelScope
    let meeting: (String) -> SavedMeeting?
    let calendar: Calendar

    var body: some View {
        Text(PanelText.notFound(PanelText.place(scope, meeting: meeting, calendar: calendar))).uiFont(14.5)
            .foregroundStyle(Theme.ink).lineSpacing(3)
            .fixedSize(horizontal: false, vertical: true)
            .panelReveal(at: 0)
    }
}

/// Under a finished answer (docs/36 0-2절 7): one green check and what it read to answer.
struct PanelDoneLine: View {
    let text: String

    var body: some View {
        HStack(spacing: 7) {
            Image(systemName: "checkmark.circle.fill").font(.system(size: 14)).foregroundStyle(Theme.live)
            Text(text).uiFont(12).foregroundStyle(Theme.ink3)
        }
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Answers

/// 다음 회의 준비: the advice, one box with 아직 안 정한 것 and 안 끝난 할 일, the sources, then the done line with 복사.
struct PrepAnswerView: View {
    let answer: PrepAnswer
    let steps: [PanelStep]
    let meeting: (String) -> SavedMeeting?
    let actions: PanelActions
    let calendar: Calendar
    /// The meeting the prep was started from; its own items show first.
    var preparedID: String?
    @State var copied = false
    @State var expanded: Bool

    init(answer: PrepAnswer, steps: [PanelStep], meeting: @escaping (String) -> SavedMeeting?, actions: PanelActions,
         calendar: Calendar, preparedID: String? = nil, expanded: Bool = false) {
        self.answer = answer; self.steps = steps; self.meeting = meeting; self.actions = actions
        self.calendar = calendar; self.preparedID = preparedID
        _expanded = State(initialValue: expanded)
    }

    private var shown: PanelShown.Prep { PanelShown.prep(answer, recent: preparedID, expanded: expanded, say: actions.say) }

    /// The meetings it read, from the steps (the asked one first, then those found alike).
    private var meetingsRead: [SavedMeeting] {
        var ids: [String] = []
        for s in steps {
            switch s {
            case .meetings(let x), .related(let x): ids += x.filter { !ids.contains($0) }
            default: break
            }
        }
        // a chat's prep reports tools, not meetings: the meetings its items come from, the prepared one first
        if ids.isEmpty {
            for id in [preparedID].compactMap({ $0 }) + (answer.open + answer.todos).map(\.meetingId) where !ids.contains(id) {
                ids.append(id)
            }
        }
        return ids.compactMap(meeting)
    }

    @Environment(\.panelReveal) private var reveal

    var body: some View {
        let shown = shown
        // the advice in clauses, then the rest at once
        VStack(alignment: .leading, spacing: 13) {
            if let advice = shown.advice { PanelRichText(text: advice) }
            rest(shown).panelReveal(at: reveal.chunkStart(chips: 0, PanelBlocks.chunkCount(shown.advice ?? "")), rise: 3)
        }
    }

    private func rest(_ shown: PanelShown.Prep) -> some View {
        VStack(alignment: .leading, spacing: 13) {
            if let reason = shown.reason { PanelReason(text: reason) }
            if !shown.groups.isEmpty {
                PanelBox {
                    ForEach(Array(shown.groups.enumerated()), id: \.offset) { _, g in
                        PanelBoxHead(g.title)
                        ForEach(Array(g.items.enumerated()), id: \.offset) { i, item in
                            Button { open(item) } label: {
                                PanelBoxRow(first: i == 0) {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(item.text).uiFont(13.5).foregroundStyle(Theme.ink).lineSpacing(1)
                                            .fixedSize(horizontal: false, vertical: true)
                                        let pieces = PanelShown.itemLine(item, meeting: meeting, calendar: calendar)
                                        if !pieces.isEmpty { pieceLine(pieces) }
                                    }
                                }
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            }
            if shown.hidden > 0 {
                Button { expanded = true } label: {
                    Text(PanelText.prepMore(shown.hidden)).uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink3)
                        .padding(.horizontal, 4).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            Wrap(spacing: 6) {
                ForEach(Array(meetingsRead.enumerated()), id: \.element.id) { _, m in
                    SourceChip(label: PanelShown.meetingChip(m, calendar: calendar), card: nil) { actions.openMeeting(PanelTarget(meetingID: m.id)) }
                }
                ForEach(answer.sources) { c in
                    SourceChip(label: c.name, card: c) { actions.openCard(c) }
                }
            }
            // the end: the done line, and 복사 for the items
            if shown.read != nil || shown.hasItems {
                HStack(alignment: .center) {
                    if let read = shown.read { PanelDoneLine(text: read) }
                    Spacer(minLength: 8)
                    if shown.hasItems { copyButton }
                }
            }
        }
    }

    private func pieceLine(_ pieces: [PanelShown.Piece]) -> some View {
        pieces.reduce(Text("")) { t, p in
            t + Text(p.text).foregroundColor(p.warn ? Theme.warn : Theme.ink.opacity(0.55))
        }
        .uiFont(12)
        .fixedSize(horizontal: false, vertical: true)
    }

    @ViewBuilder private var copyButton: some View {
        if copied {
            DoneMark(text: PanelText.copied).frame(height: 18)
        } else {
            Button {
                actions.copy(PanelShown.markdown(answer, meeting: meeting, calendar: calendar))
                copied = true
                Task { try? await Task.sleep(for: .seconds(3)); copied = false }
            } label: {
                Text(PanelText.copy).uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink3)
                    .padding(.horizontal, 4).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
    }

    private func open(_ item: PanelItem) { actions.openMeeting(PanelShown.target(for: item, meeting: meeting)) }
}

/// 이유 보기: the reason, then 대화 기록에서 with the lines it rests on (speaker dot, name, time).
struct WhyAnswerView: View {
    let answer: WhyAnswer
    let meetingID: String
    let meeting: (String) -> SavedMeeting?
    let actions: PanelActions
    let calendar: Calendar
    @Environment(\.panelReveal) private var reveal

    var body: some View {
        let m = meeting(meetingID)
        let lines = answer.keys.compactMap { k in m?.transcript.first { $0.key == k } }
        // the reason in clauses, then the lines it rests on and the done line
        let rest = reveal.chunkStart(chips: 0, PanelMotion.chunks(answer.reason).count)
        VStack(alignment: .leading, spacing: 13) {
            PanelSentence(text: answer.reason) { n in
                if let t = PanelShown.whyTarget(n, answer: answer, meetingID: meetingID) { actions.openMeeting(t) }
            }
            whyRest(m: m, lines: lines).panelReveal(at: rest, rise: 3)
        }
    }

    private func whyRest(m: SavedMeeting?, lines: [TranscriptLine]) -> some View {
        VStack(alignment: .leading, spacing: 13) {
            if let m, !lines.isEmpty {
                PanelBox {
                    PanelBoxHead(PanelText.whyLines)
                    ForEach(Array(lines.enumerated()), id: \.offset) { i, l in
                        Button { actions.openMeeting(PanelTarget(meetingID: meetingID, keys: [l.key])) } label: {
                            PanelBoxRow(first: i == 0) { line(l, n: i + 1, in: m) }
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            // it read the one meeting's transcript
            if let read = PanelText.read(meetings: 1, sources: 0) { PanelDoneLine(text: read) }
        }
    }

    private func line(_ l: TranscriptLine, n: Int, in m: SavedMeeting) -> some View {
        let name = m.lineName(l) ?? ""
        // as everywhere a speaker shows: a light tint of their colour for the dot, the letter in the colour itself
        let color = l.voice.map { Theme.speaker(m.speakerColor($0, people: [])) } ?? Theme.ink.opacity(0.5)
        return HStack(alignment: .top, spacing: 9) {
            Text(String(name.prefix(1))).uiFont(10, weight: .bold).foregroundStyle(color)
                .frame(width: 18, height: 18)
                .background(Circle().fill(color.opacity(0.14)))
                .padding(.top, 1)
            VStack(alignment: .leading, spacing: 1) {
                (Text("\(n)").font(.system(size: 9.5, weight: .bold)).baselineOffset(3)
                    + Text(" ") + Text(name).fontWeight(.semibold).foregroundColor(Theme.ink)
                    + Text(" · \(SavedMeeting.format("HH:mm", l.at, calendar))"))
                    .uiFont(12).foregroundStyle(Theme.ink3)
                Text(l.text).uiFont(13.5).foregroundStyle(Theme.ink).lineSpacing(1)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// 전체에서 묻기: the sources numbered as the answer cites them, the answer, the done line.
struct AskAnswerView: View {
    let answer: PanelAsk
    let meeting: (String) -> SavedMeeting?
    let actions: PanelActions
    let calendar: Calendar

    static func meetingID(_ c: Card) -> String? {
        c.path.hasPrefix("meeting/") ? String(c.path.dropFirst("meeting/".count)) : nil
    }

    @Environment(\.panelReveal) private var reveal

    var body: some View {
        let meetings = answer.cards.filter { Self.meetingID($0) != nil }.count
        let chips = answer.cards.count
        // the sources first, then the answer, then the done line (mockup v9)
        VStack(alignment: .leading, spacing: 13) {
            Wrap(spacing: 6) {
                ForEach(Array(answer.cards.enumerated()), id: \.element.id) { i, c in
                    SourceChip(label: label(c), card: Self.meetingID(c) == nil ? c : nil, n: i + 1) { open(c) }
                        .panelReveal(at: reveal.chipStart(i), fade: PanelMotion.chipFade, rise: 3)
                }
            }
            PanelRichText(text: answer.answer ?? "", afterChips: chips) { n in
                if answer.cards.indices.contains(n - 1) { open(answer.cards[n - 1]) }
            }
            if let read = PanelText.read(meetings: meetings, sources: answer.cards.count - meetings) {
                PanelDoneLine(text: read)
                    .panelReveal(at: reveal.doneAt(chips: chips, chunks: PanelBlocks.chunkCount(answer.answer ?? "")), rise: 3)
            }
        }
    }

    private func label(_ c: Card) -> String {
        guard let id = Self.meetingID(c), let m = meeting(id) else { return c.name }
        return PanelShown.meetingChip(m, calendar: calendar)
    }

    private func open(_ c: Card) {
        if let id = Self.meetingID(c) { actions.openMeeting(PanelTarget(meetingID: id)) } else { actions.openCard(c) }
    }
}

// MARK: - Pieces

/// An answer sentence: the engine's [1] marks drawn as small raised numbers; with `cite`, a number opens its source.
/// While the answer comes in, its clauses fade in one after another after `afterChips` sources, each number with its
/// clause (docs/36 0-2절 6).
/// An answer's lines (docs/38 4-1절): paragraphs, and `-` or numbered list items drawn with their mark hanging. Each
/// line comes in clause by clause after the ones before it, as one `PanelSentence` would.
enum PanelBlocks {
    struct Block: Equatable {
        /// "•" or "3." for a list item, nil for a paragraph line
        var mark: String?
        var text: String
        /// a blank line came before it
        var gap: Bool
    }

    static func of(_ text: String) -> [Block] {
        var out: [Block] = []
        var gap = false
        for raw in text.components(separatedBy: "\n") {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.isEmpty { gap = !out.isEmpty; continue }
            var b = Block(mark: nil, text: line, gap: gap)
            if let r = line.range(of: #"^[-*•]\s+"#, options: .regularExpression) {
                b.mark = "•"
                b.text = String(line[r.upperBound...])
            } else if let r = line.range(of: #"^\d{1,2}[.)]\s+"#, options: .regularExpression) {
                b.mark = String(line[r].filter(\.isNumber)) + "."
                b.text = String(line[r.upperBound...])
            }
            out.append(b)
            gap = false
        }
        return out
    }

    /// How many clauses the lines come in by, all together.
    static func chunkCount(_ text: String) -> Int {
        of(text).reduce(0) { $0 + PanelMotion.chunks($1.text).count }
    }
}

struct PanelRichText: View {
    let text: String
    var afterChips = 0
    var cite: ((Int) -> Void)? = nil
    @Environment(\.panelReveal) private var reveal
    /// Once the answer is all in, its lines are AppKit text in one selection: a drag runs across paragraphs and list
    /// items (SwiftUI's own selection stops at each `Text`).
    @State private var selection = TextSelectionGroup()

    var body: some View {
        if reveal.t == nil { selectable } else { revealing }
    }

    private var selectable: some View {
        let blocks = PanelBlocks.of(text)
        return VStack(alignment: .leading, spacing: 5) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { i, b in
                let line = SelectableText(id: "a.\(i)", text: Citations.selectable(b.text, size: 14.5).string, group: selection,
                                          size: 14.5, lineSpacing: 4, color: Theme.ink,
                                          styled: { Citations.selectable(b.text, size: $0, linked: true) }, cite: cite)
                Group {
                    if let mark = b.mark {
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Text(mark).uiFont(14.5).foregroundStyle(Theme.ink3).monospacedDigit()
                                .frame(minWidth: 10, alignment: .leading)
                            line
                        }
                    } else {
                        line
                    }
                }
                .padding(.top, b.gap ? 6 : 0)
            }
        }
    }

    private var revealing: some View {
        let blocks = PanelBlocks.of(text)
        let starts = blocks.reduce(into: [0]) { $0.append($0.last! + PanelMotion.chunks($1.text).count) }
        return VStack(alignment: .leading, spacing: 5) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { i, b in
                Group {
                    if let mark = b.mark {
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Text(mark).uiFont(14.5).foregroundStyle(Theme.ink3).monospacedDigit()
                                .frame(minWidth: 10, alignment: .leading)
                                .panelReveal(at: reveal.chunkStart(chips: afterChips, starts[i]))
                            PanelSentence(text: b.text, afterChips: afterChips, chunkOffset: starts[i], cite: cite)
                        }
                    } else {
                        PanelSentence(text: b.text, afterChips: afterChips, chunkOffset: starts[i], cite: cite)
                    }
                }
                .padding(.top, b.gap ? 6 : 0)
            }
        }
    }
}

struct PanelSentence: View {
    let text: String
    var afterChips = 0
    /// clauses of the same answer drawn before this text (`PanelRichText`)
    var chunkOffset = 0
    var cite: ((Int) -> Void)? = nil
    @Environment(\.panelReveal) private var reveal

    private var styled: AttributedString {
        guard reveal.t != nil else { return Citations.styled(text, size: 14.5, linked: cite != nil) }
        var out = AttributedString()
        for (j, chunk) in PanelMotion.chunks(text).enumerated() {
            let o = reveal.opacity(from: reveal.chunkStart(chips: afterChips, chunkOffset + j), fade: PanelMotion.chunkFade)
            var a = Citations.styled(chunk, size: 14.5)
            for run in a.runs {
                // the raised numbers are drawn at half ink, the words at full
                a[run.range].foregroundColor = Theme.ink.opacity((run.foregroundColor == nil ? 1 : 0.5) * o)
            }
            out += a
        }
        return out
    }

    var body: some View {
        Text(styled).uiFont(14.5).foregroundStyle(Theme.ink).lineSpacing(4)
            .fixedSize(horizontal: false, vertical: true)
            .textSelection(.enabled)
            .tint(Theme.ink.opacity(0.55))
            .environment(\.openURL, OpenURLAction { url in
                guard let n = Citations.number(of: url) else { return .systemAction }
                cite?(n)
                return .handled
            })
    }
}

/// A source under an answer: its number when the sentence cites it, its service's mark, its name.
struct SourceChip: View {
    let label: String
    let card: Card?
    var n: Int? = nil
    let action: () -> Void
    @State var hover = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                if let n { Text("\(n)").font(.system(size: 9.5, weight: .bold)).foregroundStyle(Theme.ink2).baselineOffset(3) }
                if let card { SourceMark(card: card, size: 11) }
                Text(label).uiFont(12).foregroundStyle(Theme.ink).lineLimit(1)
            }
            .padding(.horizontal, 8).frame(height: 24)
            .frame(maxWidth: 300, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(hover ? Theme.fill : Theme.paper))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Theme.hairline))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .help(card.map { "\($0.byline ?? $0.path) · \($0.opensIn)" } ?? label)
    }
}

/// The inner box of an answer: faint grey ground, a hairline, corners of 14.
struct PanelBox<Content: View>: View {
    @ViewBuilder let content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 0) { content }
            .padding(.horizontal, 12).padding(.vertical, 2)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill))
            .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
    }
}

struct PanelBoxHead: View {
    let title: String
    init(_ title: String) { self.title = title }
    var body: some View {
        Text(title).uiFont(12, weight: .bold).foregroundStyle(Theme.ink2).padding(.top, 9).padding(.bottom, 3)
    }
}

/// A row of a box: a hairline above, except under a heading.
struct PanelBoxRow<Content: View>: View {
    let first: Bool
    @ViewBuilder let content: Content
    @State var hover = false

    var body: some View {
        content
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.vertical, 8)
            .background(alignment: .center) {
                Theme.fillHover.opacity(hover ? 1 : 0).padding(.horizontal, -12)
            }
            .overlay(alignment: .top) {
                if !first { Rectangle().fill(Theme.hairline).frame(height: 1) }
            }
            .contentShape(Rectangle())
            .onHover { hover = $0 }
    }
}
