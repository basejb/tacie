import Foundation

extension SavedMeeting {
    /// Who the speakers are and what each line is named: what naming changes, so it can be put back (되돌리기).
    struct Naming: Equatable {
        let voices: [VoiceEntry]?
        let lineNames: [String: String]
        /// Each line's speaker key: naming lines on their own during a meeting moves them to another voice (`split`).
        var lineVoices: [String: String] = [:]
    }

    var naming: Naming {
        Naming(voices: voices,
               lineNames: Dictionary(transcript.compactMap { l in l.name.map { (l.key, $0) } }, uniquingKeysWith: { a, _ in a }),
               lineVoices: Dictionary(transcript.compactMap { l in l.voice.map { (l.key, $0) } }, uniquingKeysWith: { a, _ in a }))
    }

    /// Puts the speakers, line names and line speakers back. Lines heard since keep no name of their own and their speaker.
    mutating func restore(_ n: Naming) {
        voices = n.voices
        for i in transcript.indices {
            transcript[i].name = n.lineNames[transcript[i].key]
            if let v = n.lineVoices[transcript[i].key] { transcript[i].voice = v }
        }
    }
}
