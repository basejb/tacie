import Foundation

/// Which saved questions to search again when the search data changes (docs/11 14절). A question is searched again
/// once for each source it was never searched with, and only once that source's first fetch is all in: the search
/// data is rebuilt on every auto sync, and searching again each time would pay for the same question over and over.
enum ResearchRules {
    enum Decision: Equatable {
        case search
        /// Not searched, but marked as seen with the current sources (an old record outside the window).
        case stamp
        case skip
    }

    /// Questions from before this rule, searched again in one go: only recent ones, and at most this many.
    static let legacyCap = 20
    static let legacyWindow: TimeInterval = 30 * 86_400

    /// The sources whose data is fully in the search data now, as keys a question remembers ("github:owner/name",
    /// "notion:<workspace>", "linear:<workspace>", "gdrive:<account>", "atlassian:<cloud id>", "asana:<user gid>", "dropbox:<account id>"). nil while a
    /// sync runs or only part of one is in. `atlassianCloud`: the saved connection's site (the engine's block names the
    /// site only). `asanaUser`: the saved connection's Asana user (the engine's block names none). `dropboxAccount`: the
    /// saved connection's account id (the engine's block names only the email).
    static func keys(_ info: IndexState?, building: Bool, atlassianCloud: String? = nil, asanaUser: String? = nil,
                     dropboxAccount: String? = nil) -> Set<String>? {
        guard let info, !building, info.refresh.partial != true else { return nil }
        var out = Set<String>()
        if let repo = info.repo, info.chunks > 0, info.connected != false { out.insert("github:\(repo)") }
        if let n = info.notion, n.connected, n.pages != nil { out.insert("notion:\(n.workspace ?? "")") }
        if let l = info.linear, l.connected, l.issues != nil { out.insert("linear:\(l.workspace ?? "")") }
        if let g = info.gdrive, g.connected, g.files != nil, g.initialDone != false { out.insert("gdrive:\(g.account ?? "")") }
        if let a = info.atlassian, a.connected, a.issues != nil || a.pages != nil, a.initialDone != false {
            out.insert("atlassian:\(atlassianCloud ?? a.site ?? "")")
        }
        if let a = info.asana, a.connected, a.tasks != nil, a.initialDone != false { out.insert("asana:\(asanaUser ?? "")") }
        if let d = info.dropbox, d.connected, d.files != nil, d.initialDone != false { out.insert("dropbox:\(dropboxAccount ?? "")") }
        return out
    }

    static func due(_ q: SavedQuestion, meetingStart: Date, keys: Set<String>, now: Date) -> Decision {
        if q.noSource == true { return keys.isEmpty ? .skip : .search }
        guard q.unanswered, !keys.isEmpty else { return .skip }
        guard let seen = q.searchedWith else {
            return now.timeIntervalSince(meetingStart) <= legacyWindow ? .search : .stamp
        }
        return keys.subtracting(seen).isEmpty ? .skip : .search
    }

    /// What a saved question remembers: the sources complete now, or while a sync runs the last complete ones (a
    /// question saved mid-sync was still searched with those, it is not an old record).
    static func stampKeys(current: Set<String>?, last: Set<String>) -> [String] {
        Array(current ?? last).sorted()
    }

    /// A search that failed (the engine said why, or the answer call came back empty) is not a result: the question
    /// keeps its chance with these sources.
    static func keepsResult(errorKind: String?, answerTried: Bool, answerGot: Bool) -> Bool {
        errorKind == nil && (!answerTried || answerGot)
    }

    /// Sources tried, adding to those tried before.
    static func merged(_ seen: [String]?, _ now: [String]) -> [String] {
        Array(Set(seen ?? []).union(now)).sorted()
    }
}
