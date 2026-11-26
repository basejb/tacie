import Foundation

/// The engine calls the panel makes. The app gives `PanelEngine.live`; tests give their own.
struct PanelEngine {
    var prep: (_ meetingIDs: [String], _ onStep: @escaping @Sendable (PanelStep) -> Void) async throws -> PrepAnswer
    var why: (_ decision: String, _ window: [TranscriptLine], _ names: (TranscriptLine) -> String) async throws -> WhyAnswer
    var ask: (_ text: String, _ meetingIDs: [String]?, _ history: [(q: String, a: String)]) async throws -> PanelAsk
    /// After a linked meeting's notes: which of the prepared items (their texts) its ledger settled.
    var followup: (_ prepared: [String], _ ledger: [LedgerEntry]) async throws -> [FollowResult] = { _, _ in throw EngineError.notRunning }
    /// POST /panel/chat (docs/38): with it, every question goes there and `prep`, `why` and `ask` are not used.
    var chat: ((_ request: PanelChatRequest, _ onStep: @escaping @Sendable (PanelStep) -> Void) async throws -> ChatAnswer)? = nil

    static func live(_ c: EngineClient) -> PanelEngine {
        PanelEngine(
            prep: { try await c.prep(meetingIDs: $0, onStep: $1) },
            why: { try await c.why(decision: $0, window: $1, names: $2) },
            ask: { try await c.panelAsk($0, meetingIDs: $1, history: $2) },
            followup: { prepared, ledger in
                try await c.followup(prepared: prepared.map { PanelItem(meetingId: "", index: 0, kind: "", text: $0, owner: nil, due: nil, also: nil) },
                                     ledger: ledger)
            },
            chat: { try await c.panelChat($0, onStep: $1) })
    }
}

/// The main window's panel: its scope, the thread on screen and the saved earlier threads (docs/36 1-2, 1-4, 1-5절).
@MainActor
final class PanelStore: ObservableObject {
    static let historyLimit = 200
    /// Lines taken on each side of a line when the decision's transcript window is built.
    static let windowRadius = 6

    @Published var isOpen = false
    @Published var current: PanelThread?
    @Published var scope: PanelScope = .all
    /// Newest first, at most `historyLimit`.
    @Published private(set) var history: [PanelThread] = []

    private let engine: () -> PanelEngine?
    private let meetings: (String) -> SavedMeeting?
    private let folder: () -> URL
    private let account: () -> AccountSpace
    private let clock: () -> Date
    private let update: (String, (inout SavedMeeting) -> Void) -> Void
    /// Every meeting, to find the ones that go on from a meeting (a prep is reused only until one of them ends).
    private let allMeetings: () -> [SavedMeeting]
    /// 회의 정리 다시 쓰기 after a fix (`MeetingModel.rewriteNotes`).
    private let rewrite: (String) -> Void
    /// 다음 회의부터 알아듣기: the word goes to the account's terms (`MeetingTerms`).
    private let remember: (String) -> Void
    /// 화자 이름 붙이기 (`MeetingModel.applyName`): meeting id, speaker key, name.
    private let nameSpeaker: (String, String, String) -> Void
    /// 같은 사람으로 합치기 (`MeetingModel.applyName(.speaker)`): meeting id, the speaker key to merge, the key to keep.
    private let mergeSpeaker: (String, String, String) -> Void
    /// 이 줄만 (`MeetingModel.applyName` with lines): meeting id, the lines' keys, the speaker key or nil, the name.
    private let moveLines: (String, [String], String?, String) -> Void
    /// 요청대로 다시 쓰기 (`MeetingModel.rewriteNotes(request:)`) and 정리 형식 바꾸기 (`MeetingModel.changeKind`).
    private let rewriteAsked: (String, String?) -> Void
    private let changeKind: (String, NotesKind) -> Void
    /// The speakers' names before and after an applied name_speaker, by "turn/proposal".
    private var namingUndos: [String: (before: SavedMeeting.Naming, after: SavedMeeting.Naming)] = [:]
    /// What each applied proposal changed, by "turn/proposal", for 되돌리기 while the app runs.
    @Published private(set) var undos: [String: ProposalUndo] = [:]
    /// The preps answered in this session, for linking the next meeting to the one it was prepared from (docs/36 1-6절).
    private var preps: [(meetingID: String, at: Date, items: [PreparedItem])] = []
    /// The prep conversation still on its way, per meeting: a second press opens it instead of asking again.
    private var preparing: [String: UUID] = [:]
    private var inFlight: [UUID: Task<Void, Never>] = [:]

