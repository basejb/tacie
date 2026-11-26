import AppKit
import CryptoKit
import Foundation
import Observation
import Security

/// What Tacie keeps of a Linear connection (keychain `ai.tacie.linear`, as JSON). Never logged.
struct LinearConnection: Codable, Equatable {
    var token: String
    var refreshToken: String
    var expiresAt: Date
    var workspace: String?
    var workspaceID: String?
    /// Teams the user turned off (docs/18 7절 6). Only these are remembered, so new teams come in on their own.
    var muted: [String] = []
    /// 오픈소스판: a personal API key pasted by the person (no refresh token, never refreshed or revoked through OAuth).
    var pasted: Bool?
}

/// `signedOut`: Tacie's server wants a Tacie session to hand over the code (sign in to Tacie first).
enum LinearConnectError: Error, Equatable { case denied, expired, failed, network, refused, signedOut }

/// Linear's sign-in with PKCE (docs/18 8절). Linear takes the code exchange and the refresh without a client secret,
/// so Tacie's server only parks the code Linear sends back (`/linear/callback`) and sends the app's sign-in window to
/// `tacie://linear` with a one-time key; the app picks the code up with that key and its Tacie session
/// (`POST /linear/code`, issue #38), then trades it with Linear directly using the verifier only the app knows. The
/// server never sees a token. Refreshing goes to Linear directly too.
struct LinearConnect {
    /// The Tacie OAuth app (docs/18 13절). Public, not a secret; official builds only (Official.xcconfig).
    static let defaultClientID = BuildValues.linearClientID ?? ""
    static let redirect = redirect(server: AccountServer.base)
    static func redirect(server: URL) -> String { server.appendingPathComponent("linear/callback").absoluteString }
    static let authorize = URL(string: "https://linear.app/oauth/authorize")!
    static let token = URL(string: "https://api.linear.app/oauth/token")!
    static let revokeURL = URL(string: "https://api.linear.app/oauth/revoke")!
    static let graphql = URL(string: "https://api.linear.app/graphql")!
    /// How long the approval page may take.
    static let patience: TimeInterval = 300

    var clientID = LinearConnect.defaultClientID
    /// (body, HTTP status). Throws only when there was no answer.
    var send: (URLRequest) async throws -> (Data, Int) = { req in
        let (data, resp) = try await URLSession.shared.data(for: req)
        return (data, (resp as? HTTPURLResponse)?.statusCode ?? 0)
    }
    /// The Tacie session the server asks for to hand over the code. Read on each call.
    var session: () -> String? = { SessionStore.store.read() }
    var now: () -> Date = Date.init

    init(clientID: String = LinearConnect.defaultClientID) {
        self.clientID = clientID
    }

    /// 64 random URL-safe characters (48 bytes): RFC 7636 wants 43–128.
    static func newVerifier() -> String { random(48) }

    /// A state with the server's mark for "picks up with a key" (`KEYED` in the server's handoff.js).
    static func newSignInState() -> String { "k2_" + random(32) }

    /// BASE64URL(SHA256(verifier)) without padding (RFC 7636 S256).
    static func challenge(_ verifier: String) -> String {
        urlSafe(Data(SHA256.hash(data: Data(verifier.utf8))))
    }

    private static func random(_ count: Int) -> String {
        var bytes = [UInt8](repeating: 0, count: count)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        return urlSafe(Data(bytes))
    }

