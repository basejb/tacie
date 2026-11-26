import Foundation

struct Card: Codable, Identifiable, Equatable {
    let id: String
    let path: String
    let kind: String
    let source: String?  // "repo", "github", "slack", "notion", "linear", "gdrive", "jira", "confluence", "asana", "dropbox", "meeting", "notes"
    let byline: String?  // outside the repo: "머지됨 · basejb · 2026-10-15 · …", "#dev-help · 2026-10-21 · 스레드 …"
    let url: String?
    let lineStart: Int
    let lineEnd: Int
    let title: String
    let snippet: String
    let score: Double?
    let githubUrl: String?
    let localPath: String?
    let chunk: Int?
    let `repeat`: Bool?

    var fileName: String { (path as NSString).lastPathComponent }
    var directory: String { (path as NSString).deletingLastPathComponent }
    /// Prose rather than code: Markdown docs, PR and issue text, Slack messages.
    var isDoc: Bool { kind != "code" }
    var inRepo: Bool { kind == "code" || kind == "docs" }
    /// What a chip says: a title (file · section, "PR #655 · …", "#channel · …") without markdown marks, or the file name.
    var name: String {
        guard isDoc else { return fileName }
        return ["**", "~~", "`"].reduce(title) { $0.replacingOccurrences(of: $1, with: "") }
    }
    /// Where a click goes: the pinned GitHub line, the PR comment or the Slack message.
    var link: URL? { (url ?? githubUrl).flatMap(URL.init(string:)) }
    /// The onboarding's example material (the engine adds 1,000,000 to its chunk ids, sample.rs SAMPLE_BASE).
    var isSample: Bool { (chunk ?? 0) >= 1_000_000 }
    /// The service's own mark from the asset catalog (as in Settings > 연결), when there is one. The example
    /// material is not on GitHub, so it gets none.
    var logo: String? {
        if isSample { return nil }
        switch kind {
        case "code", "docs", "github": return "ServiceGitHub"
        case "notion": return "ServiceNotion"
        case "linear": return "ServiceLinear"
        case "gdrive": return "ServiceGoogleDrive"
        case "jira": return "ServiceJira"
        case "confluence": return "ServiceConfluence"
        case "asana": return "ServiceAsana"
        case "dropbox": return "ServiceDropbox"
        case "notes": return opensInObsidian ? "ServiceObsidian" : nil
        default: return nil
        }
    }
    /// A note from an Obsidian vault opens in Obsidian (docs/34); a note from another folder opens in its default app.
    var opensInObsidian: Bool { url?.hasPrefix("obsidian://") == true }
    /// A small mark for material that is not a repo file.
    var symbol: String? {
        switch kind {
        case "github": return path.hasPrefix("github/pull/") ? "arrow.triangle.pull" : "smallcircle.filled.circle"
        case "slack": return "number"
        case "notion": return "doc.text"
        case "linear", "jira", "asana": return "checklist"
        case "confluence": return "doc.text"
        case "meeting": return "person.2"
        case "dropbox": return "doc.richtext"
        case "notes": return "doc.plaintext"
        case "gdrive":
            // the byline starts with the kind ("Google Sheets · 내 드라이브 · …")
            if byline?.hasPrefix("Google Sheets") == true || byline?.hasPrefix("Excel") == true { return "tablecells" }
            if byline?.hasPrefix("Google Slides") == true || byline?.hasPrefix("PowerPoint") == true { return "rectangle.on.rectangle" }
            return "doc.richtext"
        default: return nil
        }
    }
    var opensIn: String {
        switch kind {
        case "slack": String(localized: "슬랙")
        case "notion": String(localized: "노션")
        case "linear": "Linear"
        case "gdrive": "Google Drive"
        case "jira": "Jira"
        case "confluence": "Confluence"
        case "asana": "Asana"
        case "dropbox": "Dropbox"
        case "notes": opensInObsidian ? "Obsidian" : String(localized: "기본 앱")
        case "meeting": String(localized: "메인 창")
        default: "GitHub"
        }
    }
}

struct AskResponse: Codable {
    let cards: [Card]
    let bestP: Double?
    let shown: Bool
    let lowConfidence: Bool
    let error: String?
    let errorKind: String?
}

/// `errorKind` is what failed outside (engine/crates/tacie-engine/src/errors.rs): key, credit, network, busy or down.
/// `skipped` is "backchannel" for "네", "아 네네." (not gated, not context); `mergedFrom` is the earlier piece this one joined.
/// `ending` says the speaker is closing the meeting ("오늘은 여기까지 하죠"; `pEnd` at or over the engine's end threshold).
/// Older engines leave both out.
/// `voice` is the speaker the voice model heard ("화자 2"), `person` the remembered person it is (an id the app sent with
/// the session). Both are nil without the voice model.
struct Heard: Codable {
    let utteranceId: String?
    let text: String
    let fired: Bool
    let pLookup: Double?
    var pEnd: Double? = nil
    var ending: Bool? = nil
    let skipped: String?
    let mergedFrom: String?
    var voice: String? = nil
    var person: String? = nil
    /// Each word and when it was said, in seconds from the start of this utterance's WAV; absent without timing.
    /// For a piece in `then`, from the start of that piece.
    var words: [HeardWord]? = nil
    /// The rest of the utterance when the engine split it where the voice changed, in order; each is a line of its own.
    var then: [Heard]? = nil
    /// Where a piece in `then` starts in the utterance's WAV (ms).
    var offsetMs: Int? = nil
    let error: String?
    let errorKind: String?
    /// Where this utterance was recognized: "local" (this Mac) or "server". Older engines leave it out.
    var stt: String? = nil
    /// The engine's on-device model failed again after a reload; the app checks the file (`LocalSTT.heard`).
    var sttBroken: Bool? = nil
}

/// The engine's answer to `POST /stt`.
struct STTStatus: Codable, Equatable {
    let on: Bool
    let ready: Bool
    var loading: Bool? = nil
    var broken: Bool? = nil
    /// "metal" or "cpu" once the model is loaded.
    var device: String? = nil
}

