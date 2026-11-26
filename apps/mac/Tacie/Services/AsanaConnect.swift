import AppKit
import AuthenticationServices
import Foundation
import Observation

/// What Tacie keeps of an Asana connection (keychain `ai.tacie.asana`, as JSON). Never logged.
struct AsanaConnection: Codable, Equatable {
    var token: String
    var refreshToken: String
    var expiresAt: Date
    /// The Asana user the token belongs to (`assignee=me`, and which saved connection is the same one).
    var userGid: String
    var userName: String
    /// Workspaces and projects the user turned off (docs/32 5절). Only these are remembered, so new ones come in on
    /// their own.
    var mutedWorkspaces: [String] = []
    var mutedProjects: [String] = []
    /// 오픈소스판: a personal access token the person pasted (no refresh token, never refreshed through the relay).
    var pasted: Bool?

    /// The body of the engine's `POST /asana/credentials` (memory only, never logged); `token: null` disconnects.
    static func engineBody(_ c: AsanaConnection?) -> [String: Any] {
        [
            "token": c?.token ?? NSNull(), "user_gid": c?.userGid ?? NSNull(),
            "muted_workspaces": c?.mutedWorkspaces ?? [], "muted_projects": c?.mutedProjects ?? [],
        ]
    }
}

/// `blocked`: the approval ended with an error that says an admin keeps Tacie out (`unauthorized_client`).
/// `signedOut`: the relay wants a Tacie session (sign in to Tacie first); the Asana connection itself is fine.
/// `notSent`: the relay answered without asking Asana (its hourly refresh cap, a bad request), so nothing changed.
enum AsanaConnectError: Error, Equatable {
    case denied, blocked, expired, failed, network, refused, signedOut, unready, notSent
}

/// Asana's sign-in through the relay (docs/32 7절). Asana wants the client secret for the first exchange and every
/// refresh, so both go through `auth.tacie.app` with the Tacie session. The app adds PKCE: the challenge goes to
/// `/asana/start`, the verifier only to `/asana/token`, so a code that leaks from the relay cannot be traded alone.
/// The relay keeps no token: the browser comes back to `tacie://asana` with a one-time key.
struct AsanaConnect {
    static let meURL = URL(string: "https://app.asana.com/api/1.0/users/me?opt_fields=gid,name")!
    /// How long the approval page may take (the relay keeps the code five minutes too).
    static let patience: TimeInterval = 300

    /// (body, HTTP status). Throws only when there was no answer.
    var send: (URLRequest) async throws -> (Data, Int) = { req in
        let (data, resp) = try await URLSession.shared.data(for: req)
        return (data, (resp as? HTTPURLResponse)?.statusCode ?? 0)
    }
    /// The Tacie session the relay asks for. Read on each call: the account can change while the app runs.
    var session: () -> String? = { SessionStore.store.read() }
    var now: () -> Date = Date.init

    private var server: AccountServer { AccountServer(send: send, session: session) }

    /// The relay's start, with this sign-in's state and its S256 challenge (`LinearConnect.challenge`).
    func startURL(state: String, challenge: String) -> URL {
        var c = URLComponents(url: AccountServer.base.appendingPathComponent("asana/start"), resolvingAgainstBaseURL: false)!
        c.queryItems = [URLQueryItem(name: "state", value: state), URLQueryItem(name: "challenge", value: challenge)]
        return c.url!
    }

    /// The callback errors that mean the workspace's admin has not let Tacie in (docs/32 5-1절). What Asana sends in
    /// its approval-required mode is not confirmed yet; any other error is a failure to try again.
    static let adminBlocks: Set<String> = ["unauthorized_client"]

    /// The one-time key from `tacie://asana?state=…&k=…`. A return that is not for the sign-in this app started
    /// (another scheme or host, another state) throws `CancellationError`: it is ignored, with nothing on the card.
    func parseReturn(_ url: URL, state: String) throws -> String {
        guard url.scheme == "tacie", url.host == "asana",
              let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
              items.first(where: { $0.name == "state" })?.value == state else { throw CancellationError() }
        let value = { (name: String) in items.first { $0.name == name }?.value }
        switch value("error") {
        case nil: break
        case "denied": throw AsanaConnectError.denied
        // the relay passes Asana's error through (asana.js kindOf): only a known admin block says so
        case let e? where Self.adminBlocks.contains(e): throw AsanaConnectError.blocked
        // Asana's own trouble or anything not known: try again
        default: throw AsanaConnectError.failed
        }
        guard let k = value("k"), !k.isEmpty else { throw AsanaConnectError.failed }
        return k
    }

