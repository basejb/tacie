import SwiftUI

/// Who spoke, on the meeting page and in the island (design notes 5-1, 5-2):
/// a coloured dot per speaker, the people strip under the title, and the transcript in turns.

/// A speaker's dot: the palette colour as a light tint with a fine ring, and the name's first letter (or the speaker's
/// number while unnamed) in the colour itself.
struct SpeakerDot: View {
    let name: String
    let key: String?
    let color: Int
    var size: CGFloat = 22

    var body: some View {
        let hue = Theme.speaker(color)
        ZStack {
            Circle().fill(hue.opacity(0.14))
            Circle().strokeBorder(hue.opacity(0.22), lineWidth: 0.5)
            Text(SavedMeeting.speakerMark(name: name, key: key))
                .font(.system(size: size * 0.46, weight: .bold)).foregroundStyle(hue)
                .lineLimit(1).minimumScaleFactor(0.6)
        }
        .frame(width: size, height: size)
    }
}

/// A speaker's name: full ink once someone named them, a step lighter while they go by "화자 N".
struct SpeakerName: View {
    let name: String
    let key: String?
    var size: CGFloat = 13
    /// A suggestion waits for this speaker (화자 추천): a dotted line under the name.
    var suggested = false
    /// The name just changed: every place it shows lights up for a moment, so a naming shows where it landed.
    @State private var lit = false
    @Environment(\.reducesMotion) private var reduceMotion

    var body: some View {
        Text(name).uiFont(size, weight: .semibold)
            .foregroundStyle(name == key ? Theme.ink2 : Theme.ink)
            .lineLimit(1)
            .overlay(alignment: .bottom) {
                // round dots, finer than Text.underline's dot pattern (which draws dashes)
                if suggested {
                    DottedLine().stroke(Theme.suggest, style: StrokeStyle(lineWidth: 1.6, lineCap: .round, dash: [0.01, 3.2]))
                        .frame(height: 1.6).offset(y: 3)
                }
            }
            .background {
                RoundedRectangle(cornerRadius: 5, style: .continuous).fill(Theme.live.opacity(lit ? 0.22 : 0))
                    .padding(.horizontal, -4).padding(.vertical, -1)
            }
            .onChange(of: name) { _, _ in
                guard !reduceMotion else { return }
                withAnimation(.easeOut(duration: 0.15)) { lit = true }
                Task {
                    try? await Task.sleep(for: .seconds(0.9))
                    withAnimation(.easeOut(duration: 0.9)) { lit = false }
                }
            }
    }
}

/// Under the title: who was in the meeting, in the order they first spoke, with their share of the talk (점유율).
/// A chip opens the namer (with `editing`).
struct PeopleStrip: View {
    let meeting: SavedMeeting
    let people: [Person]
    var editing: SpeakerEditing? = nil

    @State private var open: String?
    @State private var hover: String?
    /// The speaker whose suggestion shows under its chip, and whether the pointer is on that card.
    @State private var peek: String?
    @State private var onCard = false

    /// `peek` starts elsewhere only when the strip is drawn on its own (a render of a suggestion's card).
    init(meeting: SavedMeeting, people: [Person], editing: SpeakerEditing? = nil, peek: String? = nil) {
        self.meeting = meeting
        self.people = people
        self.editing = editing
        _peek = State(initialValue: peek)
    }

    private var suggestions: [String: Person] { editing?.suggestions ?? [:] }

    var body: some View {
        Wrap(spacing: 6) {
            ForEach(meeting.attendees, id: \.key) { a in
                chip(a)
            }
            Text(suggestions.isEmpty ? String(localized: "이름을 누르면 바꿀 수 있어요") : String(localized: "점선 이름에 올리면 추천이 보여요"))
                .uiFont(12).foregroundStyle(Theme.ink4)
                .padding(.leading, 6).frame(height: 28)
        }
        .onChange(of: suggestions.keys.sorted()) { if let p = peek, suggestions[p] == nil { peek = nil } }
    }

