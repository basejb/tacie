import Foundation

/// Cards from past meetings (docs/30): one opens that meeting's page in the main window, not a link.
extension Card {
    /// The meeting a card came from, from its path `meeting/<id>`; nil for every other card.
    var meetingID: String? {
        guard kind == "meeting", path.hasPrefix("meeting/") else { return nil }
        return String(path.dropFirst("meeting/".count))
    }
}

@MainActor
enum MeetingCards {
    /// Shows the meeting in the main window (set by the app).
    static var openMeeting: ((String) -> Void)?
    /// Whether the meeting is still on this Mac (set by the app).
    static var meetingExists: ((String) -> Bool)?

    /// A meeting card whose meeting was deleted since: it is shown faded and opens nothing.
    static func gone(_ card: Card) -> Bool {
        guard let id = card.meetingID else { return false }
        return !(meetingExists?(id) ?? true)
    }

    /// True for a meeting's card, which never goes to a link: its meeting opens, unless it was deleted.
    static func open(_ card: Card) -> Bool {
        guard let id = card.meetingID else { return false }
        if !gone(card) { openMeeting?(id) }
        return true
    }
}