    /// The first token pair: the relay trades the parked code with our secret and the verifier, answering only here.
    func token(state: String, k: String, verifier: String) async throws -> (token: String, refresh: String, expiresIn: TimeInterval) {
        let (data, status) = try await relay("asana/token", ["state": state, "k": k, "verifier": verifier])
        let t = try answer(data, status: status)
        guard let refresh = t.refresh else { throw AsanaConnectError.failed }
        return (t.token, refresh, t.expiresIn)
    }

    /// A new access token for the saved refresh token. Asana does not rotate the refresh token, so `refresh` is nil
    /// unless Asana sent a new one (then it replaces the saved one). Only the relay's `refused` (Asana's
    /// `invalid_grant`) means the connection is gone; `signed_out` is about the Tacie session, not this connection.
    func refresh(_ c: AsanaConnection) async throws -> (token: String, refresh: String?, expiresIn: TimeInterval) {
        let (data, status) = try await relay("asana/refresh", ["refresh_token": c.refreshToken])
        return try answer(data, status: status)
    }

    /// Who the token belongs to, asked of Asana directly.
    func me(token: String) async throws -> (gid: String, name: String) {
        var req = URLRequest(url: Self.meURL)
        req.httpMethod = "GET"
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        req.timeoutInterval = 20
        if Task.isCancelled { throw CancellationError() }
        let data: Data, status: Int
        do { (data, status) = try await send(req) } catch {
            if (error as? URLError)?.code == .cancelled || error is CancellationError { throw CancellationError() }
            throw AsanaConnectError.network
        }
        let user = ((try? JSONSerialization.jsonObject(with: data)) as? [String: Any])?["data"] as? [String: Any]
        guard (200..<300).contains(status), let gid = user?["gid"] as? String, !gid.isEmpty else {
            throw AsanaConnectError.failed
        }
        return (gid, user?["name"] as? String ?? "")
    }

    private func relay(_ path: String, _ body: [String: String]) async throws -> (Data, Int) {
        if Task.isCancelled { throw CancellationError() }
        do { return try await server.authorized(path, body: body) } catch AccountError.signedOut {
            throw AsanaConnectError.signedOut
        } catch {
            if Task.isCancelled { throw CancellationError() }
            throw AsanaConnectError.network
        }
    }