    /// The card follows the pointer a beat late: it comes 0.25 s after the pointer rests on a dotted name, and stays while
    /// the pointer moves down onto it.
    private func settlePeek() {
        let want = hover.flatMap { suggestions[$0] != nil && open == nil ? $0 : nil } ?? (onCard ? peek : nil)
        guard want != peek else { return }
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(want == nil ? 200 : 250))
            let now = hover.flatMap { suggestions[$0] != nil && open == nil ? $0 : nil } ?? (onCard ? peek : nil)
            if now == want { withAnimation(Theme.fast) { peek = want } }
        }
    }

    private func chip(_ a: (key: String, name: String, share: Int)) -> some View {
        let lit = editing != nil && (hover == a.key || open == a.key)
        return HStack(spacing: 7) {
            SpeakerDot(name: a.name, key: a.key, color: meeting.speakerColor(a.key, people: people), size: 20)
            SpeakerName(name: a.name, key: a.key, size: 12.5, suggested: suggestions[a.key] != nil)
            Text("점유율 \(a.share)%").uiFont(11.5).foregroundStyle(Theme.ink4).monospacedDigit()
        }
        .padding(.leading, 4).padding(.trailing, 11)
        .frame(height: 28)
        .background(lit ? Theme.fill : .clear, in: Capsule())
        .background(Theme.paper, in: Capsule())
        // inside the chip's frame: a stroke on the edge loses its outer half where the row clips
        .overlay(Capsule().strokeBorder(Theme.ink.opacity(0.1)))
        .contentShape(Capsule())
        .onHover {
            hover = $0 ? a.key : (hover == a.key ? nil : hover)
            settlePeek()
        }
        .onTapGesture {
            if editing != nil {
                peek = nil
                open = a.key
            }
        }
        .overlay(alignment: .topLeading) {
            if peek == a.key, let p = suggestions[a.key], let editing {
                SuggestCard(person: p, accept: {
                    peek = nil
                    editing.acceptSuggestion(a.key, p.id)
                }, dismiss: {
                    peek = nil
                    editing.dismissSuggestion(a.key, p.id)
                })
                .onHover {
                    onCard = $0
                    settlePeek()
                }
                .offset(y: 34)
                .transition(.opacity.combined(with: .offset(y: -3)))
            }
        }
        .zIndex(peek == a.key ? 1 : 0)
        .popover(isPresented: Binding(get: { open == a.key }, set: { if !$0 { open = nil } }), arrowEdge: .bottom) {
            if let editing {
                Namer(meeting: meeting, target: NamerTarget(key: a.key), people: people, editing: editing) { open = nil }
            }
        }
    }
}

/// A level line through the middle of its frame, for a dotted stroke.
struct DottedLine: Shape {
    func path(in r: CGRect) -> Path {
        Path { p in
            p.move(to: CGPoint(x: r.minX + 1, y: r.midY))
            p.addLine(to: CGPoint(x: r.maxX, y: r.midY))
        }
    }
}

/// Under a dotted name: the remembered person the speaker sounds like, with ✓ (맞아요) and ✕ (아니에요).
struct SuggestCard: View {
    let person: Person
    let accept: () -> Void
    let dismiss: () -> Void

    var body: some View {
        HStack(spacing: 7) {
            SpeakerDot(name: person.name, key: nil, color: person.color, size: 20)
            HStack(spacing: 5) {
                Text("\(person.name)일까요?").uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink)
                Text("\(SavedMeeting.format("M월 d일", person.last, MainView.calendar)) 회의").uiFont(11.5).foregroundStyle(Theme.ink3)
            }
            .fixedSize()
            .padding(.trailing, 2)
            Button(action: accept) {
                Image(systemName: "checkmark").uiFont(10, weight: .bold).foregroundStyle(Theme.onStrong)
                    .frame(width: 22, height: 22).background(Theme.ink, in: Circle())
            }
            .buttonStyle(PressStyle()).help("맞아요").accessibilityLabel("맞아요")
            Button(action: dismiss) {
                Image(systemName: "xmark").uiFont(9.5, weight: .bold).foregroundStyle(Theme.ink3)
                    .frame(width: 22, height: 22).contentShape(Circle())
            }
            .buttonStyle(PressStyle()).help("아니에요").accessibilityLabel("아니에요")
        }
        .padding(.leading, 5).padding(.trailing, 4).frame(height: 32)
        .background { Capsule().fill(Theme.raised).shadow(color: Theme.ink.opacity(0.12), radius: 10, y: 4) }
        .overlay(Capsule().strokeBorder(Theme.ink.opacity(0.1)))
        .fixedSize()
    }
}

/// The transcript, laid open under "대화 기록": lines in a row from one speaker make a turn, with the dot, the name and
/// the time on top and the words beneath. A meeting saved before speakers shows its lines one by one, with no names.
struct TranscriptTurns: View {
    let meeting: SavedMeeting
    let people: [Person]
    let calendar: Calendar
    var editing: SpeakerEditing? = nil
    /// With the recording to play: words to click, unheard stretches between the turns (meeting-recording spec 5-2, 5-6).
    var playback: TranscriptPlayback? = nil
    /// A turn drawn as if the pointer were on it ("turn <first line key>"), for renders.
    private let hover: String?

