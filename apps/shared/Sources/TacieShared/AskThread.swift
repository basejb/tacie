import Foundation
import Observation

/// What `/panel/chat` needs from an engine; `EngineClient` is the real one, tests give a fake.
protocol PanelChatting: Sendable {
    func panelChat(_ r: PanelChatRequest, onStep: @Sendable (PanelStep) -> Void) async throws -> ChatAnswer
}

extension EngineClient: PanelChatting {}

/// Why a turn has no answer, from the engine's `error_kind` (docs/40 6-5).
enum AskThreadFailure: Equatable {
    case signedOut, limit, down, network

    init(kind: String?) {
        switch kind {
        case "key": self = .signedOut
        case "credit": self = .limit
        case "network": self = .network
        default: self = .down
        }
    }
}

/// The old server-side answer (`/meetings/ask`) the all-meetings thread falls back to when the engine is missing or
/// fails (not on a cancel, the month's limit or a refused session).
struct FallbackAnswer: Equatable {
    struct Cited: Equatable {
        let id: String
        let title: String
        let start: String
        var quote: String? = nil
    }
    let answer: String?
    let meetings: [Cited]
    let near: [Cited]
}

/// The state behind the iPhone ask screen and the meeting page's AI (docs/40 6-2, 6-5, 6-6): a list of turns, each
/// question sent to the app's own engine `/panel/chat`. Holds no iOS types; the caller keeps the app alive while
/// `isWaiting` (beginBackgroundTask) and builds `engine` from the engine's and the feed's readiness.
///
/// On the ask screen (all meetings) the engine may hand a fix to one meeting (`handoff`, docs/40 12절): the same
/// question goes again, in the same turn, as the meeting page would send it (that meeting's id, its transcript and
/// ledger refs, and nothing of any other meeting). Once per question; the second answer's proposals are that meeting's.
@MainActor @Observable
final class AskThread {
    struct Turn: Identifiable {
        let id = UUID()
        let question: String
        var steps: [PanelStep] = []
        var answer: ChatAnswer?
        var proposals: [PanelProposal] = []
        /// The meeting the question was handed to (ask screen only): the proposals are for it.
        var handoff: String?
        var failure: AskThreadFailure?
        var usedFallback = false
        var fallback: FallbackAnswer?
    }

    static let historyLimit = 6

    private(set) var turns: [Turn] = []
    let meetingID: String?
    /// One meeting's transcript lines and ledger refs, keyed by its id; empty when it has no transcript.
    private let transcript: (String) -> (lines: [String: [[String: String]]], refs: [String: [[String]]])
    private let engine: () async -> PanelChatting?
    private let fallback: (String, [String]) async -> FallbackAnswer?

    /// True while a question has neither an answer nor a failure.
    var isWaiting: Bool { turns.contains { $0.answer == nil && $0.failure == nil && $0.fallback == nil && !$0.usedFallback } }

    /// `engine` is nil when the engine is missing or not ready (counted as `engine_missing`); an engine that answers
    /// with an error falls back as `engine_failed`. `fallback` gets the question and the earlier questions of the thread.
    init(meetingID: String?,
         transcript: @escaping (String) -> (lines: [String: [[String: String]]], refs: [String: [[String]]]),
         engine: @escaping () async -> PanelChatting?,
         fallback: @escaping (String, [String]) async -> FallbackAnswer?) {
        self.meetingID = meetingID
        self.transcript = transcript
        self.engine = engine
        self.fallback = fallback
    }

    func ask(_ q: String) async {
        let turn = Turn(question: q)
        turns.append(turn)
        await run(turn.id)
    }

    /// Asks a failed turn again in place (the person pressed it).
    func retry(_ id: Turn.ID) async {
        // only a turn that failed: one still streaming has no failure, and a second run would race the first
        guard let i = turns.firstIndex(where: { $0.id == id }), turns[i].failure != nil else { return }
        turns[i].failure = nil
        turns[i].answer = nil
        turns[i].steps = []
        turns[i].proposals = []
        turns[i].handoff = nil
        turns[i].fallback = nil
        turns[i].usedFallback = false
        await run(id)
    }

    // MARK: -

    private func index(_ id: Turn.ID) -> Int? { turns.firstIndex { $0.id == id } }

    /// The last 6 answered turns before `i`.
    private func history(before i: Int) -> [(q: String, a: String)] {
        let pairs: [(q: String, a: String)] = turns[..<i].compactMap { t in
            guard let a = t.answer?.answer ?? t.fallback?.answer, t.failure == nil else { return nil }
            return (t.question, a)
        }
        return Array(pairs.suffix(Self.historyLimit))
    }

