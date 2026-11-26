import Foundation

/// Who spoke in a saved meeting (design notes): each line carries the engine's speaker key ("화자 2", Speaker 2), and the
/// meeting keeps a table from key to the name the person gave it. A merged key points at the key it joined (`same`). A
/// line renamed on its own (This line only) keeps its name on the line. A line name that none of the meeting's speakers
/// goes by is someone the voices did not tell apart: they take part under a key of their own ("@지수", `linePersonKey`),
/// with no voice and nothing remembered.
extension SavedMeeting {
    /// The kinds of people's colours (Theme's speaker palette).
    static let speakerColors = 6

    /// The key a key ends up at after merges. A loop (which `merge` does not make) stops where it would repeat.
    func resolved(_ key: String) -> String {
        var key = key
        var seen: Set<String> = [key]
        while let next = voices?.first(where: { $0.key == key })?.same, seen.insert(next).inserted { key = next }
        return key
    }

    /// The name given to the speaker (or to the one it was merged into), else the key itself.
    func displayName(key: String) -> String {
        if Self.isLinePerson(key) { return String(key.dropFirst()) }
        let k = resolved(key)
        return voices?.first { $0.key == k }?.name ?? k
    }

    /// The key of someone named on lines only: a person the voices heard as part of another speaker.
    static func linePersonKey(_ name: String) -> String { "@" + name }
    static func isLinePerson(_ key: String) -> Bool { key.hasPrefix("@") }

    /// Who a line counts for: a line named on its own goes to the speaker who shows that name (its own speaker first),
    /// else to a person of its own (`linePersonKey`). Other lines go to their speaker, after merges.
    func owner(of l: TranscriptLine) -> String? { owner(of: l, names: speakerNames) }

    /// The lines that count for `key` (a speaker after merges, or someone named on lines only).
    func lines(of key: String) -> [String] {
        let names = speakerNames, k = Self.isLinePerson(key) ? key : resolved(key)
        return transcript.filter { owner(of: $0, names: names) == k }.map(\.key)
    }

    /// The name each speaker of the meeting shows, to the speaker's key (after merges); the first speaker heard keeps a
    /// name two of them share.
    private var speakerNames: [String: String] {
        var out: [String: String] = [:]
        for l in transcript {
            guard let k = l.voice.map(resolved) else { continue }
            let n = displayName(key: k)
            if out[n] == nil { out[n] = k }
        }
        return out
    }

    private func owner(of l: TranscriptLine, names: [String: String]) -> String? {
        guard let own = l.name else { return l.voice.map(resolved) }
        if let v = l.voice.map(resolved), displayName(key: v) == own { return v }
        return names[own] ?? Self.linePersonKey(own)
    }

    /// What a transcript line shows as its speaker: its own name, its speaker's, or nothing before speakers.
    func lineName(_ l: TranscriptLine) -> String? {
        l.name ?? l.voice.map { displayName(key: $0) }
    }

    /// True once any line has a speaker. Meetings saved before speakers, or without the voice model, show none.
    var hasSpeakers: Bool { transcript.contains { $0.voice != nil } }

    /// Who took part, in the order they first spoke, with their share of the talk in percent.
    /// Share counts characters (spaces aside), so a long answer weighs more than a short "네". The lines carry
    /// only a start time, so time spoken is not known. A speaker with any text shows at least 1.
    var attendees: [(key: String, name: String, share: Int)] {
        var keys: [String] = []
        var chars: [String: Int] = [:]
        let names = speakerNames
        let blank = CharacterSet.whitespacesAndNewlines  // once: the list asks every row's attendees on each change
        for l in transcript where l.voice != nil {
            guard let k = owner(of: l, names: names) else { continue }
            if chars[k] == nil { keys.append(k) }
            chars[k, default: 0] += l.text.unicodeScalars.reduce(0) { blank.contains($1) ? $0 : $0 + 1 }
        }
        let total = chars.values.reduce(0, +)
        return keys.map { k in
            let c = chars[k] ?? 0
            let share = total == 0 || c == 0 ? 0 : max(1, Int((Double(c) * 100 / Double(total)).rounded()))
            return (k, displayName(key: k), share)
        }
    }

