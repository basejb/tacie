import Foundation

/// What the bridge needs from the app: the account and engine it answers for, and the meeting files.
/// The app's own (`LiveMCPHost`) wraps `EngineProcess`, `MeetingModel` and the meeting folder; tests give a fake.
protocol MCPHost: AnyObject {
    /// The account space in use (nil: signed out).
    @MainActor var accountID: String? { get }
    /// Changes on every engine start (`EngineProcess.generation`).
    @MainActor var engineGeneration: Int { get }
    @MainActor var engineReady: Bool { get }
    /// The search model is on this Mac.
    @MainActor var modelReady: Bool { get }
    @MainActor var meetingRunning: Bool { get }
    /// POST to the engine's `/tools/*`; the reply's JSON as it is.
    @MainActor func engineTool(_ path: String, _ body: [String: Any]) async throws -> Data
    /// One meeting file of the account in use (never the whole folder).
    @MainActor func meeting(id: String) -> SavedMeeting?
    /// The speaker name each line shows on the meeting page.
    @MainActor func lineName(_ m: SavedMeeting) -> (TranscriptLine) -> String
}

/// Answers `tacie-mcp` on the local socket (docs/38 2-2절, 3절). Off, it holds nothing: no socket, no task.
/// Each request is pinned to the account and the engine it started with; a result that comes back after either
/// changed is dropped and answered `switching`, so another account's meetings never go out.
@MainActor
final class MCPBridge {
    static let maxInFlight = 2
    static let perMinute = 60
    /// `mcp_tool_called` events sent per rolling minute; the rest are dropped.
    static let eventsPerMinute = 10

    /// Seconds per request (the engine call has the same limit); the first of the two to end answers.
    var timeout: TimeInterval = 30

    private let host: MCPHost
    private let folder: URL
    private let now: () -> Date
    private let allowed: () -> Bool
    /// One listener for the whole app: its connection cap is per instance.
    private var socket: MCPSocket?
    private var listening = false
    /// Set by `suspend` while the account's space changes; only `resume` opens again.
    private var held = false
    private var inFlight: [UUID: Pending] = [:]
    private var recent: [Date] = []
    private var sentEvents: [Date] = []
    private let capture: (AnalyticsEvent) -> Void

    init(host: MCPHost, folder: URL = AccountSpace.base.appendingPathComponent("mcp"), now: @escaping () -> Date = Date.init,
         allowed: @escaping () -> Bool = { MCPAccess.open() }, capture: ((AnalyticsEvent) -> Void)? = nil) {
        self.capture = capture ?? { Analytics.capture($0) }
        self.host = host
        self.folder = folder
        self.now = now
        self.allowed = allowed
    }

    // MARK: - opening and closing

    /// Opens the socket when the person's switch, the server and the build all allow it; closes it otherwise.
    func refresh() {
        guard !held else { return }
        if allowed() { open() } else { close() }
    }

    /// The account's space changed (after `suspend` and the new engine's start): reads the new account's switch.
    func resume() {
        held = false
        refresh()
    }

    /// Before the engine stops for an account switch: no new requests, the ones in flight answer `switching`
    /// (waiting two seconds at most for their replies to get out), then the socket closes. Stays closed until `resume`.
    func suspend() async {
        held = true
        listening = false
        closing += 1
        for p in inFlight.values { p.cancel(.switching) }
        await drain()
        closeSocket()
    }

    /// Waits until no request is in flight and every reply handed to the socket is written, two seconds at most.
    private func drain() async {
        let deadline = Date().addingTimeInterval(Self.drainSeconds)
        while !inFlight.isEmpty || (socket?.owedReplies ?? 0) > 0, Date() < deadline {
            try? await Task.sleep(for: .milliseconds(20))
        }
    }

    static let drainSeconds: TimeInterval = 2
    /// Bumped by each close and open, so a close that waited for replies does not shut a socket opened since.
    private var closing = 0

    /// The app is quitting: everything closes now and `mcp.sock` goes.
    func stop() {
        held = true
        listening = false
        closing += 1
        for p in inFlight.values { p.cancel(.switching) }
        closeSocket()
    }

