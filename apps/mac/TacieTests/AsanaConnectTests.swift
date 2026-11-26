import CryptoKit
import XCTest
@testable import Tacie

final class AsanaConnectTests: XCTestCase {
    /// Answers by path from queues of (status, JSON) and records what it was asked. Throws when `down`.
    private final class Server: @unchecked Sendable {
        var answers: [String: [(Int, String)]]
        var asked: [URLRequest] = []
        var down = false
        init(_ answers: [String: [(Int, String)]] = [:]) { self.answers = answers }
        func requests(_ path: String) -> [URLRequest] { asked.filter { $0.url?.path == path } }
        func json(_ req: URLRequest) -> [String: String] {
            (try? JSONSerialization.jsonObject(with: req.httpBody ?? Data())) as? [String: String] ?? [:]
        }
    }

    private static let tokenPath = "/asana/token"
    private static let refreshPath = "/asana/refresh"
    private static let mePath = "/api/1.0/users/me"

    private func api(_ server: Server, session: String? = "sess") -> AsanaConnect {
        var api = AsanaConnect()
        api.session = { session }
        api.send = { req in
            server.asked.append(req)
            await Task.yield()
            if server.down { throw URLError(.notConnectedToInternet) }
            let path = req.url?.path ?? ""
            guard var queue = server.answers[path], !queue.isEmpty else { return (Data("{}".utf8), 500) }
            let (status, body) = queue.removeFirst()
            server.answers[path] = queue
            return (Data(body.utf8), status)
        }
        api.now = { [soon] in soon }
        return api
    }

    private let soon = Date(timeIntervalSince1970: 1_000_000)
    private func conn(_ token: String = "a1", refresh: String = "r1", expires: TimeInterval = 600, user: String = "u1",
                      workspaces: [String] = [], projects: [String] = []) -> AsanaConnection {
        AsanaConnection(token: token, refreshToken: refresh, expiresAt: soon.addingTimeInterval(expires), userGid: user,
                        userName: "Mina", mutedWorkspaces: workspaces, mutedProjects: projects)
    }

    private final class Box: @unchecked Sendable {
        var disk: AsanaConnection?
        var kept: [AsanaConnection] = []
        var pushed: [AsanaConnection?] = []
        var forgot = false
        var marker = false
        var connected = 0
    }

    @MainActor
    private func link(_ server: Server, saved: AsanaConnection?, disk: AsanaConnection? = nil, box: Box? = nil,
                      session: String? = "sess") -> (AsanaLink, Box) {
        let box = box ?? Box()
        if box.disk == nil { box.disk = disk ?? saved }
        let link = AsanaLink(load: false)
        link.api = api(server, session: session)
        link.now = { [soon] in soon }
        link.load = { box.disk }
        link.keep = { c in box.disk = c; box.kept.append(c) }
        link.forget = { box.disk = nil; box.forgot = true }
        link.marker = .init(set: { box.marker = $0 }, isSet: { box.marker })
        link.lock = .none
        link.push = { box.pushed.append($0) }
        link.onConnected = { box.connected += 1 }
        link.setForTesting(saved)
        return (link, box)
    }

    private let tokenOK = #"{"token":"a2","refresh_token":"r2","expires_in":3600}"#
    /// Asana does not rotate the refresh token: a refresh answers the access token only.
    private let refreshOK = #"{"token":"a2","expires_in":3600}"#
    private let meOK = #"{"data":{"gid":"u1","name":"Mina"}}"#

    private func back(_ query: String, host: String = "asana", scheme: String = "tacie") -> URL {
        URL(string: "\(scheme)://\(host)?\(query)")!
    }