    /// Names every line of the speaker (and of any merged into it). `person` is the remembered person, or nil.
    /// A blank name changes nothing.
    mutating func rename(key: String, to name: String, person: String?) {
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !n.isEmpty else { return }
        let k = resolved(key)
        let moved = lines(of: k).filter { id in transcript.contains { $0.key == id && $0.name != nil } }
        edit(k) {
            $0.name = n
            $0.person = person
        }
        carryLines(moved, to: n)
        // the speaker's own lines that were named this before (the name of someone on lines only) are its own again
        carryLines(transcript.filter { $0.name == n && $0.voice.map(resolved) == k }.map(\.key), to: n)
    }

    /// Names one line only (이 줄만). A blank name, or the name of the line's own speaker, gives the line back to it.
    mutating func renameLine(_ lineKey: String, to name: String) {
        guard let i = transcript.firstIndex(where: { $0.key == lineKey }) else { return }
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let own = transcript[i].voice.map { displayName(key: $0) }
        transcript[i].name = n.isEmpty || n == own ? nil : n
    }

    /// Lines named after a speaker keep following the speaker when its name changes.
    private mutating func carryLines(_ ids: [String], to name: String) {
        for id in ids { renameLine(id, to: name) }
    }

    /// The same person heard as two speakers: `key`'s lines show `target`'s name from now on.
    /// A name given to `key` and none to `target` goes over to `target`, so a name someone typed never disappears.
    /// Into someone named on lines only, the speaker takes that name: the two count as one from then on.
    mutating func merge(_ key: String, into target: String) {
        if Self.isLinePerson(target) {
            rename(key: key, to: displayName(key: target), person: nil)
            return
        }
        let from = resolved(key), to = resolved(target)
        guard from != to else { return }
        let moved = lines(of: from).filter { id in transcript.contains { $0.key == id && $0.name != nil } }
        let carried = voices?.first { $0.key == from }
        if voices?.first(where: { $0.key == to })?.name == nil, let name = carried?.name {
            edit(to) {
                $0.name = name
                $0.person = carried?.person
            }
        }
        edit(from) {
            $0.same = to
            $0.auto = nil
        }
        carryLines(moved, to: displayName(key: to))
    }

    /// A name typed in the namer. A speaker the engine linked to a remembered person keeps the link only while the
    /// name is still that person's: a different name means it was the wrong person.
    mutating func renameTyped(key: String, to name: String, people: [Person]) {
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let linked = voices?.first { $0.key == resolved(key) }?.person
        let keep = linked.flatMap { id in people.first { $0.id == id } }?.name == n ? linked : nil
        rename(key: key, to: n, person: keep)
    }

    // MARK: the namer

    /// What a turn's name opens the namer for: the speaker of its first line (a turn may join voices under one name),
    /// with the turn's lines for 이 발언만.
    /// A turn named on its own opens it for whoever shows that name (someone named on lines only, too).
    func namerTarget(turn t: Turn) -> NamerTarget? {
        guard let first = t.lines.first, first.voice != nil, let k = owner(of: first) else { return nil }
        return NamerTarget(key: k, lines: t.lines.map(\.key))
    }

    /// Whether `key` has lines outside `lines`: only then 전체 and 이 발언만 differ.
    func speaks(_ key: String, beyond lines: [String]) -> Bool {
        let these = Set(lines)
        return self.lines(of: key).contains { !these.contains($0) }
    }

    /// The meeting's other speakers the namer offers to merge with, in the order they first spoke.
    func namerOthers(for key: String) -> [(key: String, name: String)] {
        let k = resolved(key)
        return attendees.filter { $0.key != k }.map { ($0.key, $0.name) }
    }