    init(engine: @escaping () -> PanelEngine?, meetings: @escaping (String) -> SavedMeeting?,
         folder: @escaping () -> URL = { AccountSpace.inUse.folder() },
         account: @escaping () -> AccountSpace = { AccountSpace.inUse },
         clock: @escaping () -> Date = Date.init,
         update: @escaping (String, (inout SavedMeeting) -> Void) -> Void = { _, _ in },
         allMeetings: @escaping () -> [SavedMeeting] = { [] },
         rewrite: @escaping (String) -> Void = { _ in },
         remember: @escaping (String) -> Void = { _ in },
         nameSpeaker: @escaping (String, String, String) -> Void = { _, _, _ in },
         mergeSpeaker: @escaping (String, String, String) -> Void = { _, _, _ in },
         moveLines: @escaping (String, [String], String?, String) -> Void = { _, _, _, _ in },
         rewriteAsked: @escaping (String, String?) -> Void = { _, _ in },
         changeKind: @escaping (String, NotesKind) -> Void = { _, _ in }) {
        self.engine = engine
        self.meetings = meetings
        self.folder = folder
        self.account = account
        self.clock = clock
        self.update = update
        self.allMeetings = allMeetings
        self.rewrite = rewrite
        self.remember = remember
        self.nameSpeaker = nameSpeaker
        self.mergeSpeaker = mergeSpeaker
        self.moveLines = moveLines
        self.rewriteAsked = rewriteAsked
        self.changeKind = changeKind
        history = Self.load(from: Self.file(folder()))
    }

    // MARK: Opening and moving

    /// Opens with `scope` as the place a new conversation starts. The conversation on screen stays (⌘J, reopening);
    /// with `startHere` (the pages' "Tacie AI") one that started elsewhere gives way to a new one there.
    func open(scope new: PanelScope, via: PanelVia, startHere: Bool = false) {
        Analytics.capture(.panelOpened(via: via, scope: PanelScopeKind(new)))
        if startHere, current?.origin != new { current = nil }
        isOpen = true
        scope = new
    }

    /// Starts a thread of the current scope with an answer that already exists (the island's card), nothing sent.
    func seed(question: String, ask: PanelAsk) {
        let turn = PanelTurn(id: UUID(), question: question, at: clock(), steps: [], body: .ask(ask))
        let t = PanelThread(id: UUID(), title: PanelThread.title(from: question), origin: scope, turns: [turn])
        current = t
        upsert(t)
    }

    /// The page in the middle changed: a meeting's id, or nil for a page about no meeting (결정 사항, 할 일). The
    /// conversation on screen stays as it is (its follow-ups keep its own origin); where a new one starts always moves
    /// to the page now on screen, also when a source in the panel moved it (docs/36 0-1절).
    func pageChanged(to meetingID: String?) {
        scope = pageScope(for: meetingID)
    }

    /// Where a conversation started from a page goes: the page's meeting, or every meeting on a page about no meeting.
    /// A meeting still going on is not searchable yet (the engine reads finished meetings), so its page starts on every
    /// meeting until it ends.
    func pageScope(for meetingID: String?) -> PanelScope {
        guard let id = meetingID else { return .all }
        if let m = meetings(id), m.end == nil { return .all }
        return .meeting(id)
    }

    /// A new conversation: it starts on the meeting being viewed (`scope`).
    func newThread() {
        current = nil
    }

    /// Opens a saved conversation; the page being viewed does not change.
    func reopen(_ id: UUID) {
        guard let t = history.first(where: { $0.id == id }) else { return }
        current = t
        isOpen = true
    }

    /// The account in use changed: the new account's history, an empty panel. Results still on their way are dropped
    /// when they arrive (`send` remembers the account it was sent under).
    func accountChanged() {
        current = nil
        scope = .all
        preps = []
        preparing = [:]
        history = Self.load(from: Self.file(folder()))
    }

    // MARK: Sending

    /// Most meetings a "다음 회의 준비" reads: the meeting and the ones it goes on from.
    static let prepChainLimit = 5

    /// The meeting itself, then the meetings it was linked from, newest first, at most `prepChainLimit`. The chain
    /// stops at a meeting that is gone or one already in it (a cycle).
    func prepMeetingIDs(for id: String) -> [String] {
        var ids = [id]
        var next = meetings(id)?.linkedFrom
        while let n = next, ids.count < Self.prepChainLimit, !ids.contains(n), let m = meetings(n) {
            ids.append(n)
            next = m.linkedFrom
        }
        return ids
    }

    // MARK: Linking a meeting to its prep

    /// How long after a prep a meeting still counts as the one it was prepared for.
    static let linkWindow: TimeInterval = 2 * 3600

