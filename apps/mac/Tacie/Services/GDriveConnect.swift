import AppKit
import CryptoKit
import Foundation
import Network
import Observation
import Security

/// What Tacie keeps of a Google Drive connection (keychain `ai.tacie.gdrive`, as JSON). Never logged.
struct GDriveConnection: Codable, Equatable {
    var token: String
    var refreshToken: String
    var expiresAt: Date
    var account: String?  // the Google account's email
    var accountID: String?  // Drive's permissionId: tells a different account apart
    /// Shared drives the user turned off (docs/19 14절). Only these are remembered, so new drives come in on their own.
    var muted: [String] = []
}

enum GDriveConnectError: Error, Equatable { case denied, expired, failed, network, refused, scope, unready }

/// The Desktop OAuth client Tacie signs in with (docs/19 7-0절). Google does not treat a Desktop client's secret as
/// confidential and requires it anyway (7-1절 1), so it ships inside the app. It is not in git: the official build
/// puts its own client file into the bundle (`scripts/gdrive-client.sh`); the open edition has none. Without it the card says
/// the connection is not ready yet.
struct GDriveClient: Equatable {
    let id: String
    let secret: String

    static var bundled: GDriveClient? {
        guard let url = Bundle.main.url(forResource: "gdrive-client", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: String],
              let id = obj["client_id"], let secret = obj["client_secret"], !id.isEmpty, !secret.isEmpty else { return nil }
        return GDriveClient(id: id, secret: secret)
    }
}

/// Google's sign-in for an installed app (docs/19 9절): Tacie listens on 127.0.0.1 itself, so no server is in the
/// middle. The browser comes back to `http://127.0.0.1:<port>` with the code, and the app trades it with PKCE and the
/// Desktop client's secret. Refreshing goes to Google directly too.
struct GDriveConnect {
    static let authorize = URL(string: "https://accounts.google.com/o/oauth2/v2/auth")!
    static let tokenURL = URL(string: "https://oauth2.googleapis.com/token")!
    static let revokeURL = URL(string: "https://oauth2.googleapis.com/revoke")!
    static let aboutURL = URL(string: "https://www.googleapis.com/drive/v3/about?fields=user(emailAddress,permissionId)")!
    static let scope = "https://www.googleapis.com/auth/drive.readonly"
    /// How long the approval page may take.
    static let patience: TimeInterval = 300

    var client: GDriveClient? = GDriveClient.bundled
    /// (body, HTTP status). Throws only when there was no answer.
    var send: (URLRequest) async throws -> (Data, Int) = { req in
        let (data, resp) = try await URLSession.shared.data(for: req)
        return (data, (resp as? HTTPURLResponse)?.statusCode ?? 0)
    }
    var now: () -> Date = Date.init

    func authorizeURL(redirect: String, state: String, challenge: String) -> URL? {
        guard let client else { return nil }
        var c = URLComponents(url: Self.authorize, resolvingAgainstBaseURL: false)!
        c.queryItems = [
            URLQueryItem(name: "client_id", value: client.id),
            URLQueryItem(name: "redirect_uri", value: redirect),
            URLQueryItem(name: "response_type", value: "code"),
            URLQueryItem(name: "scope", value: Self.scope),
            URLQueryItem(name: "state", value: state),
            URLQueryItem(name: "code_challenge", value: challenge),
            URLQueryItem(name: "code_challenge_method", value: "S256"),
            // a refresh token comes only with offline access, and only on a consent screen
            URLQueryItem(name: "access_type", value: "offline"),
            URLQueryItem(name: "prompt", value: "consent"),
        ]
        return c.url
    }

    /// The first token pair. Drive read access must be in the granted scopes (the user can untick it).
    func exchange(code: String, verifier: String, redirect: String) async throws -> GDriveConnection {
        guard let client else { throw GDriveConnectError.unready }
        let (data, status) = try await answer(form(Self.tokenURL, [
            "grant_type": "authorization_code", "code": code, "redirect_uri": redirect, "client_id": client.id,
            "client_secret": client.secret, "code_verifier": verifier,
        ]))
        let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        if let scope = obj?["scope"] as? String, !scope.split(separator: " ").contains(Substring(Self.scope)) {
            throw GDriveConnectError.scope
        }
        return try pair(obj, status: status, keeping: nil)
    }