    /// A speaker in the iPhone namer's list: how many lines count for them, and the first, to tell who they are.
    func namerLines(of key: String) -> (count: Int, first: TranscriptLine?) {
        let keys = lines(of: key)
        return (keys.count, keys.first.flatMap { k in transcript.first { $0.key == k } })
    }

    /// The remembered people the namer offers: those no speaker of this meeting already is.
    func namerPeople(_ people: [Person]) -> [Person] {
        let taken = Set((voices ?? []).filter { $0.same == nil }.compactMap(\.person))
        return people.filter { !taken.contains($0.id) }
    }

    /// How a turn shows its speaker: a line named on its own after another speaker of the meeting shows as that
    /// speaker, after a remembered person in their colour, else as the speaker who said it.
    /// Someone named on lines only takes a remembered person's colour when they share the name, else one of their own.
    func turnLook(_ t: Turn, people: [Person]) -> (key: String?, color: Int) {
        guard let first = t.lines.first, first.voice != nil, let k = owner(of: first) else {
            return (t.key, t.key.map { speakerColor($0, people: people) } ?? 0)
        }
        return (k, speakerColor(k, people: people))
    }

    /// A commitment's owner as shown: a speaker key gives its name (even a key with no lines: the key itself),
    /// older files' "me"/"them" give 나/상대, and a name said for someone outside the meeting stays as it is.
    func ownerName(_ owner: String) -> String {
        if owner.range(of: #"^화자 \d+$"#, options: .regularExpression) != nil { return displayName(key: owner) }
        return Self.who(owner)
    }

    /// A commitment's owner on the meeting page: as `ownerName`, except an in-person meeting's "me"/"them" (older files)
    /// shows no owner, as those meetings always did (one microphone heard everyone, so "me" said nothing of who).
    func shownOwner(_ owner: String) -> String? {
        if mode != "remote", owner == "me" || owner == "them" { return nil }
        return ownerName(owner)
    }

    /// The key a commitment's owner stands for: notes written again from the transcript name owners as the lines show
    /// them, so a name one of the meeting's speakers shows goes back to that speaker's key (after merges). Keys, names of
    /// someone outside the meeting and "me"/"them" stay as they are.
    func ownerKey(_ owner: String) -> String {
        if Self.speakerNumber(owner) != nil { return owner }
        let keys = attendees.map(\.key).filter { !Self.isLinePerson($0) } + (voices ?? []).map { resolved($0.key) }
        return keys.first { displayName(key: $0) == owner } ?? owner
    }

    /// Speakers linked to a remembered person who is no longer among `people` (forgotten) lose the link; their names stay.
    /// Returns the keys that lost it.
    @discardableResult
    mutating func unlink(keeping people: [Person]) -> Set<String> {
        let known = Set(people.map(\.id))
        var dropped: Set<String> = []
        for e in voices ?? [] {
            guard let p = e.person, !known.contains(p) else { continue }
            dropped.insert(e.key)
            edit(e.key) { $0.person = nil }
        }
        return dropped
    }

    /// Which of the palette's colours a speaker gets: a remembered person keeps theirs across meetings,
    /// anyone else goes by number, 화자 1 first.
    static func colorIndex(for key: String, person: String?, personColor: Int?) -> Int {
        if person != nil, let c = personColor { return ((c % speakerColors) + speakerColors) % speakerColors }
        let n = Int(key.split(separator: " ").last ?? "") ?? 1
        return ((n - 1) % speakerColors + speakerColors) % speakerColors
    }

    /// Lines in a row said by the same name, as the transcript shows them.
    struct Turn {
        let key: String?  // the first line's speaker, after merges
        let name: String?
        let at: Date
        let lines: [TranscriptLine]
        let question: Bool
    }

    var turns: [Turn] {
        var out: [Turn] = []
        for l in transcript {
            let name = lineName(l)
            if let t = out.last, t.name == name {
                out[out.count - 1] = Turn(key: t.key, name: name, at: t.at, lines: t.lines + [l], question: t.question || l.question)
            } else {
                out.append(Turn(key: l.voice.map(resolved), name: name, at: l.at, lines: [l], question: l.question))
            }
        }
        return out
    }

    // MARK: the meeting page

    /// The palette colour a speaker of this meeting shows in: its remembered person's (after merges), else by number.
    /// Someone named on lines only: a remembered person's colour by name, else the colours after the speakers', in the
    /// order they first spoke.
    func speakerColor(_ key: String, people: [Person]) -> Int {
        if Self.isLinePerson(key) {
            let name = displayName(key: key)
            if let p = people.first(where: { $0.name == name }) { return Self.colorIndex(for: key, person: p.id, personColor: p.color) }
            let order = attendees.map(\.key).filter(Self.isLinePerson).firstIndex(of: key) ?? 0
            return Self.colorIndex(for: "화자 \(highestSpeaker + order + 1)", person: nil, personColor: nil)
        }
        let k = resolved(key)
        let person = voices?.first { $0.key == k }?.person
        return Self.colorIndex(for: k, person: person, personColor: people.first { $0.id == person }?.color)
    }

    /// The letter in a speaker's dot: the name's first letter, or the number while the speaker goes by its key.
    static func speakerMark(name: String, key: String?) -> String {
        if name == key, let n = speakerNumber(name) { return String(n) }
        return name.first.map { String($0).uppercased() } ?? ""
    }

    /// What Tacie found for a question line of the transcript: the answer to the question with the same words, else to one
    /// the line holds (a line joined with the words before it keeps growing; the question was asked with the shorter words).
    func foundAnswer(for l: TranscriptLine) -> String? {
        guard l.question else { return nil }
        let text = l.text.trimmingCharacters(in: .whitespacesAndNewlines)
        let found = questions.filter { !$0.unanswered && $0.answer != nil }
        let asked = { (q: SavedQuestion) in q.question.trimmingCharacters(in: .whitespacesAndNewlines) }
        return (found.first { asked($0) == text } ?? found.first { !asked($0).isEmpty && text.contains(asked($0)) })?.answer
    }

    /// The first part of an answer, for the line under its question: no citations, links or marks, cut at a word, then "…".
    static func answerStart(_ answer: String, limit: Int = 24) -> String {
        var s = answer.replacingOccurrences(of: #"\s*\(?https?://[^\s)]+\)?"#, with: "", options: .regularExpression)
            .replacingOccurrences(of: #"\s*\[\d+\]"#, with: "", options: .regularExpression)
            .replacingOccurrences(of: #"\*\*|__|`"#, with: "", options: .regularExpression)
            .replacingOccurrences(of: #"\s+"#, with: " ", options: .regularExpression)
            .trimmingCharacters(in: .whitespaces)
        if s.count > limit {
            let cut = s.prefix(limit + 1)
            s = cut.last == " " ? String(cut) : cut.lastIndex(of: " ").map { String(cut[..<$0]) } ?? String(s.prefix(limit))
        }
        return s.trimmingCharacters(in: CharacterSet(charactersIn: " .,!?·…")) + "…"
    }

    // MARK: the live meeting and the engine

    /// The names table the engine gets during a meeting: every key whose shown name is not the key itself, merged keys
    /// too (a key merged into an unnamed one goes by that one's key).
    func engineNames() -> [String: String] {
        let keys = Set(transcript.compactMap(\.voice)).union((voices ?? []).map(\.key))
        var out: [String: String] = [:]
        for k in keys {
            let n = displayName(key: k)
            if n != k { out[k] = n }
        }
        return out
    }

    /// The highest "화자 N" any line has, 0 before speakers: a renewed session numbers new voices after it.
    var highestSpeaker: Int {
        transcript.compactMap { $0.voice.flatMap(Self.speakerNumber) }.max() ?? 0
    }

    /// The engine's ids ("s1u3") of the lines named on their own (이 줄만) heard in session `epoch`: they stay out of
    /// their speaker's voice. Lines of an earlier session are not that session's to leave out.
    func ownLines(epoch: Int) -> [String] {
        let prefix = "e\(epoch):"
        return transcript.filter { $0.name != nil && $0.key.hasPrefix(prefix) }.map { String($0.key.dropFirst(prefix.count)) }
    }

    /// The engine knew a speaker as a remembered person: the speaker takes their name unless it (or the speaker it was
    /// merged into) already has one. True when the table changed.
    mutating func adopt(key: String, name: String, person: String) -> Bool {
        let k = resolved(key)
        guard voices?.first(where: { $0.key == k })?.name == nil else { return false }
        rename(key: k, to: name, person: person)
        return voices?.first(where: { $0.key == k })?.name != nil
    }

    /// Who said a line, for notes written again from the transcript: the line's name when it has a speaker,
    /// else the channel ("me"/"them") of a meeting saved before speakers.
    func notesSpeaker(_ l: TranscriptLine) -> String? {
        l.voice != nil ? lineName(l) : l.speaker
    }

    // MARK: people in the notes' prose

    /// A person the notes name: `[[이름]]` as the engine sends it (notes_overview_v2), `[[화자 2]]` as kept.
    static let personRef = try! NSRegularExpression(pattern: #"\[\[([^\[\]]{1,40})\]\]"#)

    /// The key a name the notes wrote stands for: a speaker key as it is, else the attendee (or named speaker) that
    /// shows the name now; nil for someone who is not one of the meeting's people.
    func refKey(_ name: String) -> String? {
        if Self.speakerNumber(name) != nil { return name }
        if let a = attendees.first(where: { $0.name == name }) { return a.key }
        return (voices ?? []).map { resolved($0.key) }.first { displayName(key: $0) == name }
    }

    /// Notes text as the meeting keeps it: each `[[이름]]` of one of its people becomes `[[key]]`, so renaming a
    /// speaker later renames them in the summary too; any other `[[이름]]` is just the name.
    func keyedRefs(_ text: String) -> String {
        Self.replaceRefs(text) { name in refKey(name).map { "[[\($0)]]" } ?? name }
    }

    /// Notes text as the page shows it: each `[[key]]` is the name the person goes by now, and the particle right
    /// after it fits that name ("지안가" → "지안이" once 화자 1 is 지안).
    func shownRefs(_ text: String) -> String {
        Self.replaceRefs(text, fitParticles: true) { key in displayName(key: key) }
    }

    /// Particles that change with the word before them: (after a final consonant, after a vowel). Longest first.
    private static let particles: [(String, String)] = [
        ("으로", "로"), ("이랑", "랑"), ("과", "와"), ("을", "를"), ("은", "는"), ("이", "가"),
    ]

    /// Whether a name ends in a final consonant as it is read, and whether that is ㄹ; nil when it cannot be told
    /// (a Latin letter): the model's particle stays then.
    private static func finalConsonant(_ name: String) -> (has: Bool, rieul: Bool)? {
        guard let last = name.unicodeScalars.last else { return nil }
        let v = Int(last.value)
        if (0xAC00...0xD7A3).contains(v) {
            let jong = (v - 0xAC00) % 28
            return (jong != 0, jong == 8)
        }
        // digits as read: 영 일 이 삼 사 오 육 칠 팔 구
        let digits: [Character: (Bool, Bool)] = ["0": (true, false), "1": (true, true), "2": (false, false), "3": (true, false),
                                                 "4": (false, false), "5": (false, false), "6": (true, false), "7": (true, true),
                                                 "8": (true, true), "9": (false, false)]
        return digits[Character(last)].map { (has: $0.0, rieul: $0.1) }
    }

    /// The particle at the start of `rest` made to fit `name`, and how long it was.
    private static func fittedParticle(after name: String, in rest: Substring) -> (String, Int)? {
        guard let end = finalConsonant(name) else { return nil }
        for (withC, withoutC) in particles {
            for form in [withC, withoutC] where rest.hasPrefix(form) {
                let fit = withC == "으로" && end.rieul ? withoutC : end.has ? withC : withoutC
                return (fit, form.count)
            }
        }
        return nil
    }

    /// The summary and topics as shown (`shownRefs`).
    var shownSummary: [String] { summary.map(shownRefs) }
    var shownTopics: [NoteTopic]? {
        topics?.map { NoteTopic(title: $0.title, points: $0.points.map(shownRefs), roles: $0.roles) }
    }

    private static func replaceRefs(_ text: String, fitParticles: Bool = false, _ with: (String) -> String) -> String {
        guard text.contains("[[") else { return text }
        let ns = text as NSString
        var out = "", last = 0
        for m in personRef.matches(in: text, range: NSRange(location: 0, length: ns.length)) {
            out += ns.substring(with: NSRange(location: last, length: m.range.location - last))
            let name = with(ns.substring(with: m.range(at: 1)).trimmingCharacters(in: .whitespaces))
            out += name
            last = m.range.location + m.range.length
            if fitParticles, let (fit, n) = fittedParticle(after: name, in: Substring(ns.substring(from: last))) {
                out += fit
                last += (ns.substring(from: last).prefix(n) as Substring).utf16.count
            }
        }
        return out + ns.substring(from: last)
    }

    private static func speakerNumber(_ key: String) -> Int? {
        guard key.range(of: #"^화자 \d+$"#, options: .regularExpression) != nil else { return nil }
        return Int(key.dropFirst("화자 ".count))
    }

    private mutating func edit(_ key: String, _ change: (inout VoiceEntry) -> Void) {
        var all = voices ?? []
        if let i = all.firstIndex(where: { $0.key == key }) {
            change(&all[i])
        } else {
            var e = VoiceEntry(key: key)
            change(&e)
            all.append(e)
        }
        voices = all
    }
}

/// The note beside another speaker of the meeting in the namer: picking one merges the two, or with 이 줄만 moves just the line.
func namerOthersNote(lineOnly: Bool) -> String {
    lineOnly ? String(localized: "이 말만 이 사람으로 바꿔요") : String(localized: "같은 사람이면 합쳐요")
}

/// What the namer lists for what was typed: the meeting's other speakers, then the people the calendar event invited
/// (docs/35 3-4), then remembered people, keeping those whose name holds the typed letters (case folded). An invited
/// name a speaker or a remembered person already carries is left out. The person the engine says the voice sounds
/// closest to (`near`) comes first among the people. With something typed the first match is chosen (a speaker's
/// key, `invited:<name>`, or a person's id); with nothing, none is.
func namerMatches(query: String, near: String? = nil, people: [Person], others: [(key: String, name: String)],
                  invited: [String] = [])
    -> (others: [(key: String, name: String)], invited: [String], people: [Person], highlighted: String?) {
    let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
    let fits = { (name: String) in q.isEmpty || name.range(of: q, options: [.caseInsensitive, .diacriticInsensitive, .widthInsensitive]) != nil }
    let o = others.filter { fits($0.name) }
    let taken = Set((others.map(\.name) + people.map(\.name)).map { $0.lowercased() })
    let i = invited.filter { !taken.contains($0.lowercased()) && fits($0) }
    var p = people.filter { fits($0.name) }
    if let near, let n = p.firstIndex(where: { $0.id == near }) { p.insert(p.remove(at: n), at: 0) }
    // the voice's closest person, when it fits what was typed, stays the default over an invited name
    let nearFits = near.flatMap { n in p.first { $0.id == n } }
    let first = q.isEmpty ? nil : (o.first?.key ?? nearFits?.id ?? i.first.map { namerInvitedID($0) } ?? p.first?.id)
    return (o, i, p, first)
}

/// The namer's handle on an invited name (so it can be chosen with the arrow keys like the other rows).
func namerInvitedID(_ name: String) -> String { "invited:" + name }

/// Opens an engine session with the remembered people; when that fails, once more without them. The people are a help,
/// never a reason to lose the meeting. Without people the error stands.
func openSession(people: [(id: String, vector: [Float])],
                 _ open: ([(id: String, vector: [Float])]) async throws -> String) async throws -> String {
    do {
        return try await open(people)
    } catch where !people.isEmpty {
        Log.info("session with remembered people failed (\(error)): opening without them")
        return try await open([])
    }
}

/// What the namer gave a speaker or a line: a typed name, another speaker of the meeting (by key), or a remembered
/// person (by id).
enum NamerPick: Equatable {
    case typed(String)
    case speaker(String)
    case person(String)
}

/// Who the namer names: a speaker (`line` nil), or one transcript line whose speaker is `key`.
struct NamerTarget: Equatable {
    let key: String
    /// The lines 이 발언만 changes (a turn's); none when the namer is for the speaker only (a chip).
    var lines: [String] = []
}

/// Korean particles that change with the last sound of the word before them.
enum Korean {
    /// "민재로", "화자 2로", "민준으로": 으로 after a final consonant other than ㄹ, 로 otherwise.
    static func withRo(_ word: String) -> String {
        guard let last = word.unicodeScalars.last else { return word }
        let v = Int(last.value)
        if (0xAC00...0xD7A3).contains(v) {
            let coda = (v - 0xAC00) % 28
            return word + (coda == 0 || coda == 8 ? "로" : "으로")  // 8 is ㄹ
        }
        // digits as read aloud: 1 일, 3 삼, 6 육, 7 칠, 8 팔, 0 영 end in a consonant; 1·7·8 in ㄹ
        if let d = Int(String(last)), "0123456789".contains(String(last)) {
            return word + ([0, 3, 6].contains(d) ? "으로" : "로")
        }
        return word + "로"
    }

    /// The 로/으로 alone, for a word shown in quotes ("나"로).
    static func ro(_ word: String) -> String { String(withRo(word).dropFirst(word.count)) }

    /// The particle alone, for a word shown in quotes ("나"와).
    static func particle(_ word: String, _ afterConsonant: String, _ afterVowel: String) -> String {
        String(with(word, afterConsonant, afterVowel).dropFirst(word.count))
    }

    /// "화자 2를", "민준을": the first form after a final consonant (ㄹ too), the second otherwise.
    static func with(_ word: String, _ afterConsonant: String, _ afterVowel: String) -> String {
        guard let last = word.unicodeScalars.last else { return word }
        let v = Int(last.value)
        if (0xAC00...0xD7A3).contains(v) { return word + ((v - 0xAC00) % 28 == 0 ? afterVowel : afterConsonant) }
        // digits as read aloud: 0 영, 1 일, 3 삼, 6 육, 7 칠, 8 팔 end in a consonant
        if let d = Int(String(last)), "0123456789".contains(String(last)) {
            return word + ([0, 1, 3, 6, 7, 8].contains(d) ? afterConsonant : afterVowel)
        }
        return word + afterVowel
    }
}

/// What the namer says, on the Mac and the iPhone: the line after a naming ("화자 2를 민호로 바꿨어요", with 되돌리기)
/// and the note under the namer. Korean fits the particles to the names; English puts the names in.
enum NamingText {
    /// After a naming: `old` is the name the speaker showed before, `new` the name it (or the line) shows now.
    static func done(_ pick: NamerPick, old: String, new: String, lineOnly: Bool) -> String {
        let new = new.trimmingCharacters(in: .whitespaces)
        if lineOnly {
            return AppLocale.isKorean ? "이 말을 \(Korean.with(new, "이", "가")) 한 말로 바꿨어요"
                : String(localized: "naming.lineRenamed", defaultValue: "이 말을 \(new)이(가) 한 말로 바꿨어요")
        }
        if case .speaker = pick {
            return AppLocale.isKorean ? "\(Korean.with(old, "을", "를")) \(Korean.with(new, "과", "와")) 합쳤어요"
                : String(localized: "naming.merged", defaultValue: "\(old)을 \(new)과 합쳤어요")
        }
        return AppLocale.isKorean ? "\(Korean.with(old, "을", "를")) \(Korean.withRo(new)) 바꿨어요"
            : String(localized: "naming.renamed", defaultValue: "\(old)을 \(new)(으)로 바꿨어요")
    }

    /// Above the iPhone namer's button: what the pick will change. `speaker` is the name the speaker shows, `lines` how
    /// many lines count for it, `target` the typed name or the other speaker's name; `merge` when another speaker was
    /// picked for all of them, `lineOnly` for 이 말만.
    static func preview(speaker: String, lines: Int, target: String, merge: Bool, lineOnly: Bool) -> String {
        let q = "\"\(target)\""
        if lineOnly {
            return AppLocale.isKorean ? "이 말 한 줄만 \(q)\(Korean.particle(target, "이", "가")) 한 말로 바꿔요. \(speaker)의 다른 말은 그대로예요."
                : String(localized: "naming.preview.line", defaultValue: "이 말 한 줄만 \(q)이(가) 한 말로 바꿔요. \(speaker)의 다른 말은 그대로예요.")
        }
        if merge {
            return AppLocale.isKorean
                ? "\(Korean.with(speaker, "과", "와")) \(q)\(Korean.particle(target, "을", "를")) 한 사람으로 합쳐요. \(speaker)의 말 \(lines)줄이 \(q)\(Korean.ro(target)) 바뀌어요."
                : String(localized: "naming.preview.merge", defaultValue: "\(speaker)의 말 \(lines)줄을 \(q)(으)로 합쳐요.")
        }
        return AppLocale.isKorean ? "\(speaker)의 말 \(lines)줄이 모두 \(q)\(Korean.ro(target)) 바뀌어요."
            : String(localized: "naming.preview.rename", defaultValue: "\(speaker)의 말 \(lines)줄이 모두 \(q)(으)로 바뀌어요.")
    }

    /// The iPhone namer's button for a pick (as `preview`).
    static func button(target: String, merge: Bool, lineOnly: Bool) -> String {
        let q = "\"\(target)\""
        if lineOnly {
            return AppLocale.isKorean ? "\(q)\(Korean.particle(target, "이", "가")) 한 말로 바꾸기"
                : String(localized: "naming.button.line", defaultValue: "\(q)이(가) 한 말로 바꾸기")
        }
        if merge {
            return AppLocale.isKorean ? "\(q)\(Korean.particle(target, "과", "와")) 합치기"
                : String(localized: "naming.button.merge", defaultValue: "\(q)과(와) 합치기")
        }
        return AppLocale.isKorean ? "\(q)\(Korean.ro(target)) 바꾸기"
            : String(localized: "naming.button.rename", defaultValue: "\(q)(으)로 바꾸기")
    }

    /// Under the namer: what a pick will change. `linePerson` is someone named on lines only (no voice of their own).
    static func footer(name: String, lineOnly: Bool, linePerson: Bool) -> String {
        if lineOnly { return String(localized: "이 말만 바꿔요. 나머지는 그대로예요.") }
        if !AppLocale.isKorean {
            return linePerson ? String(localized: "speakerNamer.allNamed", defaultValue: "\(name)(으)로 이름 붙인 말을 모두 바꿔요.")
                : String(localized: "speakerNamer.allFrom", defaultValue: "\(name)(으)로 나온 말을 모두 바꿔요.")
        }
        return linePerson ? "\(Korean.withRo(name)) 이름 붙인 말을 모두 바꿔요." : "\(Korean.withRo(name)) 나온 말을 모두 바꿔요."
    }
}
