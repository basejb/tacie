import AppKit
import Foundation
import Observation
import Security

/// What Tacie keeps of a Notion connection (keychain `ai.tacie.notion`, as JSON). Never logged.
struct NotionConnection: Codable, Equatable {
    var token: String
    var refreshToken: String?
    var workspace: String?
}

/// `signedOut`: the relay wants a Tacie session (sign in to Tacie first); the Notion connection itself is fine.
enum NotionConnectError: Error, Equatable { case denied, expired, failed, network, refused, signedOut }

/// Notion's OAuth through Tacie's small server (docs/13 2절): Notion wants a client secret to hand out and renew a
/// token, so the server does those two steps. The app opens the approval page in its sign-in window with a random
/// `state`; the server sends it back to `tacie://notion` with a one-time key, and the token answers only the app's
/// `POST /notion/token {state, k}` with its Tacie session (issue #38). The server keeps no token.
struct NotionConnect {
    static let server = AccountServer.base
    /// How long the approval page may take (the server keeps the code five minutes too).
    static let patience: TimeInterval = 300

    /// (body, HTTP status). Throws only when there was no answer.
    var send: (URLRequest) async throws -> (Data, Int) = { req in
        let (data, resp) = try await URLSession.shared.data(for: req)
        return (data, (resp as? HTTPURLResponse)?.statusCode ?? 0)
    }
    /// The Tacie session the server asks for. Read on each call: the account can change while the app runs.
    var session: () -> String? = { SessionStore.store.read() }

    private var server: AccountServer { AccountServer(send: send, session: session) }

    /// 43 random URL-safe characters (32 bytes).
    static func newState() -> String {
        var bytes = [UInt8](repeating: 0, count: 32)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        return Data(bytes).base64EncodedString()
            .replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    /// A state with the server's mark for "picks up with a key" (`KEYED` in the server's handoff.js).
    static func newSignInState() -> String { "k2_" + newState() }

    func startURL(state: String) -> URL {
        var c = URLComponents(url: Self.server.appendingPathComponent("notion/start"), resolvingAgainstBaseURL: false)!
        c.queryItems = [URLQueryItem(name: "state", value: state)]
        return c.url!
    }

    /// The one-time key from `tacie://notion?state=…&k=…`. A return that is not for the sign-in this app started
    /// throws `CancellationError`: it is ignored.
    func parseReturn(_ url: URL, state: String) throws -> String {
        let value = try WebSignIn.returned(url, host: "notion", state: state)
        if let error = value("error") { throw error == "denied" ? NotionConnectError.denied : .failed }
        guard let k = value("k"), !k.isEmpty else { throw NotionConnectError.failed }
        return k
    }

    /// The connection: the server trades the parked code with its secret and answers only here.
    func token(state: String, k: String) async throws -> NotionConnection {
        let (data, status) = try await relay("notion/token", ["state": state, "k": k])
        if status == 401 { throw NotionConnectError.signedOut }
        if status == 400 { throw NotionConnectError.expired }
        return try Self.connection(data, status: status)
    }

    /// A new token for a refused one. `.refused`: Notion no longer takes the refresh token, so connect again.
    /// `.signedOut` (the server's 403): no Tacie session, which says nothing about the Notion connection.
    func refresh(_ refreshToken: String) async throws -> NotionConnection {
        let (data, status) = try await relay("notion/refresh", ["refresh_token": refreshToken])
        if status == 401 { throw NotionConnectError.refused }
        if status == 403 { throw NotionConnectError.signedOut }
        return try Self.connection(data, status: status)
    }

    private func relay(_ path: String, _ body: [String: String]) async throws -> (Data, Int) {
        if Task.isCancelled { throw CancellationError() }
        do { return try await server.authorized(path, body: body) } catch AccountError.signedOut {
            throw NotionConnectError.signedOut
        } catch {
            if Task.isCancelled { throw CancellationError() }
            throw NotionConnectError.network
        }
    }

    static func connection(_ data: Data, status: Int) throws -> NotionConnection {
        guard let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw NotionConnectError.failed }
        if let error = obj["error"] as? String { throw error == "denied" ? NotionConnectError.denied : NotionConnectError.failed }
        guard (200..<300).contains(status), let token = obj["token"] as? String, !token.isEmpty else {
            throw NotionConnectError.failed
        }
        return NotionConnection(token: token, refreshToken: obj["refresh_token"] as? String,
                                workspace: obj["workspace"] as? String)
    }
}

/// The keychain item of the Notion connection.
enum NotionStore {
    /// The account in use (AccountSpace): `connection` signed out, `connection@<id>` signed in.
    static var store: KeychainTokenStore {
        KeychainTokenStore(service: "ai.tacie.notion", account: AccountSpace.inUse.key("connection"), label: String(localized: "Tacie 노션 연결"))
    }

    static func load() -> NotionConnection? {
        guard let text = store.read(), let data = text.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(NotionConnection.self, from: data)
    }

    static func save(_ c: NotionConnection) throws {
        try store.save(String(decoding: try JSONEncoder().encode(c), as: UTF8.self))
    }

    static func delete() throws { try store.delete() }
}

/// The Notion connection as the cards show it (docs/13 1절): connecting in the browser, connected to a workspace, or
/// why not. IndexKeeper hands the connection to the engine (`push`) and asks for a new token when Notion refuses one.
@Observable
@MainActor
final class NotionLink {
    enum Phase: Equatable {
        case idle
        case waiting  // the approval page is open in the browser
        case failed(String)  // what the card says, with 다시 연결
    }

