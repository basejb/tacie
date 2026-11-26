import AppKit
import Foundation
import Observation

/// What Tacie keeps of a Dropbox connection (keychain `ai.tacie.dropbox`, as JSON). Never logged.
struct DropboxConnection: Codable, Equatable {
    var token: String
    /// Dropbox never sends a new one on a refresh: this one stays for the life of the grant.
    var refreshToken: String
    var expiresAt: Date
    /// Dropbox's `account_id`: the engine requires it with a token, and it tells a different account apart.
    var accountID: String
    var account: String?  // the Dropbox account's email
    /// Top folders the user turned off. Only these are remembered, so new folders come in on their own.
    var mutedFolders: [String] = []

    /// The body of the engine's `POST /dropbox/credentials` (memory only, never logged); `token: null` disconnects.
    static func engineBody(_ c: DropboxConnection?) -> [String: Any] {
        [
            "token": c?.token ?? NSNull(), "account": c?.account ?? NSNull(), "account_id": c?.accountID ?? NSNull(),
            "muted_folders": c?.mutedFolders ?? [],
        ]
    }
}

extension DropboxConnection {
    /// An item saved before muted folders existed reads as none turned off. Anything else missing throws, so a broken
    /// item is never taken for "not connected".
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        token = try c.decode(String.self, forKey: .token)
        refreshToken = try c.decode(String.self, forKey: .refreshToken)
        expiresAt = try c.decode(Date.self, forKey: .expiresAt)
        accountID = try c.decode(String.self, forKey: .accountID)
        account = try c.decodeIfPresent(String.self, forKey: .account)
        mutedFolders = try c.decodeIfPresent([String].self, forKey: .mutedFolders) ?? []
    }
}

/// Who the token belongs to. `team`: a Dropbox Business account. `paperLegacy`: Paper docs live outside the files
/// (unknown when the features call did not answer).
struct DropboxAccount: Equatable {
    let id: String
    let email: String?
    let team: Bool
    var paperLegacy: Bool? = nil
}

enum DropboxConnectError: Error, Equatable { case denied, expired, failed, network, refused, scope }

/// Dropbox's sign-in for an installed app: PKCE with the public app key and no secret, so no server is in the middle.
/// The browser comes back to `http://127.0.0.1:<port>/dropbox` (Dropbox ignores the loopback port) and the app trades
/// the code itself. Refreshing goes to Dropbox directly too.
struct DropboxConnect {
    /// Public: it ships in the app. There is no client secret. Official builds only (Official.xcconfig); the open
    /// edition brings its own Dropbox app key (docs/41 0절).
    static let defaultAppKey = BuildValues.dropboxAppKey ?? ""
    var appKey = DropboxConnect.defaultAppKey
    static let authorize = URL(string: "https://www.dropbox.com/oauth2/authorize")!
    static let tokenURL = URL(string: "https://api.dropboxapi.com/oauth2/token")!
    static let accountURL = URL(string: "https://api.dropboxapi.com/2/users/get_current_account")!
    static let featuresURL = URL(string: "https://api.dropboxapi.com/2/users/features/get_values")!
    static let revokeURL = URL(string: "https://api.dropboxapi.com/2/auth/token/revoke")!
    /// The granted scopes must hold both, or the files cannot be read.
    static let scopes: Set<String> = ["files.content.read", "files.metadata.read"]
    static let redirectPath = "/dropbox"
    /// How long the approval page may take (an unapproved account never comes back to the redirect).
    static let patience: TimeInterval = 300
    /// Dropbox's access tokens last four hours.
    static let tokenLife: TimeInterval = 14400

    /// (body, HTTP status). Throws only when there was no answer.
    var send: (URLRequest) async throws -> (Data, Int) = { req in
        let (data, resp) = try await URLSession.shared.data(for: req)
        return (data, (resp as? HTTPURLResponse)?.statusCode ?? 0)
    }
    var now: () -> Date = Date.init