    private static func urlSafe(_ d: Data) -> String {
        d.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    func authorizeURL(state: String, challenge: String) -> URL {
        var c = URLComponents(url: Self.authorize, resolvingAgainstBaseURL: false)!
        c.queryItems = [
            URLQueryItem(name: "client_id", value: clientID),
            URLQueryItem(name: "redirect_uri", value: Self.redirect),
            URLQueryItem(name: "response_type", value: "code"),
            URLQueryItem(name: "scope", value: "read"),
            URLQueryItem(name: "state", value: state),
            URLQueryItem(name: "code_challenge", value: challenge),
            URLQueryItem(name: "code_challenge_method", value: "S256"),
            URLQueryItem(name: "prompt", value: "consent"),
            URLQueryItem(name: "actor", value: "user"),
        ]
        return c.url!
    }

    /// The one-time key from `tacie://linear?state=…&k=…`. A return that is not for the sign-in this app started
    /// throws `CancellationError`: it is ignored.
    func parseReturn(_ url: URL, state: String) throws -> String {
        let value = try WebSignIn.returned(url, host: "linear", state: state)
        if let error = value("error") { throw error == "denied" ? LinearConnectError.denied : .failed }
        guard let k = value("k"), !k.isEmpty else { throw LinearConnectError.failed }
        return k
    }

    /// The code the server parked for this sign-in, handed over for the key and the Tacie session.
    func code(state: String, k: String) async throws -> String {
        if Task.isCancelled { throw CancellationError() }
        let data: Data, status: Int
        do {
            (data, status) = try await AccountServer(send: send, session: session).authorized("linear/code", body: ["state": state, "k": k])
        } catch AccountError.signedOut {
            throw LinearConnectError.signedOut
        } catch {
            if Task.isCancelled { throw CancellationError() }
            throw LinearConnectError.network
        }
        if status == 401 { throw LinearConnectError.signedOut }
        if status == 400 { throw LinearConnectError.expired }
        let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        guard (200..<300).contains(status), let code = obj?["code"] as? String, !code.isEmpty else {
            throw LinearConnectError.failed
        }
        return code
    }

    /// The first token pair. The workspace is asked separately (`organization`) so the pair is saved first.
    func exchange(code: String, verifier: String) async throws -> LinearConnection {
        let (data, status) = try await answer(form(Self.token, [
            "grant_type": "authorization_code", "code": code, "redirect_uri": Self.redirect,
            "client_id": clientID, "code_verifier": verifier,
        ]))
        return try pair(data, status: status, keeping: nil)
    }

    /// A new pair for the saved one. Only `invalid_grant` means the connection is gone (`.refused`).
    func refresh(_ c: LinearConnection) async throws -> LinearConnection {
        let (data, status) = try await answer(form(Self.token, [
            "grant_type": "refresh_token", "refresh_token": c.refreshToken, "client_id": clientID,
        ]))
        return try pair(data, status: status, keeping: c)
    }

    /// The workspace the token belongs to.
    func organization(token: String) async throws -> (id: String, name: String) {
        var req = URLRequest(url: Self.graphql)
        req.httpMethod = "POST"
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = Data(#"{"query":"{ organization { id name } }"}"#.utf8)
        req.timeoutInterval = 20
        let (data, status) = try await answer(req)
        let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        let org = (obj?["data"] as? [String: Any])?["organization"] as? [String: Any]
        guard (200..<300).contains(status), let id = org?["id"] as? String, let name = org?["name"] as? String else {
            throw LinearConnectError.failed
        }
        return (id, name)
    }

    /// Ends the grant on Linear's side: the refresh token first, then the access token. Errors are ignored.
    func revoke(_ c: LinearConnection) async {
        _ = try? await answer(form(Self.revokeURL, ["token": c.refreshToken, "token_type_hint": "refresh_token"]))
        _ = try? await answer(form(Self.revokeURL, ["token": c.token, "token_type_hint": "access_token"]))
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

    private func pair(_ data: Data, status: Int, keeping old: LinearConnection?) throws -> LinearConnection {
        let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        if let error = obj?["error"] as? String { throw error == "invalid_grant" ? LinearConnectError.refused : .failed }
        guard (200..<300).contains(status), let token = obj?["access_token"] as? String, !token.isEmpty,
              let refresh = obj?["refresh_token"] as? String, !refresh.isEmpty else {
            throw LinearConnectError.failed
        }
        let seconds = (obj?["expires_in"] as? Double) ?? 86_399
        return LinearConnection(token: token, refreshToken: refresh, expiresAt: now().addingTimeInterval(seconds),
                                workspace: old?.workspace, workspaceID: old?.workspaceID, muted: old?.muted ?? [])
    }

    private func answer(_ req: URLRequest) async throws -> (Data, Int) {
        if Task.isCancelled { throw CancellationError() }
        do { return try await send(req) } catch {
            if (error as? URLError)?.code == .cancelled || error is CancellationError { throw CancellationError() }
            throw LinearConnectError.network
        }
    }
}

/// The keychain item of the Linear connection.
enum LinearStore {
    /// The account in use (AccountSpace): `connection` signed out, `connection@<id>` signed in.
    static var store: KeychainTokenStore {
        KeychainTokenStore(service: "ai.tacie.linear", account: AccountSpace.inUse.key("connection"), label: String(localized: "Tacie Linear 연결"))
    }

    static func load() -> LinearConnection? {
        guard let text = store.read(), let data = text.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(LinearConnection.self, from: data)
    }

    static func save(_ c: LinearConnection) throws {
        try store.save(String(decoding: try JSONEncoder().encode(c), as: UTF8.self))
    }

    static func delete() throws { try store.delete() }
}

/// One refresh at a time across processes (a dev and a release Tacie, or two windows of tests), with a file lock in
/// Application Support. `.none` is for tests.
struct RefreshLock {
    var path: URL?

    static let none = RefreshLock(path: nil)
    /// `<name>-refresh.lock` (`linear`, `atlassian`): one lock per connection whose refresh token rotates.
    static func named(_ name: String) -> RefreshLock {
        let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent(Build.supportFolder, isDirectory: true)
        return RefreshLock(path: dir.appendingPathComponent("\(name)-refresh.lock"))
    }

    @MainActor
    func run(_ body: @MainActor () async -> Void) async {
        guard let path else { return await body() }
        try? FileManager.default.createDirectory(at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
        let fd = open(path.path, O_CREAT | O_RDWR, 0o600)
        guard fd >= 0 else { return await body() }
        defer { close(fd) }
        // flock blocks: wait on a background thread so the main thread stays free
        await Task.detached { _ = flock(fd, LOCK_EX) }.value
        defer { flock(fd, LOCK_UN) }
        await body()
    }
}

/// A marker written before a token request and removed once the new pair is in the keychain. If Tacie dies in
/// between, the next launch refreshes at once, inside Linear's 30-minute replay grace.
struct RefreshMarker {
    var set: (Bool) -> Void
    var isSet: () -> Bool

    /// `<name>-refresh.pending` (`linear`, `atlassian`; SpaceAdoption.items moves it with the account).
    static func named(_ name: String) -> RefreshMarker {
        // beside the account's other data: the keychain pair it guards is the account's (AccountSpace), looked up on
        // each use since the account can change while the app runs
        var file: URL { AccountSpace.inUse.folder().appendingPathComponent("\(name)-refresh.pending") }
        return RefreshMarker(
            set: { on in
                let file = file
                if on {
                    try? FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
                    FileManager.default.createFile(atPath: file.path, contents: Data())
                } else {
                    try? FileManager.default.removeItem(at: file)
                }
            },
            isSet: { FileManager.default.fileExists(atPath: file.path) })
    }
}

/// The Linear connection as the cards show it (docs/18 7절). The app is the only place that refreshes the token
/// (8절): an hour before it ends, at launch if it already did, or when the engine says Linear refused it.
@Observable
@MainActor
final class LinearLink {
    enum Phase: Equatable {
        case idle
        case waiting  // the approval page is open in the browser
        case failed(String)  // what the card says, with 다시 연결
    }

    private(set) var phase: Phase = .idle
    private(set) var saved: LinearConnection?
    /// Linear refused the refresh token: connect again. Found items stay searchable for seven days (docs/18 0절).
    private(set) var lost = false
    var connected: Bool { saved != nil }
    var workspace: String? { saved?.workspace }

    var api = LinearConnect()
    var now: () -> Date = Date.init
    /// Set by IndexKeeper: hands a connection (nil: none) to the engine.
    var push: (LinearConnection?) async -> Void = { _ in }
    /// Shows the approval page and answers the `tacie://` address it ends at (tests replace it).
    var authenticate: (URL) async throws -> URL = { url in try await WebSignIn().run(url) }
    /// Called once a connection is made (analytics, set by IndexKeeper).
    var onConnected: () -> Void = {}
    /// 오픈소스판: asks the service whether a pasted token still works (tests replace it).
    var checkPasted: (LinearConnection) async -> PastedTokens.Check = { await PastedTokens.check(.linear, token: $0.token) }
    var load: () -> LinearConnection? = { LinearStore.load() }
    var keep: (LinearConnection) throws -> Void = { try LinearStore.save($0) }
    var forget: () -> Void = { try? LinearStore.delete() }
    var lock = RefreshLock.named("linear")
    var marker = RefreshMarker.named("linear")

    private(set) var waitingTask: Task<Void, Never>?
    private var refreshing: Task<Void, Never>?
    /// A new pair whose keychain write failed: kept in memory, written again on the next call.
    private var unsaved: LinearConnection?
    private var renewedAt: Date?
    /// Bumped by connect and disconnect; a refresh started before drops its result.
    private var generation = 0
    /// The account is changing (`settle` → `reload`): no connection or refresh starts.
    private var held = false

    init(load: Bool = true) {
        if load { saved = LinearStore.load() }
    }

    /// Before the account changes (AccountSpace): a connection being made stops, a refresh in flight ends first, so
    /// a rotated pair always lands in the keychain item of the account it belongs to.
    /// From here until `reload` nothing new starts (a refresh asked for meanwhile is dropped), and a pair that only
    /// lived in memory gets one more try at the keychain.
    func settle() async {
        held = true
        waitingTask?.cancel()
        await waitingTask?.value
        waitingTask = nil
        await refreshing?.value
        if let pending = unsaved {
            save(pending)
            if unsaved == nil { marker.set(false) } else { Log.info("linear: the keychain still refused the new pair") }
        }
    }

    /// After the account changed: the new account's connection, or none.
    func reload() {
        held = false
        generation += 1
        saved = load()
        unsaved = nil
        lost = false
        renewedAt = nil
        phase = .idle
    }

    /// A connection without the keychain (tests). Counts as a new connection.
    func setForTesting(_ c: LinearConnection?) {
        generation += 1
        saved = c
    }

    /// `--debug-control`'s `linear-preview`: a card state without the keychain or Linear (captures).
    func preview(_ c: LinearConnection?, lost: Bool = false, phase: Phase = .idle) {
        generation += 1
        saved = c
        self.lost = lost
        self.phase = phase
    }

    /// Opens Linear's approval page and waits for the code, then trades it for a token pair.
    func connect() {
        guard !held else { return }
        waitingTask?.cancel()
        // no Linear app registered in this build: Linear would only answer "include client_id"
        guard !api.clientID.isEmpty else {
            waitingTask = nil
            phase = .failed(String(localized: "Linear 연결은 아직 준비 중이에요"))
            return
        }
        let state = LinearConnect.newSignInState()
        let verifier = LinearConnect.newVerifier()
        let start = api.authorizeURL(state: state, challenge: LinearConnect.challenge(verifier))
        let authenticate = authenticate
        phase = .waiting
        waitingTask = Task {
            do {
                let back = try await WebSignIn.within(LinearConnect.patience, expired: LinearConnectError.expired) {
                    try await authenticate(start)
                }
                let k = try api.parseReturn(back, state: state)
                let code = try await api.code(state: state, k: k)
                var c = try await api.exchange(code: code, verifier: verifier)
                generation += 1
                save(c)
                saved = c
                lost = false
                if let org = try? await api.organization(token: c.token) {
                    c.workspaceID = org.id
                    c.workspace = org.name
                    save(c)
                    saved = c
                }
                phase = .idle
                Log.info("linear connected")
                onConnected()
                await push(c)
            } catch is CancellationError {
                // closed by the user, or a return that was not ours: the card goes back quietly
                if !Task.isCancelled { phase = .idle }
            } catch {
                let e = error as? LinearConnectError ?? .failed
                phase = .failed(Self.words(e))
                Log.info("linear connect failed: \(e)")
            }
        }
    }

    func cancel() {
        waitingTask?.cancel()
        waitingTask = nil
        phase = .idle
    }

    /// 오픈소스판: a personal API key the person pasted (checked already, `PastedTokens`). Saved as it is, with no refresh
    /// token; when Linear refuses it later the card asks for a new one.
    func connectPasted(token: String, org: PastedTokens.Found) async {
        guard !held else { return }
        waitingTask?.cancel()
        generation += 1
        let same = saved?.workspaceID != nil && saved?.workspaceID == org.id ? saved : nil
        let c = LinearConnection(token: token, refreshToken: "", expiresAt: .distantFuture, workspace: org.name,
                                 workspaceID: org.id, muted: same?.muted ?? [], pasted: true)
        save(c)
        marker.set(false)
        saved = c
        lost = false
        renewedAt = nil
        phase = .idle
        Log.info("linear connected with a pasted key")
        onConnected()
        await push(c)
    }

    /// Forgets the connection here and in the engine first, then ends it on Linear's side (an offline Mac is not
    /// held up: the card is already off).
    func disconnect() async {
        waitingTask?.cancel()
        generation += 1
        let old = saved
        forget()
        saved = nil
        unsaved = nil
        lost = false
        phase = .idle
        Log.info("linear disconnected")
        await push(nil)
        if let old, old.pasted != true { await api.revoke(old) }
    }

    /// Turns teams off (or back on) and hands the change to the engine.
    func setMuted(_ ids: [String]) async {
        guard var c = load() ?? saved else { return }
        if let s = saved, c.workspaceID != s.workspaceID { c = s }
        c.muted = ids.sorted()
        save(c)
        saved = c
        await push(c)
    }

    /// Refreshes the token if it ends within an hour (or a refresh was cut short). One at a time.
    func ensureFresh() async {
        await fresh(force: false)
    }

    /// The engine says Linear refused the token: one forced try, at most every ten minutes.
    func renew() async {
        guard saved != nil, !lost else { return }
        if let at = renewedAt, now().timeIntervalSince(at) < 600 { return }
        renewedAt = now()
        // a pasted token has nothing to refresh: ask the service once, and only a refusal asks for a new one (one
        // refused request may be a single query's permission, not the token)
        if let c = saved, c.pasted == true {
            if case .rejected = await checkPasted(c) {
            lost = true
                Log.info("linear refused the pasted token")
            }
            return
        }
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

    private func save(_ c: LinearConnection) {
        do {
            try keep(c)
            unsaved = nil
        } catch {
            unsaved = c
            Log.info("linear: the keychain did not take the connection, kept in memory")
        }
    }

    private func refreshNow(force: Bool) async {
        if let pending = unsaved {
            save(pending)
            if unsaved == nil { marker.set(false) }
            return
        }
        guard saved != nil, !lost, saved?.pasted != true else { return }
        let started = generation
        await lock.run {
            guard var current = self.saved, started == self.generation else { return }
            // another Tacie may have refreshed already: take its pair
            if let disk = self.load(), disk.workspaceID == current.workspaceID, disk.refreshToken != current.refreshToken,
               disk.expiresAt > current.expiresAt {
                self.saved = disk
                current = disk
                if disk.expiresAt.timeIntervalSince(self.now()) >= 3600 && !self.marker.isSet() {
                    await self.push(disk)
                    return
                }
            }
            let due = current.expiresAt.timeIntervalSince(self.now()) < 3600
            guard force || due || self.marker.isSet() else { return }
            self.marker.set(true)
            do {
                let next = try await self.api.refresh(current)
                guard started == self.generation, self.saved?.refreshToken == current.refreshToken else {
                    Log.info("linear: a refresh finished after the connection changed, dropped")
                    return
                }
                // only the token fields are new: muted teams or the workspace may have changed meanwhile
                var merged = self.saved ?? next
                merged.token = next.token
                merged.refreshToken = next.refreshToken
                merged.expiresAt = next.expiresAt
                self.save(merged)
                if self.unsaved == nil { self.marker.set(false) }
                self.saved = merged
                Log.info("linear token renewed")
                await self.push(merged)
            } catch LinearConnectError.refused {
                self.marker.set(false)
                self.lost = true
                Log.info("linear refresh refused")
            } catch {
                // the marker stays: if Linear did rotate the pair, the next try replays inside the grace
                Log.info("linear refresh failed: \(error as? LinearConnectError ?? .failed)")
            }
        }
    }

    static func words(_ e: LinearConnectError) -> String {
        switch e {
        case .denied: String(localized: "연결하지 않았어요")
        case .expired: String(localized: "시간이 지났어요")
        case .refused: String(localized: "Linear 연결이 끊겼어요")
        case .signedOut: SettingsText.atlassianSignedOut
        case .network, .failed: String(localized: "연결하지 못했어요")
        }
    }
}