    private func open() {
        if listening { return }
        closing += 1  // a close still waiting for replies leaves this socket alone
        // MCPSocket makes only the last folder (`mcp`); its parent is the app's support folder.
        try? FileManager.default.createDirectory(at: folder.deletingLastPathComponent(), withIntermediateDirectories: true)
        let s = socket ?? MCPSocket(folder: folder)
        socket = s
        do {
            try s.open { [weak self] line, reply in
                Task { @MainActor in
                    guard let self else { return reply(MCPWire.fail(.off)) }
                    reply(await self.serve(line))
                }
            }
            listening = true
        } catch {
            Log.info("mcp socket not opened: \(error)")
        }
    }

    /// Turned off: new requests answer `off` at once, the ones in flight answer `off`, and the socket closes once their
    /// replies are written (two seconds at most), so the AI app reads "turned off" instead of a dropped connection.
    private func close() {
        listening = false
        closing += 1
        for p in inFlight.values { p.cancel(.off) }
        guard !inFlight.isEmpty || (socket?.owedReplies ?? 0) > 0 else { return closeSocket() }
        let id = closing
        Task { @MainActor [weak self] in
            await self?.drain()
            guard let self, self.closing == id, !self.listening else { return }
            self.closeSocket()
        }
    }

    private func closeSocket() {
        socket?.close()
    }

    // MARK: - requests

    /// One request line from the socket to its reply line.
    private func serve(_ line: Data) async -> Data {
        guard listening else { return MCPWire.fail(held ? .switching : .off) }
        switch MCPWire.decode(line) {
        case .failure(let e): return MCPWire.fail(e)
        case .success(let req): return await handle(req)
        }
    }

    /// Answers one request and counts it for the usage numbers (numbers and closed values only, docs/38 7절).
    func handle(_ req: MCPRequest) async -> Data {
        let started = now()
        let data = await process(req)
        report(req, reply: data, ms: Int((now().timeIntervalSince(started) * 1000).rounded()))
        return data
    }

    private func report(_ req: MCPRequest, reply: Data, ms: Int) {
        let t = now()
        sentEvents.removeAll { t.timeIntervalSince($0) >= 60 }
        guard sentEvents.count < Self.eventsPerMinute else { return }
        sentEvents.append(t)
        let obj = (try? JSONSerialization.jsonObject(with: reply) as? [String: Any]) ?? [:]
        var failure = AMCPFailure.none, results = 0
        if obj["ok"] as? Bool == true {
            if let r = obj["result"] as? [String: Any] {
                results = ["cards", "meetings", "entries", "lines"].compactMap { (r[$0] as? [Any])?.count }.first ?? 0
            }
        } else {
            failure = (obj["error"] as? String).flatMap(AMCPFailure.init(rawValue:)) ?? .down
        }
        capture(.mcpToolCalled(tool: AMCPTool(rawValue: req.tool.rawValue) ?? .search, ms: max(0, ms), results: results,
                               failure: failure, client: AMCPClient(rawValue: req.client.rawValue) ?? .other))
    }

    private func process(_ req: MCPRequest) async -> Data {
        guard host.engineReady else { return MCPWire.fail(.notReady) }
        if req.tool == .search, host.meetingRunning { return MCPWire.fail(.busy) }
        let t = now()
        recent.removeAll { t.timeIntervalSince($0) >= 60 }
        guard inFlight.count < Self.maxInFlight, recent.count < Self.perMinute else { return MCPWire.fail(.busy) }
        recent.append(t)

        let account = host.accountID, generation = host.engineGeneration
        let id = UUID()
        let result: Outcome = await withCheckedContinuation { c in
            let p = Pending(c)
            inFlight[id] = p
            p.work = Task { @MainActor [weak self] in
                guard let self else { return p.finish(.failure(.switching)) }
                p.finish(await self.run(req))
            }
            let limit = timeout
            p.timer = Task { @MainActor in
                try? await Task.sleep(for: .seconds(limit))
                guard !Task.isCancelled else { return }
                p.finish(.failure(.busy))
                p.work?.cancel()
            }
        }
        inFlight[id] = nil
        guard host.accountID == account, host.engineGeneration == generation else { return MCPWire.fail(.switching) }
        switch result {
        case .success(let value): return MCPWire.ok(value.object)
        case .failure(let e): return MCPWire.fail(e)
        }
    }