    /// The meeting a meeting starting at `start` goes on from, with the items its prep listed: the last prep answered
    /// within two hours before `start`. A prep of a meeting still going on does not count.
    func prepLink(startingAt start: Date) -> (from: String, items: [PreparedItem])? {
        guard PanelFeatures.current.meetingLink else { return nil }
        let gap = { (p: (meetingID: String, at: Date, items: [PreparedItem])) in start.timeIntervalSince(p.at) }
        guard let p = preps.last(where: { 0...Self.linkWindow ~= gap($0) && meetings($0.meetingID)?.end != nil }) else { return nil }
        return (p.meetingID, p.items)
    }

    /// Most prepared items a follow-up sends: the engine's reply has room for about this many.
    static let followUpLimit = 20

    /// After a linked meeting's notes: which prepared items the meeting settled, the first `followUpLimit` of them.
    /// A failed request leaves them unjudged, and the page then shows no block.
    func followUp(_ id: String) async {
        guard PanelFeatures.current.meetingLink, let m = meetings(id), m.notes == .done, let prepared = m.prepared, !prepared.isEmpty, let engine = engine() else { return }
        // the first ones are the prep's first view (`preparedItems`); the rest stay unjudged and the block leaves them out
        let sent = prepared.prefix(Self.followUpLimit)
        var results: [FollowResult] = []
        if !m.ledger.isEmpty {
            do {
                results = try await engine.followup(sent.map(\.text), m.ledger)
            } catch {
                Log.info("prepared items not judged: \(Self.kind(of: error))")
                return
            }
        }
        update(id) { m in
            guard var items = m.prepared, items.count == prepared.count else { return }
            for i in sent.indices {
                let r = results.first { $0.i == i }
                items[i].decided = r?.decided ?? false
                items[i].how = (r?.decided ?? false) ? r?.how : nil
            }
            m.prepared = items
        }
    }

    // MARK: Questions and examples

    static var prepQuestion: String { String(localized: "panel.suggest.prepAsk", defaultValue: "다음 회의 준비해 줘") }
    static var openQuestion: String { String(localized: "panel.suggest.undecided", defaultValue: "이 회의에서 아직 안 정한 거 뭐야?") }
    static var summaryQuestion: String { String(localized: "panel.suggest.summary", defaultValue: "이 회의에서 정한 거 정리해 줘") }
    static var delayedQuestion: String { String(localized: "panel.suggest.delayed", defaultValue: "이번 달에 미뤄진 결정 뭐 있어?") }
    static var talkedQuestion: String { String(localized: "panel.suggest.talked", defaultValue: "요즘 회의에서 자주 나온 이야기 뭐야?") }
    static var myTodosQuestion: String { String(localized: "panel.suggest.myTodos", defaultValue: "내가 맡은 할 일 뭐 남았어?") }
    /// Most characters of a decision quoted in its example question and title.
    static let whyQuoteLimit = 18
    static func whyQuestion(_ decision: String) -> String {
        String(localized: "panel.suggest.why", defaultValue: "'\(Self.cut(decision, whyQuoteLimit))' 왜 이렇게 정했어?")
    }
    static func prepTitle(_ meeting: String) -> String {
        PanelThread.title(meeting, around: { String(localized: "panel.title.prep", defaultValue: "\($0) 다음 회의 준비") })
    }
    static func whyTitle(_ decision: String) -> String {
        String(localized: "panel.title.why", defaultValue: "'\(Self.cut(decision, whyQuoteLimit))' 정한 이유")
    }

    private static func cut(_ text: String, _ n: Int) -> String {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return t.count > n ? String(t.prefix(n)) + "…" : t
    }

    /// The empty screen's example questions for what is being viewed (docs/36 0-1절).
    func suggestions(for scope: PanelScope) -> [PanelSuggestion] {
        switch scope {
        case .all:
            return [.ask(Self.delayedQuestion), .ask(Self.talkedQuestion), .ask(Self.myTodosQuestion)]
        case .meeting(let id):
            let m = meetings(id)
            // a meeting still going on is not searchable yet: every meeting's examples, nothing about this one
            if let m, m.end == nil { return suggestions(for: .all) }
            var out: [PanelSuggestion] = [.ask(Self.openQuestion)]
            if m?.end != nil { out.append(.prep) }
            if let i = m?.ledger.firstIndex(where: { $0.kind == "decision" }) { out.append(.why(meetingID: id, ledgerIndex: i)) }
            if out.count < 3 { out.append(.ask(Self.summaryQuestion)) }
            return out
        }
    }

    /// What an example shows as its text.
    func text(of s: PanelSuggestion) -> String {
        switch s {
        case .ask(let t): return t
        case .prep: return Self.prepQuestion
        case .why(let id, let i):
            guard let m = meetings(id), m.ledger.indices.contains(i) else { return "" }
            return Self.whyQuestion(m.ledger[i].text)
        }
    }

