import Foundation

/// The WebSocket under `ShadowStt`: `URLSessionWebSocketTask` in the app, a fake in tests.
protocol ShadowSocket: AnyObject {
    func send(text: String) async throws
    func send(data: Data) async throws
    /// One JSON message from Soniox.
    func receive() async throws -> String
    func close()
}

/// The development build's shadow transcription (docs/25 "그림자 모드"): one Soniox real-time connection per meeting,
/// fed with the same samples as the Utterancer. At each VAD cut the meeting calls `finalize`, which sends
/// `{"type":"finalize"}` and returns the final tokens up to the next `<fin>`. Anything that goes wrong stops the shadow
/// for the rest of the meeting; it never touches the meeting's own transcription.
@MainActor
final class ShadowStt {
    private let socket: ShadowSocket
    private let config: String
    private let finTimeout: Duration
    /// Soniox closes a connection that hears nothing for a while: while no samples come (a paused meeting, a dropped
    /// mic), a keepalive goes out this often. Silence is not padded with zeros, which would move Soniox time ahead.
    private let keepaliveAfter: Duration
    private var lastSent: ContinuousClock.Instant?
    private var keeper: Task<Void, Never>?
    private(set) var stopped = false
    /// The sentence being said, for the meeting page: final text so far (trimmed) and the current partial tokens.
    var onLive: ((String, String) -> Void)?
    /// The stream position this connection started at: an utterance that began earlier only has its tail here.
    private(set) var startStreamMs = 0

    /// Audio sent so far, in ms of Soniox time.
    private var sentMs = 0
    /// Soniox time `baseSoniox` is stream position `baseStream` (moves when the Utterancer's clock starts over).
    private var baseSoniox = 0
    private var baseStream = 0
    /// Sends go out one after another, in the order they were asked for.
    private var chain: Task<Void, Never>?
    private var reader: Task<Void, Never>?
    /// Final tokens since the last `<fin>`.
    private var tokens: [SonioxToken] = []
    /// Final translated tokens since the last `<fin>` (translation on, docs/30): they arrive before it.
    private var translated: [SonioxToken] = []
    /// `done` nil: a cut whose words are only flushed (it began before this connection), nobody waits for them.
    private struct Pending { let id: Int; let startMs: Int; let endMs: Int; let sentAt: ContinuousClock.Instant; let done: CheckedContinuation<ShadowResult?, Never>? }
    private var pending: [Pending] = []
    private var nextId = 0
    private let clock = ContinuousClock()

    init(socket: ShadowSocket, config: String, finTimeout: Duration = .seconds(2), keepaliveAfter: Duration = .seconds(10)) {
        self.socket = socket
        self.config = config
        self.finTimeout = finTimeout
        self.keepaliveAfter = keepaliveAfter
    }