    private func answer(_ data: Data, status: Int) throws -> (token: String, refresh: String?, expiresIn: TimeInterval) {
        let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        let error = obj?["error"] as? String
        switch status {
        case 200..<300: break
        case 401: throw error == "refused" ? AsanaConnectError.refused : .signedOut
        case 400 where error == "expired": throw AsanaConnectError.expired
        case 503: throw AsanaConnectError.unready
        case 429: throw AsanaConnectError.notSent  // busy: the relay's hourly cap
        case 400 where error == "bad_request": throw AsanaConnectError.notSent
        default: throw AsanaConnectError.failed  // 502 failed
        }
        guard let token = obj?["token"] as? String, !token.isEmpty else { throw AsanaConnectError.failed }
        let refresh = (obj?["refresh_token"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        return (token, refresh, (obj?["expires_in"] as? Double) ?? 3600)
    }
}

/// The keychain item of the Asana connection.
enum AsanaStore {
    static let service = "ai.tacie.asana"

    /// The account in use (AccountSpace): `connection` signed out, `connection@<id>` signed in.
    static var store: KeychainTokenStore {
        KeychainTokenStore(service: service, account: AccountSpace.inUse.key("connection"),
                           label: String(localized: "Tacie Asana 연결"))
    }

    /// nil when there is no item (or it does not decode); throws when the keychain could not answer (locked,
    /// refused), so a failed read is never taken for "not connected".
    static func load(from store: any TokenStore = store) throws -> AsanaConnection? {
        guard let text = try store.readChecked(), let data = text.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(AsanaConnection.self, from: data)
    }

    static func save(_ c: AsanaConnection) throws {
        try store.save(String(decoding: try JSONEncoder().encode(c), as: UTF8.self))
    }

    static func delete() throws { try store.delete() }
}

/// The Asana connection as the cards show it (docs/32 8절). The app is the only place that refreshes the token (one
/// hour): when less than `refreshBefore` is left (the 30-minute timer, launch), when a refresh was cut short (the
/// marker), and when the engine says Asana refused the token. One refresh at a time, also across processes (the
/// lock), and a newer token another Tacie saved is taken instead. Asana does not rotate the refresh token, so a lost
/// answer is simply asked again with the same one.
@Observable
@MainActor
final class AsanaLink {
    enum Phase: Equatable {
        case idle
        case waiting  // the approval page is open in the browser
        case failed(String)  // what the card says, with 다시 연결
    }

    /// More than the keeper's 30-minute timer, so a token never runs out between two ticks.
    static let refreshBefore: TimeInterval = 40 * 60

    private(set) var phase: Phase = .idle
    private(set) var saved: AsanaConnection?
    /// Asana refused the refresh token: connect again. Found items stay searchable for seven days (docs/32 5-1절).
    private(set) var lost = false
    /// When this app saw the refusal: the card's seven days count from it until the engine reports its `lost_since`.
    private(set) var lostAt: Date?
    /// The relay wanted a Tacie session for the last refresh: the card asks to sign in to Tacie. Not a lost connection.
    private(set) var signedOut = false
    var connected: Bool { saved != nil }
    /// The keychain could not be read (locked, refused): whether there is a connection is unknown, so launch hands
    /// the engine nothing rather than "none", which would drop the found items.
    private(set) var unreadable = false
    var userName: String? { saved?.userName }

    var api = AsanaConnect()
    var now: () -> Date = Date.init
    /// Set by IndexKeeper: hands a connection (nil: none) to the engine.
    var push: (AsanaConnection?) async -> Void = { _ in }
    /// Called once a connection is made (analytics, set by IndexKeeper).
    var onConnected: () -> Void = {}
    /// 오픈소스판: asks the service whether a pasted token still works (tests replace it).
    var checkPasted: (AsanaConnection) async -> PastedTokens.Check = { await PastedTokens.check(.asana, token: $0.token) }
    var load: () throws -> AsanaConnection? = { try AsanaStore.load() }
    var keep: (AsanaConnection) throws -> Void = { try AsanaStore.save($0) }
    var forget: () -> Void = { try? AsanaStore.delete() }
    var lock = RefreshLock.named("asana")
    var marker = RefreshMarker.named("asana")
    /// Shows the approval page and answers the `tacie://` address it ends at (tests replace it).
    var authenticate: (URL) async throws -> URL = { url in try await WebSignIn().run(url) }

    private(set) var waitingTask: Task<Void, Never>?
    private var refreshing: Task<Void, Never>?
    /// A new token whose keychain write failed: kept in memory, written again on the next call.
    private var unsaved: AsanaConnection?
    private var renewedAt: Date?
    /// Bumped by connect and disconnect; a refresh started before drops its result.
    private var generation = 0
    /// The account is changing (`settle` → `reload`): no connection or refresh starts.
    private var held = false
    /// Waits before a retry; false when cancelled (tests answer at once and move `now`).
    var wait: (TimeInterval) async -> Bool = { s in (try? await Task.sleep(for: .seconds(s))) != nil }
    /// A refresh whose answer was lost (`failed`, no answer) is tried again: up to `retryLimit` times, `retryEvery`
    /// apart, inside `retryWindow` from the first try. After that the marker makes the timer or the next launch try.
    static let retryEvery: TimeInterval = 60
    static let retryLimit = 3
    static let retryWindow: TimeInterval = 600
    private(set) var retryTask: Task<Void, Never>?
    private var retryFrom: Date?
    private var retries = 0
    /// Which scheduled retry is the current one (a retry that fails schedules the next while it still runs).
    private var retryID = 0

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
            Log.info("asana: the keychain could not be read (\((error as? KeychainTokenStore.Failure).map { "status \($0.status)" } ?? "other"))")
        }
    }