struct HeardWord: Codable, Equatable {
    let text: String
    let start: Double
    let end: Double
}

/// POST /sessions/{id}/voices: one speaker of the meeting, its voice over the lines it said and how long it spoke.
/// `person` is the remembered person the engine knew it as; `near` the one it sounds closest to without being sure.
struct EngineVoice: Codable, Equatable {
    let voice: String
    let channel: String
    let vector: [Float]
    let seconds: Float
    let person: String?
    let near: String?
    /// Only when the engine groups speakers with the speaker-grouping model (docs/04 4절).
    var diarVector: [Float]? = nil

    var print: MeetingPrint {
        MeetingPrint(key: voice, channel: channel, vector: vector, seconds: seconds, person: person, diarVector: diarVector)
    }
}

struct Found: Codable {
    let cards: [Card]
    let shown: Bool
    let lowConfidence: Bool?
    let errorKind: String?
}

/// `cited` holds the chunk ids the line cites; `dropped` says why the engine hid a line ("citation", "length").
struct Answer: Codable {
    let answer: String?
    let cited: [Int]?
    let dropped: String?
    let error: String?
}

/// POST /sessions/{id}/ledger at the end of a meeting. kind: decision, commitment or open_question; every item
/// points at utterances the engine heard (it drops the ones that do not). Commitments name who ("나", "상대") and when.
struct LedgerItem: Codable, Equatable {
    let kind: String
    let text: String
    let utteranceIds: [String]
    var owner: String? = nil
    var due: String? = nil
}

/// One topic of the meeting in the order it came up: how the talk went and ended, in 1–3 sentences.
struct NoteTopic: Codable, Equatable {
    let title: String
    let points: [String]
    /// What each point is, in the same order: "background", "discussion", "conclusion", "open" or "" (docs/24 7-3).
    /// Only the staged notes of a Korean meeting have them; older notes and other languages have none.
    var roles: [String]? = nil

    /// The role of the point at `i`, if the notes gave one.
    func role(_ i: Int) -> String? {
        guard let roles, i < roles.count, !roles[i].isEmpty else { return nil }
        return roles[i]
    }
}

/// The meeting's notes: title, summary and topics come with the ledger from the same call (absent from older engines).
struct Ledger: Codable {
    let items: [LedgerItem]
    var title: String? = nil
    var summary: [String]? = nil
    var topics: [NoteTopic]? = nil
    /// The recording kind the notes were written for, and who picked it ("auto" | "user"); absent from older engines.
    var kind: String? = nil
    var kindSource: String? = nil
    let error: String?
    let errorKind: String?
}

/// POST /questions/review (docs/24 4절).
struct QuestionReviewReply: Codable {
    struct Item: Codable {
        let id: String
        let verdict: String
        let question: String?
        let answer: String?
        let utteranceIds: [String]
    }
    let questions: [Item]
    var error: String? = nil
    var errorKind: String? = nil
}

/// GET /index: what the index was built from, and the background refresh (engine/crates/tacie-engine/src/refresh.rs).
struct IndexState: Codable {
    struct Refresh: Codable {
        struct Progress: Codable { let done: Int; let total: Int }  // total: chunks to embed this run
        let state: String  // idle, running, clearing, done, failed
        let step: String?
        var progress: Progress? = nil
        let embedded: Int?
        let error: String?
        let errorKind: String?
        let warnings: [String]?  // sources that kept their saved copy (github, slack), or "github-lost"
        var partial: Bool? = nil  // part of this sync is searchable already (a big sync goes in part by part)
    }
    let chunks: Int
    let commit: String?
    let builtAt: String?
    let refresh: Refresh
    // Older engines leave these out.
    let files: Int?
    let githubItems: Int?
    let bytes: Int?
    let repo: String?
    let branch: String?
    let symbolsMissing: Int?
    let githubLogin: String?
    let connected: Bool?
    let cleared: Bool?
    let githubLost: Bool?  // GitHub cannot see the repo: its PRs and issues are out of search
    let modelReady: Bool?  // the search model is on this Mac (false while it downloads)
    let sample: SampleInfo?  // the onboarding's sample data (docs/11 12절)
    let notion: NotionInfo?  // the Notion connection the engine holds and the pages found (docs/13)
    let linear: LinearInfo?  // the Linear connection the engine holds and the issues found (docs/18)
    var gdrive: GDriveInfo? = nil  // the Google Drive connection the engine holds and the files found (docs/19)
    var atlassian: AtlassianInfo? = nil  // the Jira · Confluence connection the engine holds and the items found (docs/31)
    var asana: AsanaInfo? = nil  // the Asana connection the engine holds and the tasks found (docs/32)
    var dropbox: DropboxInfo? = nil  // the Dropbox connection the engine holds and the files found (docs/33)
    var notes: NotesInfo? = nil  // the note folders chosen on this Mac and the notes found (docs/34)

    /// Nothing chosen: `chosen` 0, `folders` empty, the rest null.
    struct NotesInfo: Codable, Equatable {
        struct Folder: Codable, Equatable, Identifiable {
            let id: String
            let path: String
            let name: String
            let vaultId: String?
            let obsidian: Bool
            let notes: Int?  // nil until the folder is read
            let missing: Bool  // the folder is gone (moved, renamed, an unplugged disk)
        }
        let chosen: Int
        let folders: [Folder]
        let notes: Int?  // in the search data
        let notDownloaded: Int?  // iCloud notes not on this Mac that took too long to come down: titles only for now
        let readAt: String?
    }

    /// Never connected: `connected` and `refused` false, the rest null.
    struct DropboxInfo: Codable {
        let connected: Bool
        let account: String?
        let files: Int?  // in the search data
        let fetchedAt: String?
        let initialDone: Bool?  // every file of the first fetch is in (a big Dropbox comes over several runs)
        let refused: Bool?  // Dropbox refused the token: the app renews it at once (the engine waits for it mid-fetch)
        let errorKind: String?  // why the last fetch failed (key, network, busy, down)
        let lostSince: String?  // the connection broke then; seven days later Dropbox is out of search
    }

