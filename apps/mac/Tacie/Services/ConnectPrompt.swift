import Foundation

/// The one-time connect card (docs/16 "연결 안내 카드", 2026-11-13): someone who has finished three meetings with nothing but
/// past meetings to search, and heard questions Tacie could not answer, gets a card in the main window's lower right:
/// connect a tool (Settings → Connections), or keep Tacie for notes only (meeting mode → Notes only). Without it,
/// people who kept Tacie for the notes met "Not found" again and again with neither way out.
///
/// It comes up the way the feedback card does (a finished meeting's notes on the main window for 5 s, never during a
/// meeting or under a modal), goes first when both could, and shows once: any answer, the close button included, ends
/// it.
enum ConnectPrompt {
    /// Finished meetings before the card may show.
    static let afterMeetings = 3
    /// Seconds a notes page stays open before the card comes up.
    static let delay: Double = 5
    /// Dev builds launched with `--connect-prompt-now`: the card comes up over any finished notes page after a second,
    /// whatever was remembered, so it can be looked at.
    static var demo: Bool { Build.isDev && ProcessInfo.processInfo.arguments.contains("--connect-prompt-now") }

    /// How the card ended. The raw values are what PostHog sees (`connect_prompt_answered`).
    typealias Choice = AConnectChoice

    /// What the card says: finished meetings and the questions in them that found no answer.
    struct Counts: Equatable {
        var meetings: Int
        var unanswered: Int
    }

    static func counts(_ meetings: [SavedMeeting]) -> Counts {
        let ended = meetings.filter { $0.end != nil }
        return Counts(meetings: ended.count, unanswered: ended.reduce(0) { $0 + $1.questions.filter(\.unanswered).count })
    }

    /// Whether the card may come up over this page now. `teamSource` is nil until the engine has said what is
    /// connected.
    static func shouldShow(done: Bool, counts: Counts, teamSource: Bool?, recordOnly: Bool, page: SavedMeeting?,
                           running: Bool, covered: Bool) -> Bool {
        guard !running, !covered, let page, page.end != nil, page.notes == .done else { return false }
        if demo { return true }
        guard !done, teamSource == false, !recordOnly else { return false }
        return counts.meetings >= afterMeetings && counts.unanswered > 0
    }

    // MARK: - remembered

    private static let doneKey = "connectPromptDone"

    static func load(_ d: UserDefaults = .standard) -> Bool { d.bool(forKey: doneKey) }
    static func save(_ done: Bool, _ d: UserDefaults = .standard) { d.set(done, forKey: doneKey) }
}
