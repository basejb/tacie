import Foundation

/// When the app starts a sync on its own (IndexKeeper.check): when the engine's check finds something new, or else
/// once the data is older than 20 hours.
enum SyncRules {
    static let maxAge: TimeInterval = 20 * 3600

    /// Never while turned off, without a GitHub connection, after the data was cleared, or while a sync or meeting is on;
    /// otherwise when nothing was built yet or the data is older than 20 hours.
    static func autoSync(enabled: Bool, connected: Bool, cleared: Bool, builtAt: Date?, busy: Bool, now: Date) -> Bool {
        guard shouldCheck(enabled: enabled, connected: connected, cleared: cleared, busy: busy) else { return false }
        guard let builtAt else { return true }
        return now.timeIntervalSince(builtAt) > maxAge
    }

    /// Whether to ask the engine what changed (every 30 minutes): the same gates as autoSync, whatever the age.
    static func shouldCheck(enabled: Bool, connected: Bool, cleared: Bool, busy: Bool) -> Bool {
        enabled && connected && !cleared && !busy
    }

    /// The sync's steps in order with their share of the bar (repo, GitHub's PRs and issues, Notion, embedding, swap).
    /// Notion's, Linear's, Google Drive's, Jira · Confluence's, Asana's and Dropbox's shares go to embedding when they are not
    /// connected. Together they take at most half the bar, so embedding always keeps a tenth.
    static func fraction(step: String?, progress: Double?, notion: Bool, linear: Bool = false, gdrive: Bool = false,
                         atlassian: Bool = false, asana: Bool = false, dropbox: Bool = false) -> Double {
        var parts: [(String, Double)] = [("notion", notion ? 0.2 : 0), ("linear", linear ? 0.15 : 0), ("gdrive", gdrive ? 0.15 : 0),
                                         ("atlassian", atlassian ? 0.15 : 0), ("asana", asana ? 0.15 : 0),
                                         ("dropbox", dropbox ? 0.15 : 0)]
        let sum = parts.reduce(0) { $0 + $1.1 }
        if sum > 0.5 { parts = parts.map { ($0.0, $0.1 * 0.5 / sum) } }
        let used = min(sum, 0.5)
        let shares: [(String, Double)] = [("repo", 0.2), ("sources", 0.15)] + parts + [("embed", 0.6 - used), ("swap", 0.05)]
        var start = 0.0
        for (name, share) in shares {
            if name == step { return start + share * (progress ?? 0) }
            start += share
        }
        return 0.02  // "준비 중": the sync was just asked for
    }

    /// One line per source of a running sync, then 검색 준비 (the main window, when more than one source is in it).
    struct Row: Equatable {
        enum State: Equatable { case waiting, running, done }
        let name: String
        let detail: String?
        let state: State
    }

    /// The rows for the engine's step: GitHub does `repo` and `sources`, Notion does `notion` (with its pages as
    /// the progress), then embedding and the swap are 검색 준비. Nil unless GitHub and Notion are both in the sync.
    static func rows(step: String?, progress: (done: Int, total: Int)?, repo: String?, notion: Bool,
                     linear: Bool = false, gdrive: Bool = false, atlassian: Bool = false, asana: Bool = false,
                     dropbox: Bool = false) -> [Row]? {
        guard let repo, notion || linear || gdrive || atlassian || asana || dropbox else { return nil }
        let order = ["repo", "sources", "notion", "linear", "gdrive", "atlassian", "asana", "dropbox", "embed", "swap"]
        let at = step.flatMap { order.firstIndex(of: $0) } ?? -1
        func state(_ steps: [String]) -> Row.State {
            let idx = steps.compactMap { order.firstIndex(of: $0) }
            if idx.contains(at) { return .running }
            return at > (idx.max() ?? 0) ? .done : .waiting
        }
        let notionState = state(["notion"])
        var pages: String?
        if notionState == .running, let p = progress, p.total > 0 { pages = String(localized: "페이지 \(p.done) / \(p.total)") }
        var rows = [Row(name: "GitHub", detail: repo.split(separator: "/").last.map(String.init), state: state(["repo", "sources"]))]
        if notion { rows.append(Row(name: String(localized: "노션"), detail: pages, state: notionState)) }
        if linear { rows.append(Row(name: "Linear", detail: nil, state: state(["linear"]))) }
        if gdrive {
            let driveState = state(["gdrive"])
            var files: String?
            if driveState == .running, let p = progress, p.total > 1 { files = String(localized: "문서 \(p.done) / \(p.total)") }
            rows.append(Row(name: "Google Drive", detail: files, state: driveState))
        }
        if atlassian {
            // the first fetch has no total to count toward (it walks back two years, newest first): how many so far
            let atState = state(["atlassian"])
            var items: String?
            if atState == .running, let p = progress, p.done > 0 { items = String(localized: "이슈·페이지 \(p.done)") }
            rows.append(Row(name: "Jira · Confluence", detail: items, state: atState))
        }
        if asana {
            // the comments of the tasks found count toward a total (the task list itself comes first, quickly)
            let asState = state(["asana"])
            var tasks: String?
            if asState == .running, let p = progress, p.total > 1 { tasks = String(localized: "task \(p.done) / \(p.total)") }
            rows.append(Row(name: "Asana", detail: tasks, state: asState))
        }
        if dropbox {
            let dbState = state(["dropbox"])
            var files: String?
            if dbState == .running, let p = progress, p.total > 1 { files = String(localized: "파일 \(p.done) / \(p.total)") }
            rows.append(Row(name: "Dropbox", detail: files, state: dbState))
        }
        rows.append(Row(name: String(localized: "검색 준비"), detail: nil, state: state(["embed", "swap"])))
        return rows
    }

    /// After a check (nil: it failed): sync when the branch or GitHub has something new, else by the 20-hour rule.
    static func afterCheck(_ c: IndexCheck?, enabled: Bool, connected: Bool, cleared: Bool, builtAt: Date?, busy: Bool,
                           now: Date) -> Bool {
        if c?.changed == true, shouldCheck(enabled: enabled, connected: connected, cleared: cleared, busy: busy) { return true }
        return autoSync(enabled: enabled, connected: connected, cleared: cleared, builtAt: builtAt, busy: busy, now: now)
    }

    /// Why GitHub could not be used, for app.log only: a fixed word picked from the engine's error, never the error
    /// itself (it can name a repo). "not-connected": the app handed the engine no token; "401": GitHub refused the
    /// token; "403"/"404": the token works but the repo is out of reach; "gh": a development build's gh is logged out.
    static func githubReason(_ error: String?) -> String {
        guard let e = error else { return "other" }
        if e.contains("GitHub is not connected") { return "not-connected" }
        if e.contains("gh is not logged in") { return "gh" }
        for code in ["401", "403", "404", "410"] where e.contains("HTTP \(code)") || e.contains("answered \(code)") {
            return code == "410" ? "404" : code
        }
        return "other"
    }
}