    private func query(_ url: URL, _ name: String) -> String? {
        URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == name }?.value
    }

    // MARK: the start and the return to the app

    func testStartURLCarriesTheStateAndAnS256Challenge() throws {
        let verifier = LinearConnect.newVerifier()
        let challenge = LinearConnect.challenge(verifier)
        let url = AsanaConnect().startURL(state: "S1", challenge: challenge)
        XCTAssertEqual(url.absoluteString, "\(AccountServer.base.absoluteString)/asana/start?state=S1&challenge=\(challenge)")
        // S256: BASE64URL(SHA256(verifier)), no padding, 43 characters the relay accepts
        let want = Data(SHA256.hash(data: Data(verifier.utf8))).base64EncodedString()
            .replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
        XCTAssertEqual(challenge, want)
        XCTAssertEqual(challenge.count, 43)
        XCTAssertNotNil(challenge.range(of: #"^[A-Za-z0-9_-]{43,128}$"#, options: .regularExpression))
        XCTAssertNotNil(verifier.range(of: #"^[A-Za-z0-9._~-]{43,128}$"#, options: .regularExpression))
    }

    func testReturnNeedsOurStateAndAKey() throws {
        let api = AsanaConnect()
        XCTAssertEqual(try api.parseReturn(back("state=S1&k=K1"), state: "S1"), "K1")
        // not ours: ignored quietly (no error on the card)
        for url in [back("state=S2&k=K1"), back("state=S1&k=K1", host: "atlassian"), back("state=S1&k=K1", scheme: "https"),
                    back("k=K1")] {
            XCTAssertThrowsError(try api.parseReturn(url, state: "S1"), url.absoluteString) { XCTAssertTrue($0 is CancellationError) }
        }
        XCTAssertThrowsError(try api.parseReturn(back("state=S1"), state: "S1")) {
            XCTAssertEqual($0 as? AsanaConnectError, .failed)
        }
        XCTAssertThrowsError(try api.parseReturn(back("state=S1&error=denied"), state: "S1")) {
            XCTAssertEqual($0 as? AsanaConnectError, .denied)
        }
        for kind in ["failed", "server_error", "temporarily_unavailable"] {
            XCTAssertThrowsError(try api.parseReturn(back("state=S1&error=\(kind)"), state: "S1")) {
                XCTAssertEqual($0 as? AsanaConnectError, .failed, kind)
            }
        }
        XCTAssertThrowsError(try api.parseReturn(back("state=S1&error=unauthorized_client"), state: "S1")) {
            XCTAssertEqual($0 as? AsanaConnectError, .blocked)
        }
        // only a known admin block says so; any other error from Asana is a failure to try again
        for kind in ["invalid_scope", "invalid_request", "something_new"] {
            XCTAssertThrowsError(try api.parseReturn(back("state=S1&error=\(kind)"), state: "S1")) {
                XCTAssertEqual($0 as? AsanaConnectError, .failed, kind)
            }
        }
    }

    @MainActor
    func testDeniedShowsOnTheCardAndSendsNothing() async {
        let server = Server()
        let (link, box) = link(server, saved: nil)
        link.authenticate = { [self] start in back("state=\(query(start, "state") ?? "")&error=denied") }
        link.connect()
        await link.waitingTask?.value
        XCTAssertEqual(link.phase, .failed(AsanaLink.words(.denied)))
        XCTAssertTrue(server.asked.isEmpty)
        XCTAssertNil(box.disk)
    }

    @MainActor
    func testAReturnForAnotherSignInIsIgnored() async {
        let server = Server()
        let (link, _) = link(server, saved: nil)
        link.authenticate = { _ in URL(string: "tacie://asana?state=someone-else&k=K1")! }
        link.connect()
        await link.waitingTask?.value
        XCTAssertEqual(link.phase, .idle)
        XCTAssertTrue(server.asked.isEmpty)
    }

    // MARK: the relay

    func testTokenSendsTheVerifierAndTheSessionBearer() async throws {
        let server = Server([Self.tokenPath: [(200, tokenOK)]])
        let t = try await api(server).token(state: "S1", k: "K1", verifier: "V1")
        XCTAssertEqual(t.token, "a2")
        XCTAssertEqual(t.refresh, "r2")
        XCTAssertEqual(t.expiresIn, 3600)
        let req = try XCTUnwrap(server.requests(Self.tokenPath).first)
        XCTAssertEqual(req.url?.absoluteString, "\(AccountServer.base.absoluteString)/asana/token")
        XCTAssertEqual(req.httpMethod, "POST")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer sess")
        XCTAssertEqual(server.json(req), ["state": "S1", "k": "K1", "verifier": "V1"])

        // the first exchange must bring a refresh token
        let noRefresh = Server([Self.tokenPath: [(200, refreshOK)]])
        do { _ = try await api(noRefresh).token(state: "S1", k: "K1", verifier: "V1"); XCTFail() } catch {
            XCTAssertEqual(error as? AsanaConnectError, .failed)
        }

        // no Tacie session: nothing is sent
        let none = Server([Self.tokenPath: [(200, tokenOK)]])
        do { _ = try await api(none, session: nil).token(state: "S1", k: "K1", verifier: "V1"); XCTFail() } catch {
            XCTAssertEqual(error as? AsanaConnectError, .signedOut)
        }
        XCTAssertTrue(none.asked.isEmpty)

        let cases: [(Int, String, AsanaConnectError)] = [
            (401, #"{"error":"signed_out"}"#, .signedOut), (400, #"{"error":"expired"}"#, .expired),
            (502, #"{"error":"failed"}"#, .failed), (503, #"{"error":"not_ready"}"#, .unready),
            (400, #"{"error":"bad_request"}"#, .notSent),
        ]
        for (status, body, want) in cases {
            let s = Server([Self.tokenPath: [(status, body)]])
            do { _ = try await api(s).token(state: "S1", k: "K1", verifier: "V1"); XCTFail("\(status)") } catch {
                XCTAssertEqual(error as? AsanaConnectError, want, "\(status) \(body)")
            }
        }
        let down = Server()
        down.down = true
        do { _ = try await api(down).token(state: "S1", k: "K1", verifier: "V1"); XCTFail() } catch {
            XCTAssertEqual(error as? AsanaConnectError, .network)
        }
    }

    func testRefreshRefusedVsFailed() async throws {
        let ok = Server([Self.refreshPath: [(200, refreshOK)]])
        let t = try await api(ok).refresh(conn())
        XCTAssertEqual(t.token, "a2")
        XCTAssertEqual(t.refresh, nil, "Asana sent no refresh token")
        let req = try XCTUnwrap(ok.requests(Self.refreshPath).first)
        XCTAssertEqual(req.url?.absoluteString, "\(AccountServer.base.absoluteString)/asana/refresh")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer sess")
        XCTAssertEqual(ok.json(req), ["refresh_token": "r1"])

        let rotated = Server([Self.refreshPath: [(200, tokenOK)]])
        let r = try await api(rotated).refresh(conn())
        XCTAssertEqual(r.refresh, "r2")

        let cases: [(Int, String, AsanaConnectError)] = [
            (401, #"{"error":"refused"}"#, .refused), (401, #"{"error":"signed_out"}"#, .signedOut),
            (502, #"{"error":"failed"}"#, .failed), (429, #"{"error":"busy"}"#, .notSent),
            (400, #"{"error":"bad_request"}"#, .notSent), (503, #"{"error":"not_ready"}"#, .unready),
        ]
        for (status, body, want) in cases {
            let s = Server([Self.refreshPath: [(status, body)]])
            do { _ = try await api(s).refresh(conn()); XCTFail("\(status)") } catch {
                XCTAssertEqual(error as? AsanaConnectError, want, "\(status) \(body)")
            }
        }
    }

    func testMeAsksAsanaDirectlyForGidAndName() async throws {
        let server = Server([Self.mePath: [(200, meOK)]])
        let me = try await api(server).me(token: "a2")
        XCTAssertEqual(me.gid, "u1")
        XCTAssertEqual(me.name, "Mina")
        let req = try XCTUnwrap(server.asked.first)
        XCTAssertEqual(req.url?.absoluteString, "https://app.asana.com/api/1.0/users/me?opt_fields=gid,name")
        XCTAssertEqual(req.httpMethod, "GET")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer a2")

        for (status, body) in [(401, "{}"), (200, #"{"data":{}}"#)] {
            let s = Server([Self.mePath: [(status, body)]])
            do { _ = try await api(s).me(token: "a2"); XCTFail("\(status)") } catch {
                XCTAssertEqual(error as? AsanaConnectError, .failed)
            }
        }
    }

    // MARK: the link

    @MainActor
    func testConnectSendsTheVerifierSavesTheUserThenPushes() async throws {
        let server = Server([Self.tokenPath: [(200, tokenOK)], Self.mePath: [(200, meOK)]])
        let (link, box) = link(server, saved: nil)
        var started: URL?
        link.authenticate = { [self] start in
            started = start
            return back("state=\(query(start, "state") ?? "")&k=K1")
        }
        link.connect()
        XCTAssertEqual(link.phase, .waiting)
        await link.waitingTask?.value
        let start = try XCTUnwrap(started)
        XCTAssertEqual(start.path, "/asana/start")
        let state = try XCTUnwrap(query(start, "state"))
        let challenge = try XCTUnwrap(query(start, "challenge"))
        XCTAssertTrue((32...128).contains(state.count))
        let sent = server.json(server.requests(Self.tokenPath)[0])
        XCTAssertEqual(sent["state"], state)
        XCTAssertEqual(sent["k"], "K1")
        let verifier = try XCTUnwrap(sent["verifier"])
        XCTAssertEqual(LinearConnect.challenge(verifier), challenge, "the verifier answers the challenge sent at start")
        XCTAssertEqual(link.phase, .idle)
        let saved = try XCTUnwrap(link.saved)
        XCTAssertEqual(saved.token, "a2")
        XCTAssertEqual(saved.refreshToken, "r2")
        XCTAssertEqual(saved.expiresAt, soon.addingTimeInterval(3600))
        XCTAssertEqual(saved.userGid, "u1")
        XCTAssertEqual(saved.userName, "Mina")
        XCTAssertEqual(box.disk, saved)
        XCTAssertEqual(box.pushed.last??.token, "a2")
        XCTAssertEqual(box.connected, 1)
    }

    @MainActor
    func testReconnectingTheSameUserKeepsMutes() async throws {
        let server = Server([Self.tokenPath: [(200, tokenOK), (200, tokenOK)], Self.mePath: [(200, meOK), (200, #"{"data":{"gid":"u2","name":"Jun"}}"#)]])
        let (link, _) = link(server, saved: conn(workspaces: ["W1"], projects: ["P1"]))
        link.authenticate = { [self] start in back("state=\(query(start, "state") ?? "")&k=K1") }
        link.connect()
        await link.waitingTask?.value
        XCTAssertEqual(link.saved?.mutedWorkspaces, ["W1"])
        XCTAssertEqual(link.saved?.mutedProjects, ["P1"])
        // another Asana user starts with everything on
        link.connect()
        await link.waitingTask?.value
        XCTAssertEqual(link.saved?.userGid, "u2")
        XCTAssertEqual(link.saved?.mutedWorkspaces, [])
        XCTAssertEqual(link.saved?.mutedProjects, [])
    }

    @MainActor
    func testRefreshWithoutANewRefreshTokenKeepsTheOldOne() async {
        let server = Server([Self.refreshPath: [(200, refreshOK)]])
        // more than the 40 minutes left: nothing to do
        let (fresh, _) = link(server, saved: conn(expires: 41 * 60))
        await fresh.ensureFresh()
        XCTAssertTrue(server.asked.isEmpty)

        let (link, box) = link(server, saved: conn(expires: 39 * 60, workspaces: ["W1"], projects: ["P1"]))
        await link.ensureFresh()
        XCTAssertEqual(server.requests(Self.refreshPath).count, 1)
        XCTAssertEqual(link.saved?.token, "a2")
        XCTAssertEqual(link.saved?.refreshToken, "r1")
        XCTAssertEqual(link.saved?.expiresAt, soon.addingTimeInterval(3600))
        XCTAssertEqual(link.saved?.mutedWorkspaces, ["W1"])
        XCTAssertEqual(link.saved?.mutedProjects, ["P1"])
        XCTAssertEqual(box.disk?.token, "a2")
        XCTAssertEqual(box.disk?.refreshToken, "r1")
        XCTAssertEqual(box.pushed.last??.token, "a2")
        XCTAssertFalse(box.marker)

        // a new one in the reply replaces it
        let rotated = Server([Self.refreshPath: [(200, tokenOK)]])
        let (l2, _) = self.link(rotated, saved: conn(expires: 60))
        await l2.ensureFresh()
        XCTAssertEqual(l2.saved?.refreshToken, "r2")
    }

    @MainActor
    func testLaunchWithAnExpiredTokenRefreshesOnce() async {
        let server = Server([Self.refreshPath: [(200, refreshOK), (200, refreshOK)]])
        let (link, _) = link(server, saved: conn(expires: -3600))
        await link.ensureFresh()
        await link.ensureFresh()
        XCTAssertEqual(server.requests(Self.refreshPath).count, 1)
        XCTAssertEqual(link.saved?.token, "a2")
    }

    @MainActor
    func testOnlyARefusalLosesTheConnection() async {
        // failed: the token stays, the connection is not lost and a retry is scheduled
        let failed = Server([Self.refreshPath: [(502, #"{"error":"failed"}"#)]])
        let (a, boxA) = link(failed, saved: conn())
        a.wait = { _ in false }
        await a.ensureFresh()
        XCTAssertEqual(a.saved?.token, "a1")
        XCTAssertEqual(a.saved?.refreshToken, "r1")
        XCTAssertFalse(a.lost)
        XCTAssertTrue(boxA.marker)
        XCTAssertNotNil(a.retryTask)

        let out = Server([Self.refreshPath: [(401, #"{"error":"signed_out"}"#)]])
        let (b, _) = link(out, saved: conn())
        await b.ensureFresh()
        XCTAssertFalse(b.lost)
        XCTAssertTrue(b.signedOut)

        let refused = Server([Self.refreshPath: [(401, #"{"error":"refused"}"#)]])
        let (c, boxC) = link(refused, saved: conn())
        await c.ensureFresh()
        XCTAssertTrue(c.lost)
        XCTAssertFalse(boxC.marker)
        XCTAssertNil(c.retryTask)
        // a lost connection does not ask again
        await c.renew()
        XCTAssertEqual(refused.requests(Self.refreshPath).count, 1)
    }

    /// A clock the retry wait moves forward.
    private final class Clock: @unchecked Sendable {
        var t: Date
        var waited: [TimeInterval] = []
        init(_ t: Date) { self.t = t }
    }

    @MainActor
    private func drainRetries(_ link: AsanaLink) async {
        while let t = link.retryTask { await t.value }
    }

    @MainActor
    func testAFailedRefreshIsRetriedLaterWithTheSameRefreshToken() async {
        let server = Server([Self.refreshPath: [(502, #"{"error":"failed"}"#), (200, refreshOK)]])
        let (link, box) = link(server, saved: conn())
        let clock = Clock(soon)
        link.now = { clock.t }
        link.wait = { s in clock.waited.append(s); clock.t = clock.t.addingTimeInterval(s + 1); return true }
        await link.ensureFresh()
        XCTAssertEqual(link.saved?.token, "a1")
        await drainRetries(link)
        XCTAssertEqual(clock.waited, [60])
        XCTAssertEqual(server.requests(Self.refreshPath).count, 2)
        XCTAssertEqual(server.json(server.requests(Self.refreshPath)[1]), ["refresh_token": "r1"])
        XCTAssertEqual(link.saved?.token, "a2")
        XCTAssertEqual(link.saved?.refreshToken, "r1")
        XCTAssertFalse(box.marker)
    }

    @MainActor
    func testAnswerThatNeverLeftTheRelayClearsTheMarker() async {
        for (status, body) in [(429, #"{"error":"busy"}"#), (400, #"{"error":"bad_request"}"#), (503, #"{"error":"not_ready"}"#)] {
            let server = Server([Self.refreshPath: [(status, body)]])
            let (link, box) = link(server, saved: conn())
            await link.ensureFresh()
            XCTAssertFalse(box.marker, "\(status)")
            XCTAssertNil(link.retryTask)
            XCTAssertFalse(link.lost)
        }
    }

    @MainActor
    func testRefreshUnderLockAdoptsANewerTokenFromDisk() async {
        // the refresh token does not rotate: a newer access token on disk is what tells another Tacie refreshed
        let server = Server([Self.refreshPath: [(200, refreshOK)]])
        let newer = conn("a9", expires: 3500)
        let (link, box) = link(server, saved: conn(), disk: newer)
        await link.ensureFresh()
        XCTAssertTrue(server.asked.isEmpty)
        XCTAssertEqual(link.saved, newer)
        XCTAssertEqual(box.pushed.last??.token, "a9")

        // another Asana user's token on disk is not taken
        let other = Server([Self.refreshPath: [(200, refreshOK)]])
        let (l2, _) = self.link(other, saved: conn(), disk: conn("a9", expires: 3500, user: "u2"))
        await l2.ensureFresh()
        XCTAssertEqual(other.requests(Self.refreshPath).count, 1)
        XCTAssertEqual(l2.saved?.token, "a2")
    }

    @MainActor
    func testTwoLinksRefreshingAtOnceMakeOneRelayCall() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("asana-lock-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let lock = RefreshLock(path: dir.appendingPathComponent("asana-refresh.lock"))
        let server = Server([Self.refreshPath: [(200, refreshOK), (200, refreshOK)]])
        // one keychain item and one marker, as two Tacie processes on one account share them
        let shared = Box()
        shared.disk = conn()
        let (a, _) = link(server, saved: conn(), box: shared)
        let (b, _) = link(server, saved: conn(), box: shared)
        a.lock = lock
        b.lock = lock
        async let one: Void = a.ensureFresh()
        async let two: Void = b.ensureFresh()
        _ = await (one, two)
        XCTAssertEqual(server.requests(Self.refreshPath).count, 1)
        XCTAssertEqual(a.saved?.token, "a2")
        XCTAssertEqual(b.saved?.token, "a2")
        XCTAssertEqual(shared.disk?.refreshToken, "r1")
    }

    @MainActor
    func testMarkerMakesTheNextLaunchRefresh() async {
        let server = Server([Self.refreshPath: [(200, refreshOK)]])
        let (link, box) = link(server, saved: conn(expires: 3500))
        box.marker = true
        await link.ensureFresh()
        XCTAssertEqual(server.requests(Self.refreshPath).count, 1)
        XCTAssertEqual(link.saved?.token, "a2")
        XCTAssertFalse(box.marker)
    }

    @MainActor
    func testDisconnectIsLocalOnlyAndPushesNil() async {
        let server = Server()
        let (link, box) = link(server, saved: conn())
        box.marker = true
        await link.disconnect()
        XCTAssertTrue(box.forgot)
        XCTAssertEqual(box.pushed.count, 1)
        XCTAssertEqual(box.pushed.last.map { $0 == nil }, true)
        XCTAssertFalse(link.connected)
        XCTAssertFalse(box.marker)
        // a revoke would cut every Mac's grant (docs/32 10-1 ⑮): nothing goes to Asana or the relay
        XCTAssertTrue(server.asked.isEmpty)
        XCTAssertFalse(server.asked.contains { $0.url?.host == "app.asana.com" })
        let body = AsanaConnection.engineBody(nil)
        XCTAssertTrue(body["token"] is NSNull)
    }

    @MainActor
    func testMutingKeepsTheTokenAndHandsItOn() async {
        let server = Server()
        let (link, box) = link(server, saved: conn())
        await link.setMuted(workspaces: ["W2", "W1"], projects: ["P1"])
        XCTAssertEqual(link.saved?.mutedWorkspaces, ["W1", "W2"])
        XCTAssertEqual(box.disk?.mutedProjects, ["P1"])
        let body = AsanaConnection.engineBody(box.pushed.last ?? nil)
        XCTAssertEqual(Set(body.keys), ["token", "user_gid", "muted_workspaces", "muted_projects"])
        XCTAssertEqual(body["token"] as? String, "a1")
        XCTAssertEqual(body["user_gid"] as? String, "u1")
        XCTAssertEqual(body["muted_workspaces"] as? [String], ["W1", "W2"])
        XCTAssertEqual(body["muted_projects"] as? [String], ["P1"])
        XCTAssertTrue(server.asked.isEmpty)
    }

    // MARK: a keychain that cannot be read

    private struct BrokenStore: TokenStore {
        struct Locked: Error {}
        var item: String?
        var locked = false
        func read() -> String? { locked ? nil : item }
        func readChecked() throws -> String? {
            if locked { throw Locked() }
            return item
        }
        func save(_ token: String) throws {}
        func delete() throws {}
    }

    func testStoreTellsNoItemFromAFailedRead() throws {
        XCTAssertEqual(AsanaStore.service, "ai.tacie.asana")
        XCTAssertNil(try AsanaStore.load(from: BrokenStore(item: nil)))
        let saved = String(decoding: try JSONEncoder().encode(conn()), as: UTF8.self)
        XCTAssertEqual(try AsanaStore.load(from: BrokenStore(item: saved)), conn())
        XCTAssertThrowsError(try AsanaStore.load(from: BrokenStore(item: saved, locked: true)))
    }

    @MainActor
    func testUnreadableKeychainHandsNothingOverAtLaunch() async {
        let (link, box) = link(Server(), saved: nil)
        link.load = { throw BrokenStore.Locked() }
        link.reload()
        XCTAssertTrue(link.unreadable)
        var sent: [AsanaConnection?] = []
        await link.handOverAtLaunch { sent.append($0) }
        XCTAssertTrue(sent.isEmpty)
        XCTAssertTrue(box.pushed.isEmpty)
        // unlocked on the timer: the connection goes to the engine
        link.load = { [self] in conn() }
        await link.retryUnreadable()
        XCTAssertFalse(link.unreadable)
        XCTAssertEqual(box.pushed, [conn()])
    }

    @MainActor
    func testSavingScopesKeepsMutedIdsTheListDoesNotShow() {
        XCTAssertEqual(AsanaLink.muted(shown: ["P1", "P2"], on: ["P1"], before: ["P1", "P9"]), ["P2", "P9"])
        XCTAssertEqual(AsanaLink.muted(shown: [], on: [], before: ["W1"]), ["W1"])
    }

    // 오픈소스판 3단계: a personal access token, pasted — never refreshed through the relay

    @MainActor
    func testConnectPastedSavesAndPushes() async {
        let (link, box) = link(Server([:]), saved: nil)
        await link.connectPasted(token: "2/x", found: PastedTokens.Found(name: "Kim", id: "u9"))
        XCTAssertEqual(link.saved?.token, "2/x")
        XCTAssertEqual(link.saved?.userGid, "u9")
        XCTAssertEqual(link.saved?.pasted, true)
        XCTAssertEqual(box.pushed.last??.token, "2/x")
        XCTAssertEqual(box.connected, 1)
    }

    @MainActor
    func testPastedRenewMarksLostWithoutCalling() async {
        let server = Server([:])
        let (link, _) = link(server, saved: nil, session: nil)
        link.checkPasted = { _ in .rejected }
        await link.connectPasted(token: "2/x", found: PastedTokens.Found(name: "Kim", id: "u9"))
        await link.renew()
        XCTAssertTrue(link.lost)
        XCTAssertFalse(link.signedOut)
        XCTAssertTrue(server.asked.isEmpty)
    }

    @MainActor
    func testPastedEnsureFreshDoesNothing() async {
        let server = Server([:])
        let (link, box) = link(server, saved: nil, session: nil)
        await link.connectPasted(token: "2/x", found: PastedTokens.Found(name: "Kim", id: "u9"))
        box.marker = true
        await link.ensureFresh()
        XCTAssertTrue(server.asked.isEmpty)
        XCTAssertFalse(link.signedOut)
    }

    @MainActor
    func testPastedSameUserKeepsMuted() async {
        let (link, _) = link(Server([:]), saved: conn(user: "u9", workspaces: ["w1"], projects: ["p1"]))
        await link.connectPasted(token: "2/x", found: PastedTokens.Found(name: "Kim", id: "u9"))
        XCTAssertEqual(link.saved?.mutedWorkspaces, ["w1"])
        XCTAssertEqual(link.saved?.mutedProjects, ["p1"])
    }

    @MainActor
    func testPastedRenewKeepsATokenThatStillWorks() async {
        let (link, _) = link(Server([:]), saved: nil)
        link.checkPasted = { _ in .ok(PastedTokens.Found(name: "Kim", id: "u9")) }
        await link.connectPasted(token: "2/x", found: PastedTokens.Found(name: "Kim", id: "u9"))
        await link.renew()
        XCTAssertFalse(link.lost)
    }
}