    /// The turns, made again only when the lines, the names or the unheard stretches change (not on hover).
    @State private var pieces = PiecesMemo()
    /// One selection over every turn: a drag runs from one turn into the next. A meeting page passes its own, so the
    /// drag goes on from its notes into the transcript.
    @State private var own = TextSelectionGroup()
    @Environment(\.reducesMotion) private var reduceMotion
    private let group: TextSelectionGroup?
    private var selection: TextSelectionGroup { group ?? own }

    /// `hover` starts elsewhere only when the transcript is drawn on its own (a render of a hovered line).
    init(meeting: SavedMeeting, people: [Person], calendar: Calendar, editing: SpeakerEditing? = nil,
         playback: TranscriptPlayback? = nil, selection: TextSelectionGroup? = nil, hover: String? = nil) {
        self.group = selection
        self.meeting = meeting
        self.people = people
        self.calendar = calendar
        self.editing = editing
        self.playback = playback
        self.hover = hover
    }

    var body: some View {
        // the paragraphs go to the selection only when the turns were made again: a long meeting has thousands
        if pieces.update(meeting, unheard: playback?.unheard ?? []) { selection.set(pieces.paragraphs, section: 1) }
        return LazyVStack(alignment: .leading, spacing: 0) {
            if meeting.hasSpeakers {
                ForEach(pieces.made, id: \.id) { p in
                    switch p {
                    case .turn(let t):
                        TurnRow(turn: t, meeting: meeting, people: people, editing: editing, selection: selection,
                                playback: playback, time: time(t.at), hover: hover == "turn " + t.lines[0].key)
                            .transition(.opacity)
                    case .unheard(let s): if let playback { UnheardRow(span: s, playback: playback) }
                    }
                }
            } else {
                // every line has no speaker here: as turns they would all run together into one
                ForEach(pieces.rows, id: \.id) { r in
                    switch r {
                    case .line(let l): line(l).transition(.opacity)
                    case .unheard(let s): if let playback { UnheardRow(span: s, playback: playback) }
                    }
                }
            }
        }
        // only a new last paragraph fades in (a meeting going on); opening a page or a naming moves nothing
        .animation(Theme.anim(.easeOut(duration: 0.3), reduce: reduceMotion), value: meeting.hasSpeakers ? pieces.made.last?.id : pieces.rows.last?.id)
    }

    private func line(_ l: TranscriptLine) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(time(l.at)).uiFont(11.5).foregroundStyle(Theme.ink4).monospacedDigit()
                .frame(width: 58, alignment: .leading)
            VStack(alignment: .leading, spacing: 0) {
                TurnText(lines: [l], id: Self.paragraphID([l]), group: selection, playback: playback)
                if let a = meeting.foundAnswer(for: l) { FoundLine(answer: a) }
            }
            Spacer(minLength: 0)
            if l.question { QuestionTag() }
        }
        .padding(.horizontal, 12).padding(.vertical, 8)
        .background(l.question ? Theme.ink.opacity(0.03) : .clear, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .padding(.horizontal, -12)
    }

    static func paragraphID(_ lines: [TranscriptLine]) -> String { "p " + lines[0].key }

    private func time(_ at: Date) -> String { SavedMeeting.format("a h:mm", at, calendar) }
}

/// One speaker's turn: what they said in a row reads as one paragraph (the recognizer's pauses do not show).
/// The pointer anywhere on it lights the turn and shows the ⌄ by the name, the one way into renaming. The pointer and
/// the namer are the turn's own, so moving over the transcript draws only the turns it passes, not all of them.
private struct TurnRow: View {
    let turn: SavedMeeting.Turn
    let meeting: SavedMeeting
    let people: [Person]
    let editing: SpeakerEditing?
    let selection: TextSelectionGroup
    let playback: TranscriptPlayback?
    let time: String

    @State private var hover: Bool
    @State private var nameHover = false
    @State private var open = false

    init(turn: SavedMeeting.Turn, meeting: SavedMeeting, people: [Person], editing: SpeakerEditing?,
         selection: TextSelectionGroup, playback: TranscriptPlayback?, time: String, hover: Bool) {
        self.turn = turn
        self.meeting = meeting
        self.people = people
        self.editing = editing
        self.selection = selection
        self.playback = playback
        self.time = time
        _hover = State(initialValue: hover)
    }

