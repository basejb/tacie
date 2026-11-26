import Foundation

/// Which of a meeting's decisions changed a decision from an earlier meeting (docs/30 8절). The past decisions come from
/// the meetings this device keeps, not from the search index: the iPhone has none. The engine asks the model once and
/// answers with pairs of positions. A failure leaves the notes as they are.
enum DecisionTrail {
    /// Past decisions sent at most (newest first), and how far back they go.
    static let pastMax = 200
    static let lookBack: TimeInterval = 180 * 24 * 3600

    struct Past: Equatable {
        let text: String
        let meeting: String
        let title: String
        let at: Date
    }

    /// The decisions of meetings that started before `m`, newest first, as their notes have them.
    static func past(before m: SavedMeeting, in all: [SavedMeeting], calendar: Calendar = .current) -> [Past] {
        let since = m.start.addingTimeInterval(-lookBack)
        let earlier = all
            .filter { $0.id != m.id && $0.end != nil && $0.notes == .done && $0.start < m.start && $0.start >= since }
            .sorted { $0.start > $1.start }
        var out: [Past] = []
        for p in earlier {
            let title = p.displayTitle(calendar: calendar)
            for e in p.ledger where e.kind == "decision" {
                out.append(Past(text: p.shownRefs(e.text), meeting: p.id, title: title, at: p.start))
                if out.count == pastMax { return out }
            }
        }
        return out
    }

    /// What changed what: for each decision of `m` (by its text) the past decision it replaced. Empty when there is
    /// nothing to compare or the engine could not say.
    static func changes(for m: SavedMeeting, in all: [SavedMeeting], client: EngineClient,
                        calendar: Calendar = .current) async -> [String: ReplacedDecision] {
        let decisions = m.ledger.filter { $0.kind == "decision" }.map(\.text)
        let past = past(before: m, in: all, calendar: calendar)
        guard !decisions.isEmpty, !past.isEmpty else { return [:] }
        do {
            let pairs = try await client.decisionChanges(
                decisions.map(m.shownRefs),
                past: past.map { (day($0.at, calendar), $0.title, $0.text) })
            var out: [String: ReplacedDecision] = [:]
            for (d, p) in pairs where decisions.indices.contains(d) && past.indices.contains(p) {
                let was = past[p]
                out[decisions[d]] = ReplacedDecision(text: was.text, meeting: was.meeting, title: was.title, at: was.at)
            }
            Log.info("decisions: \(decisions.count) compared with \(past.count), \(out.count) changed")
            return out
        } catch {
            Log.info("decisions: not compared (\(error.localizedDescription))")
            return [:]
        }
    }

    /// Puts the changes on the meeting's decisions; the others lose any they had (the notes were written again).
    static func mark(_ m: inout SavedMeeting, _ changes: [String: ReplacedDecision]) {
        for i in m.ledger.indices where m.ledger[i].kind == "decision" {
            m.ledger[i].replaces = changes[m.ledger[i].text]
        }
    }

    static func day(_ date: Date, _ calendar: Calendar) -> String {
        let c = calendar.dateComponents([.year, .month, .day], from: date)
        return String(format: "%04d-%02d-%02d", c.year ?? 0, c.month ?? 0, c.day ?? 0)
    }
}