    /// The 30-minute timer: a keychain that could not be read is read again; once it answers, what it holds (or
    /// none) goes to the engine like at launch.
    func retryUnreadable() async {
        guard unreadable, !held else { return }
        loadSaved()
        guard !unreadable else { return }
        Log.info("asana: the keychain could be read again")
        await push(saved)
    }

    /// Launch's hand-over to the engine: the saved connection, or none. Nothing when the keychain could not be read.
    func handOverAtLaunch(_ send: (AsanaConnection?) async -> Void) async {
        if unreadable {
            Log.info("asana: not handed over, the keychain could not be read")
            return
        }
        await send(saved)
    }

    /// Before the account changes (AccountSpace): a connection being made stops, a refresh in flight ends first, so
    /// a new token always lands in the keychain item of the account it belongs to.
    /// From here until `reload` nothing new starts, and a token that only lived in memory gets one more try.
    func settle() async {
        held = true
        endRetries()
        waitingTask?.cancel()
        await waitingTask?.value
        waitingTask = nil
        await refreshing?.value
        if let pending = unsaved {
            save(pending)
            if unsaved == nil { marker.set(false) } else { Log.info("asana: the keychain still refused the new token") }
        }
    }

    /// After the account changed: the new account's connection, or none.
    func reload() {
        held = false
        generation += 1
        loadSaved()
        unsaved = nil
        lost = false
        lostAt = nil
        signedOut = false
        renewedAt = nil
        endRetries()
        phase = .idle
    }

    /// A connection without the keychain (tests). Counts as a new connection.
    func setForTesting(_ c: AsanaConnection?) {
        generation += 1
        saved = c
    }

    /// `--debug-control` and the render tests: a card state without the keychain or Asana.
    func preview(_ c: AsanaConnection?, lost: Bool = false, signedOut: Bool = false, phase: Phase = .idle) {
        generation += 1
        saved = c
        self.lost = lost
        lostAt = lost ? now() : nil
        self.signedOut = signedOut
        self.phase = phase
    }

    /// Opens Asana's approval page through the relay and waits for `tacie://asana` with this sign-in's state, then
    /// trades the one-time key and the PKCE verifier for the token pair, asks who the user is, keeps it and hands it
    /// to the engine.
    func connect() {
        guard !held else { return }
        waitingTask?.cancel()
        let state = NotionConnect.newState()
        let verifier = LinearConnect.newVerifier()
        let start = api.startURL(state: state, challenge: LinearConnect.challenge(verifier))
        let authenticate = authenticate
        phase = .waiting
        waitingTask = Task {
            do {
                let back = try await withThrowingTaskGroup(of: URL.self) { group in
                    group.addTask { try await authenticate(start) }
                    group.addTask {
                        try await Task.sleep(for: .seconds(AsanaConnect.patience))
                        throw AsanaConnectError.expired
                    }
                    defer { group.cancelAll() }
                    return try await group.next()!
                }
                let k = try api.parseReturn(back, state: state)
                let t = try await api.token(state: state, k: k, verifier: verifier)
                let me = try await api.me(token: t.token)
                // the same user again keeps what was turned off; another user starts with everything on
                let same = saved?.userGid == me.gid ? saved : nil
                let c = AsanaConnection(
                    token: t.token, refreshToken: t.refresh, expiresAt: now().addingTimeInterval(t.expiresIn),
                    userGid: me.gid, userName: me.name,
                    mutedWorkspaces: same?.mutedWorkspaces ?? [], mutedProjects: same?.mutedProjects ?? [])
                generation += 1
                endRetries()
                save(c)
                marker.set(false)
                saved = c
                lost = false
                lostAt = nil
                signedOut = false
                phase = .idle
                Log.info("asana connected")
                onConnected()
                await push(c)
            } catch is CancellationError {
                // closed by the user, or a return that was not ours: the card goes back quietly
                if !Task.isCancelled { phase = .idle }
            } catch {
                let e = error as? AsanaConnectError ?? .failed
                phase = .failed(Self.words(e))
                Log.info("asana connect failed: \(e)")
            }
        }
    }

