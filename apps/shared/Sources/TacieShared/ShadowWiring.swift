import Foundation

/// What MeetingModel decides about the development build's Soniox shadow (docs/25 "그림자 모드"), kept apart so it can
/// be tested without an engine or a socket.
/// Where a meeting's speech is transcribed (docs/25). `stream`: Soniox real-time first, the engine's file path when
/// Soniox has no text. `shadow` (dev only): the file path, with Soniox alongside for comparison. `server`: the file path.
enum SttPath: Equatable { case stream, shadow, server }

enum ShadowWiring {
    /// The AI choice changed during a meeting: a Soniox connection that the new choice would not open closes at once,
    /// so a switch to on-device processing (or away from Soniox) sends no more of the meeting's sound out.
    static func stopsOnChange(current: SttPath, next: SttPath) -> Bool { current != .server && next == .server }

    /// Practice, the sample meeting and on-device recognition keep the file path everywhere. A shipped app streams; a
    /// development build follows the developer page (`DevSettings.sttPath`: nil or "stream", "shadow", "server").
    /// The open edition's provider (2-B, `stt.provider`): ElevenLabs transcribes utterances (file path), Soniox streams.
    /// While the on-device model is still downloading, a Soniox user streams: the "server" that covers that time in the
    /// official app does not exist in the open edition. Without a provider every rule is today's.
    /// The open edition has no Soniox of its own (2-D): without a provider ("키 하나로 쓰기") or with OpenRouter it
    /// transcribes utterances through OpenRouter scribe-v2.
    static func path(isDev: Bool, choice: String?, practice: Bool, sample: Bool, localStt: Bool,
                     localReady: Bool = true, provider: String? = nil, open: Bool = false) -> SttPath {
        if practice || sample { return .server }
        if localStt { return !localReady && provider == "soniox" ? .stream : .server }
        switch provider {
        case "elevenlabs": return .server
        case "soniox": return .stream
        default: if open { return .server }
        }
        guard isDev else { return .stream }
        switch choice {
        case "shadow": return .shadow
        case "server": return .server
        default: return .stream
        }
    }

    /// What Soniox heard as the utterance's transcript for the engine (word times in seconds from the utterance start),
    /// or nil when it heard nothing: then the engine transcribes the WAV itself.
    static func transcript(_ r: ShadowResult) -> StreamTranscript? {
        // Every word here was finalized after the previous `<fin>`, so no other line has it: a word that ends before the
        // VAD's start is still this utterance's, and is kept at 0. Silero marks speech 0.1–0.3 s late and the pre-roll
        // is only about 140 ms, so a short first word ("네", "근데") often ends before the start; dropping it lost it for
        // good, since the meeting's end does not transcribe again ("네 그거는" was saved as "그거는", 2026-11-17).
        // A fragment shorter than 400 ms is not sent on its own (Utterancer `minFrames`) and its words come here too.
        // Bound: such a fragment plus the 700 ms pause that ends it plus the late start fit well inside 3 s; a word
        // older than that is noise the VAD never took for speech, and is dropped.
        let kept = r.words.filter { $0.end > r.startMs - earliestWordMs }
        #if DEBUG
        if kept.count < r.words.count { Log.info("stream: \(r.words.count - kept.count) words before the utterance dropped") }
        #endif
        let text = (kept.count == r.words.count || r.words.isEmpty ? r.text : kept.map(\.text).joined(separator: " "))
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return nil }
        let words = kept.map { StreamTranscript.Word(text: $0.text, start: Double(max($0.start - r.startMs, 0)) / 1000,
                                                     end: Double(max($0.end - r.startMs, 0)) / 1000) }
        return StreamTranscript(text: text, words: words, translation: r.translation, translatedFrom: r.translatedFrom)
    }

    /// How far before the utterance's start a word may end and still be kept (`transcript`).
    static let earliestWordMs = 3000

    /// A Soniox connection lives 10 minutes (its temporary key's session cap): a new one takes over after 9.
    static func shouldRotate(ageSeconds: Double) -> Bool { ageSeconds >= 540 }

    /// Seconds to wait before opening a connection again after `failures` failed ones in this meeting; nil: stop, the
    /// meeting stays on the file path. Every key the relay mints costs money, so a connection that keeps failing
    /// (a firewall that blocks only Soniox) must not take a new key every few seconds.
    static func retryDelay(failures: Int) -> Double? {
        switch failures {
        case 0: 10
        case 1: 30
        case 2: 120
        default: nil
        }
    }

    /// The Soniox terms: the engine's team terms, then the invited people's names (docs/35 3-4) not already there,
    /// while the list stays within `cap`. Engine terms are never cut for names.
    static func terms(engine: [String], names: [String], cap: Int = 40) -> [String] {
        var out = engine
        var seen = Set(engine.map { $0.lowercased() })
        for n in names where out.count < cap && seen.insert(n.lowercased()).inserted { out.append(n) }
        return out
    }

    /// The first message on the Soniox connection. `terms`: the meeting's team terms (`GET /sessions/{id}/terms`).
    /// `translateTo`: the app's language when translation is on (docs/30, `MeetingTranslation`). Speech already in that
    /// language comes back untranslated, at no extra cost. `languages`: the hints (`SpeechLanguages`).
    static func config(terms: [String], translateTo: String? = nil,
                       languages: [String] = SpeechLanguages.current) -> String {
        // a Mac with only Korean and English sends what it always did
        let domain = languages == ["ko", "en"] ? "software team meeting, Korean speech with English technical terms"
            : "software team meeting; speech may be in several languages, with English technical terms"
        var context: [String: Any] = [
            "general": [["key": "domain", "value": domain]],
        ]
        if !terms.isEmpty { context["terms"] = terms }
        var obj: [String: Any] = [
            "model": "stt-rt-v6", "audio_format": "pcm_s16le", "sample_rate": 16000, "num_channels": 1,
            "language_hints": languages, "context": context,
        ]
        if let translateTo { obj["translation"] = ["type": "one_way", "target_language": translateTo] }
        let data = (try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }
}

