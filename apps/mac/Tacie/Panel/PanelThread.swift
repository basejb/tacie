import Foundation

/// What a panel thread is about: one meeting, or every meeting and connected source (docs/36 1-2절).
enum PanelScope: Codable, Equatable {
    case meeting(String)  // the meeting's id
    case all
}

/// What the panel asks the engine for. `why` points at one ledger entry of a meeting.
enum PanelKind: Equatable {
    case prep
    case why(meetingID: String, ledgerIndex: Int)
    case ask
}

/// The answer of one turn, as it is shown and saved. `failed` keeps the engine's `error_kind` (key, credit, network, ...).
enum PanelTurnBody: Codable, Equatable {
    case prep(PrepAnswer)
    case why(WhyAnswer, meetingID: String)
    case ask(PanelAsk)
    /// An answer of POST /panel/chat (docs/38), whatever was asked.
    case chat(ChatAnswer)
    case failed(kind: String)
}

extension PanelTurnBody {
    /// The prep it shows: a prep answer, or the whole prep a chat answer carries.
    var prepAnswer: PrepAnswer? {
        switch self {
        case .prep(let a): return a
        case .chat(let a): return a.prep
        default: return nil
        }
    }

    /// What the usage numbers take from an answer (docs/16): how many sources it shows, whether the meetings and sources
    /// did not say, and why it failed. No text.
    var usage: (sources: Int, notFound: Bool, error: PanelError?) {
        func err(_ k: String?) -> PanelError? { k.map(PanelError.init(kind:)) }
        switch self {
        case .prep(let a): return (a.sources.count, false, err(a.errorKind))
        // not found is what the screen shows as not found (PanelTurnView), so the engine's no-answer sentence counts too
        case .why(let a, _): return (a.keys.count, a.errorKind == nil && !PanelShown.found(a.reason, sources: a.keys.count), err(a.errorKind))
        case .ask(let a): return (a.cards.count, a.errorKind == nil && !PanelShown.found(a.answer, sources: a.cards.count), err(a.errorKind))
        case .chat(let a): return (a.cards.count, a.errorKind == nil && PanelText.isNoAnswer(a.answer), err(a.errorKind))
        // "none": nothing to ask (a prep outside a meeting, a decision that is gone), not a failure
        case .failed(let kind): return (0, false, kind == "none" ? nil : PanelError(kind: kind))
        }
    }
}

struct PanelTurn: Codable, Equatable, Identifiable {
    let id: UUID
    /// The text shown as the person's bubble.
    let question: String
    let at: Date
    /// The progress lines shown while searching (prep and chat).
    var steps: [PanelStep]
    /// nil while the request runs.
    var body: PanelTurnBody?
    /// When the answer came back to this question asked here (nil for one that came with the turn): the panel lets
    /// it come in piece by piece from then (docs/36 0-2절 5–7).
    var answeredAt: Date? = nil
    /// What the person did with each proposal card under the answer (docs/38 12절), by proposal id; nil until anything.
    var proposalStates: [String: ProposalState]? = nil

    /// The answer's proposals (a chat answer's only).
    var proposals: [PanelProposal] {
        if case .chat(let a)? = body { return a.proposals }
        return []
    }

    /// The answer as plain text, for the history sent with the next question; nil while running or when it failed
    /// (also an ask or why that came back with an error: it would read as an empty answer).
    var answerText: String? {
        switch body {
        case .prep(let a): return a.advice
        case .why(let a, _): return a.errorKind == nil ? a.reason : nil
        case .ask(let a): return a.errorKind == nil ? a.answer ?? "" : nil
        case .chat(let a): return a.errorKind == nil ? a.answer ?? "" : nil
        case .failed, nil: return nil
        }
    }
}

/// One conversation of the panel: a title, the place it started (docs/36 0-1절) and its turns.
struct PanelThread: Codable, Identifiable, Equatable {
    static let titleLimit = 28

    let id: UUID
    var title: String
    /// The meeting the conversation started on, or every meeting; it decides which meetings its questions read.
    let origin: PanelScope
    var turns: [PanelTurn]

    init(id: UUID, title: String, origin: PanelScope, turns: [PanelTurn]) {
        self.id = id
        self.title = title
        self.origin = origin
        self.turns = turns
    }

    /// The first message, trimmed and cut to `titleLimit` characters with "…".
    static func title(from text: String) -> String {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.count > titleLimit ? String(t.prefix(titleLimit)) + "…" : t
    }

    /// A title made around a part (a meeting's name, a decision): the part is cut so the whole is no longer than
    /// a cut typed title, and the words around it stay.
    static func title(_ part: String, around make: (String) -> String) -> String {
        let p = part.trimmingCharacters(in: .whitespacesAndNewlines)
        let full = make(p)
        let over = full.count - titleLimit
        guard over > 0 else { return full }
        let cut = make(String(p.prefix(max(0, p.count - over))) + "…")
        // words around that are too long by themselves: cut the whole like a typed title
        return cut.count > titleLimit + 1 ? title(from: cut) : cut
    }

    private enum Keys: String, CodingKey { case id, title, origin, scope, turns }

    /// Saved lists from before conversations have `scope` and no `title` or `origin`: the scope is the origin and the
    /// first question is the title.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        id = try c.decode(UUID.self, forKey: .id)
        turns = try c.decode([PanelTurn].self, forKey: .turns)
        if let o = try c.decodeIfPresent(PanelScope.self, forKey: .origin) {
            origin = o
        } else {
            origin = try c.decode(PanelScope.self, forKey: .scope)
        }
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? Self.title(from: turns.first?.question ?? "")
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        try c.encode(id, forKey: .id)
        try c.encode(title, forKey: .title)
        try c.encode(origin, forKey: .origin)
        try c.encode(turns, forKey: .turns)
    }
}

/// A question the empty screen offers (docs/36 0-1절).
enum PanelSuggestion: Equatable {
    case ask(String)
    case prep
    case why(meetingID: String, ledgerIndex: Int)
}
