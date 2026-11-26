import AppKit
import AuthenticationServices
import Foundation
import Observation

/// What Tacie keeps of a Jira · Confluence connection (keychain `ai.tacie.atlassian`, as JSON). Never logged.
struct AtlassianConnection: Codable, Equatable {
    var token: String
    var refreshToken: String
    var expiresAt: Date
    /// The site's cloud id: the engine's paths and API addresses use it.
    var cloud: String
    var site: String  // the site's name ("acme")
    var siteURL: String  // https://acme.atlassian.net
    /// Projects and spaces the user turned off (docs/31 5절 6). Only these are remembered, so new ones come in on their own.
    var mutedProjects: [String] = []
    var mutedSpaces: [String] = []
    /// 오픈소스판: an API token the person pasted, with the login email it belongs to (no refresh token; the engine calls
    /// the site with Basic auth).
    var pasted: Bool?
    var email: String?

    /// The body of the engine's `POST /atlassian/credentials` (memory only, never logged); `token: null` disconnects.
    static func engineBody(_ c: AtlassianConnection?) -> [String: Any] {
        var body: [String: Any] = [
            "token": c?.token ?? NSNull(), "cloud": c?.cloud ?? NSNull(), "site": c?.site ?? NSNull(),
            "site_url": c?.siteURL ?? NSNull(), "muted_projects": c?.mutedProjects ?? [],
            "muted_spaces": c?.mutedSpaces ?? [],
        ]
        if let email = c?.email { body["email"] = email }
        return body
    }
}

/// `blocked`: the approval ended with an error other than the user's cancel, an admin's block among them (docs/31 5-1절).
/// `signedOut`: the relay wants a Tacie session (sign in to Tacie first); the Atlassian connection itself is fine.
/// `notSent`: the relay answered without asking Atlassian (its hourly refresh cap, a bad request), so the pair did not
/// change.
enum AtlassianConnectError: Error, Equatable {
    case denied, blocked, expired, failed, network, refused, signedOut, unready, notSent
}

/// Atlassian's sign-in through the relay (docs/31 7절, 10-2절). Atlassian wants the client secret for the first
/// exchange and every refresh, so both go through `auth.tacie.app` with the Tacie session. The relay keeps no token:
/// the browser comes back to `tacie://atlassian` with a one-time key, and the token answers only that request.
struct AtlassianConnect {
    static let resourcesURL = URL(string: "https://api.atlassian.com/oauth/token/accessible-resources")!
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

    func startURL(state: String) -> URL {
        var c = URLComponents(url: AccountServer.base.appendingPathComponent("atlassian/start"), resolvingAgainstBaseURL: false)!
        c.queryItems = [URLQueryItem(name: "state", value: state)]
        return c.url!
    }

    /// The one-time key from `tacie://atlassian?state=…&k=…`. A return that is not for the sign-in this app started
    /// (another scheme or host, another state) throws `CancellationError`: it is ignored, with nothing on the card.
    func parseReturn(_ url: URL, state: String) throws -> String {
        let value = try WebSignIn.returned(url, host: "atlassian", state: state)
        switch value("error") {
        case nil: break
        case "denied": throw AtlassianConnectError.denied
        // Atlassian's own trouble, not a refusal: try again
        case "failed", "server_error", "temporarily_unavailable": throw AtlassianConnectError.failed
        default: throw AtlassianConnectError.blocked
        }
        guard let k = value("k"), !k.isEmpty else { throw AtlassianConnectError.failed }
        return k
    }

    /// The first token pair: the relay trades the parked code with our secret and answers only here.
    func token(state: String, k: String) async throws -> (token: String, refresh: String, expiresIn: TimeInterval) {
        let (data, status) = try await relay("atlassian/token", ["state": state, "k": k])
        return try pair(data, status: status, keeping: nil)
    }

    /// A new pair for the saved one (Atlassian rotates the refresh token). Only the relay's `refused` (Atlassian's
    /// `invalid_grant`) means the connection is gone; `signed_out` is about the Tacie session, not this connection.
    func refresh(_ c: AtlassianConnection) async throws -> (token: String, refresh: String, expiresIn: TimeInterval) {
        let (data, status) = try await relay("atlassian/refresh", ["refresh_token": c.refreshToken])
        return try pair(data, status: status, keeping: c.refreshToken)
    }