    var body: some View {
        let t = turn
        let look = meeting.turnLook(t, people: people)
        let lit = editing != nil && (hover || open)
        return HStack(alignment: .top, spacing: 12) {
            if let name = t.name {
                SpeakerDot(name: name, key: look.key, color: look.color, size: 26)
                    .padding(.top, 1)
            } else {
                Color.clear.frame(width: 26, height: 26)  // heard before the voice model knew who spoke
            }
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    if let name = t.name { nameButton(name: name, key: look.key) }
                    Text(time).uiFont(11.5).foregroundStyle(Theme.ink3).monospacedDigit()
                        .frame(height: 22)
                    if t.question { QuestionTag() }
                }
                TurnText(lines: t.lines, id: TranscriptTurns.paragraphID(t.lines), group: selection, playback: playback)
                ForEach(t.lines.compactMap { meeting.foundAnswer(for: $0) }, id: \.self) { FoundLine(answer: $0) }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .background(Theme.ink.opacity((t.question ? 0.03 : 0) + (lit ? 0.03 : 0)),
                    in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .contentShape(Rectangle())
        .onHover { inside in
            withAnimation(.easeOut(duration: inside ? 0.14 : 0.2)) { hover = inside }
        }
        .padding(.horizontal, -12)
    }

    /// The turn's name opens the namer for its speaker, the way a chip of the people strip does. The ⌄ shows while
    /// the pointer is on the turn; the fill while it is on the name.
    @ViewBuilder private func nameButton(name: String, key: String?) -> some View {
        if let editing, let target = meeting.namerTarget(turn: turn) {
            let lit = hover || open
            let onName = nameHover || open
            // the ⌄ always takes its room and only fades, so the time beside the name never shifts
            HStack(spacing: 2) {
                SpeakerName(name: name, key: key)
                Image(systemName: "chevron.down").uiFont(9, weight: .bold).foregroundStyle(Theme.ink3)
                    .opacity(lit ? 1 : 0)
                    .offset(y: lit ? 0 : -2)
            }
            .padding(.horizontal, 6).frame(height: 22)
            .background(Theme.fillHover.opacity(onName ? 1 : 0), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .contentShape(Rectangle())
            .padding(.horizontal, -6)
            .onHover { inside in
                withAnimation(.easeOut(duration: inside ? 0.12 : 0.2)) { nameHover = inside }
            }
            .help("이름 바꾸기")
            .onTapGesture { open = true }
            .popover(isPresented: $open, arrowEdge: .bottom) {
                Namer(meeting: meeting, target: target, people: people, editing: editing) { open = false }
            }
        } else {
            SpeakerName(name: name, key: key).frame(height: 22)
        }
    }
}

/// A turn's words as one paragraph of the transcript's selection. With the recording, a click on a word plays from
/// there and the word being heard is lit; a drag selects, on into the next turns.
struct TurnText: View {
    let lines: [TranscriptLine]
    let id: String
    let group: TextSelectionGroup
    let playback: TranscriptPlayback?

    /// The shown words a space apart: the same text with or without the recording, so a selection keeps its letters.
    static func text(_ lines: [TranscriptLine]) -> String {
        lines.flatMap { $0.shownWords.map(\.text) }.joined(separator: " ")
    }

    var body: some View {
        var words: [PlayableWord] = []
        var current: Int?
        var at = 0
        for l in lines {
            let lit = playback?.focus?.line(l.key).word.flatMap { l.shownIndex(ofWord: $0) }
            for (i, w) in l.shownWords.enumerated() {
                let n = (w.text as NSString).length
                if lit == i { current = words.count }
                words.append(PlayableWord(range: NSRange(location: at, length: n), ms: w.ms))
                at += n + 1
            }
        }
        let live = playback.map { !$0.dimmed } ?? false
        return SelectableText(id: id, text: Self.text(lines), group: group,
                              words: playback == nil ? [] : words, current: current,
                              play: live ? playback?.play : nil)
            .opacity(playback?.dimmed == true ? 0.4 : 1)
    }
}

/// The last turns (or lines, for a meeting without speakers) made for a transcript and their paragraphs for the
/// selection, kept while the lines, the names and the unheard stretches stay the same.
final class PiecesMemo {
    private var key: (lines: [TranscriptLine], voices: [VoiceEntry]?, unheard: [AudioSpan], speakers: Bool)?
    private(set) var made: [TranscriptPiece] = []
    private(set) var rows: [TranscriptRow] = []
    private(set) var paragraphs: [(id: String, text: String)] = []

    /// Makes them again when needed; true when it did.
    func update(_ m: SavedMeeting, unheard: [AudioSpan]) -> Bool {
        let speakers = m.hasSpeakers
        if let k = key, k.speakers == speakers, k.lines == m.transcript, k.voices == m.voices, k.unheard == unheard { return false }
        if speakers {
            made = m.transcriptPieces(unheard: unheard)
            rows = []
            paragraphs = made.compactMap {
                if case .turn(let t) = $0 { return (TranscriptTurns.paragraphID(t.lines), TurnText.text(t.lines)) }
                return nil
            }
        } else {
            made = []
            rows = TranscriptRow.rows(lines: m.transcript, unheard: unheard)
            paragraphs = rows.compactMap {
                if case .line(let l) = $0 { return (TranscriptTurns.paragraphID([l]), TurnText.text([l])) }
                return nil
            }
        }
        key = (m.transcript, m.voices, unheard, speakers)
        return true
    }
}

/// The 질문 tag beside the time of a turn with a question.
struct QuestionTag: View {
    var body: some View {
        Text("질문").uiFont(10.5, weight: .semibold).foregroundStyle(Theme.ink2)
            .padding(.horizontal, 6).frame(height: 18).background(Theme.ink.opacity(0.06), in: Capsule())
    }
}

/// Under a question line: what Tacie found for it, its first part on one line.
struct FoundLine: View {
    let answer: String

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "arrow.turn.down.right").uiFont(10, weight: .semibold)
            Text("Tacie의 답").uiFont(11.5, weight: .semibold)
            Text("·").uiFont(11.5).foregroundStyle(Theme.ink4)
            Text(SavedMeeting.answerStart(answer)).uiFont(11.5).lineLimit(1).truncationMode(.tail)
        }
        .foregroundStyle(Theme.ink3)
        .padding(.top, 4)
    }
}