    func cancel() {
        waitingTask?.cancel()
        waitingTask = nil
        phase = .idle
    }

    /// 오픈소스판: a personal access token the person pasted (checked already, `PastedTokens`, which found the user). The
    /// same user again keeps what was turned off, like a sign-in.
    func connectPasted(token: String, found: PastedTokens.Found) async {
        guard !held else { return }
        guard let gid = found.id else { return }
        waitingTask?.cancel()
        let same = saved?.userGid == gid ? saved : nil
        var c = AsanaConnection(
            token: token, refreshToken: "", expiresAt: .distantFuture, userGid: gid, userName: found.name ?? "",
            mutedWorkspaces: same?.mutedWorkspaces ?? [], mutedProjects: same?.mutedProjects ?? [])
        c.pasted = true
        generation += 1
        endRetries()
        save(c)
        marker.set(false)
        saved = c
        lost = false
        lostAt = nil
        signedOut = false
        renewedAt = nil
        phase = .idle
        Log.info("asana connected with a pasted token")
        onConnected()
        await push(c)
    }

    /// Forgets the connection here and in the engine only. Asana's revoke is never called: it would end the grant on
    /// every Mac signed in to this Asana account (docs/32 0절, 10-1 ⑮). The card points at Asana's app settings.
    func disconnect() async {
        waitingTask?.cancel()
        generation += 1
        endRetries()
        forget()
        marker.set(false)
        saved = nil
        unsaved = nil
        lost = false
        lostAt = nil
        signedOut = false
        phase = .idle
        Log.info("asana disconnected")
        await push(nil)
    }

    /// 워크스페이스·프로젝트 고르기's turned-off ids to save: those turned off in the list, plus the ones turned off
    /// before that the list does not show now (one gone for a while is not turned back on by saving).
    nonisolated static func muted(shown: [String], on: Set<String>, before: [String]) -> [String] {
        let list = Set(shown)
        return Array(Set(shown.filter { !on.contains($0) }).union(before.filter { !list.contains($0) })).sorted()
    }

    /// Turns workspaces and projects off (or back on) and hands the change to the engine.
    func setMuted(workspaces: [String], projects: [String]) async {
        guard var c = unsaved ?? (try? load()) ?? saved else { return }
        if let s = saved, c.userGid != s.userGid { c = s }
        c.mutedWorkspaces = workspaces.sorted()
        c.mutedProjects = projects.sorted()
        save(c)
        saved = c
        await push(c)
    }

    /// Refreshes the token if less than `refreshBefore` is left (or a refresh was cut short). One at a time.
    func ensureFresh() async {
        await fresh(force: false)
    }

