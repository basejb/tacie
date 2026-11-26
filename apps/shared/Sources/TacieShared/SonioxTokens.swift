import Foundation

/// One token of a Soniox real-time reply (docs/25). Tokens are pieces of words, not words: a new word starts at a
/// token that begins with a space ("회", "사", "가", " 좀"). Times are ms of audio sent on the connection.
struct SonioxToken: Decodable, Equatable {
    let text: String
    let startMs: Int?
    let endMs: Int?
    let isFinal: Bool
    let confidence: Double?
    /// With translation on (docs/30): "translation" for translated text (no times), "original" or "none" for speech.
    var translationStatus: String? = nil
    /// A translated token's source language ("en").
    var sourceLanguage: String? = nil

    /// Translated text, not speech: it stays out of the utterance's text and words.
    var isTranslation: Bool { translationStatus == "translation" }

    enum CodingKeys: String, CodingKey {
        case text, confidence
        case startMs = "start_ms", endMs = "end_ms", isFinal = "is_final"
        case translationStatus = "translation_status", sourceLanguage = "source_language"
    }

    init(text: String, startMs: Int?, endMs: Int?, isFinal: Bool, confidence: Double?) {
        self.text = text
        self.startMs = startMs
        self.endMs = endMs
        self.isFinal = isFinal
        self.confidence = confidence
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        text = try c.decode(String.self, forKey: .text)
        startMs = try c.decodeIfPresent(Int.self, forKey: .startMs)
        endMs = try c.decodeIfPresent(Int.self, forKey: .endMs)
        isFinal = try c.decodeIfPresent(Bool.self, forKey: .isFinal) ?? false
        confidence = try c.decodeIfPresent(Double.self, forKey: .confidence)
        translationStatus = try c.decodeIfPresent(String.self, forKey: .translationStatus)
        sourceLanguage = try c.decodeIfPresent(String.self, forKey: .sourceLanguage)
    }
}

/// Turning the Utterancer's samples into what Soniox takes, and Soniox's tokens back into a line and its words.
enum SonioxTokens {
    /// 16 kHz float samples as little-endian 16-bit PCM (`pcm_s16le`), clamped to ±1.
    static func pcm16(_ samples: [Float]) -> Data {
        var out = Data(capacity: samples.count * 2)
        for s in samples {
            let v = Int16((max(-1, min(1, s)) * 32767).rounded()).littleEndian
            withUnsafeBytes(of: v) { out.append(contentsOf: $0) }
        }
        return out
    }

    /// The tokens as one line.
    static func text(_ tokens: [SonioxToken]) -> String {
        tokens.map(\.text).joined().trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Words in the recording: tokens joined until the next one that starts with a space. `offsetMs` is where Soniox
    /// time 0 sits on the Utterancer's stream clock, so the words land where `SpokenWord` expects them.
    static func words(_ tokens: [SonioxToken], offsetMs: Int) -> [SpokenWord] {
        var words: [SpokenWord] = []
        var text = "", start: Int?, end: Int?
        func flush() {
            let t = text.trimmingCharacters(in: .whitespaces)
            if !t.isEmpty, let s = start, let e = end { words.append(SpokenWord(text: t, start: s + offsetMs, end: e + offsetMs)) }
            text = ""; start = nil; end = nil
        }
        for tok in tokens {
            if tok.text.first == " " { flush() }
            text += tok.text
            if start == nil { start = tok.startMs }
            end = tok.endMs ?? end
        }
        flush()
        return words
    }
}