    /// No `scope` parameter: the app's console scopes are granted. A refresh token comes only with offline access.
    func authorizeURL(redirect: String, state: String, challenge: String, korean: Bool = AppLocale.isKorean) -> URL {
        var c = URLComponents(url: Self.authorize, resolvingAgainstBaseURL: false)!
        c.queryItems = [
            URLQueryItem(name: "client_id", value: appKey),
            URLQueryItem(name: "response_type", value: "code"),
            URLQueryItem(name: "redirect_uri", value: redirect),
            URLQueryItem(name: "code_challenge", value: challenge),
            URLQueryItem(name: "code_challenge_method", value: "S256"),
            URLQueryItem(name: "token_access_type", value: "offline"),
            URLQueryItem(name: "state", value: state),
            URLQueryItem(name: "locale", value: korean ? "ko" : "en"),
        ]
        return c.url!
    }

    /// The first token pair. File read access must be in the granted scopes.
    func exchange(code: String, verifier: String, redirect: String) async throws
        -> (token: String, refresh: String, expiresIn: TimeInterval, accountID: String) {
        let (data, status) = try await answer(form([
            "code": code, "grant_type": "authorization_code", "redirect_uri": redirect, "code_verifier": verifier,
            "client_id": appKey,
        ]))
        let obj = try tokenAnswer(data, status: status)
        if let scope = obj["scope"] as? String, !Self.scopes.isSubset(of: Set(scope.split(separator: " ").map(String.init))) {
            throw DropboxConnectError.scope
        }
        guard let token = obj["access_token"] as? String, !token.isEmpty,
              let refresh = obj["refresh_token"] as? String, !refresh.isEmpty,
              let id = obj["account_id"] as? String, !id.isEmpty else { throw DropboxConnectError.failed }
        return (token, refresh, Self.expiresIn(obj), id)
    }

    /// A fresh access token. The refresh token never changes. Only `invalid_grant` means the connection is gone.
    func refresh(_ c: DropboxConnection) async throws -> (token: String, expiresIn: TimeInterval) {
        let (data, status) = try await answer(form([
            "grant_type": "refresh_token", "refresh_token": c.refreshToken, "client_id": appKey,
        ]))
        let obj = try tokenAnswer(data, status: status)
        guard let token = obj["access_token"] as? String, !token.isEmpty else { throw DropboxConnectError.failed }
        return (token, Self.expiresIn(obj))
    }

    /// The account the token belongs to. A `team` field means a Dropbox Business account.
    func account(token: String) async throws -> DropboxAccount {
        let (data, status) = try await answer(rpc(Self.accountURL, token: token, body: "null"))
        let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        guard (200..<300).contains(status), let id = obj?["account_id"] as? String, !id.isEmpty else {
            throw DropboxConnectError.failed
        }
        let team = obj?["team"].map { !($0 is NSNull) } ?? false
        return DropboxAccount(id: id, email: obj?["email"] as? String, team: team)
    }

    /// Whether Paper docs live outside the files (`paper_as_files` off). Nil when Dropbox did not say.
    func paperLegacy(token: String) async -> Bool? {
        let body = #"{"features":[{".tag":"paper_as_files"}]}"#
        guard let (data, status) = try? await answer(rpc(Self.featuresURL, token: token, body: body)),
              (200..<300).contains(status),
              let values = ((try? JSONSerialization.jsonObject(with: data)) as? [String: Any])?["values"] as? [[String: Any]],
              let flag = values.first(where: { $0[".tag"] as? String == "paper_as_files" })?["paper_as_files"] as? [String: Any],
              let enabled = flag["enabled"] as? Bool else { return nil }
        return !enabled
    }

    /// Ends this grant on Dropbox's side. A token that ran out gets one refresh and one more try. Errors are ignored.
    func revoke(_ c: DropboxConnection) async {
        let first = try? await answer(rpc(Self.revokeURL, token: c.token, body: "null"))
        Log.info("dropbox revoke \(first.map { String($0.1) } ?? "no answer")")
        guard first?.1 == 401, let fresh = try? await refresh(c) else { return }
        let second = try? await answer(rpc(Self.revokeURL, token: fresh.token, body: "null"))
        Log.info("dropbox revoke after refresh \(second.map { String($0.1) } ?? "no answer")")
    }