    /// The engine says Asana refused the token (it waits mid-fetch for a new one): a forced refresh, at most once a
    /// minute.
    func renew() async {
        guard saved != nil, !lost else { return }
        if let at = renewedAt, now().timeIntervalSince(at) < 60 { return }
        renewedAt = now()
        // a pasted token has nothing to refresh: ask the service once, and only a refusal asks for a new one (one
        // refused request may be a single query's permission, not the token)
        if let c = saved, c.pasted == true {
            if case .rejected = await checkPasted(c) {
            lost = true
            lostAt = now()
                Log.info("asana refused the pasted token")
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
        // cleared inside the task, so whoever waits on it finds the place free once it is done
        let task = Task {
            await self.refreshNow(force: force)
            self.refreshing = nil
        }
        refreshing = task
        await task.value
    }

    private func save(_ c: AsanaConnection) {
        do {
            try keep(c)
            unsaved = nil
        } catch {
            unsaved = c
            Log.info("asana: the keychain did not take the connection, kept in memory")
        }
    }

    /// One refresh under the lock. Call through `ensureFresh` or `renew`, which keep it to one at a time here.
    func refreshNow(force: Bool) async {
        if let pending = unsaved {
            save(pending)
            if unsaved == nil { marker.set(false) }
            return
        }
        guard saved != nil, !lost, saved?.pasted != true else { return }
        let started = generation
        await lock.run {
            guard var current = self.saved, started == self.generation else { return }
            // another Tacie may have refreshed already: take its token. The refresh token stays the same, so a newer
            // access token that lasts longer is what tells.
            if let disk = try? self.load(), disk.userGid == current.userGid, disk.token != current.token,
               disk.expiresAt > current.expiresAt {
                self.saved = disk
                current = disk
                if disk.expiresAt.timeIntervalSince(self.now()) >= Self.refreshBefore && !self.marker.isSet() {
                    await self.push(disk)
                    return
                }
            }
            let due = current.expiresAt.timeIntervalSince(self.now()) < Self.refreshBefore
            guard force || due || self.marker.isSet() else { return }
            self.marker.set(true)
            let askedAt = self.now()
            do {
                let next = try await self.api.refresh(current)
                guard started == self.generation, self.saved?.refreshToken == current.refreshToken else {
                    Log.info("asana: a refresh finished after the connection changed, dropped")
                    return
                }
                self.endRetries()
                // only the token fields are new: muted workspaces and projects may have changed meanwhile
                var merged = self.saved ?? current
                merged.token = next.token
                // kept unless Asana sent a new one
                if let refresh = next.refresh { merged.refreshToken = refresh }
                merged.expiresAt = self.now().addingTimeInterval(next.expiresIn)
                self.save(merged)
                if self.unsaved == nil { self.marker.set(false) }
                self.saved = merged
                self.signedOut = false
                Log.info("asana token renewed")
                await self.push(merged)
            } catch AsanaConnectError.refused {
                self.endRetries()
                self.marker.set(false)
                if !self.lost { self.lostAt = self.now() }
                self.lost = true
                Log.info("asana refresh refused")
            } catch AsanaConnectError.signedOut {
                // the relay stopped before Asana: nothing changed
                self.endRetries()
                self.marker.set(false)
                self.signedOut = true
                Log.info("asana refresh needs a Tacie session")
            } catch let e as AsanaConnectError where e == .notSent || e == .unready {
                // the relay answered without asking Asana: the timer tries later
                self.endRetries()
                self.marker.set(false)
                Log.info("asana refresh not sent: \(e)")
            } catch {
                // the token stays and the marker too: the same refresh token is asked again shortly
                Log.info("asana refresh failed: \(error as? AsanaConnectError ?? .failed)")
                self.retryLater(from: askedAt, started: started)
            }
        }
    }

    /// Schedules the next try of a refresh that failed, if the retry window still has room for it.
    private func retryLater(from askedAt: Date, started: Int) {
        let from = retryFrom ?? askedAt
        retryFrom = from
        guard retries < Self.retryLimit,
              now().addingTimeInterval(Self.retryEvery).timeIntervalSince(from) < Self.retryWindow else {
            Log.info("asana: no more refresh retries, the marker stays for the timer or the next launch")
            endRetries()
            return
        }
        retryTask?.cancel()
        retryID += 1
        let id = retryID
        retryTask = Task { [weak self, wait] in
            guard await wait(Self.retryEvery), let self else { return }
            guard started == self.generation, self.marker.isSet(), !self.lost else {
                self.endRetries()
                return
            }
            self.retries += 1
            // the failed refresh may still be finishing (its own task scheduled this one)
            if let running = self.refreshing { await running.value }
            await self.fresh(force: true)
            // kept until the retry's refresh is done, unless that refresh scheduled the next one
            if self.retryID == id { self.retryTask = nil }
        }
    }

    private func endRetries() {
        retryTask?.cancel()
        retryTask = nil
        retryFrom = nil
        retries = 0
    }

    static func words(_ e: AsanaConnectError) -> String {
        switch e {
        case .denied: String(localized: "연결하지 않았어요")
        case .expired: String(localized: "시간이 지났어요")
        case .blocked: String(localized: "회사 Asana 관리자가 Tacie를 허용해야 연결돼요")
        case .refused: String(localized: "Asana 연결이 끊겼어요")
        case .signedOut: SettingsText.atlassianSignedOut
        case .unready: String(localized: "Asana 연결은 아직 준비 중이에요")
        case .network, .failed, .notSent: String(localized: "연결하지 못했어요")
        }
    }
}
