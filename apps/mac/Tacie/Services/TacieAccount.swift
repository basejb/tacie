import AppKit
import AuthenticationServices
import Foundation
import Observation

/// The Tacie account as the server tells it (GET /me, docs/14).
struct AccountInfo: Codable, Equatable {
    struct Month: Codable, Equatable {
        let spentUsd: Double
        let limitUsd: Double
    }
    let email: String?
    let name: String?
    let picture: String?
    let status: String  // waiting (until approved), active, blocked
    let month: Month
    /// The id the usage numbers use for this account (docs/16 3절); an older server sends none.
    var analyticsId: String? = nil
    /// The speaker model to group voices with ("campp-ft" or "eres2net", `SpeakerModel`); an older server sends none.
    var speakerModel: String? = nil
    /// Whether AI apps may read meetings through MCP: "all", "paid" or "off" (`MCPAccess`); an older server sends none.
    var mcp: String? = nil
    /// The news email consent (server mail-news.js): nil until the person chose; an older server sends none.
    var mailNews: Bool? = nil
    /// When the person last chose (ISO 8601).
    var mailNewsAt: String? = nil

    var analyticsStatus: AAccount { AAccount(rawValue: status) ?? .none }
}

enum AccountError: Error, Equatable { case signedOut, network, failed }

/// The account server (docs/14): one-time code → session, the account, sign out, delete.
struct AccountServer {
    static let base = BuildValues.server(BuildValues.accountServer)
    /// The engine's AI addresses while the account is used (engine settings.rs `RELAYS`): the relay nearest this Mac
    /// (`RelayRegion`), which takes the same session as this server. Sign-in, the account and Notion stay here in Seoul.
    static func openRouterRelay(_ region: RelayRegion) -> String { region.root + "/ai/openrouter" }
    static func jevRelay(_ region: RelayRegion) -> String { region.root + "/ai/jev" }

    /// (body, status). Throws only when there was no answer.
    var send: (URLRequest) async throws -> (Data, Int) = { req in
        let (data, resp) = try await URLSession.shared.data(for: req)
        return (data, (resp as? HTTPURLResponse)?.statusCode ?? 0)
    }
    /// The session `authorized` sends: the one kept now. Read on each call (the account can change while the app runs).
    var session: () -> String? = { SessionStore.store.read() }