    private static func expiresIn(_ obj: [String: Any]) -> TimeInterval {
        (obj["expires_in"] as? Double) ?? tokenLife
    }

    /// The token endpoint's answer: `invalid_grant` is `.refused`, any other error or status is `.failed`.
    private func tokenAnswer(_ data: Data, status: Int) throws -> [String: Any] {
        let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        if let error = obj?["error"] as? String { throw error == "invalid_grant" ? DropboxConnectError.refused : .failed }
        guard (200..<300).contains(status), let obj else { throw DropboxConnectError.failed }
        return obj
    }

    private func form(_ fields: [String: String]) -> URLRequest {
        var req = URLRequest(url: Self.tokenURL)
        req.httpMethod = "POST"
        req.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        var c = URLComponents()
        c.queryItems = fields.sorted { $0.key < $1.key }.map { URLQueryItem(name: $0.key, value: $0.value) }
        req.httpBody = Data((c.percentEncodedQuery ?? "").replacingOccurrences(of: "+", with: "%2B").utf8)
        req.timeoutInterval = 30
        return req
    }

    /// A Dropbox RPC call: always JSON, even with no arguments (`null`; a form type gets 400).
    private func rpc(_ url: URL, token: String, body: String) -> URLRequest {
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = Data(body.utf8)
        req.timeoutInterval = 20
        return req
    }

    private func answer(_ req: URLRequest) async throws -> (Data, Int) {
        if Task.isCancelled { throw CancellationError() }
        do { return try await send(req) } catch {
            if (error as? URLError)?.code == .cancelled || error is CancellationError { throw CancellationError() }
            throw DropboxConnectError.network
        }
    }
}

/// The keychain item of the Dropbox connection.
enum DropboxStore {
    static let service = "ai.tacie.dropbox"

    /// The account in use (AccountSpace): `connection` signed out, `connection@<id>` signed in.
    static var store: KeychainTokenStore {
        KeychainTokenStore(service: service, account: AccountSpace.inUse.key("connection"),
                           label: String(localized: "Tacie Dropbox 연결"))
    }

    /// nil when there is no item; throws when the keychain could not answer (locked, refused) or the item does not
    /// decode, so a failed read is never taken for "not connected" (handing the engine nil drops the found files).
    static func load(from store: any TokenStore = store) throws -> DropboxConnection? {
        guard let text = try store.readChecked() else { return nil }
        return try JSONDecoder().decode(DropboxConnection.self, from: Data(text.utf8))
    }

    static func save(_ c: DropboxConnection) throws {
        try store.save(String(decoding: try JSONEncoder().encode(c), as: UTF8.self))
    }

    static func delete() throws { try store.delete() }
}

/// The Dropbox connection as the cards show it. The app is the only place that refreshes the token (four hours): when
/// less than `refreshBefore` is left (the 30-minute timer, launch), and at once when the engine says Dropbox refused
/// it. The refresh token never changes, so a refresh whose answer was lost is simply asked again on the next tick.
@Observable
@MainActor
final class DropboxLink {
    enum Phase: Equatable {
        case idle
        case waiting  // the approval page is open in the browser
        case failed(String)  // what the card says, with 다시 연결
    }

    /// More than the keeper's 30-minute timer, so a token never runs out between two ticks.
    static let refreshBefore: TimeInterval = 40 * 60

    private(set) var phase: Phase = .idle
    private(set) var saved: DropboxConnection?
    /// Dropbox refused the refresh token: connect again. Found files stay searchable for seven days.
    private(set) var lost = false
    /// When this app saw the refusal: the card's seven days count from it until the engine reports its `lost_since`.
    private(set) var lostAt: Date?
    /// The keychain could not be read (locked, refused, an item that does not decode): whether there is a connection
    /// is unknown, so launch hands the engine nothing rather than "none", which would drop the found files.
    private(set) var unreadable = false
    /// `dropbox-preview` shows a made-up state: nothing done meanwhile reads or writes the keychain, reaches the engine
    /// or calls Dropbox, so the real connection is never overwritten or wiped. Off, `connect` and `reload` end it.
    private(set) var previewing = false
    var connected: Bool { saved != nil }
    var account: String? { saved?.account }