    static func day(_ d: Date = Date()) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd"
        return f.string(from: d)
    }

    private func run(_ id: Turn.ID) async {
        guard let i = index(id) else { return }
        let question = turns[i].question
        guard let chat = await engine() else {
            await settle(id, failure: .down, kind: nil, cancelled: false, reason: .engineMissing)
            return
        }
        var r = PanelChatRequest(text: question, history: history(before: i), today: Self.day())
        if let mid = meetingID { r = Self.scoped(r, to: mid, transcript(mid)) }
        let result = await attemptRetrying(chat, r, id)
        guard let j = index(id) else { return }
        switch result {
        case .success(let a):
            if let k = a.errorKind {
                await settle(id, failure: AskThreadFailure(kind: k), kind: k, cancelled: false, reason: .engineFailed)
            } else if meetingID == nil, let to = a.handoff?.meetingId, turns[j].handoff == nil {
                turns[j].handoff = to
                await handOff(id, chat: chat, Self.scoped(r, to: to, transcript(to)))
            } else {
                turns[j].answer = a
                turns[j].proposals = a.proposals
            }
        case .failure(let e):
            let kind = (e as? EngineError)?.kind ?? ((e as? URLError) != nil ? "network" : nil)
            // a cancelled call is not a network failure: it ends as a plain "no answer" the person can ask again
            let cancelled = Self.isCancel(e)
            await settle(id, failure: cancelled ? .down : AskThreadFailure(kind: kind), kind: kind,
                         cancelled: cancelled, reason: .engineFailed)
        }
    }

    /// The request a meeting page sends: that meeting only, with its transcript and refs (none of any other meeting).
    static func scoped(_ r: PanelChatRequest, to id: String,
                       _ t: (lines: [String: [[String: String]]], refs: [String: [[String]]])) -> PanelChatRequest {
        var r = r
        r.meetingIDs = [id]
        r.transcripts = t.lines.filter { $0.key == id }
        r.refs = t.refs.filter { $0.key == id }
        return r
    }

    /// The handed-off question, asked on the meeting: the steps so far stay (the hand-off's line leads into the
    /// meeting's own), and its answer is the turn's. It does not hand off again, and a failure has no older route to
    /// fall back to (as on the meeting page).
    private func handOff(_ id: Turn.ID, chat: PanelChatting, _ r: PanelChatRequest) async {
        let before = index(id).map { turns[$0].steps } ?? []
        let result = await attemptRetrying(chat, r, id, keeping: before)
        guard let j = index(id) else { return }
        switch result {
        case .success(let a):
            if let k = a.errorKind {
                turns[j].failure = AskThreadFailure(kind: k)
            } else {
                turns[j].answer = a
                turns[j].proposals = a.proposals
            }
        case .failure(let e):
            let kind = (e as? EngineError)?.kind ?? ((e as? URLError) != nil ? "network" : nil)
            turns[j].failure = Self.isCancel(e) ? .down : AskThreadFailure(kind: kind)
        }
    }

    /// One call, and once more when the stream broke (docs/40 6-5).
    private func attemptRetrying(_ chat: PanelChatting, _ r: PanelChatRequest, _ id: Turn.ID,
                                 keeping before: [PanelStep] = []) async -> Result<ChatAnswer, Error> {
        var result = await attempt(chat, r, id, keeping: before)
        if case .failure(let e) = result, Self.worthRetrying(e) {
            result = await attempt(chat, r, id, keeping: before)
            let ok: Bool
            if case .success = result { ok = true } else { ok = false }
            Analytics.capture(.askRetried(succeeded: ok))
        }
        return result
    }

    /// The engine gave no answer. The ask screen answers from the old `/meetings/ask` instead (docs/40 6-5), unless
    /// the person cancelled, or the error is the month's limit (the limit state stays) or the session (the old ask
    /// sends the same session and would be refused too). The meeting page has no fallback.
    private func settle(_ id: Turn.ID, failure: AskThreadFailure, kind: String?, cancelled: Bool,
                        reason: AFallbackReason) async {
        guard let i = index(id) else { return }
        let falls = meetingID == nil && !cancelled && !Task.isCancelled && kind != "credit" && kind != "key"
        guard falls else { turns[i].failure = failure; return }
        let earlier = turns[..<i].map(\.question)
        Analytics.capture(.askFallback(reason: reason))
        let a = await fallback(turns[i].question, earlier)
        guard let j = index(id) else { return }
        turns[j].usedFallback = true
        turns[j].fallback = a
        if a == nil { turns[j].failure = failure }
    }

    private static func isCancel(_ e: Error) -> Bool {
        e is CancellationError || (e as? URLError)?.code == .cancelled
    }

    /// A key or credit error will not change by asking again.
    private static func worthRetrying(_ e: Error) -> Bool {
        if isCancel(e) { return false }
        guard let k = (e as? EngineError)?.kind else { return true }
        return k != "key" && k != "credit"
    }

    /// One call; the steps it reports are kept in order on the turn as they arrive, after `before`.
    private func attempt(_ chat: PanelChatting, _ r: PanelChatRequest, _ id: Turn.ID,
                         keeping before: [PanelStep] = []) async -> Result<ChatAnswer, Error> {
        let seen = StepBox(before)
        if let i = index(id) { turns[i].steps = before }
        let out: Result<ChatAnswer, Error>
        do {
            out = .success(try await chat.panelChat(r) { step in
                let all = seen.add(step)
                Task { @MainActor [weak self] in
                    guard let self, let i = self.index(id), self.turns[i].answer == nil else { return }
                    self.turns[i].steps = all
                }
            })
        } catch {
            out = .failure(error)
        }
        // the hops above may still be queued: the box has the whole list
        if let i = index(id) { turns[i].steps = seen.all }
        return out
    }
}

private final class StepBox: @unchecked Sendable {
    private let lock = NSLock()
    private var steps: [PanelStep]
    init(_ steps: [PanelStep] = []) { self.steps = steps }
    func add(_ s: PanelStep) -> [PanelStep] { lock.lock(); defer { lock.unlock() }; steps.append(s); return steps }
    var all: [PanelStep] { lock.lock(); defer { lock.unlock() }; return steps }
}
