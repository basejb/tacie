import Foundation

/// The ask box's answer closes when the focus goes elsewhere, like Spotlight: another app, or another Tacie window.
/// Only the ask box outside a meeting. Answers heard in a meeting keep their own timers (and a typed question in a
/// meeting stays too: the meeting app taking the focus back is the usual case there, not leaving).
enum AskBlur {
    /// - asking: the ask field is open on the pill.
    /// - typedAnswer: the answer on the island came from the ask field.
    /// - inMeeting: a meeting is starting, listening or paused.
    /// - held: something else is showing the ask on purpose (the onboarding's 3단계 asks through the island).
    static func closes(asking: Bool, typedAnswer: Bool, inMeeting: Bool, held: Bool) -> Bool {
        guard !inMeeting, !held else { return false }
        return asking || typedAnswer
    }
}
