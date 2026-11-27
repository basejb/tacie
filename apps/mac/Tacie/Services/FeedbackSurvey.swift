import Foundation

/// The one-time feedback card (docs/16 "피드백 카드", 2026-11-10): after three meetings have notes, the next time a finished
/// meeting's notes are on the main window for 5 s, a card in the window's lower right asks what should get better. It never
/// shows during a meeting or on the island. A send, a skip or the close button ends it for good; left alone and the
/// window closed, it comes back once more on a later notes page, then never again.
///
/// The picks go to PostHog as fixed values (`feedback_sent`); the picks and the written line go to Tacie's server under
/// the account (`POST /feedback`), and only when Send is pressed.
enum FeedbackSurvey {
    /// Meetings with notes before the card may show.
    static let afterNotes = 3
    /// Seconds a notes page stays open before the card comes up.
    static let delay: Double = 5
    /// Times it may come up at most (the second only when the first was left alone).
    static let maxShows = 2
    static let textMax = 300
    /// Dev builds launched with `--feedback-now`: the card comes up over any finished notes page after a second, whatever
    /// was remembered, so it can be looked at.
    /// The card sends to Tacie's server: only with a session (#56). A source build has none (it asks on GitHub instead).
    static func offered(signedIn: Bool) -> Bool { signedIn && Build.hasAccount }
    static var demo: Bool { Build.isDev && ProcessInfo.processInfo.arguments.contains("--feedback-now") }

    /// What the person may wish for (`AWish`, shared with the usage numbers).
    typealias Wish = AWish

    /// What is remembered between launches.
    struct Record: Equatable {
        var shows = 0
        var finished = false
    }

    /// Meetings that ended and got their notes written.
    static func notesCount(_ meetings: [SavedMeeting]) -> Int {
        meetings.filter { $0.end != nil && $0.notes == .done }.count
    }

    /// Whether the card may come up over this page now.
    static func shouldShow(record: Record, notes: Int, page: SavedMeeting?, running: Bool, covered: Bool) -> Bool {
        guard demo || (!record.finished && record.shows < maxShows && notes >= afterNotes), !running, !covered,
              let page, page.end != nil, page.notes == .done else { return false }
        return true
    }

    /// The written line as it goes: trimmed and cut to `textMax`.
    static func cleaned(_ text: String) -> String {
        String(text.trimmingCharacters(in: .whitespacesAndNewlines).prefix(textMax))
    }

    // MARK: - remembered

    private static let showsKey = "feedbackSurveyShows"
    private static let finishedKey = "feedbackSurveyFinished"

    static func load(_ d: UserDefaults = .standard) -> Record {
        Record(shows: d.integer(forKey: showsKey), finished: d.bool(forKey: finishedKey))
    }

    static func save(_ r: Record, _ d: UserDefaults = .standard) {
        d.set(r.shows, forKey: showsKey)
        d.set(r.finished, forKey: finishedKey)
    }
}
