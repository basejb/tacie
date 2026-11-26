import Foundation

/// The iPhone's live text in a meeting (what Soniox is hearing before the engine's line comes back), kept apart so it
/// can be tested without a stream. `held` are the sentences already cut, oldest first; `said` and `partial` are the
/// final and partial words of the one being said now, as the current Soniox connection last sent them.
///
/// At a cut the words on screen are held while Soniox finalizes. Until its `<fin>`, Soniox may still send those same
/// words as the sentence being said: they are hidden then, or the sentence would show twice. After `<fin>` the live
/// words are a new sentence and always show, even when they repeat the held one ("네" after "네 맞아요", "yes" after "yes,
/// right").
struct LiveText: Equatable {
    struct Held: Equatable {
        let id: Int
        var text: String
        /// Cut, and Soniox has not closed it with `<fin>` yet.
        var awaitingFin: Bool
    }

    private(set) var held: [Held] = []
    private(set) var said = ""
    private(set) var partial = ""

    init(held: [Held] = []) { self.held = held }

    /// The current connection's words for the sentence being said.
    mutating func heard(said: String, partial: String) {
        self.said = said.trimmingCharacters(in: .whitespaces)
        self.partial = partial.trimmingCharacters(in: .whitespaces)
    }

    /// A new connection took over. The old one no longer writes the live text, so the words it last sent (perhaps the
    /// echo of a cut sentence, which its `<fin>` would have cleared) go now.
    mutating func swapped() { heard(said: "", partial: "") }

    /// The utterance `id` was cut: the words on screen are held (nothing when there are none).
    mutating func cut(id: Int) {
        let text = [said, partial].filter { !$0.isEmpty }.joined(separator: " ")
        guard !text.isEmpty else { return }
        held.append(Held(id: id, text: text, awaitingFin: true))
        held.sort { $0.id < $1.id }
    }

    /// Soniox closed the utterance `id`: its transcript takes the cut text's place. No transcript: the cut text stays
    /// until the engine's line arrives. `streamStopped`: the connection stopped (no `<fin>` in time, a receive error)
    /// and no `<fin>` will clear its words, so they go now; they are the held sentence's echo.
    mutating func finished(id: Int, transcript: String?, streamStopped: Bool = false) {
        let t = transcript?.trimmingCharacters(in: .whitespaces) ?? ""
        if let i = held.firstIndex(where: { $0.id == id }) {
            if !t.isEmpty { held[i].text = t }
            held[i].awaitingFin = false
        } else if !t.isEmpty {
            held.append(Held(id: id, text: t, awaitingFin: false))
            held.sort { $0.id < $1.id }
        }
        if t.isEmpty, streamStopped { heard(said: "", partial: "") }
    }

    /// The engine's line for `id` came back (or failed): it leaves the live text.
    mutating func done(id: Int) { held.removeAll { $0.id == id } }

    /// The live text as shown: the final part (held sentences, then `said`) and the grey partial tail.
    var shown: (final: String, partial: String) { Self.compose(held: held, said: said, partial: partial) }

    static func compose(held: [Held], said: String, partial: String) -> (final: String, partial: String) {
        var said = said.trimmingCharacters(in: .whitespaces)
        var partial = partial.trimmingCharacters(in: .whitespaces)
        if let cut = held.last, cut.awaitingFin {
            // Soniox re-sending the sentence just cut, perhaps corrected in its spacing ("회 사" → "회사")
            let c = squeezed(cut.text)
            if !said.isEmpty, c.hasPrefix(squeezed(said)) { said = "" }
            if !partial.isEmpty, c.hasSuffix(squeezed(partial)) { partial = "" }
        }
        let final = (held.map(\.text) + [said]).filter { !$0.isEmpty }.joined(separator: " ")
        return (final, partial)
    }

    private static func squeezed(_ s: String) -> String { s.filter { !$0.isWhitespace } }
}