    /// A question the person wrote.
    func ask(_ text: String) {
        send(text, kind: .ask)
    }

    /// An example question. A prep goes to the engine and into a conversation of its own, unless one for the meeting
    /// is already there and still current: that one is opened, nothing is sent.
    func suggest(_ s: PanelSuggestion) {
        switch s {
        case .ask(let t):
            send(t, kind: .ask, suggested: true)
        case .prep:
            // every meeting has no one meeting to prepare: no example offers it there
            guard case .meeting(let id) = scope else { return }
            if let t = preparing[id].flatMap({ p in history.first { $0.id == p } }) ?? reusablePrep(for: id) {
                reopen(t.id)
                return
            }
            current = nil
            let name = meetings(id)?.displayTitle(calendar: .current) ?? ""
            send(Self.prepQuestion, kind: .prep, title: Self.prepTitle(name))
            preparing[id] = current?.id
        case .why(let id, let i):
            guard let m = meetings(id), m.ledger.indices.contains(i) else { return }
            current = nil
            send(text(of: s), kind: .why(meetingID: id, ledgerIndex: i), title: Self.whyTitle(m.ledger[i].text), origin: .meeting(id))
        }
    }

    /// The newest answered prep conversation of the meeting, unless a meeting it read from, or one that goes on from
    /// it, ended after that answer (then the prep is out of date).
    private func reusablePrep(for id: String) -> PanelThread? {
        let found = history.first { t in
            guard t.origin == .meeting(id), let a = t.turns.first?.body?.prepAnswer else { return false }
            return a.errorKind == nil
        }
        guard let t = found, let at = t.turns.first?.at else { return nil }
        let chain = prepMeetingIDs(for: id).compactMap(meetings)
        let later = allMeetings().filter { $0.linkedFrom == id }
        return (chain + later).contains { ($0.end ?? .distantPast) > at } ? nil : t
    }

    /// `text` is what the person sees as their bubble. `title` and `origin` apply when this starts a conversation.
    /// `suggested` marks an example question of the empty screen (for the usage numbers only).
    func send(_ text: String, kind: PanelKind, title: String? = nil, origin: PanelScope? = nil, suggested: Bool = false) {
        var t = current ?? PanelThread(id: UUID(), title: title ?? PanelThread.title(from: text), origin: origin ?? scope, turns: [])
        let turn = PanelTurn(id: UUID(), question: text, at: clock(), steps: [], body: nil)
        let earlier = t.turns.compactMap { q in q.answerText.map { (q: q.question, a: $0) } }
        t.turns.append(turn)
        current = t
        upsert(t)
        let token = account()
        let threadID = t.id, turnID = turn.id
        let scope = t.origin
        let asked = clock()
        Analytics.capture(.panelAsked(kind: Self.askKind(kind, turn: t.turns.count, suggested: suggested), scope: PanelScopeKind(scope), turn: t.turns.count, place: .mac))
        inFlight[turnID] = Task { [weak self] in
            guard let self else { return }
            let body = await self.run(kind, text: text, scope: scope, history: earlier) { step in
                Task { @MainActor [weak self] in self?.addStep(step, thread: threadID, turn: turnID, account: token) }
            }
            self.inFlight[turnID] = nil
            if case .prep = kind, case .meeting(let id) = scope, self.preparing[id] == threadID { self.preparing[id] = nil }
            let u = body.usage
            Analytics.capture(.panelAnswered(ms: max(0, Int(self.clock().timeIntervalSince(asked) * 1000)), sources: u.sources,
                                             notFound: u.notFound, errorKind: u.error, place: .mac))
            guard self.account() == token else { return }  // another account's space is open now
            if let a = body.prepAnswer, a.errorKind == nil, case .meeting(let id) = scope {
                self.preps.append((id, self.clock(), Self.preparedItems(a, recent: id)))
            }
            self.finish(thread: threadID, turn: turnID, body: body)
        }
    }

    /// A prep's items as carried into the next meeting: what its first view shows (each group's lead, `PanelShown.ordered`),
    /// then the rest, in the order the expanded answer shows them.
    static func preparedItems(_ a: PrepAnswer, recent: String) -> [PreparedItem] {
        let groups = [a.open, a.todos].map { PanelShown.ordered($0, recent: recent) }
        let first = groups.flatMap { $0.items.prefix($0.first) }
        let rest = groups.flatMap { $0.items.dropFirst($0.first) }
        return (first + rest).map { PreparedItem(text: $0.text, kind: $0.kind) }
    }

