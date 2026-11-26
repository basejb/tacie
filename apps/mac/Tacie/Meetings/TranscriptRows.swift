import Foundation
import Observation

/// What the meeting page lays out under "Transcript" when the meeting has its recording (design notes 5-2, 5-6): the
/// lines, with the stretches that could not be transcribed between them, and for every shown word the place in the
/// recording it plays from.
enum TranscriptRow {
    case line(TranscriptLine)
    case unheard(AudioSpan)

    var id: String {
        switch self {
        case .line(let l): return l.key
        case .unheard(let s): return "unheard \(s.start)"
        }
    }

    /// The lines in their order, each unheard stretch put before the first placed line that starts after it.
    /// Lines with no place in the recording (`audioAt` nil) stay where they are and hold nothing back.
    static func rows(lines: [TranscriptLine], unheard: [AudioSpan]) -> [TranscriptRow] {
        var spans = unheard.sorted { $0.start < $1.start }[...]
        var out: [TranscriptRow] = []
        for l in lines {
            if let at = l.audioAt {
                while let s = spans.first, s.start < at {
                    out.append(.unheard(s))
                    spans = spans.dropFirst()
                }
            }
            out.append(.line(l))
        }
        out += spans.map { .unheard($0) }
        return out
    }
}

/// A turn of the transcript or an unheard stretch between turns.
enum TranscriptPiece {
    case turn(SavedMeeting.Turn)
    case unheard(AudioSpan)

    var id: String {
        switch self {
        case .turn(let t): return t.lines[0].key
        case .unheard(let s): return "unheard \(s.start)"
        }
    }
}

extension SavedMeeting {
    /// The transcript in turns, a turn ending where an unheard stretch sits.
    func transcriptPieces(unheard: [AudioSpan]) -> [TranscriptPiece] {
        var out: [TranscriptPiece] = []
        var run: [TranscriptLine] = []
        func close() {
            guard !run.isEmpty else { return }
            var part = self
            part.transcript = run
            out += part.turns.map { .turn($0) }
            run = []
        }
        for row in TranscriptRow.rows(lines: transcript, unheard: unheard) {
            switch row {
            case .line(let l): run.append(l)
            case .unheard(let s):
                close()
                out.append(.unheard(s))
            }
        }
        close()
        return out
    }
}

extension TranscriptLine {
    /// The line's words as shown (split at spaces) and where each plays from: shown word i goes with spoken word i;
    /// one past the spoken words takes the last of them; a line without words plays from its start.
    var shownWords: [(text: String, ms: Int?)] {
        let spoken = words ?? []
        return text.split(separator: " ", omittingEmptySubsequences: true).enumerated().map { i, w in
            (String(w), spoken.isEmpty ? audioAt : spoken[min(i, spoken.count - 1)].start)
        }
    }

    /// The shown word to light for spoken word `i` (-1: the line as a whole, nothing to light).
    func shownIndex(ofWord i: Int) -> Int? {
        let n = text.split(separator: " ", omittingEmptySubsequences: true).count
        guard i >= 0, n > 0 else { return nil }
        return min(i, n - 1)
    }
}

extension RecordingPlayer {
    /// "6:12", "1:02:03": a place in the recording as the page shows it.
    nonisolated static func timeLabel(_ ms: Int) -> String {
        let s = max(0, ms) / 1000
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
    }
}

/// The word the recording is at.
struct CurrentWord: Equatable {
    let line: String
    let word: Int
}

/// Which word is being heard, kept per line: moving to the next word redraws only the line that loses the light and the
/// line that gets it, not the page or the other turns.
@Observable
@MainActor
final class PlaybackFocus {
    @ObservationIgnored private var lines: [String: LineFocus] = [:]
    @ObservationIgnored private(set) var current: CurrentWord?
    /// Told when the word changes: the transcript tab's text view, which draws every word itself.
    @ObservationIgnored var onChange: ((CurrentWord?) -> Void)?

    func line(_ key: String) -> LineFocus {
        if let l = lines[key] { return l }
        let l = LineFocus()
        lines[key] = l
        return l
    }

    func set(_ c: CurrentWord?) {
        guard c != current else { return }
        if let old = current, old.line != c?.line { line(old.line).word = nil }
        current = c
        if let c { line(c.line).word = c.word }
        onChange?(c)
    }
}

/// The spoken word being heard in one line; nil while the sound is elsewhere.
@Observable
@MainActor
final class LineFocus {
    var word: Int?
}