/// Who asked, over an answer or beside a question to write down: the small dot, the name and the time.
struct AskerLine: View {
    let name: String
    let key: String
    let color: Int
    let time: String

    var body: some View {
        HStack(spacing: 6) {
            SpeakerDot(name: name, key: key, color: color, size: 16)
            Text(verbatim: "\(name) · \(time)").uiFont(12).foregroundStyle(Theme.ink3)
        }
    }
}

// MARK: the namer (spec 5-3)

/// What the namer can change, from the window: the meeting model names speakers and lines, the vault keeps people.
struct SpeakerEditing {
    /// The remembered person the engine says a speaker of the meeting going on sounds closest to.
    var near: (String) -> String? = { _ in nil }
    /// A pick for the speaker `key`, or for `lines` only (이 줄만) when there are some.
    var apply: (NamerPick, _ key: String, _ lines: [String]) -> Void
    var renamePerson: (_ id: String, _ name: String) -> Void
    var forgetPerson: (_ id: String) -> Void
    /// What the last naming did, with 되돌리기 (⌘Z does the same).
    var toast: NamingToast? = nil
    var undo: () -> Void = {}
    var dropToast: (NamingToast) -> Void = { _ in }
    /// Unnamed speakers of a meeting that ended (by key after merges) with the remembered person they sound closest to
    /// (화자 추천): a dotted line under the name, ✓ and ✕ on hover, and first in the namer.
    var suggestions: [String: Person] = [:]
    var acceptSuggestion: (_ key: String, _ person: String) -> Void = { _, _ in }
    var dismissSuggestion: (_ key: String, _ person: String) -> Void = { _, _ in }
}

/// At the bottom of the page for a moment after a naming: what changed, and 되돌리기 with its key.
struct NamingToastView: View {
    let toast: NamingToast
    let undo: () -> Void
    let gone: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "checkmark").uiFont(11, weight: .bold).foregroundStyle(Theme.ink2)
            Text(toast.text).uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink).lineLimit(1)
            Rectangle().fill(Theme.hairline).frame(width: 1, height: 18)
            Button(action: undo) {
                HStack(spacing: 6) {
                    Text("되돌리기").uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink)
                    Text("⌘Z").uiFont(11, weight: .medium).foregroundStyle(Theme.ink3)
                        .padding(.horizontal, 5).frame(height: 18)
                        .background(Theme.ink.opacity(0.06), in: RoundedRectangle(cornerRadius: 4, style: .continuous))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(PressStyle())
        }
        .padding(.horizontal, 16).frame(height: 42)
        .background { Capsule().fill(Theme.raised).shadow(color: Theme.ink.opacity(0.1), radius: 14, y: 5) }
        .overlay(Capsule().strokeBorder(Theme.ink.opacity(0.1)))
        .appearanceScheme()
        .task(id: toast.id) {
            try? await Task.sleep(for: .seconds(6))
            guard !Task.isCancelled else { return }
            gone()
        }
    }
}