    static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }()

    func exchange(code: String) async throws -> (token: String, info: AccountInfo) {
        struct Answer: Decodable { let token: String; let account: AccountInfo }
        let (data, status) = try await call("auth/exchange", method: "POST", token: nil, body: ["code": code])
        guard status == 200, let a = try? Self.decoder.decode(Answer.self, from: data) else { throw AccountError.failed }
        return (a.token, a.account)
    }

    func me(token: String) async throws -> AccountInfo {
        let (data, status) = try await call("me", method: "GET", token: token, body: nil)
        if status == 401 { throw AccountError.signedOut }
        guard status == 200, let info = try? Self.decoder.decode(AccountInfo.self, from: data) else { throw AccountError.failed }
        return info
    }

    /// The news email consent (POST /me/mail); the account as it stands after.
    func mailNews(token: String, on: Bool) async throws -> AccountInfo {
        let (data, status) = try await call("me/mail", method: "POST", token: token, body: ["news": on])
        if status == 401 { throw AccountError.signedOut }
        guard status == 200, let info = try? Self.decoder.decode(AccountInfo.self, from: data) else { throw AccountError.failed }
        return info
    }

    func signOut(token: String) async {
        _ = try? await call("auth/logout", method: "POST", token: token, body: [:])
    }

    /// The feedback card's picks and line (server feedback.js); only on Send.
    func feedback(token: String, tags: [String], text: String) async throws {
        let (_, status) = try await call("feedback", method: "POST", token: token, body: ["tags": tags.joined(separator: ","), "text": text])
        if status == 401 { throw AccountError.signedOut }
        guard status == 200 else { throw AccountError.failed }
    }

    func delete(token: String) async throws {
        let (_, status) = try await call("me", method: "DELETE", token: token, body: nil)
        guard status == 200 || status == 401 else { throw AccountError.failed }
    }

    /// POST with the current Tacie session as the Bearer, for the relay's routes that act for the account (Atlassian's
    /// token and refresh). No session: `AccountError.signedOut`, nothing is sent.
    func authorized(_ path: String, body: [String: String]) async throws -> (Data, Int) {
        guard let token = session(), !token.isEmpty else { throw AccountError.signedOut }
        return try await call(path, method: "POST", token: token, body: body)
    }

    private func call(_ path: String, method: String, token: String?, body: [String: Any]?) async throws -> (Data, Int) {
        var req = URLRequest(url: Self.base.appendingPathComponent(path))
        req.httpMethod = method
        req.timeoutInterval = 20
        if let token { req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        if let body {
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        do { return try await send(req) } catch { throw AccountError.network }
    }
}

/// The session token, in the keychain only (`ai.tacie.session`). Never logged.
/// A development build opened with `--signed-out` starts signed out and keeps its session in memory only, so the real
/// session in the keychain stays as it is (to look at the sign-in step while signed in elsewhere).
enum SessionStore {
    static var store: any TokenStore = Build.isDev && ProcessInfo.processInfo.arguments.contains("--signed-out")
        ? MemoryTokenStore()
        : KeychainTokenStore(service: "ai.tacie.session", account: "token", label: String(localized: "Tacie 로그인"))
}

/// A token kept only while the app runs (`--signed-out`).
final class MemoryTokenStore: TokenStore {
    private var token: String?
    func read() -> String? { token }
    func save(_ token: String) throws { self.token = token }
    func delete() throws { token = nil }
}

/// Signing in to Tacie and the account's state (docs/14 1절). The session goes to the engine in place of the AI keys
/// (IndexKeeper.aiKeys), and the engine's AI addresses point at the relay while one exists (EngineProcess).
@Observable
@MainActor
final class TacieAccount: NSObject {
    enum Phase: Equatable { case idle, signingIn, failed(String) }

    static let shared = TacieAccount()

    private(set) var phase: Phase = .idle
    private(set) var token: String?
    private(set) var info: AccountInfo?
    var server = AccountServer()
    /// Keeps the token (nil: deletes it). Tests keep it nowhere.
    var keep: (String?) -> Void = { t in
        if let t { try? SessionStore.store.save(t) } else { try? SessionStore.store.delete() }
    }
    /// Which sign-in this Mac used (설정 → 계정 says "GitHub 계정으로 로그인했어요"). Kept beside the session.
    private(set) var provider: ASignInWith?
    /// The sign-in window open now, or the one that last failed (its row shows why).
    private(set) var tried: ASignInWith = .google
    var keepProvider: (ASignInWith?) -> Void = { p in
        guard SessionStore.store is KeychainTokenStore else { return }  // `--signed-out` leaves the real one alone
        UserDefaults.standard.set(p?.rawValue, forKey: TacieAccount.providerKey)
    }
    nonisolated static let providerKey = "account.signedInWith"
    /// The account's space on this Mac (AccountSpace: its `analytics_id`), kept so the app opens it at launch before
    /// GET /me answers. Nil signed out, or for a session from before spaces until GET /me answers.
    private(set) var spaceID: String?
    var keepSpace: (String?) -> Void = { id in
        guard SessionStore.store is KeychainTokenStore else { return }  // `--signed-out` leaves the real one alone
        UserDefaults.standard.set(id, forKey: TacieAccount.spaceKey)
    }
    nonisolated static let spaceKey = "account.space"
    /// Asks for a deleted account's space to be erased (AppDelegate erases it once it has left it). Tests keep nothing.
    var eraseSpace: (String) -> Void = { SpaceAdoption().requestErase($0) }
    /// The space to open at launch: the account kept with the session.
    nonisolated static var savedSpace: AccountSpace {
        guard !(Build.isDev && ProcessInfo.processInfo.arguments.contains("--signed-out")) else { return .signedOut }
        return AccountSpace(id: UserDefaults.standard.string(forKey: spaceKey))
    }
    /// Set by AppDelegate: the session changed, so the engine gets new keys (and the relay, if it was not on).
    var changed: () async -> Void = {}
    /// After each `/me` answer is kept: the MCP socket opens or closes with the server's switch (MCPBridge.refresh).
    var mcpChanged: () -> Void = {}
    /// Set by AppDelegate to the PostHog side; nothing in tests.
    var analytics: AccountAnalytics = .none
    private var web: ASWebAuthenticationSession?
    /// The sign-in screen's "새 기능 소식 받기" box (NewsConsentBox): sent once the sign-in ends. Off until ticked.
    var newsAtSignIn = false

    init(load: Bool = true) {
        super.init()
        if load {
            token = SessionStore.store.read()
            // sessions from before GitHub sign-in (2026-11-01) were all Google
            if token != nil { provider = UserDefaults.standard.string(forKey: Self.providerKey).flatMap(ASignInWith.init) ?? .google }
            if token != nil { spaceID = Self.savedSpace.id }
        }
    }

    /// Tacie sign-in, the account and everything Tacie's server does for it: only in a build with the official values
    /// (#56). A source build has none.
    nonisolated static let offered = Build.hasAccount
    var signedIn: Bool { token != nil }
    /// This month's Tacie AI is used up. Only a used-up answer from the last GET /me is asked again (the limit may have
    /// changed, or the month turned); an unknown one counts as not used up, and the relay still says no.
    func usedUp() async -> Bool {
        guard let m = info?.month, m.spentUsd >= m.limitUsd else { return false }
        await refresh()
        guard let m = info?.month else { return false }
        return m.spentUsd >= m.limitUsd
    }
    /// Tacie AI answers for this account now.
    var usable: Bool { info?.status == "active" }

    /// Opens Google's or GitHub's sign-in over Tacie (it shares Safari's sign-in) and keeps the session it ends with.
    func signIn(with p: ASignInWith = .google) {
        guard phase != .signingIn else { return }
        tried = p
        let state = NotionConnect.newState()
        var c = URLComponents(url: AccountServer.base.appendingPathComponent("auth/\(p.rawValue)/start"), resolvingAgainstBaseURL: false)!
        c.queryItems = [URLQueryItem(name: "state", value: state)]
        phase = .signingIn
        let session = ASWebAuthenticationSession(url: c.url!, callbackURLScheme: "tacie") { [weak self] url, error in
            Task { @MainActor in await self?.finish(url: url, error: error, state: state, with: p) }
        }
        session.presentationContextProvider = self
        session.prefersEphemeralWebBrowserSession = false
        web = session
        let before = Set(NSApp.windows.map(ObjectIdentifier.init))
        NSApp.activate(ignoringOtherApps: true)
        if !session.start() {
            phase = .failed(String(localized: "로그인 창을 열지 못했어요."))
            return
        }
        // The sign-in window came up behind Tacie's own windows (2026-10-27): whatever window it opens goes in front.
        Task { await bringNewWindowsForward(since: before) }
    }

    /// For two seconds, any window that was not there before the sign-in started is put in front and made key.
    private func bringNewWindowsForward(since before: Set<ObjectIdentifier>) async {
        for _ in 0..<10 {
            try? await Task.sleep(for: .milliseconds(200))
            guard phase == .signingIn else { return }
            let new = NSApp.windows.filter { !before.contains(ObjectIdentifier($0)) && $0.isVisible }
            guard !new.isEmpty else { continue }
            NSApp.activate(ignoringOtherApps: true)
            for w in new {
                w.orderFrontRegardless()
                w.makeKey()
            }
            return
        }
    }

    func cancelSignIn() {
        web?.cancel()
        web = nil
        phase = .idle
    }

    private func finish(url: URL?, error: Error?, state: String, with p: ASignInWith) async {
        web = nil
        if let error = error as? ASWebAuthenticationSessionError, error.code == .canceledLogin {
            phase = .idle
            return
        }
        let items = url.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems } ?? []
        let value = { (name: String) in items.first { $0.name == name }?.value }
        guard value("state") == state, let code = value("code") else {
            let why: String = switch value("error") {
            case "denied": String(localized: "로그인하지 않았어요.")
            case "no_email": String(localized: "GitHub에 확인된 이메일이 없어요. GitHub 설정 → Emails에서 기본 이메일을 확인해 주세요.")
            default: String(localized: "로그인하지 못했어요. 다시 시도해 주세요.")
            }
            phase = .failed(why)
            return
        }
        do {
            try await exchange(code: code, with: p)
            phase = .idle
        } catch {
            phase = .failed(error as? AccountError == .network ? String(localized: "인터넷 연결을 확인해 주세요.") : String(localized: "로그인하지 못했어요. 다시 시도해 주세요."))
        }
    }

    /// The one-time code from the sign-in window becomes the session.
    func exchange(code: String, with p: ASignInWith = .google) async throws {
        let (t, i) = try await server.exchange(code: code)
        keep(t)
        keepProvider(p)
        token = t
        info = i
        provider = p
        setSpace(i.analyticsId)
        SpeakerModel.remember(i.speakerModel)
        MCPAccess.remember(i.mcp)
        mcpChanged()
        Log.info("signed in with \(p.rawValue) (\(i.status))")
        analytics.signedIn(i.analyticsStatus, i.analyticsId, p)
        // only a tick is sent: an unticked box leaves a returning account's earlier choice as it was
        if newsAtSignIn, i.mailNews != true { _ = await setNews(true) }
        newsAtSignIn = false
        await changed()
    }

    /// Asks the server how the account stands (approval, this month's use). A session the server no longer knows
    /// signs out here too.
    func refresh() async {
        guard let token else { return }
        do {
            let i = try await server.me(token: token)
            info = i
            SpeakerModel.remember(i.speakerModel)
            MCPAccess.remember(i.mcp)
            mcpChanged()
            analytics.refreshed(i.analyticsStatus, i.analyticsId)
            // a session from before spaces learns its space here, and its data moves there (AppDelegate)
            if spaceID == nil, AccountSpace(id: i.analyticsId).id != nil {
                setSpace(i.analyticsId)
                await changed()
            }
        } catch AccountError.signedOut {
            Log.info("session ended on the server")
            analytics.signedOut(.expired)
            await forget()
        } catch {}
    }

    func signOut() async {
        if let token { await server.signOut(token: token) }
        analytics.signedOut(.user)
        await forget()
        Log.info("signed out")
    }

    /// The news email consent: the 새로 생긴 기능 window's box and Settings → 계정. False when it did not go through.
    func setNews(_ on: Bool) async -> Bool {
        guard let token else { return false }
        do {
            info = try await server.mailNews(token: token, on: on)
            return true
        } catch {
            return false
        }
    }

    /// The 새로 생긴 기능 window asks: never chosen, or a yes two years old (Korean law asks for it again).
    var asksNews: Bool {
        guard let info else { return false }
        guard let on = info.mailNews else { return true }
        guard on, let at = info.mailNewsAt.flatMap(Self.iso) else { return false }
        return Date().timeIntervalSince(at) > 730 * 86400
    }

    private static func iso(_ s: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.date(from: s) ?? ISO8601DateFormatter().date(from: s)
    }

    /// Sends the feedback card under this account; false when signed out or the server did not take it.
    func sendFeedback(wishes: [FeedbackSurvey.Wish], text: String) async -> Bool {
        guard Self.offered, let token else { return false }
        do { try await server.feedback(token: token, tags: wishes.map(\.rawValue), text: text); return true } catch { return false }
    }

    /// The account and its usage go from the server, and its space on this Mac goes with it (meetings, recordings,
    /// people, connections, search data): nothing could open it again. It is erased once the app has left it.
    func deleteAccount() async -> Bool {
        guard let token else { return false }
        do { try await server.delete(token: token) } catch { return false }
        if let spaceID { eraseSpace(spaceID) }
        analytics.signedOut(.deleted)
        await forget()
        Log.info("account deleted")
        return true
    }

    private func forget() async {
        keep(nil)
        keepProvider(nil)
        setSpace(nil)
        token = nil
        info = nil
        provider = nil
        await changed()
    }

    private func setSpace(_ id: String?) {
        spaceID = AccountSpace(id: id).id
        keepSpace(spaceID)
    }

    /// A session without the keychain (tests).
    func setForTesting(token: String?, info: AccountInfo?, spaceID: String? = nil) {
        self.token = token
        self.info = info
        self.spaceID = spaceID
    }
}

