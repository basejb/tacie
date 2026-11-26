import Foundation

/// What the engine keeps of a finished meeting to search it later (docs/30): the summary, the topics and the ledger,
/// with every person shown by name. Never the transcript or the questions Tacie answered.
struct MeetingDigest: Codable, Equatable {
    struct Topic: Codable, Equatable {
        let title: String
        let points: [String]
    }

    struct Entry: Codable, Equatable {
        let kind: String
        let text: String
        let owner: String?
        let due: String?
        let done: Bool
        /// The earlier decision this one changed; left out of the body when there is none.
        var replaces: Replaced? = nil
    }

    struct Replaced: Codable, Equatable {
        let text: String
        let at: String  // the earlier meeting's day, "2026-10-30"
        let title: String
    }

    let id: String
    let title: String
    let start: String
    let end: String
    let summary: [String]
    let topics: [Topic]
    let ledger: [Entry]

    /// The body `POST /meetings` reads.
    var json: [String: Any] {
        guard let data = try? JSONEncoder().encode(self),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return [:] }
        return object
    }
}

extension SavedMeeting {
    /// Ended, with notes that say something: the engine can search it (docs/30).
    var searchable: Bool { end != nil && notes == .done && !(summary.isEmpty && ledger.isEmpty) }

    /// The meeting as the engine searches it; nil while it runs, before its notes, without notes, or with nothing in them.
    func digest(calendar: Calendar = .current) -> MeetingDigest? {
        guard searchable, let end else { return nil }
        let iso = ISO8601DateFormatter()
        iso.timeZone = calendar.timeZone
        iso.formatOptions = [.withInternetDateTime]
        return MeetingDigest(
            id: id,
            title: displayTitle(calendar: calendar),
            start: iso.string(from: start),
            end: iso.string(from: end),
            summary: summary.map(shownRefs),
            topics: (topics ?? []).map { MeetingDigest.Topic(title: shownRefs($0.title), points: $0.points.map(shownRefs)) },
            ledger: ledger.map {
                MeetingDigest.Entry(kind: $0.kind, text: shownRefs($0.text), owner: $0.owner.flatMap(shownOwner),
                                    due: $0.due, done: $0.done,
                                    replaces: $0.replaces.map { .init(text: $0.text, at: DecisionTrail.day($0.at, calendar), title: $0.title) })
            })
    }
}