/// The naming popover (mock4 `Namer`): a field with the speaker's dot, the meeting's other speakers (picking one merges),
/// the remembered people (while remembering is on), and what the change will do. Opened for one line it starts with the
/// choice between all of the line's speaker's lines and this one.
struct Namer: View {
    let meeting: SavedMeeting
    let target: NamerTarget
    let people: [Person]
    let editing: SpeakerEditing
    let done: () -> Void

    @State private var query = ""
    @State private var all = true
    @State private var chosen: String?  // moved to with the arrow keys
    @State private var hover: String?
    @State private var menu: String?
    @State private var renaming: String?
    @State private var draft = ""
    @FocusState private var fieldFocused: Bool
    @FocusState private var draftFocused: Bool
    @AppStorage("rememberPeople") private var remember = true

    /// `query`, `all` and `hover` start elsewhere only when the namer is drawn on its own (a render of its states).
    init(meeting: SavedMeeting, target: NamerTarget, people: [Person], editing: SpeakerEditing,
         query: String = "", all: Bool = true, hover: String? = nil, done: @escaping () -> Void) {
        self.meeting = meeting
        self.target = target
        self.people = people
        self.editing = editing
        self.done = done
        _query = State(initialValue: query)
        _all = State(initialValue: all)
        _hover = State(initialValue: hover)
    }

    private var speaker: String { meeting.resolved(target.key) }
    /// Opened for a turn of a speaker who also spoke elsewhere: 전체 or 이 발언만.
    private var partial: Bool { !target.lines.isEmpty && meeting.speaks(target.key, beyond: target.lines) }
    private var name: String { meeting.displayName(key: speaker) }
    /// The suggested person (화자 추천) for the whole speaker, shown first while nothing is typed or it fits what is.
    private var suggested: Person? {
        guard !partial || all, let p = editing.suggestions[speaker] else { return nil }
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        return q.isEmpty || p.name.range(of: q, options: [.caseInsensitive, .widthInsensitive]) != nil ? p : nil
    }
    private var matches: (others: [(key: String, name: String)], invited: [String], people: [Person], highlighted: String?) {
        var m = namerMatches(query: query, near: editing.near(speaker), people: remember ? meeting.namerPeople(people) : [],
                             others: meeting.namerOthers(for: speaker), invited: meeting.invitedNames)
        if let s = suggested {
            m.people.removeAll { $0.id == s.id }
            if m.highlighted == nil || m.highlighted == s.id { m.highlighted = s.id }
        }
        return m
    }
    private var highlighted: String? { chosen ?? matches.highlighted }