    private func run(_ req: MCPRequest) async -> Outcome {
        let a = req.args
        do {
            switch req.tool {
            case .search:
                guard let q = a["query"] as? String, !q.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                    return .failure(.badRequest)
                }
                var body: [String: Any] = ["text": q]
                if let ids = a["meeting_ids"] {
                    guard let list = ids as? [String] else { return .failure(.badRequest) }
                    body["meeting_ids"] = list
                }
                guard host.modelReady else { return .failure(.model) }
                // a meeting may have started while this one waited its turn: never slow its own lookups
                guard !host.meetingRunning else { return .failure(.busy) }
                var out = try Self.object(await host.engineTool("/tools/search", body))
                // the reason a search went unranked, in the wire's words (credit is the monthly limit)
                if let reason = out["reason"] as? String { out["reason"] = Self.code(reason).rawValue }
                return .success(Box(out))
            case .listMeetings:
                let body = a.filter { ["from", "to", "text", "ids", "limit"].contains($0.key) }
                return .success(Box(try Self.object(await host.engineTool("/tools/meetings", body))))
            case .listLedger:
                let body = a.filter { ["kind", "owner", "done", "from", "to", "meeting_ids", "limit"].contains($0.key) }
                return .success(Box(try Self.object(await host.engineTool("/tools/ledger", body))))
            case .readTranscript:
                guard let id = a["meeting_id"] as? String, let m = host.meeting(id: id) else { return .failure(.badRequest) }
                let out = MCPTranscript.read(m, around: a["around"] as? String, before: a["before"] as? Int,
                                             after: a["after"] as? Int, offset: a["offset"] as? Int,
                                             limit: a["limit"] as? Int, name: host.lineName(m))
                return .success(Box(out))
            }
        } catch {
            return .failure(Self.code(for: error))
        }
    }

    // MARK: - errors

    /// An engine `error_kind` (ServiceError) in the wire's words: credit is the monthly limit.
    static func code(_ kind: String) -> MCPError {
        switch kind {
        case "credit": return .limit
        case "key": return .key
        case "network": return .network
        case "busy": return .busy
        case "model": return .model
        default: return .down
        }
    }

    static func code(for error: Error) -> MCPError {
        if error is CancellationError { return .switching }
        if let e = error as? MCPError { return e }
        if let e = error as? EngineError {
            switch e {
            case .notRunning: return .notReady
            case .http(let status, _):
                if let kind = e.kind { return code(kind) }
                return status == 400 ? .badRequest : .down
            }
        }
        if let e = error as? URLError {
            switch e.code {
            case .cancelled: return .switching
            case .timedOut: return .busy
            default: return .notReady  // the engine went away (restarting or crashed)
            }
        }
        return .down
    }

    private static func object(_ data: Data) throws -> [String: Any] {
        guard let o = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw MCPError.down }
        return o
    }
}

/// A JSON result handed between tasks on the main actor.
private struct Box: @unchecked Sendable {
    let object: Any
    init(_ object: Any) { self.object = object }
}

private typealias Outcome = Result<Box, MCPError>

/// One request in flight: whichever ends first — the work, the timer or a cancel — answers, once.
@MainActor
private final class Pending {
    private var continuation: CheckedContinuation<Outcome, Never>?
    var work: Task<Void, Never>?
    var timer: Task<Void, Never>?

    init(_ c: CheckedContinuation<Outcome, Never>) { continuation = c }

    func finish(_ r: Outcome) {
        guard let c = continuation else { return }
        continuation = nil
        timer?.cancel()
        c.resume(returning: r)
    }

    func cancel(_ code: MCPError) {
        finish(.failure(code))
        work?.cancel()
    }
}