    /// The site the token reaches. Atlassian answers one row per product of a site (Jira, Confluence), so rows are
    /// merged by id; the first site is the one Tacie reads.
    func resources(token: String) async throws -> (cloud: String, site: String, siteURL: String) {
        var req = URLRequest(url: Self.resourcesURL)
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        req.timeoutInterval = 20
        let (data, status) = try await answer(req)
        let rows = (try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]] ?? []
        var order: [String] = []
        var sites: [String: (name: String, url: String)] = [:]
        for row in rows {
            guard let id = row["id"] as? String, !id.isEmpty else { continue }
            if sites[id] == nil { order.append(id) }
            let old = sites[id]
            let name = (row["name"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? old?.name ?? ""
            let url = (row["url"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? old?.url ?? ""
            sites[id] = (name, url)
        }
        guard (200..<300).contains(status), let id = order.first, let s = sites[id], !s.url.isEmpty else {
            throw AtlassianConnectError.failed
        }
        return (id, s.name.isEmpty ? s.url : s.name, s.url)
    }

    private func relay(_ path: String, _ body: [String: String]) async throws -> (Data, Int) {
        if Task.isCancelled { throw CancellationError() }
        do { return try await server.authorized(path, body: body) } catch AccountError.signedOut {
            throw AtlassianConnectError.signedOut
        } catch {
            if Task.isCancelled { throw CancellationError() }
            throw AtlassianConnectError.network
        }
    }

    private func pair(_ data: Data, status: Int, keeping old: String?) throws -> (token: String, refresh: String, expiresIn: TimeInterval) {
        let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        let error = obj?["error"] as? String
        switch status {
        case 200..<300: break
        case 401: throw error == "refused" ? AtlassianConnectError.refused : .signedOut
        case 400 where error == "expired": throw AtlassianConnectError.expired
        case 503: throw AtlassianConnectError.unready
        case 429: throw AtlassianConnectError.notSent  // busy: the relay's hourly cap
        case 400 where error == "bad_request": throw AtlassianConnectError.notSent
        default: throw AtlassianConnectError.failed  // 502 failed: Atlassian may have rotated the pair
        }
        guard let token = obj?["token"] as? String, !token.isEmpty,
              let refresh = (obj?["refresh_token"] as? String).flatMap({ $0.isEmpty ? nil : $0 }) ?? old else {
            throw AtlassianConnectError.failed
        }
        return (token, refresh, (obj?["expires_in"] as? Double) ?? 3600)
    }

    private func answer(_ req: URLRequest) async throws -> (Data, Int) {
        if Task.isCancelled { throw CancellationError() }
        do { return try await send(req) } catch {
            if (error as? URLError)?.code == .cancelled || error is CancellationError { throw CancellationError() }
            throw AtlassianConnectError.network
        }
    }
}

/// The keychain item of the Jira · Confluence connection.
enum AtlassianStore {
    /// The account in use (AccountSpace): `connection` signed out, `connection@<id>` signed in.
    static var store: KeychainTokenStore {
        KeychainTokenStore(service: "ai.tacie.atlassian", account: AccountSpace.inUse.key("connection"),
                           label: String(localized: "Tacie Jira · Confluence 연결"))
    }

    /// nil when there is no item (or it does not decode); throws when the keychain could not answer (locked,
    /// refused), so a failed read is never taken for "not connected".
    static func load(from store: any TokenStore = store) throws -> AtlassianConnection? {
        guard let text = try store.readChecked(), let data = text.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(AtlassianConnection.self, from: data)
    }

    static func save(_ c: AtlassianConnection) throws {
        try store.save(String(decoding: try JSONEncoder().encode(c), as: UTF8.self))
    }

    static func delete() throws { try store.delete() }
}

/// The Jira · Confluence connection as the cards show it (docs/31 8절). The app is the only place that refreshes the
/// token (one hour, and Atlassian rotates the refresh token): when less than `refreshBefore` is left (the 30-minute
/// timer, launch), when a refresh was cut short (the marker), and when the engine says Atlassian refused the token.
/// One refresh at a time, also across processes (the lock), and a newer pair another Tacie saved is taken instead.
@Observable
@MainActor
final class AtlassianLink {
    enum Phase: Equatable {
        case idle
        case waiting  // the approval page is open in the browser
        case failed(String)  // what the card says, with 다시 연결
    }

    /// More than the keeper's 30-minute timer, so a token never runs out between two ticks (docs/31 10-2절).
    static let refreshBefore: TimeInterval = 40 * 60

    private(set) var phase: Phase = .idle
    private(set) var saved: AtlassianConnection?
    /// Atlassian refused the refresh token: connect again. Found items stay searchable for seven days (docs/31 5-1절).
    private(set) var lost = false
    /// The relay wanted a Tacie session for the last refresh: the card asks to sign in to Tacie. Not a lost connection.
    private(set) var signedOut = false
    var connected: Bool { saved != nil }
    /// The keychain could not be read (locked, refused): whether there is a connection is unknown, so launch hands
    /// the engine nothing rather than "none", which would drop the found items.
    private(set) var unreadable = false
    var site: String? { saved?.site }

    var api = AtlassianConnect()
    var now: () -> Date = Date.init
    /// Set by IndexKeeper: hands a connection (nil: none) to the engine.
    var push: (AtlassianConnection?) async -> Void = { _ in }
    /// Called once a connection is made (analytics, set by IndexKeeper).
    var onConnected: () -> Void = {}
    /// 오픈소스판: asks the service whether a pasted token still works (tests replace it).
    var checkPasted: (AtlassianConnection) async -> PastedTokens.Check = { await PastedTokens.check(.atlassian, token: $0.token, email: $0.email, site: $0.siteURL) }
    var load: () throws -> AtlassianConnection? = { try AtlassianStore.load() }
    var keep: (AtlassianConnection) throws -> Void = { try AtlassianStore.save($0) }
    var forget: () -> Void = { try? AtlassianStore.delete() }
    var lock = RefreshLock.named("atlassian")
    var marker = RefreshMarker.named("atlassian")
    /// Shows the approval page and answers the `tacie://` address it ends at (tests replace it).
    var authenticate: (URL) async throws -> URL = { url in try await WebSignIn().run(url) }

    private(set) var waitingTask: Task<Void, Never>?
    private var refreshing: Task<Void, Never>?
    /// A new pair whose keychain write failed: kept in memory, written again on the next call.
    private var unsaved: AtlassianConnection?
    private var renewedAt: Date?
    /// Bumped by connect and disconnect; a refresh started before drops its result.
    private var generation = 0
    /// The account is changing (`settle` → `reload`): no connection or refresh starts.
    private var held = false
    /// Waits before a retry; false when cancelled (tests answer at once and move `now`).
    var wait: (TimeInterval) async -> Bool = { s in (try? await Task.sleep(for: .seconds(s))) != nil }
    /// A refresh whose answer was lost is tried again inside Atlassian's 10-minute reuse window (docs/31 7절):
    /// up to `retryLimit` times, `retryEvery` apart, while the window from the first try lasts.
    static let retryEvery: TimeInterval = 60
    static let retryLimit = 3
    static let reuseWindow: TimeInterval = 600
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
            Log.info("atlassian: the keychain could not be read (\((error as? KeychainTokenStore.Failure).map { "status \($0.status)" } ?? "other"))")
        }
    }

    /// The 30-minute timer: a keychain that could not be read is read again; once it answers, what it holds (or
    /// none) goes to the engine like at launch.
    func retryUnreadable() async {
        guard unreadable, !held else { return }
        loadSaved()
        guard !unreadable else { return }
        Log.info("atlassian: the keychain could be read again")
        await push(saved)
    }

    /// Launch's hand-over to the engine: the saved connection, or none. Nothing when the keychain could not be read.
    func handOverAtLaunch(_ send: (AtlassianConnection?) async -> Void) async {
        if unreadable {
            Log.info("atlassian: not handed over, the keychain could not be read")
            return
        }
        await send(saved)
    }

    /// Before the account changes (AccountSpace): a connection being made stops, a refresh in flight ends first, so
    /// a rotated pair always lands in the keychain item of the account it belongs to.
    /// From here until `reload` nothing new starts, and a pair that only lived in memory gets one more try.
    func settle() async {
        held = true
        endRetries()
        waitingTask?.cancel()
        await waitingTask?.value
        waitingTask = nil
        await refreshing?.value
        if let pending = unsaved {
            save(pending)
            if unsaved == nil { marker.set(false) } else { Log.info("atlassian: the keychain still refused the new pair") }
        }
    }

    /// After the account changed: the new account's connection, or none.
    func reload() {
        held = false
        generation += 1
        loadSaved()
        unsaved = nil
        lost = false
        signedOut = false
        renewedAt = nil
        endRetries()
        phase = .idle
    }

    /// A connection without the keychain (tests). Counts as a new connection.
    func setForTesting(_ c: AtlassianConnection?) {
        generation += 1
        saved = c
    }

    /// `--debug-control`'s `atlassian-preview` and the render tests: a card state without the keychain or Atlassian.
    func preview(_ c: AtlassianConnection?, lost: Bool = false, signedOut: Bool = false, phase: Phase = .idle) {
        generation += 1
        saved = c
        self.lost = lost
        self.signedOut = signedOut
        self.phase = phase
    }

    /// Opens Atlassian's approval page through the relay and waits for `tacie://atlassian` with this sign-in's state,
    /// then trades the one-time key for the token pair, finds the site, keeps it and hands it to the engine.
    func connect() {
        guard !held else { return }
        waitingTask?.cancel()
        let state = NotionConnect.newState()
        let start = api.startURL(state: state)
        let authenticate = authenticate
        phase = .waiting
        waitingTask = Task {
            do {
                let back = try await WebSignIn.within(AtlassianConnect.patience, expired: AtlassianConnectError.expired) {
                    try await authenticate(start)
                }
                let k = try api.parseReturn(back, state: state)
                let t = try await api.token(state: state, k: k)
                let place = try await api.resources(token: t.token)
                // the same site again keeps what was turned off; another site starts with everything on
                let same = saved?.cloud == place.cloud ? saved : nil
                let c = AtlassianConnection(
                    token: t.token, refreshToken: t.refresh, expiresAt: now().addingTimeInterval(t.expiresIn),
                    cloud: place.cloud, site: place.site, siteURL: place.siteURL,
                    mutedProjects: same?.mutedProjects ?? [], mutedSpaces: same?.mutedSpaces ?? [])
                generation += 1
                endRetries()
                save(c)
                marker.set(false)
                saved = c
                lost = false
                signedOut = false
                phase = .idle
                Log.info("atlassian connected")
                onConnected()
                await push(c)
            } catch is CancellationError {
                // closed by the user, or a return that was not ours: the card goes back quietly
                if !Task.isCancelled { phase = .idle }
            } catch {
                let e = error as? AtlassianConnectError ?? .failed
                phase = .failed(Self.words(e))
                Log.info("atlassian connect failed: \(e)")
            }
        }
    }

    func cancel() {
        waitingTask?.cancel()
        waitingTask = nil
        phase = .idle
    }

    /// 오픈소스판: an API token and the login email the person pasted (checked already, `PastedTokens`, which found the
    /// site's cloud id). The same site again keeps what was turned off, like a sign-in.
    func connectPasted(token: String, email: String, found: PastedTokens.Found) async {
        guard !held else { return }
        guard let cloud = found.cloud, let siteURL = found.siteURL else { return }
        waitingTask?.cancel()
        let same = saved?.cloud == cloud ? saved : nil
        var c = AtlassianConnection(
            token: token, refreshToken: "", expiresAt: .distantFuture, cloud: cloud,
            site: found.name ?? siteURL, siteURL: siteURL,
            mutedProjects: same?.mutedProjects ?? [], mutedSpaces: same?.mutedSpaces ?? [])
        c.pasted = true
        c.email = email
        generation += 1
        endRetries()
        save(c)
        marker.set(false)
        saved = c
        lost = false
        signedOut = false
        renewedAt = nil
        phase = .idle
        Log.info("atlassian connected with a pasted token")
        onConnected()
        await push(c)
    }

    /// Forgets the connection here and in the engine. Atlassian has no revoke API: the card points at the account's
    /// connected apps (docs/31 5절 7).
    func disconnect() async {
        waitingTask?.cancel()
        generation += 1
        endRetries()
        forget()
        marker.set(false)
        saved = nil
        unsaved = nil
        lost = false
        signedOut = false
        phase = .idle
        Log.info("atlassian disconnected")
        await push(nil)
    }

    /// 프로젝트·스페이스 고르기's turned-off ids to save: those turned off in the list, plus the ones turned off before
    /// that the list does not show now (a project or space gone for a while is not turned back on by saving).
    nonisolated static func muted(shown: [String], on: Set<String>, before: [String]) -> [String] {
        let list = Set(shown)
        return Array(Set(shown.filter { !on.contains($0) }).union(before.filter { !list.contains($0) })).sorted()
    }

    /// Turns projects and spaces off (or back on) and hands the change to the engine.
    func setMuted(projects: [String], spaces: [String]) async {
        guard var c = unsaved ?? (try? load()) ?? saved else { return }
        if let s = saved, c.cloud != s.cloud { c = s }
        c.mutedProjects = projects.sorted()
        c.mutedSpaces = spaces.sorted()
        save(c)
        saved = c
        await push(c)
    }

    /// Refreshes the token if less than `refreshBefore` is left (or a refresh was cut short). One at a time.
    func ensureFresh() async {
        await fresh(force: false)
    }

    /// The engine says Atlassian refused the token (it waits mid-fetch for a new one): a forced refresh, at most once
    /// a minute.
    func renew() async {
        guard saved != nil, !lost else { return }
        if let at = renewedAt, now().timeIntervalSince(at) < 60 { return }
        renewedAt = now()
        // a pasted token has nothing to refresh: ask the service once, and only a refusal asks for a new one (one
        // refused request may be a single query's permission, not the token)
        if let c = saved, c.pasted == true {
            if case .rejected = await checkPasted(c) {
            lost = true
                Log.info("atlassian refused the pasted token")
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

    private func save(_ c: AtlassianConnection) {
        do {
            try keep(c)
            unsaved = nil
        } catch {
            unsaved = c
            Log.info("atlassian: the keychain did not take the connection, kept in memory")
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
            // another Tacie may have refreshed already: take its pair
            if let disk = try? self.load(), disk.cloud == current.cloud, disk.refreshToken != current.refreshToken,
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
                    Log.info("atlassian: a refresh finished after the connection changed, dropped")
                    return
                }
                self.endRetries()
                // only the token fields are new: muted projects and spaces may have changed meanwhile
                var merged = self.saved ?? current
                merged.token = next.token
                merged.refreshToken = next.refresh
                merged.expiresAt = self.now().addingTimeInterval(next.expiresIn)
                self.save(merged)
                if self.unsaved == nil { self.marker.set(false) }
                self.saved = merged
                self.signedOut = false
                Log.info("atlassian token renewed")
                await self.push(merged)
            } catch AtlassianConnectError.refused {
                self.endRetries()
                self.marker.set(false)
                self.lost = true
                Log.info("atlassian refresh refused")
            } catch AtlassianConnectError.signedOut {
                // the relay stopped before Atlassian: the pair did not change
                self.endRetries()
                self.marker.set(false)
                self.signedOut = true
                Log.info("atlassian refresh needs a Tacie session")
            } catch let e as AtlassianConnectError where e == .notSent || e == .unready {
                // the relay answered without asking Atlassian: the pair did not change, the timer tries later
                self.endRetries()
                self.marker.set(false)
                Log.info("atlassian refresh not sent: \(e)")
            } catch {
                // the marker stays: if Atlassian did rotate the pair, the old refresh token works again only for
                // ten minutes, so the retry comes inside that window
                Log.info("atlassian refresh failed: \(error as? AtlassianConnectError ?? .failed)")
                self.retryLater(from: askedAt, started: started)
            }
        }
    }

    /// Schedules the next try of a refresh whose answer was lost, if the reuse window still has room for it.
    private func retryLater(from askedAt: Date, started: Int) {
        let from = retryFrom ?? askedAt
        retryFrom = from
        guard retries < Self.retryLimit,
              now().addingTimeInterval(Self.retryEvery).timeIntervalSince(from) < Self.reuseWindow else {
            Log.info("atlassian: no more refresh retries, the marker stays for the next launch")
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

    static func words(_ e: AtlassianConnectError) -> String {
        switch e {
        case .denied: String(localized: "연결하지 않았어요")
        case .expired: String(localized: "시간이 지났어요")
        case .blocked: SettingsText.atlassianBlocked
        case .refused: String(localized: "Jira · Confluence 연결이 끊겼어요")
        case .signedOut: SettingsText.atlassianSignedOut
        case .unready: String(localized: "Jira · Confluence 연결은 아직 준비 중이에요")
        case .network, .failed, .notSent: String(localized: "연결하지 못했어요")
        }
    }
}