    /// A fresh access token for the saved connection. Only `invalid_grant` means the connection is gone (`.refused`).
    func refresh(_ c: GDriveConnection) async throws -> GDriveConnection {
        guard let client else { throw GDriveConnectError.unready }
        let (data, status) = try await answer(form(Self.tokenURL, [
            "grant_type": "refresh_token", "refresh_token": c.refreshToken, "client_id": client.id,
            "client_secret": client.secret,
        ]))
        return try pair((try? JSONSerialization.jsonObject(with: data)) as? [String: Any], status: status, keeping: c)
    }

    /// The account the token belongs to: its email and Drive's permissionId.
    func about(token: String) async throws -> (email: String?, id: String) {
        var req = URLRequest(url: Self.aboutURL)
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        req.timeoutInterval = 20
        let (data, status) = try await answer(req)
        let user = ((try? JSONSerialization.jsonObject(with: data)) as? [String: Any])?["user"] as? [String: Any]
        guard (200..<300).contains(status), let id = user?["permissionId"] as? String, !id.isEmpty else {
            throw GDriveConnectError.failed
        }
        return (user?["emailAddress"] as? String, id)
    }

    /// Ends the grant on Google's side (the refresh token ends the whole grant). Errors are ignored.
    func revoke(_ c: GDriveConnection) async {
        _ = try? await answer(form(Self.revokeURL, ["token": c.refreshToken]))
    }

    private func form(_ url: URL, _ fields: [String: String]) -> URLRequest {
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        var c = URLComponents()
        c.queryItems = fields.sorted { $0.key < $1.key }.map { URLQueryItem(name: $0.key, value: $0.value) }
        req.httpBody = Data((c.percentEncodedQuery ?? "").replacingOccurrences(of: "+", with: "%2B").utf8)
        req.timeoutInterval = 30
        return req
    }

    /// Google keeps the refresh token on a refresh (measured, docs/19 7-1절 2), so the old one stays unless a new one comes.
    private func pair(_ obj: [String: Any]?, status: Int, keeping old: GDriveConnection?) throws -> GDriveConnection {
        if let error = obj?["error"] as? String { throw error == "invalid_grant" ? GDriveConnectError.refused : .failed }
        guard (200..<300).contains(status), let token = obj?["access_token"] as? String, !token.isEmpty,
              let refresh = (obj?["refresh_token"] as? String) ?? old?.refreshToken, !refresh.isEmpty else {
            throw GDriveConnectError.failed
        }
        let seconds = (obj?["expires_in"] as? Double) ?? 3599
        return GDriveConnection(token: token, refreshToken: refresh, expiresAt: now().addingTimeInterval(seconds),
                                account: old?.account, accountID: old?.accountID, muted: old?.muted ?? [])
    }

    private func answer(_ req: URLRequest) async throws -> (Data, Int) {
        if Task.isCancelled { throw CancellationError() }
        do { return try await send(req) } catch {
            if (error as? URLError)?.code == .cancelled || error is CancellationError { throw CancellationError() }
            throw GDriveConnectError.network
        }
    }
}

/// What came back to the loopback address.
enum LoopbackReply: Equatable {
    case code(String)
    case error(String)

    /// Reads the browser's redirect off one loopback request line: `GET /?code=…&state=…`. Nil for any other request
    /// (a favicon, a wrong state, and with a `path` any other path; no `path` takes any).
    static func read(requestLine: String, state: String, path: String? = nil) -> LoopbackReply? {
    let parts = requestLine.split(separator: " ")
    guard parts.count >= 2, parts[0] == "GET",
          let url = URLComponents(string: "http://127.0.0.1" + parts[1]), let items = url.queryItems else { return nil }
    if let path, url.path != path { return nil }
    let value = { (name: String) in items.first { $0.name == name }?.value }
    guard value("state") == state else { return nil }
    if let error = value("error") { return .error(error) }
    if let code = value("code"), !code.isEmpty { return .code(code) }
    return nil
    }
}

/// What the browser page says once the redirect came (per service).
struct LoopbackPages: Equatable {
    let connected: String
    let declined: String

    static var gdrive: LoopbackPages {
        LoopbackPages(connected: String(localized: "Google Drive를 연결했어요. 이 창을 닫고 Tacie로 돌아가세요."),
                      declined: String(localized: "연결하지 않았어요. 이 창을 닫아도 돼요."))
    }