    /// Never connected: `connected` and `refused` false, the rest null.
    struct AsanaInfo: Codable {
        let connected: Bool
        let refused: Bool?  // Asana refused the token: the app renews it at once (the engine waits for it mid-fetch)
        let workspaces: Int?  // workspaces fetched (not muted, not blocked by an admin)
        let tasks: Int?  // tasks in the search data
        let fetchedAt: String?
        let initialDone: Bool?  // the first fetch is all in (a big workspace comes over several runs)
        let errorKind: String?  // why the last fetch failed (key, network, busy, down)
        let lostSince: String?  // the connection broke then; seven days later the tasks are out of search
        let blockedWorkspaces: [String]?  // names of the workspaces an admin closed to outside apps (not fetched)
    }

    /// Never connected: `connected` and `refused` false, the rest null.
    struct AtlassianInfo: Codable {
        let connected: Bool
        let site: String?
        let refused: Bool?  // Atlassian refused the token: the app renews it at once (the engine waits for it mid-fetch)
        let issues: Int?  // Jira issues in the search data
        let pages: Int?  // Confluence pages and blog posts in the search data
        let fetchedAt: String?
        let initialDone: Bool?  // the first fetch reached two years back (it comes newest first, over hours on a big site)
        let errorKind: String?  // why the last fetch failed (key, network, busy, down)
        let lostSince: String?  // the connection broke then; seven days later the items are out of search
        let pausedUntil: String?  // Atlassian's hourly limit: fetching waits until then (null once it is over)
    }

    struct GDriveInfo: Codable {
        let connected: Bool
        let account: String?
        let refused: Bool?  // Drive refused the token: the app renews it at once (the engine may be waiting for it)
        let files: Int?  // in the search data
        let drives: Int?  // shared drives the account is in
        let fetchedAt: String?
        let initialDone: Bool?  // every file of the first fetch is in (a big Drive comes in batches)
        let errorKind: String?  // why the last fetch failed (key, network, busy, down)
        let lostSince: String?  // the connection broke then; seven days later Drive is out of search
    }

    struct LinearInfo: Codable {
        let connected: Bool
        let workspace: String?
        let refused: Bool?  // Linear refused the token on the last sync
        let issues: Int?  // in the search data
        let teams: Int?  // visible teams
        let fetchedAt: String?
        let errorKind: String?  // why the last fetch failed (key, network, busy, down)
        let lostSince: String?  // the connection broke then; seven days later Linear is out of search
    }

    struct NotionInfo: Codable {
        let connected: Bool
        let workspace: String?
        let refused: Bool?  // Notion refused the token on the last sync
        let pages: Int?  // in the search data
        let fetchedAt: String?
    }

    struct SampleInfo: Codable {
        struct Question: Codable { let text: String; let use: String? }  // use: "ask" or "spoken"
        let ready: Bool
        let questions: [Question]
    }
}

/// POST /index/check: whether the branch or GitHub has anything past the index (nil: could not be told).
struct IndexCheck: Codable { let code: Bool?; let github: Bool?; var notion: Bool? = nil; var linear: Bool? = nil; var gdrive: Bool? = nil; var atlassian: Bool? = nil; var asana: Bool? = nil; var dropbox: Bool? = nil; let changed: Bool }

/// One row of the shared drive picker.
struct GDriveDrive: Codable, Identifiable, Equatable {
    let id: String
    let name: String
    let files: Int
}

/// One row of the Dropbox folder picker: a top folder, shared or team folders included.
struct DropboxFolder: Codable, Identifiable, Equatable {
    let id: String
    let name: String
    let path: String
    let files: Int
    let teamFolder: Bool
}

/// The Asana workspaces and projects with their task counts (the workspace · project picker).
struct AsanaScopes: Codable, Equatable {
    struct Workspace: Codable, Identifiable, Equatable {
        let gid: String
        let name: String
        let tasks: Int
        /// An admin blocks it: nothing is fetched, so the sheet shows it greyed out and never counts it as picked.
        /// An engine that does not send it means not blocked.
        var blocked = false
        var id: String { gid }

        init(gid: String, name: String, tasks: Int, blocked: Bool = false) {
            self.gid = gid
            self.name = name
            self.tasks = tasks
            self.blocked = blocked
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            gid = try c.decode(String.self, forKey: .gid)
            name = try c.decode(String.self, forKey: .name)
            tasks = try c.decode(Int.self, forKey: .tasks)
            blocked = try c.decodeIfPresent(Bool.self, forKey: .blocked) ?? false
        }
    }
    struct Project: Codable, Identifiable, Equatable {
        let gid: String
        let workspace: String
        let name: String
        let tasks: Int
        var id: String { gid }
    }
    let workspaces: [Workspace]
    let projects: [Project]
}

/// The Jira projects and Confluence spaces with their counts (the project · space picker).
struct AtlassianScopes: Codable, Equatable {
    struct Scope: Codable, Identifiable, Equatable {
        let id: String
        let key: String
        let name: String
        let count: Int

        private enum CodingKeys: String, CodingKey { case id, key, name, issues, pages }

        init(id: String, key: String, name: String, count: Int) {
            self.id = id
            self.key = key
            self.name = name
            self.count = count
        }

        /// A project counts `issues`, a space `pages`.
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            id = try c.decode(String.self, forKey: .id)
            key = try c.decodeIfPresent(String.self, forKey: .key) ?? ""
            name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
            count = try c.decodeIfPresent(Int.self, forKey: .issues) ?? c.decodeIfPresent(Int.self, forKey: .pages) ?? 0
        }

        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(id, forKey: .id)
            try c.encode(key, forKey: .key)
            try c.encode(name, forKey: .name)
            try c.encode(count, forKey: .issues)
        }
    }
    let projects: [Scope]
    let spaces: [Scope]
}

/// One row of the Linear team picker.
struct LinearTeam: Codable, Identifiable, Equatable {
    let id: String
    let name: String
    let issues: Int
}

struct GitHubRepo: Codable, Identifiable, Equatable {
    let fullName: String
    let `private`: Bool
    let defaultBranch: String
    let pushedAt: String?
    var id: String { fullName }
}