    var body: some View {
        let m = matches
        VStack(alignment: .leading, spacing: 0) {
            if partial { scope }
            field
            if let s = suggested {
                NamerLabel(text: "추천")
                NamerRow(on: true, note: String(localized: "\(SavedMeeting.format("M월 d일", s.last, MainView.calendar)) 회의")) {
                    SpeakerDot(name: s.name, key: nil, color: s.color, size: 22)
                    Text(s.name).uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink).lineLimit(1)
                } more: {
                    if highlighted == s.id {
                        Text("↩").uiFont(11, weight: .semibold).foregroundStyle(Theme.suggest)
                    }
                }
                .background(Theme.suggestTint.opacity(highlighted == s.id || hover == s.id ? 1 : 0.6),
                            in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(Theme.suggest.opacity(highlighted == s.id ? 0.45 : 0.25)))
                .onHover { hover = $0 ? s.id : (hover == s.id ? nil : hover) }
                .onTapGesture { pick(.person(s.id)) }
            }
            if !m.others.isEmpty {
                NamerLabel(text: "이 회의의 다른 사람")
                ForEach(m.others, id: \.key) { o in
                    NamerRow(on: highlighted == o.key || hover == o.key, note: namerOthersNote(lineOnly: partial && !all)) {
                        SpeakerDot(name: o.name, key: o.key, color: meeting.speakerColor(o.key, people: people), size: 22)
                        Text(o.name).uiFont(13.5, weight: .medium).foregroundStyle(o.name == o.key ? Theme.ink2 : Theme.ink)
                            .lineLimit(1)
                    }
                    .onHover { hover = $0 ? o.key : (hover == o.key ? nil : hover) }
                    .onTapGesture { pick(.speaker(o.key)) }
                }
            }
            if !m.invited.isEmpty {
                NamerLabel(text: "이 회의에 초대된 사람")
                ForEach(m.invited, id: \.self) { name in
                    let id = namerInvitedID(name)
                    // no voice yet, so no colour: the calendar mark says where the name came from
                    NamerRow(on: highlighted == id || hover == id, note: "") {
                        Image(systemName: "calendar").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink3)
                            .frame(width: 22, height: 22)
                        Text(name).uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink).lineLimit(1)
                    }
                    .onHover { hover = $0 ? id : (hover == id ? nil : hover) }
                    .onTapGesture { pick(.typed(name)) }
                }
            }
            if !m.people.isEmpty {
                NamerLabel(text: "지난 회의 참석자")
                ForEach(m.people) { p in personRow(p) }
            }
            Rectangle().fill(Theme.hairline).frame(height: 1).padding(.vertical, 10)
            HStack(alignment: .firstTextBaseline, spacing: 7) {
                Image(systemName: "waveform").uiFont(11, weight: .medium).foregroundStyle(Theme.ink3)
                Text(footer).uiFont(11.5).foregroundStyle(Theme.ink3).lineSpacing(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 8)
        }
        .padding(10)
        .frame(width: 320)
        .appearanceScheme()
        .foregroundStyle(Theme.ink)
        .presentationBackground(Theme.paper)
        .onAppear { fieldFocused = true }
        .onChange(of: query) { chosen = nil }
    }

    /// 화자 2 전체 / 이 발언만, all first.
    private var scope: some View {
        HStack(spacing: 2) {
            segment("\(name) 전체", on: all) { all = true }
            segment("이 말만", on: !all) { all = false }
        }
        .padding(2).background(Theme.ink.opacity(0.055), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .padding(.bottom, 10)
    }

    private func segment(_ title: LocalizedStringKey, on: Bool, _ action: @escaping () -> Void) -> some View {
        Text(title).uiFont(12.5, weight: on ? .semibold : .medium).foregroundStyle(on ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3))
            .lineLimit(1)
            .frame(maxWidth: .infinity).frame(height: 26)
            .background {
                if on {
                    RoundedRectangle(cornerRadius: 7, style: .continuous).fill(Theme.chipOn)
                        .shadow(color: .black.opacity(0.08), radius: 1, y: 1)
                }
            }
            .contentShape(Rectangle())
            .onTapGesture(perform: action)
    }

    private var field: some View {
        HStack(spacing: 8) {
            SpeakerDot(name: name, key: speaker, color: meeting.speakerColor(speaker, people: people), size: 22)
            TextField("", text: $query, prompt: Text("이름을 쓰거나 아래에서 골라요").foregroundStyle(Theme.ink4))
                .textFieldStyle(.plain).uiFont(13.5).foregroundStyle(Theme.ink)
                .focused($fieldFocused)
                .onSubmit(submit)
                .onKeyPress(.downArrow) { move(1) }
                .onKeyPress(.upArrow) { move(-1) }
        }
        .padding(.horizontal, 8).frame(height: 38)
        .background(Theme.paper, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).stroke(Theme.ink.opacity(0.3), lineWidth: 1.5))
    }

    @ViewBuilder private func personRow(_ p: Person) -> some View {
        if renaming == p.id {
            HStack(spacing: 9) {
                SpeakerDot(name: draft.isEmpty ? p.name : draft, key: nil, color: p.color, size: 22)
                TextField("", text: $draft, prompt: Text(p.name).foregroundStyle(Theme.ink4))
                    .textFieldStyle(.plain).uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink)
                    .focused($draftFocused)
                    .onSubmit {
                        editing.renamePerson(p.id, draft)
                        renaming = nil
                        fieldFocused = true
                    }
                    .onExitCommand { renaming = nil; fieldFocused = true }
            }
            .padding(.horizontal, 8).frame(height: 34)
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Theme.ink.opacity(0.3), lineWidth: 1.5))
            .onAppear { draftFocused = true }
        } else {
            let lit = hover == p.id || menu == p.id
            NamerRow(on: highlighted == p.id || lit, note: String(localized: "\(SavedMeeting.format("M월 d일", p.last, MainView.calendar)) 회의")) {
                SpeakerDot(name: p.name, key: nil, color: p.color, size: 22)
                Text(p.name).uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink).lineLimit(1)
            } more: {
                if lit {
                    Button { menu = p.id } label: {
                        Image(systemName: "ellipsis").uiFont(12, weight: .semibold).foregroundStyle(Theme.ink2)
                            .frame(width: 24, height: 24)
                            .background(Theme.ink.opacity(0.08), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .popover(isPresented: Binding(get: { menu == p.id }, set: { if !$0 { menu = nil } }), arrowEdge: .trailing) {
                        PersonMenu(meetings: p.meetings, rename: {
                            menu = nil
                            draft = p.name
                            renaming = p.id
                        }, forget: {
                            menu = nil
                            editing.forgetPerson(p.id)
                        })
                    }
                }
            }
            .onHover { hover = $0 ? p.id : (hover == p.id ? nil : hover) }
            .onTapGesture { pick(.person(p.id)) }
        }
    }

    private var footer: String {
        let first = NamingText.footer(name: name, lineOnly: partial && !all, linePerson: SavedMeeting.isLinePerson(speaker))
        // 이 말만, or named on lines only (no voice of their own): nothing to recognise next time
        // no count: the strip shows each speaker's share (점유율 32%) and a line count here would be a second number for one thing
        guard remember, !(partial && !all), !SavedMeeting.isLinePerson(speaker) else { return first }
        return first + String(localized: " 다음 회의부터도 이 사람을 알아봐요.")
    }

    /// Return: the chosen entry, else what was typed as a new name.
    private func submit() {
        if let h = highlighted {
            let m = matches
            if m.others.contains(where: { $0.key == h }) {
                pick(.speaker(h))
            } else if let name = m.invited.first(where: { namerInvitedID($0) == h }) {
                pick(.typed(name))
            } else {
                pick(.person(h))  // the suggestion is a person too
            }
        } else if !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            pick(.typed(query))
        }
    }

    private func pick(_ p: NamerPick) {
        if case .person(let id) = p, let s = suggested, s.id == id {
            editing.acceptSuggestion(speaker, id)
        } else {
            editing.apply(p, speaker, partial && !all ? target.lines : [])
        }
        done()
    }

    private func move(_ by: Int) -> KeyPress.Result {
        let m = matches
        let ids = (suggested.map { [$0.id] } ?? []) + m.others.map(\.key) + m.invited.map(namerInvitedID) + m.people.map(\.id)
        guard !ids.isEmpty else { return .ignored }
        let at = highlighted.flatMap { ids.firstIndex(of: $0) }
        chosen = ids[max(0, min(ids.count - 1, at.map { $0 + by } ?? (by > 0 ? 0 : ids.count - 1)))]
        return .handled
    }
}