    static var dropbox: LoopbackPages {
        LoopbackPages(connected: String(localized: "Dropbox를 연결했어요. 이 창을 닫고 Tacie로 돌아가세요."),
                      declined: String(localized: "연결하지 않았어요. 이 창을 닫아도 돼요."))
    }
}

/// A one-shot HTTP listener on 127.0.0.1 (never on other interfaces: docs/19 15-1절) for Google's redirect.
final class LoopbackListener: @unchecked Sendable {
    private let listener: NWListener
    private let queue = DispatchQueue(label: "ai.tacie.loopback")
    private var done: CheckedContinuation<LoopbackReply, Error>?
    /// A redirect that came before `reply()` was asked.
    private var early: LoopbackReply?
    private let state: String
    private let path: String?
    private let pages: LoopbackPages

    init(state: String, path: String? = nil, pages: LoopbackPages = .gdrive) throws {
        let params = NWParameters.tcp
        params.requiredLocalEndpoint = NWEndpoint.hostPort(host: "127.0.0.1", port: .any)
        params.allowLocalEndpointReuse = true
        listener = try NWListener(using: params)
        self.state = state
        self.path = path
        self.pages = pages
    }

    /// Starts listening and answers the port.
    func start() async throws -> UInt16 {
        try await withCheckedThrowingContinuation { (c: CheckedContinuation<UInt16, Error>) in
            var resumed = false
            listener.stateUpdateHandler = { [weak self] s in
                guard !resumed else { return }
                switch s {
                case .ready:
                    resumed = true
                    c.resume(returning: self?.listener.port?.rawValue ?? 0)
                case .failed(let e):
                    resumed = true
                    c.resume(throwing: e)
                default: break
                }
            }
            listener.newConnectionHandler = { [weak self] conn in self?.serve(conn) }
            listener.start(queue: queue)
        }
    }

    /// Waits for the redirect. Cancelling the task stops the listener.
    func reply() async throws -> LoopbackReply {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (c: CheckedContinuation<LoopbackReply, Error>) in
                queue.async {
                    if let got = self.early {
                        self.early = nil
                        c.resume(returning: got)
                    } else {
                        self.done = c
                    }
                }
            }
        } onCancel: {
            self.finish(.failure(CancellationError()))
        }
    }

    func stop() { finish(.failure(CancellationError())) }

    private func finish(_ r: Result<LoopbackReply, Error>) {
        queue.async {
            self.listener.cancel()
            guard let c = self.done else { return }
            self.done = nil
            c.resume(with: r)
        }
    }

    private func serve(_ conn: NWConnection) {
        conn.start(queue: queue)
        conn.receive(minimumIncompleteLength: 1, maximumLength: 16_384) { [weak self] data, _, _, _ in
            guard let self else { return conn.cancel() }
            let text = data.map { String(decoding: $0, as: UTF8.self) } ?? ""
            let line = text.split(separator: "\r\n", maxSplits: 1).first.map(String.init) ?? ""
            let got = LoopbackReply.read(requestLine: line, state: self.state, path: self.path)
            let page: String
            switch got {
            case .code: page = self.pages.connected
            case .error: page = self.pages.declined
            case nil: page = ""
            }
            let body = Data("<!doctype html><meta charset=utf-8><title>Tacie</title><p style=\"font:16px -apple-system;margin:40px\">\(page)</p>".utf8)
            let head = "HTTP/1.1 \(got == nil ? "404 Not Found" : "200 OK")\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: \(body.count)\r\nConnection: close\r\n\r\n"
            conn.send(content: Data(head.utf8) + body, completion: .contentProcessed { _ in conn.cancel() })
            guard let got else { return }
            self.listener.cancel()
            if let c = self.done {
                self.done = nil
                c.resume(returning: got)
            } else {
                self.early = got
            }
        }
    }
}

/// The keychain item of the Google Drive connection.
enum GDriveStore {
    /// The account in use (AccountSpace): `connection` signed out, `connection@<id>` signed in.
    static var store: KeychainTokenStore {
        KeychainTokenStore(service: "ai.tacie.gdrive", account: AccountSpace.inUse.key("connection"), label: String(localized: "Tacie Google Drive 연결"))
    }