struct GitHubRepos: Codable {
    let repos: [GitHubRepo]
    let needsApproval: [String]
    let needsSso: [String]
}

struct GitHubBranches: Codable {
    let branches: [String]
    let `default`: String
}

/// GET /config: the models, thresholds and key files the engine runs with. With no key file at all, `jev` comes as null.
struct EngineConfig: Codable {
    struct Models: Codable { let stt: String; let answer: String; let jev: String? }
    struct Thresholds: Codable { let gate: Double; let show: Double }
    /// state: ok, missing, refused, credit. inApp: the key came from the app (else a dev key file); nil from an older engine.
    struct Key: Codable { let name: String; let path: String; let state: String; let inApp: Bool? }
    let models: Models
    let modelDefaults: Models
    let thresholds: Thresholds
    let thresholdDefaults: Thresholds
    let keys: [Key]
    let logText: Bool
}

struct EngineHealth: Codable {
    struct Index: Codable { let chunks: Int; let repo: String?; let commit: String? }
    let ready: Bool
    let index: Index
}

/// One progress line of POST /panel/prep before the answer. `tool` is one of POST /panel/chat (docs/38 6절): a tool
/// starting with the arguments it may show (`count` nil), then the same tool done with how many it found; the name
/// "start" opens the question.
enum PanelStep: Equatable, Codable {
    case meetings([String]), related([String]), sources(Int), writing
    case tool(String, detail: [String: String], count: Int?)
}

/// An item of a meeting's ledger as the panel shows it: where it is (`meetingId`, `index` in that meeting's ledger).
/// `also` lists other meetings that raised the same open question.
struct PanelItem: Codable, Equatable {
    let meetingId: String
    let index: Int
    let kind: String
    let text: String
    let owner: String?
    let due: String?
    let also: [String]?
}

/// The final line of POST /panel/prep.
struct PrepAnswer: Codable, Equatable {
    let advice: String
    let open: [PanelItem]
    let todos: [PanelItem]
    let sources: [Card]
    let meetingsRead: Int
    let sourcesRead: Int
    let errorKind: String?
}

/// POST /panel/why: why a decision was made, and the transcript lines that say so. A failure reply carries `error_kind`
/// and may leave out `reason` and `keys` (older engines did), so both read as empty when missing.
struct WhyAnswer: Codable, Equatable {
    let reason: String
    let keys: [String]
    let errorKind: String?

    init(reason: String, keys: [String], errorKind: String?) {
        self.reason = reason
        self.keys = keys
        self.errorKind = errorKind
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        reason = try c.decodeIfPresent(String.self, forKey: .reason) ?? ""
        keys = try c.decodeIfPresent([String].self, forKey: .keys) ?? []
        errorKind = try c.decodeIfPresent(String.self, forKey: .errorKind)
    }
}

/// POST /panel/ask: `answer` is nil when the meetings and sources do not say.
struct PanelAsk: Codable, Equatable {
    let answer: String?
    let cited: [Int]
    let cards: [Card]
    let errorKind: String?
}

/// The `done` line of POST /panel/chat (docs/38 6절): the answer, the sources it cites (its [n] are their order), the
/// ledger items and transcript lines it cites, and the whole prep when it prepared a meeting. A failure carries
/// `error_kind` and a nil `answer`.
struct ChatAnswer: Codable, Equatable {
    struct Item: Codable, Equatable { let meetingId: String; let index: Int }
    struct Line: Codable, Equatable { let meetingId: String; let key: String }
    let answer: String?
    let cards: [Card]
    let items: [Item]
    let lines: [Line]
    let prep: PrepAnswer?
    let errorKind: String?
    /// Changes the answer proposes for the meeting on the page (docs/38 12절), shown as cards under it; empty when none.
    var proposals: [PanelProposal] = []
    /// The iPhone all-meetings chat handed the question to one meeting (`open_meeting`, docs/40 12절): the app asks the
    /// same question again with that meeting's transcript. nil from every other chat and from older engines.
    struct Handoff: Codable, Equatable { let meetingId: String }
    var handoff: Handoff?

    init(answer: String?, cards: [Card], items: [Item] = [], lines: [Line] = [], prep: PrepAnswer? = nil, errorKind: String? = nil,
         proposals: [PanelProposal] = [], handoff: Handoff? = nil) {
        self.handoff = handoff
        self.answer = answer
        self.cards = cards
        self.items = items
        self.lines = lines
        self.prep = prep
        self.errorKind = errorKind
        self.proposals = proposals
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        answer = try c.decodeIfPresent(String.self, forKey: .answer)
        cards = try c.decodeIfPresent([Card].self, forKey: .cards) ?? []
        items = try c.decodeIfPresent([Item].self, forKey: .items) ?? []
        lines = try c.decodeIfPresent([Line].self, forKey: .lines) ?? []
        // a prep that failed half way has no advice or sources: the answer still comes without it
        prep = try? c.decodeIfPresent(PrepAnswer.self, forKey: .prep)
        errorKind = try c.decodeIfPresent(String.self, forKey: .errorKind)
        // one proposal that does not read leaves the others
        struct Lossy: Decodable {
            let p: PanelProposal?
            init(from decoder: Decoder) throws { p = try? PanelProposal(from: decoder) }
        }
        proposals = (try? c.decodeIfPresent([Lossy].self, forKey: .proposals))??.compactMap(\.p) ?? []
        // a hand-off that does not read is no hand-off: the answer stays as it came
        handoff = try? c.decodeIfPresent(Handoff.self, forKey: .handoff)
    }
}

/// What POST /panel/chat reads besides the question: on a meeting page, the meeting and the ones it goes on from
/// (that meeting first), their transcript lines (key, speaker name, "HH:mm", text) and, per meeting, each ledger
/// entry's line keys; from every meeting, all empty. `today` is "yyyy-MM-dd" on this Mac.
struct PanelChatRequest {
    var text: String
    var meetingIDs: [String] = []
    var history: [(q: String, a: String)] = []
    var today: String
    var transcripts: [String: [[String: String]]] = [:]
    var refs: [String: [[String]]] = [:]
}