    var api = DropboxConnect()
    var now: () -> Date = Date.init
    /// Set by IndexKeeper: hands a connection (nil: none) to the engine.
    var push: (DropboxConnection?) async -> Void = { _ in }
    var open: (URL) -> Void = { NSWorkspace.shared.open($0) }
    /// Called once a connection is made (analytics, set by IndexKeeper).
    var onConnected: (DropboxAccount) -> Void = { _ in }
    var load: () throws -> DropboxConnection? = { try DropboxStore.load() }
    var keep: (DropboxConnection) throws -> Void = { try DropboxStore.save($0) }
    var forget: () -> Void = { try? DropboxStore.delete() }
    /// One refresh at a time, also across processes.
    var lock = RefreshLock.named("dropbox")
    /// How long the approval page may take (tests shorten it).
    var patience: TimeInterval = DropboxConnect.patience
    /// Opens the loopback listener: answers the redirect address and waits for the browser (tests replace it).
    var listen: (_ state: String) async throws -> (redirect: String, reply: () async throws -> LoopbackReply) = { state in
        let l = try LoopbackListener(state: state, path: DropboxConnect.redirectPath, pages: .dropbox)
        let port = try await l.start()
        return ("http://127.0.0.1:\(port)\(DropboxConnect.redirectPath)", { try await l.reply() })
    }

    private(set) var waitingTask: Task<Void, Never>?
    private var refreshing: Task<Void, Never>?
    private var renewedAt: Date?
    /// Bumped by connect and disconnect; a refresh started before drops its result.
    private var generation = 0
    /// The account is changing (`settle` → `reload`): no connection or refresh starts.
    private var held = false
    /// The last hand-over to the engine: the next one waits for it, so they land in the order they were made.
    private var handing: Task<Void, Never>?

    init(load: Bool = true) {
        if load { loadSaved() }
    }

    /// The saved connection from the keychain, or none; a failed read leaves `unreadable` set.
    private func loadSaved() {
        do {
            saved = try load()
            unreadable = false
        } catch {
            saved = nil
            unreadable = true
            Log.info("dropbox: the keychain could not be read (\((error as? KeychainTokenStore.Failure).map { "status \($0.status)" } ?? "other"))")
        }
    }

    /// The 30-minute timer: a keychain that could not be read is read again; once it answers, what it holds (or
    /// none) goes to the engine like at launch.
    func retryUnreadable() async {
        guard unreadable, !held, !previewing else { return }
        loadSaved()
        guard !unreadable else { return }
        Log.info("dropbox: the keychain could be read again")
        await hand(saved)
    }

    /// Launch's hand-over to the engine: the saved connection, or none. Nothing when the keychain could not be read.
    func handOverAtLaunch(_ send: @escaping (DropboxConnection?) async -> Void) async {
        if previewing { return }
        if unreadable {
            Log.info("dropbox: not handed over, the keychain could not be read")
            return
        }
        await hand(saved, via: send)
    }