    /// A question after the first one in a thread is a follow-up, whatever it asks the engine.
    private static func askKind(_ kind: PanelKind, turn: Int, suggested: Bool) -> PanelAskKind {
        if turn > 1, case .ask = kind { return .followUp }
        switch kind {
        case .prep: return .suggestionPrep
        case .why: return .suggestionWhy
        case .ask: return suggested ? .suggestionAsk : .typed
        }
    }

    /// Waits for the requests on their way (tests).
    func settle() async {
        for task in Array(inFlight.values) { await task.value }
    }

    private func run(_ kind: PanelKind, text: String, scope: PanelScope, history: [(q: String, a: String)],
                     onStep: @escaping @Sendable (PanelStep) -> Void) async -> PanelTurnBody {
        guard let engine = engine() else { return .failed(kind: "engine") }
        if let chat = engine.chat {
            do {
                return .chat(try await chat(chatRequest(kind, text: text, scope: scope, history: history), onStep))
            } catch {
                return .failed(kind: Self.kind(of: error))
            }
        }
        do {
            switch kind {
            case .prep:
                guard case .meeting(let id) = scope else { return .failed(kind: "none") }
                return .prep(try await engine.prep(prepMeetingIDs(for: id), onStep))
            case .why(let id, let index):
                guard let m = meetings(id), m.ledger.indices.contains(index) else { return .failed(kind: "none") }
                let entry = m.ledger[index]
                let a = try await engine.why(entry.text, Self.whyWindow(for: entry, in: m.transcript)) { m.lineName($0) ?? "" }
                return .why(a, meetingID: id)
            case .ask:
                var ids: [String]?
                // docs/36 2-2: the meeting and the meetings it goes on from
                if case .meeting(let id) = scope { ids = prepMeetingIDs(for: id) }
                return .ask(try await engine.ask(text, ids, history))
            }
        } catch {
            return .failed(kind: Self.kind(of: error))
        }
    }

    /// What POST /panel/chat reads for a question (docs/38 3-3절). A why example asks with the whole decision (its bubble
    /// shows it cut); on a meeting page the meeting and the ones it goes on from come with their transcripts and each
    /// ledger entry's line keys.
    func chatRequest(_ kind: PanelKind, text: String, scope: PanelScope, history: [(q: String, a: String)]) -> PanelChatRequest {
        var r = PanelChatRequest(text: text, history: history, today: Self.day(clock()))
        if case .why(let id, let i) = kind, let m = meetings(id), m.ledger.indices.contains(i) {
            r.text = String(localized: "panel.suggest.why", defaultValue: "'\(m.ledger[i].text)' 왜 이렇게 정했어?")
        }
        guard case .meeting(let id) = scope else { return r }
        r.meetingIDs = prepMeetingIDs(for: id)
        for mid in r.meetingIDs {
            guard let m = meetings(mid), !m.transcript.isEmpty else { continue }
            r.transcripts[mid] = m.transcript.map {
                // the speaker's key too (docs/38 12절): naming or merging speakers points at it, whatever name it shows now
                ["key": $0.key, "speaker": m.lineName($0) ?? "", "voice": $0.voice.map(m.resolved) ?? "",
                 "at": EngineClient.whyTime.string(from: $0.at), "text": $0.text]
            }
            r.refs[mid] = m.ledger.map { $0.refs ?? [] }
        }
        return r
    }

