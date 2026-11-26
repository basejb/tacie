import Foundation

/// What happens around the notes once a meeting ends or its kind changes (docs/23 3-5), kept apart from
/// MeetingModel so the tests can reach it.
enum NotesFlow {
    /// Whether the main window opens by itself once the notes are in: no questions were asked, the meeting settled
    /// something, and it was a meeting. An interview or a talk (a video watched to the end) stays in the list.
    static func opensAfterNotes(asked: Bool, kind: NotesKind, hasLedger: Bool) -> Bool {
        !asked && kind == .meeting && hasLedger
    }

    /// The person picked another kind: keep it at once (the button shows it while the notes are written again).
    /// false when it is the kind already shown.
    static func markKindChange(_ m: inout SavedMeeting, to kind: NotesKind) -> Bool {
        guard kind != m.shownKind else { return false }
        m.kind = kind
        m.kindSource = "user"
        m.notes = .waiting
        m.notesProblem = nil
        // the old kind's notes do not fit the new sections: they go now (the person was asked when any were theirs),
        // so neither an empty reply nor a failed one leaves them under the new headings
        m.summary = []
        m.topics = []
        m.ledger = []
        return true
    }

    /// Whether the notes hold the person's own work a kind change would drop: a line they changed or added, or a
    /// checked to-do.
    static func hasPersonsWork(_ m: SavedMeeting) -> Bool {
        m.ledger.contains { $0.edited == true || $0.done }
    }

    /// A reply written for an earlier pick, after the person picked again: it is not kept.
    static func isStale(_ m: SavedMeeting, replyKind: String?) -> Bool {
        m.kindSource == "user" && replyKind != nil && replyKind != m.kind?.rawValue
    }

    /// The kind 다시 쓰기 asks for: the person's pick, or nil to let the engine pick again.
    static func rewriteKind(_ m: SavedMeeting) -> NotesKind? {
        m.kindSource == "user" ? m.kind : nil
    }
}