    static func load() -> GDriveConnection? {
        guard let text = store.read(), let data = text.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(GDriveConnection.self, from: data)
    }

    static func save(_ c: GDriveConnection) throws {
        try store.save(String(decoding: try JSONEncoder().encode(c), as: UTF8.self))
    }

    static func delete() throws { try store.delete() }
}

/// The Google Drive connection as the cards show it (docs/19 8절, 11절). The app is the only place that refreshes the
/// token (one hour): when less than `refreshBefore` is left (the 30-minute timer, launch), and at once when the engine
/// says Drive refused it (a long first fetch outlives a token; the engine waits for the new one).
@Observable
@MainActor
final class GDriveLink {
    enum Phase: Equatable {
        case idle
        case waiting  // the approval page is open in the browser
        case failed(String)  // what the card says, with 다시 연결
    }

    /// More than the keeper's 30-minute timer, so a token never runs out between two ticks.
    static let refreshBefore: TimeInterval = 40 * 60

    private(set) var phase: Phase = .idle
    private(set) var saved: GDriveConnection?
    /// Google refused the refresh token: connect again. Found files stay searchable for seven days.
    private(set) var lost = false
    var connected: Bool { saved != nil }
    var account: String? { saved?.account }

    var api = GDriveConnect()
    var now: () -> Date = Date.init
    /// Set by IndexKeeper: hands a connection (nil: none) to the engine.
    var push: (GDriveConnection?) async -> Void = { _ in }
    var open: (URL) -> Void = { NSWorkspace.shared.open($0) }
    /// Called once a connection is made (analytics, set by IndexKeeper).
    var onConnected: () -> Void = {}
    var keep: (GDriveConnection) throws -> Void = { try GDriveStore.save($0) }
    var forget: () -> Void = { try? GDriveStore.delete() }
    /// Opens the loopback listener: answers the redirect address and waits for the browser (tests replace it).
    var listen: (_ state: String) async throws -> (redirect: String, reply: () async throws -> LoopbackReply) = { state in
        let l = try LoopbackListener(state: state, pages: .gdrive)
        let port = try await l.start()
        return ("http://127.0.0.1:\(port)", { try await l.reply() })
    }

    private(set) var waitingTask: Task<Void, Never>?
    private var refreshing: Task<Void, Never>?
    private var renewedAt: Date?
    /// Bumped by connect and disconnect; a refresh started before drops its result.
    private var generation = 0
    /// The account is changing (`settle` → `reload`): no connection or refresh starts.
    private var held = false

    init(load: Bool = true) {
        if load { saved = GDriveStore.load() }
    }

    /// Before the account changes (AccountSpace): a connection being made stops, a refresh in flight ends first.
    /// From here until `reload` nothing new starts (a refresh asked for meanwhile is dropped).
    func settle() async {
        held = true
        waitingTask?.cancel()
        await waitingTask?.value
        waitingTask = nil
        await refreshing?.value
    }

    /// After the account changed: the new account's connection, or none.
    func reload() {
        held = false
        generation += 1
        saved = GDriveStore.load()
        lost = false
        renewedAt = nil
        phase = .idle
    }

    /// `--debug-control`'s `gdrive-preview`: a card state without the keychain or Google (captures).
    func preview(_ c: GDriveConnection?, lost: Bool = false, phase: Phase = .idle) {
        generation += 1
        saved = c
        self.lost = lost
        self.phase = phase
    }