    /// Before the account changes (AccountSpace): a connection being made stops, a refresh in flight ends first, so
    /// a new token always lands in the keychain item of the account it belongs to. Nothing new starts until `reload`.
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
        previewing = false
        generation += 1
        loadSaved()
        lost = false
        lostAt = nil
        renewedAt = nil
        phase = .idle
    }

    /// `--debug-control` and the render tests: a card state without the keychain or Dropbox, until `preview(nil)`
    /// (off), `connect` or `reload`.
    func preview(_ c: DropboxConnection?, lost: Bool = false, unreadable: Bool = false, phase: Phase = .idle) {
        seed(c, lost: lost, unreadable: unreadable, phase: phase)
        previewing = c != nil || lost || unreadable || phase != .idle
    }

    /// Tests: a state as if it came from the keychain, so what is done next is real.
    func seed(_ c: DropboxConnection?, lost: Bool = false, unreadable: Bool = false, phase: Phase = .idle) {
        generation += 1
        saved = c
        self.lost = lost
        lostAt = lost ? now() : nil
        self.unreadable = unreadable
        self.phase = phase
    }

    /// Opens Dropbox's approval page and waits for the browser to come back to 127.0.0.1, then trades the code, asks
    /// whose account it is, keeps it and hands it to the engine.
    func connect() {
        guard !held else { return }
        if previewing {
            // back to the real connection first, so the same account keeps its muted folders and nothing made up stays
            previewing = false
            generation += 1
            loadSaved()
            lost = false
            lostAt = nil
        }
        waitingTask?.cancel()
        let state = NotionConnect.newState()
        let verifier = LinearConnect.newVerifier()
        let patience = patience
        phase = .waiting
        waitingTask = Task {
            do {
                // one redirect for the approval page and the exchange: Dropbox compares them
                let (redirect, reply) = try await listen(state)
                open(api.authorizeURL(redirect: redirect, state: state, challenge: LinearConnect.challenge(verifier)))
                let got = try await withThrowingTaskGroup(of: LoopbackReply.self) { group in
                    group.addTask { try await reply() }
                    group.addTask {
                        try await Task.sleep(for: .seconds(patience))
                        throw DropboxConnectError.expired
                    }
                    defer { group.cancelAll() }
                    return try await group.next()!
                }
                guard case .code(let code) = got else { throw DropboxConnectError.denied }
                let t = try await api.exchange(code: code, verifier: verifier, redirect: redirect)
                var who = try await api.account(token: t.token)
                who.paperLegacy = await api.paperLegacy(token: t.token)
                // the flag swallows a cancel (best effort): a connection cancelled or disconnected meanwhile is not kept
                try Task.checkCancellation()
                // the same account again keeps what was turned off; another account starts with everything on
                let same = saved?.accountID == who.id ? saved : nil
                let c = DropboxConnection(token: t.token, refreshToken: t.refresh,
                                          expiresAt: now().addingTimeInterval(t.expiresIn), accountID: who.id,
                                          account: who.email, mutedFolders: same?.mutedFolders ?? [])
                generation += 1
                save(c)
                saved = c
                lost = false
                lostAt = nil
                unreadable = false
                phase = .idle
                Log.info("dropbox connected")
                await hand(c)
                // a disconnect or cancel that landed during the push must not count as connected
                try Task.checkCancellation()
                onConnected(who)
            } catch is CancellationError {
                // closed by the user: the card goes back quietly
                if !Task.isCancelled { phase = .idle }
            } catch {
                let e = error as? DropboxConnectError ?? .failed
                phase = .failed(Self.words(e))
                Log.info("dropbox connect failed: \(e)")
            }
        }
    }

    func cancel() {
        waitingTask?.cancel()
        waitingTask = nil
        phase = .idle
    }

    /// Forgets the connection here and in the engine first, then ends this grant on Dropbox's side.
    func disconnect() async {
        guard !previewing else { return }
        waitingTask?.cancel()
        waitingTask = nil
        generation += 1
        let old = saved
        forget()
        saved = nil
        lost = false
        lostAt = nil
        unreadable = false
        renewedAt = nil
        phase = .idle
        Log.info("dropbox disconnected")
        await hand(nil)
        if let old { await api.revoke(old) }
    }

    /// 폴더 고르기's turned-off ids to save: those turned off in the list, plus the ones turned off before that the
    /// list does not show now (one gone for a while is not turned back on by saving).
    nonisolated static func muted(shown: [String], on: Set<String>, before: [String]) -> [String] {
        let list = Set(shown)
        return Array(Set(shown.filter { !on.contains($0) }).union(before.filter { !list.contains($0) })).sorted()
    }

    /// Turns top folders off (or back on) and hands the change to the engine.
    func setMuted(_ ids: [String]) async {
        guard !previewing, var c = saved else { return }
        c.mutedFolders = ids.sorted()
        save(c)
        saved = c
        await hand(c)
    }

    /// Refreshes the token if less than `refreshBefore` is left. One at a time.
    func ensureFresh() async {
        await fresh(force: false)
    }

    /// The engine says Dropbox refused the token (it waits mid-fetch for a new one): a forced refresh, at most once a
    /// minute.
    func renew() async {
        guard saved != nil, !lost, !previewing else { return }
        if let at = renewedAt, now().timeIntervalSince(at) < 60 { return }
        renewedAt = now()
        await fresh(force: true)
    }

    private func fresh(force: Bool) async {
        guard !held, !previewing else { return }
        if let running = refreshing {
            await running.value
            return
        }
        // cleared inside the task, so whoever waits on it finds the place free once it is done
        let task = Task {
            await self.refreshNow(force: force)
            self.refreshing = nil
        }
        refreshing = task
        await task.value
    }

    /// Hands `c` to the engine one at a time: a push waits for the one before it, and one whose connection changed
    /// while it waited (connect, disconnect, account change) is dropped. So a disconnect's nil can never be overtaken
    /// by a connect's push still on its way, and the engine's last word is always the link's current state.
    private func hand(_ c: DropboxConnection?, via send: ((DropboxConnection?) async -> Void)? = nil) async {
        guard !previewing else { return }
        let made = generation
        let before = handing
        let task = Task {
            await before?.value
            guard made == self.generation else {
                Log.info("dropbox: a hand-over from before the connection changed, dropped")
                return
            }
            await (send ?? self.push)(c)
        }
        handing = task
        await task.value
    }

    private func save(_ c: DropboxConnection) {
        do { try keep(c) } catch { Log.info("dropbox: the keychain did not take the connection, kept in memory") }
    }

    /// One refresh under the lock. Call through `ensureFresh` or `renew`, which keep it to one at a time here.
    func refreshNow(force: Bool) async {
        guard saved != nil, !lost, !previewing else { return }
        let started = generation
        await lock.run {
            guard var current = self.saved, started == self.generation else { return }
            // another Tacie may have refreshed already: take its token. The refresh token stays the same, so a newer
            // access token that lasts longer is what tells.
            if let disk = try? self.load(), disk.accountID == current.accountID, disk.token != current.token,
               disk.expiresAt > current.expiresAt {
                var merged = current
                merged.token = disk.token
                merged.expiresAt = disk.expiresAt
                self.saved = merged
                current = merged
                if merged.expiresAt.timeIntervalSince(self.now()) >= Self.refreshBefore {
                    await self.hand(merged)
                    return
                }
            }
            guard force || current.expiresAt.timeIntervalSince(self.now()) < Self.refreshBefore else { return }
            do {
                let next = try await self.api.refresh(current)
                guard started == self.generation, var merged = self.saved, merged.accountID == current.accountID else {
                    Log.info("dropbox: a refresh finished after the connection changed, dropped")
                    return
                }
                // only the token fields are new: muted folders may have changed meanwhile
                merged.token = next.token
                merged.expiresAt = self.now().addingTimeInterval(next.expiresIn)
                self.save(merged)
                self.saved = merged
                Log.info("dropbox token renewed")
                await self.hand(merged)
            } catch DropboxConnectError.refused {
                if !self.lost { self.lostAt = self.now() }
                self.lost = true
                Log.info("dropbox refresh refused")
            } catch {
                // the token stays: the same refresh token is asked again on the next tick
                Log.info("dropbox refresh failed: \(error as? DropboxConnectError ?? .failed)")
            }
        }
    }

    static func words(_ e: DropboxConnectError) -> String {
        switch e {
        case .denied: String(localized: "연결하지 않았어요")
        case .expired: String(localized: "시간이 지났어요")
        case .refused: String(localized: "Dropbox 연결이 끊겼어요")
        case .scope: String(localized: "파일 읽기를 허용해야 연결돼요")
        case .network, .failed: String(localized: "연결하지 못했어요")
        }
    }
}
