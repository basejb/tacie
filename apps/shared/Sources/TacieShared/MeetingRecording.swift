import Foundation

extension Notification.Name {
    /// A meeting began listening: the main window's player stops, so its sound does not go into the meeting.
    static let meetingStarted = Notification.Name("ai.tacie.meetingStarted")
}

/// What MeetingModel decides about a meeting's recording, kept apart so it can be tested without an engine.
enum MeetingRecording {
    /// 개인정보's 회의 녹음: on by default (2026-11-03; off before). Read once when a meeting starts; a change mid-meeting
    /// waits for the next one.
    static var setting: Bool { UserDefaults.standard.object(forKey: "recordMeetings") as? Bool ?? true }

    /// The onboarding's practice and the sample meeting never record, whatever the setting says.
    static func wanted(setting: Bool, practice: Bool, sample: Bool) -> Bool {
        setting && !practice && !sample
    }

    /// A transcribed utterance as a transcript line. `startMs` is where it starts in the recording, nil when the
    /// meeting is not recorded: then the line has no place and no words.
    /// `speaker`: the channel ("me" for the mic in a remote meeting), nil otherwise.
    static func line(for h: Heard, key: String, at: Date, startMs: Int?, speaker: String? = nil) -> TranscriptLine {
        TranscriptLine(key: key, at: at, speaker: speaker, text: h.text, question: h.fired, voice: h.voice,
                       audioAt: startMs, words: startMs.flatMap { s in h.words.map { SpokenWord.placed($0, at: s) } })
    }

    /// An utterance and the pieces the engine split off it (`then`), each with where it starts in the recording.
    static func pieces(of h: Heard, startMs: Int?) -> [(heard: Heard, startMs: Int?)] {
        [(h, startMs)] + (h.then ?? []).map { piece in (piece, startMs.map { $0 + (piece.offsetMs ?? 0) }) }
    }

    /// The stretch of the recording an utterance's WAV (44-byte header, 16 kHz 16-bit mono) covers.
    static func span(wav: Data, at startMs: Int) -> AudioSpan {
        AudioSpan(start: startMs, end: startMs + max(wav.count - 44, 0) / 2 / 16)
    }
}