    /// Opens Google's approval page and waits for the browser to come back to 127.0.0.1, then trades the code.
    func connect() {
        guard !held else { return }
        waitingTask?.cancel()
        guard api.client != nil else {
            waitingTask = nil
            phase = .failed(String(localized: "Google Drive 연결은 아직 준비 중이에요"))
            return
        }
        let state = Self.random(24)
        let verifier = Self.random(48)
        phase = .waiting
        waitingTask = Task {
            do {
                let (redirect, reply) = try await listen(state)
                guard let url = api.authorizeURL(redirect: redirect, state: state, challenge: Self.challenge(verifier)) else {
                    throw GDriveConnectError.unready
                }
                open(url)
                let got = try await withThrowingTaskGroup(of: LoopbackReply.self) { group in
                    group.addTask { try await reply() }
                    group.addTask {
                        try await Task.sleep(for: .seconds(GDriveConnect.patience))
                        throw GDriveConnectError.expired
                    }
                    defer { group.cancelAll() }
                    return try await group.next()!
                }
                guard case .code(let code) = got else { throw GDriveConnectError.denied }
                var c = try await api.exchange(code: code, verifier: verifier, redirect: redirect)
                let who = try await api.about(token: c.token)
                c.account = who.email
                c.accountID = who.id
                generation += 1
                save(c)
                saved = c
                lost = false
                phase = .idle
                Log.info("gdrive connected")
                onConnected()
                await push(c)
            } catch is CancellationError {
            } catch {
                let e = error as? GDriveConnectError ?? .failed
                phase = .failed(Self.words(e))
                Log.info("gdrive connect failed: \(e)")
            }
        }
    }

    func cancel() {
        waitingTask?.cancel()
        waitingTask = nil
        phase = .idle
    }

    /// Forgets the connection here and in the engine first, then ends it on Google's side.
    func disconnect() async {
        waitingTask?.cancel()
        generation += 1
        let old = saved
        forget()
        saved = nil
        lost = false
        phase = .idle
        Log.info("gdrive disconnected")
        await push(nil)
        if let old { await api.revoke(old) }
    }

    /// Turns shared drives off (or back on) and hands the change to the engine.
    func setMuted(_ ids: [String]) async {
        guard var c = saved else { return }
        c.muted = ids.sorted()
        save(c)
        saved = c
        await push(c)
    }

    /// Refreshes the token if less than `refreshBefore` is left. One at a time.
    func ensureFresh() async {
        await fresh(force: false)
    }

    /// The engine says Drive refused the token (it may be waiting mid-fetch): a forced refresh, at most once a minute.
    func renew() async {
        guard saved != nil, !lost else { return }
        if let at = renewedAt, now().timeIntervalSince(at) < 60 { return }
        renewedAt = now()
        await fresh(force: true)
    }

    private func fresh(force: Bool) async {
        guard !held else { return }
        if let running = refreshing {
            await running.value
            return
        }
        let task = Task { await self.refreshNow(force: force) }
        refreshing = task
        await task.value
        refreshing = nil
    }

    private func save(_ c: GDriveConnection) {
        do { try keep(c) } catch { Log.info("gdrive: the keychain did not take the connection, kept in memory") }
    }

    private func refreshNow(force: Bool) async {
        guard let current = saved, !lost else { return }
        guard force || current.expiresAt.timeIntervalSince(now()) < Self.refreshBefore else { return }
        let started = generation
        do {
            let next = try await api.refresh(current)
            guard started == generation, var merged = saved else {
                Log.info("gdrive: a refresh finished after the connection changed, dropped")
                return
            }
            // only the token fields are new: muted drives may have changed meanwhile
            merged.token = next.token
            merged.refreshToken = next.refreshToken
            merged.expiresAt = next.expiresAt
            save(merged)
            saved = merged
            Log.info("gdrive token renewed")
            await push(merged)
        } catch GDriveConnectError.refused {
            lost = true
            Log.info("gdrive refresh refused")
        } catch {
            Log.info("gdrive refresh failed: \(error as? GDriveConnectError ?? .failed)")
        }
    }

    static func words(_ e: GDriveConnectError) -> String {
        switch e {
        case .denied: String(localized: "연결하지 않았어요")
        case .expired: String(localized: "시간이 지났어요")
        case .refused: String(localized: "Google Drive 연결이 끊겼어요")
        case .scope: String(localized: "Drive 읽기를 허용해야 연결돼요")
        case .unready: String(localized: "Google Drive 연결은 아직 준비 중이에요")
        case .network, .failed: String(localized: "연결하지 못했어요")
        }
    }

    /// URL-safe random characters (`count` bytes).
    static func random(_ count: Int) -> String {
        var bytes = [UInt8](repeating: 0, count: count)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        return urlSafe(Data(bytes))
    }

    /// BASE64URL(SHA256(verifier)) without padding (RFC 7636 S256).
    static func challenge(_ verifier: String) -> String {
        urlSafe(Data(SHA256.hash(data: Data(verifier.utf8))))
    }

    private static func urlSafe(_ d: Data) -> String {
        d.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}