    /// "yyyy-MM-dd" in this Mac's time zone, POSIX digits.
    static func day(_ d: Date) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd"
        return f.string(from: d)
    }

    private static func kind(of error: Error) -> String {
        if let e = error as? EngineError {
            if case .notRunning = e { return "engine" }
            return e.kind ?? "down"
        }
        // the engine is on this Mac: a refused connection means it is not there, not that the internet is down
        if let u = error as? URLError { return u.code == .cannotConnectToHost ? "engine" : "network" }
        return "down"
    }

    // MARK: Why window

    /// The transcript lines the engine reads for a decision: `windowRadius` lines around each line the notes wrote it
    /// from (`refs`); without refs, around the line sharing the most words with the decision; without such a line, none.
    static func whyWindow(for entry: LedgerEntry, in transcript: [TranscriptLine]) -> [TranscriptLine] {
        let centers = MCPTranscript.centers(for: entry, in: transcript)
        var keep = Set<Int>()
        for c in centers {
            for i in max(0, c - windowRadius)...min(transcript.count - 1, c + windowRadius) { keep.insert(i) }
        }
        return keep.sorted().map { transcript[$0] }
    }

    // MARK: Updating threads

    private func addStep(_ step: PanelStep, thread: UUID, turn: UUID, account token: AccountSpace) {
        guard account() == token else { return }
        mutate(thread: thread, turn: turn) { $0.steps.append(step) }
    }

    // MARK: Proposals (docs/38 12절)

    typealias FixChoice = ProposalApplier.FixChoice

    /// The apply rules shared with the iPhone (fix, ledger, title). Names, merges, moves and asked rewrites stay the
    /// Mac's own: they go through remembered people and the notes kinds.
    private var applier: ProposalApplier {
        ProposalApplier(update: update, rewrite: rewrite, remember: remember,
                        canRewrite: { !$0.fromIPhone && !$0.transcript.isEmpty && $0.end != nil }, place: .mac)
    }

    private static func undoKey(_ turn: UUID, _ p: PanelProposal) -> String { "\(turn.uuidString)/\(p.id)" }

    /// Not while the notes are being written again: they would land over what comes back.
    func canUndo(_ p: PanelProposal, turn: UUID) -> Bool {
        let key = Self.undoKey(turn, p)
        if let n = namingUndos[key] { return meetings(p.meetingId)?.naming == n.after }
        return undos[key] != nil && meetings(p.meetingId)?.notes != .waiting
    }

    /// The speaker key a name_speaker points at: a speaker key, or the name a speaker or a line's person goes by now.
    func speakerKey(_ p: PanelProposal) -> String? {
        meetings(p.meetingId).flatMap { ProposalApplier.speakerKey(p, in: $0) }
    }

    static func speakerKeys(_ m: SavedMeeting) -> Set<String> { ProposalApplier.speakerKeys(m) }

    /// The speakers a merge_speakers takes away and the one they become, as keys this meeting still has; nil when the
    /// one to keep is gone. Speakers already merged into it are left out.
    func mergeKeys(_ p: PanelProposal) -> (from: [String], into: String)? {
        guard let m = meetings(p.meetingId), let into = p.into?.key else { return nil }
        let keys = Self.speakerKeys(m)
        guard keys.contains(into) else { return nil }
        let from = (p.from ?? []).map(\.key).filter { keys.contains($0) && $0 != into }
        return from.isEmpty ? nil : (from, into)
    }

    /// Merges the speakers into one, as the namer's "같은 사람" on the page does.
    func applyMerge(_ p: PanelProposal, turn: UUID) {
        guard let (from, into) = mergeKeys(p), let before = meetings(p.meetingId)?.naming else { return }
        for k in from { mergeSpeaker(p.meetingId, k, into) }
        if let after = meetings(p.meetingId)?.naming { namingUndos[Self.undoKey(turn, p)] = (before, after) }
        setState(.applied, of: p, turn: turn)
        Analytics.capture(.panelProposal(kind: .mergeSpeakers, action: .applied, places: from.count, lines: p.lines ?? 0,
                                         rewrite: false, remember: false, place: .mac))
    }

    /// The lines a move_lines moves that the meeting still has.
    func movedKeys(_ p: PanelProposal) -> [String] {
        guard let m = meetings(p.meetingId) else { return [] }
        return (p.moved ?? []).map(\.key).filter { k in m.transcript.contains { $0.key == k } }
    }

    /// Gives the lines to the speaker (or a new name), as 이 줄만 in the namer does.
    func applyMove(_ p: PanelProposal, turn: UUID) {
        let keys = movedKeys(p)
        guard !keys.isEmpty, let target = p.target, let m = meetings(p.meetingId) else { return }
        let to = target.key.flatMap { Self.speakerKeys(m).contains($0) ? $0 : nil }
        moveLines(p.meetingId, keys, to, target.name)
        if let after = meetings(p.meetingId)?.naming { namingUndos[Self.undoKey(turn, p)] = (m.naming, after) }
        setState(.applied, of: p, turn: turn)
        Analytics.capture(.panelProposal(kind: .moveLines, action: .applied, places: keys.count, lines: keys.count, rewrite: false, remember: false, place: .mac))
    }

    /// Gives each chosen line of an assign_lines to its speaker (이 줄만 per target), the lines still in the meeting only.
    func applyAssign(_ p: PanelProposal, turn: UUID, keys chosen: Set<String>) {
        guard let m = meetings(p.meetingId) else { return }
        let known = Self.speakerKeys(m)
        let rows = (p.assigned ?? []).filter { a in chosen.contains(a.key) && known.contains(a.to.key) && m.transcript.contains { $0.key == a.key } }
        guard !rows.isEmpty else { return }
        let before = m.naming
        for (to, group) in Dictionary(grouping: rows, by: { $0.to.key }) {
            moveLines(p.meetingId, group.map(\.key), to, group[0].to.name)
        }
        if let after = meetings(p.meetingId)?.naming { namingUndos[Self.undoKey(turn, p)] = (before, after) }
        setState(.applied, of: p, turn: turn)
        Analytics.capture(.panelProposal(kind: .assignLines, action: .applied, places: rows.count, lines: rows.count, rewrite: false, remember: false, place: .mac))
    }

    /// Whether the notes can be written again for this proposal: a meeting of this Mac that ended, with its transcript.
    func canWriteAgain(_ p: PanelProposal) -> Bool {
        guard let m = meetings(p.meetingId) else { return false }
        return !m.fromIPhone && !m.transcript.isEmpty && m.end != nil && m.notes != .waiting
    }

    /// 다시 쓰기 with the person's request, or in another kind. The notes from before come back with 되돌리기.
    func applyRewrite(_ p: PanelProposal, turn: UUID) {
        guard canWriteAgain(p), let m = meetings(p.meetingId) else { return }
        var kind: NotesKind?
        if p.kind == PanelProposal.changeKind {
            guard let k = p.notesKind.flatMap(NotesKind.init(rawValue:)), k != m.shownKind else { return }
            kind = k
        }
        undos[Self.undoKey(turn, p)] = ProposalUndo(notes: ProposalUndo.Notes(m), rewrote: true)
        if let kind { changeKind(p.meetingId, kind) } else { rewriteAsked(p.meetingId, p.request) }
        setState(.applied, of: p, turn: turn)
        Analytics.capture(.panelProposal(kind: AProposalKind(p.kind), action: .applied, places: 1, lines: 0, rewrite: true, remember: false, place: .mac))
    }

    /// The title the person picks: kept as theirs (`titleEdited`), as when renamed on the page.
    func applyTitle(_ p: PanelProposal, turn: UUID) {
        guard let u = applier.apply(p, choice: nil) else { return }
        undos[Self.undoKey(turn, p)] = u
        setState(.applied, of: p, turn: turn)
    }

    /// Gives the speaker the name, as the namer on the page does (the whole meeting shows it).
    func applyName(_ p: PanelProposal, turn: UUID) {
        guard let key = speakerKey(p), let name = p.name, let before = meetings(p.meetingId)?.naming else { return }
        nameSpeaker(p.meetingId, key, name)
        if let after = meetings(p.meetingId)?.naming { namingUndos[Self.undoKey(turn, p)] = (before, after) }
        setState(.applied, of: p, turn: turn)
        Analytics.capture(.panelProposal(kind: .nameSpeaker, action: .applied, places: 1, lines: p.lines ?? 0, rewrite: false, remember: false, place: .mac))
    }

    /// Whether the card may write the notes again: a meeting of this Mac with its transcript, once lines change.
    func canRewrite(_ p: PanelProposal) -> Bool {
        meetings(p.meetingId).map(applier.canRewrite) ?? false
    }

    /// Applies a fix card: the chosen places into the meeting, then the notes again from the fixed transcript and the
    /// word into the account's terms when chosen. Nothing changes for a meeting that is gone or still going on.
    func applyFix(_ p: PanelProposal, turn: UUID, choice: FixChoice) {
        guard let u = applier.apply(p, choice: choice) else { return }
        undos[Self.undoKey(turn, p)] = u
        setState(.applied, of: p, turn: turn)
    }

    func applyLedger(_ p: PanelProposal, turn: UUID) {
        guard let u = applier.apply(p, choice: nil) else { return }
        undos[Self.undoKey(turn, p)] = u
        setState(.applied, of: p, turn: turn)
    }

    /// 되돌리기: what the apply changed and is still as it left it comes back. The remembered word stays.
    func undo(_ p: PanelProposal, turn: UUID) {
        let key = Self.undoKey(turn, p)
        if let n = namingUndos[key] {
            update(p.meetingId) { m in if m.naming == n.after { m.restore(n.before) } }
            namingUndos[key] = nil
            setState(.undone, of: p, turn: turn)
            Analytics.capture(.panelProposal(kind: AProposalKind(p.kind), action: .undone, places: 0, lines: 0, rewrite: false, remember: false, place: .mac))
            return
        }
        guard let u = undos[key] else { return }
        applier.undo(p, u)
        undos[key] = nil
        setState(.undone, of: p, turn: turn)
    }

    func dismiss(_ p: PanelProposal, turn: UUID) {
        setState(.dismissed, of: p, turn: turn)
        Analytics.capture(.panelProposal(kind: AProposalKind(p.kind), action: .dismissed,
                                         places: 0, lines: 0, rewrite: false, remember: false, place: .mac))
    }

    /// Development build: presses the newest proposal card's button as the card would, with the card's defaults
    /// (every place, 다시 쓰기 when offered, 기억하기 on). `panel-apply`, `panel-undo`, `panel-dismiss`.
    func debugCard(_ command: String) {
        guard let t = current, let turn = t.turns.last(where: { !$0.proposals.isEmpty }), let p = turn.proposals.last else { return }
        switch command {
        case "panel-undo": undo(p, turn: turn.id)
        case "panel-dismiss": dismiss(p, turn: turn.id)
        default:
            switch p.kind {
            case PanelProposal.fixWords:
                let places = p.places ?? []
                applyFix(p, turn: turn.id, choice: .init(places: places, rewrite: places.contains { $0.at.inTranscript } && canRewrite(p), remember: true))
            case PanelProposal.editLedger: applyLedger(p, turn: turn.id)
            case PanelProposal.nameSpeaker: applyName(p, turn: turn.id)
            case PanelProposal.mergeSpeakers: applyMerge(p, turn: turn.id)
            case PanelProposal.moveLines: applyMove(p, turn: turn.id)
            case PanelProposal.assignLines: applyAssign(p, turn: turn.id, keys: Set((p.assigned ?? []).map(\.key)))
            case PanelProposal.renameTitle: applyTitle(p, turn: turn.id)
            default: applyRewrite(p, turn: turn.id)
            }
        }
    }

    private func setState(_ s: ProposalState, of p: PanelProposal, turn: UUID) {
        guard let thread = [current].compactMap({ $0 }).first(where: { t in t.turns.contains { $0.id == turn } })?.id
                ?? history.first(where: { t in t.turns.contains { $0.id == turn } })?.id else { return }
        mutate(thread: thread, turn: turn) { t in
            var states = t.proposalStates ?? [:]
            states[p.id] = s
            t.proposalStates = states
        }
        save()
    }

    private func finish(thread: UUID, turn: UUID, body: PanelTurnBody) {
        let now = clock()
        mutate(thread: thread, turn: turn) {
            $0.body = body
            $0.answeredAt = now
        }
        if case .chat(let a) = body {
            for p in a.proposals {
                let lines = p.places?.filter { $0.at.inTranscript }.count ?? p.lines ?? 0
                let places = p.places?.count ?? p.from?.count ?? (p.kind == PanelProposal.nameSpeaker ? 1 : (p.changes?.count ?? 0) + (p.add?.count ?? 0))
                Analytics.capture(.panelProposal(kind: AProposalKind(p.kind), action: .shown,
                                                 places: places, lines: lines, rewrite: false, remember: false, place: .mac))
            }
        }
        save()
    }

    /// The thread may be on screen, in the history, or both (as copies): the result goes to each.
    private func mutate(thread: UUID, turn: UUID, _ change: (inout PanelTurn) -> Void) {
        func apply(_ t: inout PanelThread) {
            guard let i = t.turns.firstIndex(where: { $0.id == turn }) else { return }
            change(&t.turns[i])
        }
        if current?.id == thread { apply(&current!) }
        if let h = history.firstIndex(where: { $0.id == thread }) { apply(&history[h]) }
    }

    private func upsert(_ t: PanelThread) {
        history.removeAll { $0.id == t.id }
        history.insert(t, at: 0)
        if history.count > Self.historyLimit { history.removeLast(history.count - Self.historyLimit) }
        save()
    }

    // MARK: Saved history

    private static func file(_ folder: URL) -> URL {
        folder.appendingPathComponent("panel", isDirectory: true).appendingPathComponent("history.json")
    }

    /// One thread of the saved list, or nil when it no longer reads (a field changed since): the others stay.
    private struct Lossy: Decodable {
        let thread: PanelThread?
        init(from decoder: Decoder) throws { thread = try? PanelThread(from: decoder) }
    }

    /// A file that does not read as a list at all is set aside (`history.broken.json`) before the next save replaces it.
    private static func load(from url: URL) -> [PanelThread] {
        guard let data = try? Data(contentsOf: url) else { return [] }
        let d = JSONDecoder()
        d.dateDecodingStrategy = .iso8601
        guard let list = try? d.decode([Lossy].self, from: data) else {
            let aside = url.deletingLastPathComponent().appendingPathComponent("history.broken.json")
            try? FileManager.default.removeItem(at: aside)
            try? FileManager.default.copyItem(at: url, to: aside)
            Log.info("panel history unreadable, set aside")
            return []
        }
        return Array(list.compactMap(\.thread).prefix(historyLimit))
    }

    /// Only answered turns are kept: a turn still running would read as stuck after a restart.
    private func save() {
        let kept: [PanelThread] = history.compactMap { t in
            var t = t
            t.turns.removeAll { $0.body == nil }
            return t.turns.isEmpty ? nil : t
        }
        let url = Self.file(folder())
        let e = JSONEncoder()
        e.dateEncodingStrategy = .iso8601
        do {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try e.encode(kept).write(to: url, options: .atomic)
        } catch {
            Log.info("panel history not saved: \(error.localizedDescription)")
        }
    }
}