    /// Sends the config. `streamMs`: the stream position of the first sample that will be fed.
    func start(streamMs: Int) {
        baseSoniox = 0
        baseStream = streamMs
        startStreamMs = streamMs
        let config = config
        enqueue { [socket] in try await socket.send(text: config) }
        reader = Task { [weak self] in await self?.read() }
        lastSent = clock.now
        let every = keepaliveAfter
        keeper = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: every / 2)
                guard let self, !self.stopped else { return }
                self.keepAliveIfQuiet()
            }
        }
    }

    private func keepAliveIfQuiet() {
        guard let last = lastSent, clock.now - last >= keepaliveAfter else { return }
        lastSent = clock.now
        enqueue { [socket] in try await socket.send(text: #"{"type":"keepalive"}"#) }
    }

    func feed(_ samples: [Float]) {
        guard !stopped, !samples.isEmpty else { return }
        sentMs += samples.count / 16
        lastSent = clock.now
        let data = SonioxTokens.pcm16(samples)
        enqueue { [socket] in try await socket.send(data: data) }
    }

    /// The Utterancer's clock started over at 0 (`resetClock`): audio fed from now on is stream 0 onwards.
    func clockReset() {
        baseSoniox = sentMs
        baseStream = 0
        startStreamMs = 0  // the new clock's stretch is all on this connection
    }

    func finalize(startMs: Int, endMs: Int) async -> ShadowResult? {
        guard !stopped else { return nil }
        // began before this connection: only its tail is here. Finalize anyway so those words leave the connection
        // (they would come back in front of the next utterance), and give nothing: the file path transcribes it.
        guard startMs >= startStreamMs else {
            pending.append(Pending(id: nextId, startMs: startMs, endMs: endMs, sentAt: clock.now, done: nil))
            nextId += 1
            enqueue { [socket] in try await socket.send(text: #"{"type":"finalize"}"#) }
            return nil
        }
        let id = nextId
        nextId += 1
        let result: ShadowResult? = await withCheckedContinuation { done in
            pending.append(Pending(id: id, startMs: startMs, endMs: endMs, sentAt: clock.now, done: done))
            enqueue { [socket] in try await socket.send(text: #"{"type":"finalize"}"#) }
            let limit = finTimeout
            Task { [weak self] in
                try? await Task.sleep(for: limit)
                self?.timedOut(id)
            }
        }
        return result
    }

    func stop() {
        guard !stopped else { return }
        stopped = true
        for p in pending { p.done?.resume(returning: nil) }
        pending = []
        let previous = chain
        let socket = socket
        chain = Task {
            await previous?.value
            try? await socket.send(text: "")
            socket.close()
        }
        reader?.cancel()
        keeper?.cancel()
    }

    /// Replaced by a new connection: no more audio comes, but a finalize sent just before may still get its `<fin>`.
    /// The connection stays open for `after` (the Mac's `closeLater`), then stops.
    func stop(after: Duration) {
        // held until then: nothing else keeps a replaced connection alive
        Task {
            try? await Task.sleep(for: after)
            self.stop()
        }
    }

    // MARK: - inside

    private func enqueue(_ work: @escaping @Sendable () async throws -> Void) {
        let previous = chain
        chain = Task { [weak self] in
            await previous?.value
            do { try await work() } catch { await self?.fail("send: \(error.localizedDescription)") }
        }
    }

    private func read() async {
        while !stopped {
            let message: String
            do { message = try await socket.receive() } catch {
                fail("receive: \(error.localizedDescription)")
                return
            }
            handle(message)
        }
    }

    private func handle(_ message: String) {
        guard let data = message.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        if let code = obj["error_code"] {
            fail("soniox error \(code)")
            return
        }
        guard let raw = obj["tokens"], let json = try? JSONSerialization.data(withJSONObject: raw),
              let list = try? JSONDecoder().decode([SonioxToken].self, from: json) else { return }
        var changed = false
        for t in list where t.isFinal {
            if t.text == "<fin>" { closeOldest() } else if t.isTranslation { translated.append(t) } else { tokens.append(t) }
            changed = true
        }
        let partial = list.filter { !$0.isFinal && !$0.isTranslation }.map(\.text).joined()
        // after <fin> the sentence is the finalize result's: the live text starts again from nothing
        if changed || !partial.isEmpty { onLive?(SonioxTokens.text(tokens), partial) }
    }

    private func closeOldest() {
        let heard = tokens
        let translatedNow = translated
        tokens = []
        translated = []
        guard !pending.isEmpty else { return }
        let p = pending.removeFirst()
        let ms = Int(((clock.now - p.sentAt) / .milliseconds(1)).rounded())
        let translation = SonioxTokens.text(translatedNow)
        p.done?.resume(returning: ShadowResult(startMs: p.startMs, endMs: p.endMs, text: SonioxTokens.text(heard),
                                              words: SonioxTokens.words(heard, offsetMs: baseStream - baseSoniox), ms: ms,
                                              translation: translation.isEmpty ? nil : translation,
                                              translatedFrom: translation.isEmpty ? nil : translatedNow.compactMap(\.sourceLanguage).first))
    }

    private func timedOut(_ id: Int) {
        guard pending.contains(where: { $0.id == id }) else { return }
        fail("no <fin> in time")
    }

    private func fail(_ why: String) {
        guard !stopped else { return }
        Log.info("shadow: off (\(why))")
        stop()
    }
}

/// `ShadowSocket` over `URLSessionWebSocketTask`, authorized with a Soniox temporary key.
final class URLSessionShadowSocket: ShadowSocket, @unchecked Sendable {
    private let task: URLSessionWebSocketTask

    init(url: URL, key: String) {
        var req = URLRequest(url: url)
        req.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        task = URLSession.shared.webSocketTask(with: req)
        task.resume()
    }

    func send(text: String) async throws { try await task.send(.string(text)) }
    func send(data: Data) async throws { try await task.send(.data(data)) }
    func receive() async throws -> String {
        switch try await task.receive() {
        case .string(let s): return s
        case .data(let d): return String(decoding: d, as: UTF8.self)
        @unknown default: return ""
        }
    }
    func close() { task.cancel(with: .normalClosure, reason: nil) }
}
