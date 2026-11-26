import Foundation

/// A remembered person to suggest for a speaker a meeting left as "Speaker N" (docs/04 section 4 "화자 추천"). The engine
/// names a speaker itself at 0.65; below that a close enough voice is only suggested, and the person confirms (✓) or
/// dismisses (✕) it on the meeting page. Measured on AMI (eval/voice_person_suggest.py, docs/01 "화자 추천"): at 0.50 with
/// the next person 0.10 behind, suggestions were right and strangers got one in 8 % of voices.
enum SpeakerSuggest {
    static let floor: Float = 0.50
    static let margin: Float = 0.10
    /// Seconds a speaker spoke in the meeting before it gets a suggestion (the engine's re-matching waits as long).
    static let minSeconds: Float = 8

    /// The pair key `dismissed` holds: a speaker of a meeting and the person not to suggest for it again.
    static func pair(_ key: String, _ person: String) -> String { key + "\u{1F}" + person }

    /// Speaker key (after merges) → person id, for the meeting's unnamed speakers. A person is suggested to one speaker
    /// only, the closest pair first; people some speaker of the meeting already is, or whose name a speaker shows, are
    /// left out. A dismissed pair is never suggested again, and the speaker gets no other suggestion then.
    static func make(meeting: SavedMeeting, prints: [MeetingPrint], people: [Person], dismissed: Set<String>) -> [String: String] {
        guard meeting.hasSpeakers, !people.isEmpty, !prints.isEmpty else { return [:] }
        let attendees = meeting.attendees
        let shown = Set(attendees.map(\.name))
        let taken = Set((meeting.voices ?? []).filter { $0.same == nil }.compactMap(\.person))
        let free = people.filter { !taken.contains($0.id) && !shown.contains($0.name) && !$0.vector.isEmpty }
        guard !free.isEmpty else { return [:] }

        var pairs: [(score: Float, key: String, person: String)] = []
        for a in attendees where a.name == a.key && !SavedMeeting.isLinePerson(a.key) {
            let ours = prints.filter { meeting.resolved($0.key) == a.key }
            let seconds = ours.reduce(0) { $0 + $1.seconds }
            guard seconds >= minSeconds, let v = VoiceVault.merge(ours.map { ($0.vector, $0.seconds) }) else { continue }
            if dismissed.contains(where: { $0.hasPrefix(a.key + "\u{1F}") }) { continue }
            let scored = free.compactMap { p in cosine(v, p.vector).map { ($0, p.id) } }.sorted { $0.0 > $1.0 }
            guard let best = scored.first, best.0 >= floor else { continue }
            let next = scored.count > 1 ? scored[1].0 : -1
            guard best.0 - next >= margin else { continue }
            pairs.append((best.0, a.key, best.1))
        }
        var out: [String: String] = [:], used: Set<String> = []
        for p in pairs.sorted(by: { $0.score > $1.score }) where !used.contains(p.person) {
            out[p.key] = p.person
            used.insert(p.person)
        }
        return out
    }

    /// Cosine of two vectors of the same length; nil when they differ or one is zero.
    static func cosine(_ a: [Float], _ b: [Float]) -> Float? {
        guard a.count == b.count, !a.isEmpty else { return nil }
        var dot: Float = 0, na: Float = 0, nb: Float = 0
        for i in 0..<a.count {
            dot += a[i] * b[i]
            na += a[i] * a[i]
            nb += b[i] * b[i]
        }
        guard na > 0, nb > 0 else { return nil }
        return dot / (na.squareRoot() * nb.squareRoot())
    }
}