/// A section's name in the namer.
struct NamerLabel: View {
    let text: LocalizedStringKey

    var body: some View {
        Text(text).uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink3)
            .padding(.leading, 8).padding(.top, 10).padding(.bottom, 2)
    }
}

/// One entry of the namer: the dot and name, a note at the right, and room for the ⋯ button.
struct NamerRow<Lead: View, More: View>: View {
    let on: Bool
    let note: String
    @ViewBuilder let lead: Lead
    @ViewBuilder let more: More

    init(on: Bool, note: String, @ViewBuilder lead: () -> Lead, @ViewBuilder more: () -> More = { EmptyView() }) {
        self.on = on
        self.note = note
        self.lead = lead()
        self.more = more()
    }

    var body: some View {
        HStack(spacing: 9) {
            lead
            Spacer(minLength: 8)
            Text(note).uiFont(11.5).foregroundStyle(Theme.ink4).lineLimit(1)
            more
        }
        .padding(.horizontal, 8).frame(height: 34)
        .background(on ? Theme.fill : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .contentShape(Rectangle())
    }
}

/// Behind the ⋯ of a remembered person: rename, forget, and how many meetings they were in.
struct PersonMenu: View {
    let meetings: Int
    let rename: () -> Void
    let forget: () -> Void

    @State private var hover: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            item(String(localized: "이름 바꾸기"), action: rename)
            item(String(localized: "기억에서 지우기"), tint: Theme.warn, action: forget)
            Rectangle().fill(Theme.hairline).frame(height: 1).padding(.vertical, 5).padding(.horizontal, 6)
            Text("회의 \(meetings)번에 나왔어요").uiFont(11.5).foregroundStyle(Theme.ink3)
                .padding(.horizontal, 10).padding(.vertical, 3)
        }
        .padding(5)
        .frame(width: 180)
        .appearanceScheme()
        .presentationBackground(Theme.paper)
    }

    private func item(_ title: String, tint: Color = Theme.ink, action: @escaping () -> Void) -> some View {
        Text(title).uiFont(13).foregroundStyle(tint)
            .padding(.horizontal, 10).frame(maxWidth: .infinity, minHeight: 28, alignment: .leading)
            .background(hover == title ? Theme.fill : .clear, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .contentShape(Rectangle())
            .onHover { hover = $0 ? title : (hover == title ? nil : hover) }
            .onTapGesture(perform: action)
    }
}