extension TacieAccount: ASWebAuthenticationPresentationContextProviding {
    nonisolated func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        MainActor.assumeIsolated { NSApp.keyWindow ?? NSApp.windows.first { $0.isVisible } ?? NSWindow() }
    }
}

/// Whether the engine talks to the Tacie AI relay (#56): only when the AI choice is Tacie AI and there is a session.
/// Nothing chosen yet with a session is today's official app (`AIMode.firstLaunch` saves `.tacie` for it). Signed out,
/// or with any other choice, the engine uses the person's own keys or this Mac. A source build never has a session.
enum Relay {
    @MainActor static var on: Bool { on(mode: AIMode.saved(), signedIn: TacieAccount.shared.signedIn) }
    static func on(mode: AIMode?, signedIn: Bool) -> Bool {
        guard signedIn else { return false }
        return mode == .tacie  // nil (nothing chosen) never relays: signing in does not choose Tacie AI (#56)
    }
}

/// Which copy of the AI relay this Mac's engine talks to: Seoul or the US (relay only). It follows the Soniox region (`SonioxRegion`): Korea, the rest of
/// Asia-Pacific and a Korean region setting go to Seoul, everywhere else (the Americas, Europe, Africa) to the US.
/// The engine sends a US call that cannot connect to Seoul once (engine settings.rs `seoul_fallback`).
enum RelayRegion: String {
    case seoul, us

    var root: String {
        switch self {
        case .seoul: BuildValues.server(BuildValues.relaySeoul).absoluteString
        case .us: BuildValues.server(BuildValues.relayUS).absoluteString
        }
    }

    static func pick(timeZone: String, regionCode: String?) -> RelayRegion {
        SonioxRegion.hint(timeZone: timeZone, regionCode: regionCode) == "jp" ? .seoul : .us
    }

    /// A development build opened with `--relay-us` or `--relay-seoul` uses that copy wherever the Mac is.
    static var current: RelayRegion {
        let args = ProcessInfo.processInfo.arguments
        if Build.isDev, args.contains("--relay-us") { return .us }
        if Build.isDev, args.contains("--relay-seoul") { return .seoul }
        return pick(timeZone: TimeZone.current.identifier, regionCode: Locale.current.region?.identifier)
    }
}