    private(set) var phase: Phase = .idle
    private(set) var saved: NotionConnection?
    /// Notion refused the token and a new one could not be had: connect again. Found pages stay searchable until then.
    private(set) var lost = false
    var connected: Bool { saved != nil }
    var workspace: String? { saved?.workspace }

    var api = NotionConnect()
    /// Set by IndexKeeper: hands a connection (nil: none) to the engine.
    var push: (NotionConnection?) async -> Void = { _ in }
    /// Shows the approval page and answers the `tacie://` address it ends at (tests replace it).
    var authenticate: (URL) async throws -> URL = { url in try await WebSignIn().run(url) }
    /// Keeps the connection in the keychain (nil: deletes it). Tests keep it nowhere.
    var keep: (NotionConnection?) -> Void = { c in
        if let c { try? NotionStore.save(c) } else { try? NotionStore.delete() }
    }
    private var waiting: Task<Void, Never>?
    var waitingForTesting: Task<Void, Never>? { waiting }
    private var renewing: Task<Void, Never>?
    /// The account is changing (`settle` → `reload`): no connection or renewal starts.
    private var held = false
    /// Bumped by `reload`: a connection or renewal that finishes for the last account is dropped.
    private var generation = 0
    private var renewedAt: Date?

    init(load: Bool = true) {
        if load { saved = NotionStore.load() }
    }

    /// A connection without the keychain (tests).
    func setForTesting(_ c: NotionConnection?) { saved = c }

    /// Opens Notion's approval page (also to pick pages again) and waits for its result.
    func connect() {
        guard !held else { return }
        waiting?.cancel()
        let started = generation
        let state = NotionConnect.newSignInState()
        let start = api.startURL(state: state)
        let authenticate = authenticate
        phase = .waiting
        waiting = Task {
            do {
                let back = try await WebSignIn.within(NotionConnect.patience, expired: NotionConnectError.expired) {
                    try await authenticate(start)
                }
                let k = try api.parseReturn(back, state: state)
                var c = try await api.token(state: state, k: k)
                guard started == generation else { return }
                if c.workspace == nil { c.workspace = saved?.workspace }
                keep(c)
                saved = c
                lost = false
                phase = .idle
                Log.info("notion connected")
                Analytics.capture(.notionConnected)
                await push(c)
            } catch is CancellationError {
                // closed by the user, or a return that was not ours: the card goes back quietly
                if !Task.isCancelled { phase = .idle }
            } catch {
                let e = error as? NotionConnectError ?? .failed
                phase = .failed(Self.words(e))
                Log.info("notion connect failed: \(e)")
            }
        }
    }

    func cancel() {
        waiting?.cancel()
        waiting = nil
        phase = .idle
    }

    /// 오픈소스판: an internal integration token the person pasted (checked already, `PastedTokens`). It has no refresh
    /// token, so when Notion refuses it later `renew` marks the connection lost and the card asks for a new one.
    func connectPasted(token: String, workspace: String?) async {
        guard !held else { return }
        waiting?.cancel()
        waiting = nil
        let c = NotionConnection(token: token, refreshToken: nil, workspace: workspace)
        keep(c)
        saved = c
        lost = false
        renewedAt = nil
        phase = .idle
        Log.info("notion connected with a pasted token")
        Analytics.capture(.notionConnected)
        await push(c)
    }

    /// Forgets the connection here and in the engine; the engine drops the found pages.
    func disconnect() async {
        waiting?.cancel()
        keep(nil)
        saved = nil
        lost = false
        phase = .idle
        Log.info("notion disconnected")
        await push(nil)
    }

    /// Before the account changes (AccountSpace): a connection being made stops, a renewal in flight ends first, so
    /// a new token always lands in the keychain item of the account it belongs to.
    /// From here until `reload` nothing new starts (a renewal asked for meanwhile is dropped).
    func settle() async {
        held = true
        waiting?.cancel()
        await waiting?.value
        waiting = nil
        await renewing?.value
    }

    /// After the account changed: the new account's connection, or none.
    func reload() {
        held = false
        generation += 1
        saved = NotionStore.load()
        lost = false
        renewedAt = nil
        phase = .idle
    }

    /// The engine says Notion refused the token: one try for a new one, at most every ten minutes.
    func renew() async {
        guard !held else { return }
        if let running = renewing { return await running.value }
        let task = Task { await self.renewNow() }
        renewing = task
        await task.value
        renewing = nil
    }

    private func renewNow() async {
        guard let s = saved, !lost else { return }
        let started = generation
        if let at = renewedAt, Date().timeIntervalSince(at) < 600 { return }
        renewedAt = Date()
        guard let r = s.refreshToken else {
            lost = true
            return
        }
        do {
            var c = try await api.refresh(r)
            guard started == generation else {
                Log.info("notion: a renewal finished after the account changed, dropped")
                return
            }
            if c.workspace == nil { c.workspace = s.workspace }
            if c.refreshToken == nil { c.refreshToken = r }
            keep(c)
            saved = c
            Log.info("notion token renewed")
            await push(c)
        } catch NotionConnectError.refused {
            guard started == generation else { return }
            lost = true
            Log.info("notion refresh refused")
        } catch {
            Log.info("notion refresh failed: \(error as? NotionConnectError ?? .failed)")
        }
    }

    static func words(_ e: NotionConnectError) -> String {
        switch e {
        case .denied: String(localized: "연결하지 않았어요. 회사 워크스페이스라면 관리자가 연결을 허용해야 할 수 있어요.")
        case .expired: String(localized: "시간이 지났어요. 다시 연결해 주세요.")
        case .network: String(localized: "인터넷 연결을 확인해 주세요.")
        case .signedOut: SettingsText.atlassianSignedOut
        case .failed, .refused: String(localized: "노션에 연결하지 못했어요.")
        }
    }
}