/// One prepared item of POST /panel/followup: whether the meeting settled it, and how.
struct FollowResult: Codable, Equatable {
    let i: Int
    let decided: Bool
    let how: String?
}

enum EngineError: LocalizedError {
    case notRunning
    case http(Int, String)

    var errorDescription: String? {
        switch self {
        case .notRunning: return String(localized: "검색 엔진이 아직 준비되지 않았어요.")
        case .http(let code, let body): return String(localized: "검색 엔진이 오류를 돌려줬어요 (\(code)). \(String(body.prefix(120)))")
        }
    }

    /// `error_kind` of a JSON error body (engine/crates/tacie-engine/src/errors.rs): key, credit, network, busy or down.
    var kind: String? {
        guard case .http(_, let body) = self,
              let obj = try? JSONSerialization.jsonObject(with: Data(body.utf8)) as? [String: Any] else { return nil }
        return obj["error_kind"] as? String
    }
}

/// Talks to the engine's HTTP server (engine/crates/tacie-engine/src/server.rs) on 127.0.0.1 with the per-launch bearer token.
struct EngineClient {
    let port: Int
    let token: String
    /// Tests give their own (a URLProtocol stub).
    var session: URLSession = .shared

    static let engineDecoder: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }()
    private static var decoder: JSONDecoder { engineDecoder }

    /// `sample`: the onboarding practice, searched in the sample data only. `exclude`: documents never to answer from
    /// (a saved question searched again leaves out its own meeting, docs/30).
    func ask(_ text: String, context: [String], sample: Bool = false, exclude: [String] = []) async throws -> AskResponse {
        var body: [String: Any] = ["text": text, "context": context, "sample": sample]
        if !exclude.isEmpty { body["exclude_paths"] = exclude }
        return try await post("/ask", body: body)
    }

    /// `gateEmpty`: questions are still told apart when there is nothing to search yet (kept, searched later).
    /// `people`: the remembered people, known by voice from the first line. `restore`: the meeting's voices so far, when a
    /// restarted engine takes the meeting over; `after`: the highest "화자 N" already used, so new voices go on from there.
    /// `lookup`: false records only (기록만 하기): the engine tells no questions apart, so no answer cards and no paid gate.
    func newSession(sample: Bool = false, gateEmpty: Bool = false, lookup: Bool = true,
                    people: [(id: String, vector: [Float])] = [],
                    restore: [MeetingPrint] = [], after: Int = 0) async throws -> String {
        struct Created: Codable { let sessionId: String }
        var body: [String: Any] = ["sample": sample, "gate_empty": gateEmpty]
        if !lookup { body["lookup"] = false }
        if !people.isEmpty { body["people"] = people.map { ["id": $0.id, "vector": $0.vector] } }
        if !restore.isEmpty {
            body["restore"] = restore.map { p in
                var d: [String: Any] = ["voice": p.key, "channel": p.channel, "vector": p.vector, "seconds": p.seconds]
                if let person = p.person { d["person"] = person }
                if let diar = p.diarVector { d["diar_vector"] = diar }
                return d
            }
        }
        if after > 0 { body["after"] = after }
        let r: Created = try await post("/sessions", body: body)
        return r.sessionId
    }

    /// The meeting's speakers with their voices. `exclude`: the engine's ids of lines named on their own (이 줄만),
    /// left out of their speaker's voice.
    func voices(session: String, exclude: [String] = []) async throws -> [EngineVoice] {
        struct Voices: Codable { let voices: [EngineVoice] }
        let r: Voices = try await post("/sessions/\(session)/voices", body: ["exclude": exclude])
        return r.voices
    }

    /// The names given to the meeting's speakers (key → name), the whole table each time: the engine's context lines and
    /// notes use them from then on.
    /// Lines named on their own while the meeting goes on (engine ids): the engine moves them out of their speaker's voice
    /// into `into` (a speaker key of the same channel) or a new voice, and compares later utterances with it. The voice
    /// they went to ("화자 7").
    func split(session: String, utterances: [String], into: String?) async throws -> String {
        struct Moved: Codable { let voice: String }
        var body: [String: Any] = ["utterances": utterances]
        if let into { body["into"] = into }
        let r: Moved = try await post("/sessions/\(session)/split", body: body)
        return r.voice
    }

    /// 기록만 하기 switched mid-meeting: utterances heard from now on are (on) or are not (off) told apart as questions.
    func lookup(session: String, on: Bool) async throws {
        struct Done: Codable { let lookup: Bool }
        let _: Done = try await post("/sessions/\(session)/lookup", body: ["on": on])
    }

    func names(session: String, _ map: [String: String]) async throws {
        struct Done: Codable {}
        let _: Done = try await post("/sessions/\(session)/names", body: ["names": map])
    }

    /// One utterance as 16 kHz mono WAV: transcribed and gated by the engine. speaker is the channel in remote meetings,
    /// "me" (the microphone) or "them" (the call).
    /// `transcript`: what Soniox heard (docs/25); the engine then skips its own transcription.
    func hear(session: String, wav: Data, gapMs: Int?, speaker: String?, transcript: StreamTranscript? = nil) async throws -> Heard {
        var req = request("/sessions/\(session)/audio")
        req.setValue("audio/wav", forHTTPHeaderField: "Content-Type")
        if let transcript { req.setValue(transcript.header, forHTTPHeaderField: "X-Transcript") }
        if let gapMs { req.setValue(String(gapMs), forHTTPHeaderField: "X-Gap-Ms") }
        if let speaker { req.setValue(speaker, forHTTPHeaderField: "X-Speaker") }
        req.httpBody = wav
        req.timeoutInterval = Self.wait(25)
        let (data, resp) = try await URLSession.shared.data(for: req)
        try check(resp, data)
        return try Self.decoder.decode(Heard.self, from: data)
    }

    func find(session: String, utterance: String) async throws -> Found {
        try await post("/sessions/\(session)/utterances/\(utterance)/cards", body: [:])
    }

    func answer(_ text: String, context: [String], chunks: [Int], sample: Bool = false) async throws -> Answer {
        try await post("/answer", body: ["text": text, "context": context, "chunk_ids": chunks, "sample": sample])
    }

    /// What the meeting decided and left open. One answer-model call over the whole transcript, so it waits longer.
    /// `kind`: the kind the person picked; nil lets the engine pick one (docs/23).
    /// The engine's notes stream waits up to 240 s and a failed stream retries the plain 30 s call.
    static let notesTimeout: TimeInterval = 300
    /// Whether an AI address is set (the Mac app sets this hook, AIAddress.isSet): the engine then waits up to 300 s on
    /// the person's own server, so the app waits longer too. Off, every wait is today's (the official app).
    nonisolated(unsafe) static var addressSet: () -> Bool = { false }
    /// How long to wait for the engine: today's value, or with an AI address at least 330 s. Meeting notes wait 1600 s:
    /// the kind, the outline, the topics, the summary and, when that fails, the older call each get 300 s on the engine.
    static func wait(_ today: TimeInterval, ledger: Bool = false, addressSet: Bool = EngineClient.addressSet()) -> TimeInterval {
        guard addressSet else { return today }
        return ledger ? 1600 : max(today, 330)
    }

    /// `language`: the app language when 설정 → 회의 → 정리 언어 is 앱 언어 (`NotesLanguage.target`); nil writes the notes
    /// in the meeting's language.
    func ledger(session: String, kind: NotesKind? = nil, language: String? = NotesLanguage.target()) async throws -> Ledger {
        var body: [String: Any] = [:]
        if let kind { body["kind"] = kind.rawValue }
        if let language { body["language"] = language }
        // the engine streams long notes for up to 4 minutes (docs/14 "회의 정리 스트리밍"), then may retry the plain call
        return try await post("/sessions/\(session)/ledger", body: body, timeout: Self.wait(Self.notesTimeout, ledger: true))
    }

    /// The notes again from a transcript the app kept: for 다시 쓰기, when the engine may have forgotten the meeting.
    /// Lines go by their key ("e0:s1u3"): ids from before an engine restart stay apart. `asked` holds the keys of questions.
    /// `speaker` says who said a line: a name for lines with a speaker (the engine takes it as an attendee), me/them for
    /// remote meetings saved before speakers.
    func notes(lines: [TranscriptLine], asked: [String], speaker: (TranscriptLine) -> String? = { $0.speaker },
               kind: NotesKind? = nil, request wish: String? = nil,
               language: String? = NotesLanguage.target()) async throws -> Ledger {
        let body: [[String: Any]] = lines.map { l in
            var d: [String: Any] = ["id": l.key, "text": l.text]
            if let s = speaker(l) { d["speaker"] = s }
            return d
        }
        var request: [String: Any] = ["lines": body, "asked": asked]
        if let kind { request["kind"] = kind.rawValue }
        // the panel's 요청대로 다시 쓰기 (docs/38 12절)
        if let wish, !wish.isEmpty { request["request"] = wish }
        if let language { request["language"] = language }
        return try await post("/ledger", body: request, timeout: Self.wait(Self.notesTimeout, ledger: true))
    }

    /// 문서로 남길 질문 read against the meeting (docs/24 4절): whether it was answered aloud, is still open, or asked for no
    /// fact, and the question rewritten to read on its own. Lines go by key as in `notes`; `line` is the key of the line
    /// the question was heard in.
    func reviewQuestions(lines: [TranscriptLine], questions: [(id: String, line: String?, text: String)],
                         speaker: (TranscriptLine) -> String? = { $0.speaker }) async throws -> QuestionReviewReply {
        let body: [[String: Any]] = lines.map { l in
            var d: [String: Any] = ["id": l.key, "text": l.text]
            if let s = speaker(l) { d["speaker"] = s }
            return d
        }
        let asked: [[String: Any]] = questions.map { q in
            var d: [String: Any] = ["id": q.id, "text": q.text]
            if let line = q.line { d["line"] = line }
            return d
        }
        return try await post("/questions/review", body: ["lines": body, "questions": asked], timeout: 45)
    }

    func health() async throws -> EngineHealth { try await get("/health") }

    func indexState() async throws -> IndexState { try await get("/index") }

    func config() async throws -> EngineConfig { try await get("/config") }

    /// Starts a refresh in the engine (or finds one running) and returns its state right away.
    func refreshIndex() async throws -> IndexState.Refresh {
        struct Started: Codable { let started: Bool; let refresh: IndexState.Refresh }
        let r: Started = try await post("/index/refresh", body: [:])
        return r.refresh
    }

    /// Asks the engine whether a sync would find anything new (git ls-remote and GitHub probes, up to 20 s each).
    /// Throws on 409, while a sync or a delete runs.
    func checkIndex() async throws -> IndexCheck {
        try await post("/index/check", body: [:], timeout: 90)
    }

    /// Hands the engine the GitHub login to use: mode "token" (with the token), "gh" (dev builds) or "none". True when connected.
    func setGitHub(mode: String, token: String?) async throws -> Bool {
        struct Answer: Codable { let connected: Bool }
        var body: [String: Any] = ["mode": mode]
        if let token { body["token"] = token }
        let r: Answer = try await post("/github/credentials", body: body)
        return r.connected
    }


    /// Development build only: a Soniox temporary key for the shadow transcription, minted by the dev engine from its key
    /// file (docs/25). The app never reads the key file itself.
    func streamKey() async throws -> StreamKey {
        try await post("/stt/stream-key", body: [:])
    }

    /// The meeting's team terms, sent to Soniox as `context.terms` (empty for the sample meeting).
    func sessionTerms(_ session: String) async throws -> [String] {
        struct Answer: Decodable { let terms: [String] }
        let r: Answer = try await get("/sessions/\(session)/terms", timeout: 10)
        return r.terms
    }

    /// Switches on-device speech recognition: `model` is the file's absolute path, nil when there is none (yet).
    func setSTT(on: Bool, model: URL?) async throws -> STTStatus {
        try await post("/stt", body: ["on": on, "model": model?.path ?? NSNull()])
    }

    /// Hands the engine the Notion connection (memory only, never logged), or nil to disconnect: the engine then drops
    /// the found pages. A change starts a sync in the engine.
    func setNotion(token: String?, workspace: String?) async throws -> Bool {
        struct Answer: Codable { let connected: Bool }
        let body: [String: Any] = ["token": token ?? NSNull(), "workspace": workspace ?? NSNull()]
        let r: Answer = try await post("/notion/credentials", body: body)
        return r.connected
    }

    /// Every finished meeting the engine may search (docs/30): the engine keeps exactly this list. Returns how many it got.
    /// Which of this meeting's decisions changed one of the past decisions (docs/30 8절): pairs of positions in the two
    /// lists. `past` is (day, meeting title, decision), newest first. An engine error comes back as no pairs.
    func decisionChanges(_ decisions: [String], past: [(String, String, String)]) async throws -> [(Int, Int)] {
        struct Pair: Codable { let decision: Int; let past: Int }
        struct Answer: Codable { let changes: [Pair] }
        let body: [String: Any] = [
            "decisions": decisions,
            "past": past.map { ["at": $0.0, "title": $0.1, "text": $0.2] },
        ]
        let r: Answer = try await post("/decisions/changes", body: body, timeout: 40)
        return r.changes.map { ($0.decision, $0.past) }
    }

    /// The Tacie session stands in for both AI keys, so the engine's AI calls and its meeting search go through the relay
    /// (docs/14, the Mac's IndexKeeper.aiKeys). Memory only, never logged.
    func setRelaySession(_ token: String) async throws {
        let _: [String: Bool] = try await post("/ai/credentials", body: ["openrouter": token, "typesafe": token])
    }

    /// Drops the session from the engine's memory: a null key removes it, as the Mac's `setAIKeys` does.
    func clearRelaySession() async throws {
        let _: [String: Bool] = try await post("/ai/credentials", body: ["openrouter": NSNull(), "typesafe": NSNull()])
    }

    func setMeetings(_ list: [MeetingDigest]) async throws -> Int {
        struct Answer: Codable { let count: Int }
        let r: Answer = try await post("/meetings", body: ["meetings": list.map(\.json)], timeout: 30)
        return r.count
    }



    /// The shared drives and their file counts (the drive picker).
    func gdriveDrives() async throws -> [GDriveDrive] {
        struct Answer: Codable { let drives: [GDriveDrive] }
        let r: Answer = try await get("/gdrive/drives", timeout: 10)
        return r.drives
    }


    /// The Jira projects and Confluence spaces with their counts (the project · space picker).
    func atlassianScopes() async throws -> AtlassianScopes {
        try await get("/atlassian/scopes", timeout: 10)
    }


    /// The Asana workspaces and projects with their task counts (the workspace · project picker).
    func asanaScopes() async throws -> AsanaScopes {
        try await get("/asana/scopes", timeout: 10)
    }



    /// The top folders and their file counts (the folder picker).
    func dropboxFolders() async throws -> [DropboxFolder] {
        struct Answer: Codable { let folders: [DropboxFolder] }
        let r: Answer = try await get("/dropbox/folders", timeout: 10)
        return r.folders
    }

    /// The visible Linear teams and their issue counts (the team picker).
    func linearTeams() async throws -> [LinearTeam] {
        struct Answer: Codable { let teams: [LinearTeam] }
        let r: Answer = try await get("/linear/teams", timeout: 10)
        return r.teams
    }

    func githubRepos() async throws -> GitHubRepos { try await get("/github/repos", timeout: 40) }

    func githubBranches(_ repo: String) async throws -> GitHubBranches {
        var c = URLComponents()
        c.queryItems = [URLQueryItem(name: "repo", value: repo)]
        return try await get("/github/branches" + (c.percentEncodedQuery.map { "?\($0)" } ?? ""), timeout: 40)
    }

    /// Starts a rebuild for a repo and branch (nil: the saved ones); `fresh` downloads every file again instead of reusing the chunk cache.
    func rebuildIndex(repo: String?, branch: String?, fresh: Bool) async throws -> IndexState.Refresh {
        struct Started: Codable { let started: Bool; let refresh: IndexState.Refresh }
        var body: [String: Any] = ["fresh": fresh]
        if let repo { body["repo"] = repo }
        if let branch { body["branch"] = branch }
        let r: Started = try await post("/index/rebuild", body: body)
        return r.refresh
    }

    /// Deletes the search data. False (409) when a sync is running.
    func clearIndex() async throws -> Bool {
        var req = request("/index/clear")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = Data("{}".utf8)
        let (data, resp) = try await URLSession.shared.data(for: req)
        if (resp as? HTTPURLResponse)?.statusCode == 409 { return false }
        try check(resp, data)
        return true
    }

    /// POST /panel/prep: reads the lines of the ndjson stream, hands each progress step to `onStep` and returns the
    /// `done` line. Throws when the stream ends without one.
    func prep(meetingIDs: [String], onStep: @Sendable (PanelStep) -> Void) async throws -> PrepAnswer {
        var req = request("/panel/prep")
        req.timeoutInterval = Self.wait(120)
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: ["meeting_ids": meetingIDs])
        let (bytes, resp) = try await session.bytes(for: req)
        if let http = resp as? HTTPURLResponse, http.statusCode != 200 {
            var data = Data()
            for try await b in bytes { data.append(b) }
            throw EngineError.http(http.statusCode, String(decoding: data, as: UTF8.self))
        }
        struct Line: Decodable {
            let step: String?
            let ids: [String]?
            let count: Int?
            let done: PrepAnswer?
        }
        for try await text in bytes.lines {
            guard !text.isEmpty else { continue }
            let line = try Self.decoder.decode(Line.self, from: Data(text.utf8))
            if let done = line.done { return done }
            switch line.step {
            case "meetings": onStep(.meetings(line.ids ?? []))
            case "related": onStep(.related(line.ids ?? []))
            case "sources": onStep(.sources(line.count ?? 0))
            case "writing": onStep(.writing)
            default: break
            }
        }
        throw EngineError.http(0, "{\"error\":\"prep stream ended without an answer\"}")
    }

    /// "HH:mm" for the why window; POSIX so a 12-hour Mac setting does not turn it into "오후 3:12".
    static let whyTime: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "HH:mm"
        return f
    }()

    /// POST /panel/why: the transcript lines around a decision go as key, speaker name, "HH:mm" and text only.
    func why(decision: String, window: [TranscriptLine], names: (TranscriptLine) -> String) async throws -> WhyAnswer {
        let lines = window.map { ["key": $0.key, "speaker": names($0), "at": Self.whyTime.string(from: $0.at), "text": $0.text] }
        return try await post("/panel/why", body: ["decision": decision, "window": lines], timeout: 60)
    }

    /// POST /panel/ask: `meetingIDs` nil searches every meeting; `history` is the earlier turns of this thread.
    func panelAsk(_ text: String, meetingIDs: [String]?, history: [(q: String, a: String)]) async throws -> PanelAsk {
        let body: [String: Any] = [
            "text": text,
            "meeting_ids": meetingIDs.map { $0 as Any } ?? NSNull(),
            "history": history.map { ["q": $0.q, "a": $0.a] },
        ]
        return try await post("/panel/ask", body: body, timeout: 60)
    }

    /// POST /panel/chat (docs/38 6절): reads the ndjson lines, hands each tool step to `onStep` and returns the `done`
    /// line. Throws when the stream ends without one.
    func panelChat(_ r: PanelChatRequest, onStep: @Sendable (PanelStep) -> Void) async throws -> ChatAnswer {
        var req = request("/panel/chat")
        req.timeoutInterval = Self.wait(120)
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let body: [String: Any] = [
            "text": r.text,
            "meeting_ids": r.meetingIDs,
            "history": r.history.map { ["q": $0.q, "a": $0.a] },
            "today": r.today,
            "transcripts": r.transcripts,
            "refs": r.refs,
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (bytes, resp) = try await session.bytes(for: req)
        if let http = resp as? HTTPURLResponse, http.statusCode != 200 {
            var data = Data()
            for try await b in bytes { data.append(b) }
            throw EngineError.http(http.statusCode, String(decoding: data, as: UTF8.self))
        }
        struct Done: Decodable { let done: ChatAnswer }
        for try await text in bytes.lines {
            guard !text.isEmpty else { continue }
            let data = Data(text.utf8)
            if text.hasPrefix("{\"done\"") { return try Self.decoder.decode(Done.self, from: data).done }
            guard let line = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let step = line["step"] as? [String: Any], let tool = step["tool"] as? String else { continue }
            if step["done"] as? Bool == true {
                onStep(.tool(tool, detail: [:], count: step["count"] as? Int ?? 0))
            } else {
                onStep(.tool(tool, detail: Self.shown(step["args"] as? [String: Any] ?? [:]), count: nil))
            }
        }
        throw EngineError.http(0, "{\"error\":\"chat stream ended without an answer\"}")
    }

    /// A step's arguments as text: lists joined with ",", true/false as "true"/"false".
    static func shown(_ args: [String: Any]) -> [String: String] {
        var out: [String: String] = [:]
        for (k, v) in args {
            switch v {
            case let s as String: out[k] = s
            case let b as Bool: out[k] = b ? "true" : "false"
            case let a as [Any]: out[k] = a.map { "\($0)" }.joined(separator: ",")
            default: out[k] = "\(v)"
            }
        }
        return out
    }

    /// POST /panel/followup: which prepared items the meeting's ledger settled.
    func followup(prepared: [PanelItem], ledger: [LedgerEntry]) async throws -> [FollowResult] {
        struct Answer: Decodable { let results: [FollowResult] }
        let body: [String: Any] = [
            "prepared": prepared.map { ["text": $0.text] },
            "ledger": ledger.map { ["kind": $0.kind, "text": $0.text] },
        ]
        let r: Answer = try await post("/panel/followup", body: body, timeout: 60)
        return r.results
    }

    func get<T: Decodable>(_ path: String, timeout: TimeInterval = 60) async throws -> T {
        var req = URLRequest(url: URL(string: "http://127.0.0.1:\(port)\(path)")!)
        req.timeoutInterval = Self.wait(timeout)
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let (data, resp) = try await URLSession.shared.data(for: req)
        try check(resp, data)
        return try Self.decoder.decode(T.self, from: data)
    }

    func request(_ path: String) -> URLRequest {
        var req = URLRequest(url: URL(string: "http://127.0.0.1:\(port)\(path)")!)
        req.httpMethod = "POST"
        req.timeoutInterval = Self.wait(15)
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return req
    }

    func post<T: Decodable>(_ path: String, body: [String: Any], timeout: TimeInterval = 15) async throws -> T {
        var req = request(path)
        req.timeoutInterval = Self.wait(timeout)
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, resp) = try await session.data(for: req)
        try check(resp, data)
        return try Self.decoder.decode(T.self, from: data)
    }

    func check(_ resp: URLResponse, _ data: Data) throws {
        guard let http = resp as? HTTPURLResponse else { return }
        if http.statusCode != 200 {
            throw EngineError.http(http.statusCode, String(decoding: data, as: UTF8.self))
        }
    }
}

/// The engine's `POST /stt/stream-key` reply (snake case, read with `engineDecoder`).
struct StreamKey: Decodable, Equatable {
    let apiKey: String
    let expiresAt: String
    let wsUrl: URL
}

/// An utterance's text from the Soniox stream, sent with its WAV as `X-Transcript` (base64 JSON, word times in seconds
/// from the utterance start).
struct StreamTranscript: Equatable {
    struct Word: Equatable, Encodable { let text: String; let start: Double; let end: Double }
    let text: String
    let words: [Word]
    /// The app's own: kept for the transcript line, not sent to the engine (docs/30).
    var translation: String? = nil
    var translatedFrom: String? = nil

    var header: String {
        struct Body: Encodable { let text: String; let words: [Word] }
        return ((try? JSONEncoder().encode(Body(text: text, words: words))) ?? Data()).base64EncodedString()
    }
}

extension EngineClient: EngineFeeding {}