/// The languages a meeting may be spoken in (2026-11-14): Korean and English always, then this Mac's own languages
/// (System Settings → Language & Region) that Soniox knows. Soniox gets them as `language_hints`; the engine gets them
/// as `TACIE_SPEECH_LANGS` and keeps their letters in the transcript (stt_clean) — without it, a Japanese line is dropped
/// as stray characters. Answers and notes follow the meeting's language on their own (engine lang.rs).
enum SpeechLanguages {
    /// Soniox's languages (ISO 639-1).
    static let soniox: Set<String> = [
        "af", "ar", "az", "be", "bg", "bn", "bs", "ca", "cs", "cy", "da", "de", "el", "en", "es", "et", "eu", "fa", "fi",
        "fr", "gl", "gu", "he", "hi", "hr", "hu", "id", "it", "ja", "kk", "kn", "ko", "lt", "lv", "mk", "ml", "mr", "ms",
        "nl", "no", "pa", "pl", "pt", "ro", "ru", "sk", "sl", "sq", "sr", "sv", "sw", "ta", "te", "th", "tl", "tr", "uk",
        "ur", "vi", "zh",
    ]

    static let current: [String] = codes(preferred: Locale.preferredLanguages)

    /// The Mac's first language as an ISO 639-1 code (nil when the Mac lists none). The engine keeps its on-device
    /// whisper on Korean for "ko" and lets it detect the language otherwise (TACIE_MAC_LANG).
    static func first(preferred: [String]) -> String? {
        guard let tag = preferred.first, var code = Locale(identifier: tag).language.languageCode?.identifier.lowercased()
        else { return nil }
        if code == "nb" || code == "nn" { code = "no" }
        if code == "fil" { code = "tl" }
        return code
    }

    /// `preferred`: BCP 47 tags in the Mac's order ("ja-JP", "zh-Hans-KR", "nb").
    static func codes(preferred: [String]) -> [String] {
        var out = ["ko", "en"]
        for tag in preferred {
            guard var code = Locale(identifier: tag).language.languageCode?.identifier.lowercased() else { continue }
            if code == "nb" || code == "nn" { code = "no" }
            if code == "fil" { code = "tl" }
            if soniox.contains(code), !out.contains(code) { out.append(code) }
        }
        return out
    }
}

/// The Utterancer's samples handed to the recorder and the shadow alike. The Utterancer has one `onSamples`; this keeps
/// a shadow from taking the recorder's place (or the other way round).
final class SampleFanout: @unchecked Sendable {
    var recorder: (([Float]) -> Void)?
    var shadow: (([Float]) -> Void)?
    private let lock = NSLock()
    private var samples = 0

    /// Where the Utterancer's stream clock is (it counts the same samples): a shadow that starts now begins here.
    var streamMs: Int { lock.withLock { samples / 16 } }

    /// The Utterancer's clock started over (`resetClock`).
    func reset() { lock.withLock { samples = 0 } }

    func feed(_ samples: [Float]) {
        lock.withLock { self.samples += samples.count }
        recorder?(samples)
        shadow?(samples)
    }
}

/// Which Soniox region transcribes this Mac's meetings (docs/25 8-2): the relay holds one project key per region and
/// serves a region without a key from Japan. Korea and the rest of Asia-Pacific go to Japan, Europe to the EU, the
/// rest to the United States. A Korean region setting wins over the time zone (someone travelling).
enum SonioxRegion {
    static func hint(timeZone: String, regionCode: String?) -> String {
        if regionCode == "KR" { return "jp" }
        if ["Asia/", "Australia/", "Pacific/"].contains(where: timeZone.hasPrefix) { return "jp" }
        if timeZone.hasPrefix("Europe/") { return "eu" }
        return "us"
    }

    static var current: String {
        hint(timeZone: TimeZone.current.identifier, regionCode: Locale.current.region?.identifier)
    }
}

