import XCTest
@testable import Tacie

final class AtlassianConnectTests: XCTestCase {
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

    private static let tokenPath = "/atlassian/token"
    private static let refreshPath = "/atlassian/refresh"
    private static let resourcesPath = "/oauth/token/accessible-resources"

    private func api(_ server: Server, session: String? = "sess") -> AtlassianConnect {
        var api = AtlassianConnect()
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
    private func conn(_ token: String = "a1", refresh: String = "r1", expires: TimeInterval = 600, cloud: String = "c1",
                      projects: [String] = [], spaces: [String] = []) -> AtlassianConnection {
        AtlassianConnection(token: token, refreshToken: refresh, expiresAt: soon.addingTimeInterval(expires), cloud: cloud,
                            site: "acme", siteURL: "https://acme.atlassian.net", mutedProjects: projects, mutedSpaces: spaces)
    }

    private final class Box: @unchecked Sendable {
        var disk: AtlassianConnection?
        var kept: [AtlassianConnection] = []
        var pushed: [AtlassianConnection?] = []
        var forgot = false
        var marker = false
        var connected = 0
    }

    @MainActor
    private func link(_ server: Server, saved: AtlassianConnection?, disk: AtlassianConnection? = nil,
                      session: String? = "sess") -> (AtlassianLink, Box) {
        let box = Box()
        box.disk = disk ?? saved
        let link = AtlassianLink(load: false)
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

    private let tokenOK = #"{"token":"a2","refresh_token":"r2","expires_in":3600,"scope":"read:jira-work"}"#
    private let resourcesOK = """
        [{"id":"c1","name":"acme","url":"https://acme.atlassian.net","scopes":["read:jira-work"]},
         {"id":"c1","name":"acme","url":"https://acme.atlassian.net","scopes":["read:page:confluence"]},
         {"id":"c2","name":"other","url":"https://other.atlassian.net","scopes":["read:jira-work"]}]
        """

    private func back(_ query: String, host: String = "atlassian", scheme: String = "tacie") -> URL {
        URL(string: "\(scheme)://\(host)?\(query)")!
    }

    // MARK: the return to the app

    func testReturnNeedsOurStateAndAKey() throws {
        let api = AtlassianConnect()
        XCTAssertEqual(try api.parseReturn(back("state=S1&k=K1"), state: "S1"), "K1")
        // not ours: ignored quietly (no error on the card)
        for url in [back("state=S2&k=K1"), back("state=S1&k=K1", host: "notion"), back("state=S1&k=K1", scheme: "https"),
                    back("k=K1")] {
            XCTAssertThrowsError(try api.parseReturn(url, state: "S1"), url.absoluteString) { XCTAssertTrue($0 is CancellationError) }
        }
        XCTAssertThrowsError(try api.parseReturn(back("state=S1"), state: "S1")) {
            XCTAssertEqual($0 as? AtlassianConnectError, .failed)
        }
        XCTAssertThrowsError(try api.parseReturn(back("state=S1&error=denied"), state: "S1")) {
            XCTAssertEqual($0 as? AtlassianConnectError, .denied)
        }
        for kind in ["failed", "server_error", "temporarily_unavailable"] {
            XCTAssertThrowsError(try api.parseReturn(back("state=S1&error=\(kind)"), state: "S1")) {
                XCTAssertEqual($0 as? AtlassianConnectError, .failed, kind)
            }
        }
    }

    func testStartURLGoesToTheRelayWithTheState() {
        let url = AtlassianConnect().startURL(state: "S1")
        XCTAssertEqual(url.absoluteString, "\(AccountServer.base.absoluteString)/atlassian/start?state=S1")
    }

    @MainActor
    func testAdminBlockIsBlocked() async throws {
        XCTAssertThrowsError(try AtlassianConnect().parseReturn(back("state=S1&error=access_restricted"), state: "S1")) {
            XCTAssertEqual($0 as? AtlassianConnectError, .blocked)
        }
        let server = Server()
        let (link, box) = link(server, saved: nil)
        link.authenticate = { start in
            let state = URLComponents(url: start, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == "state" }?.value
            return URL(string: "tacie://atlassian?state=\(state ?? "")&error=unauthorized_client")!
        }
        link.connect()
        await link.waitingTask?.value
        XCTAssertEqual(link.phase, .failed(AtlassianLink.words(.blocked)))
        XCTAssertTrue(server.asked.isEmpty)
        XCTAssertNil(box.disk)
    }

    @MainActor
    func testAReturnForAnotherSignInIsIgnored() async {
        let server = Server()
        let (link, _) = link(server, saved: nil)
        link.authenticate = { _ in URL(string: "tacie://atlassian?state=someone-else&k=K1")! }
        link.connect()
        await link.waitingTask?.value
        XCTAssertEqual(link.phase, .idle)
        XCTAssertTrue(server.asked.isEmpty)
    }

    // MARK: the relay

    func testTokenSendsTheSessionBearer() async throws {
        let server = Server([Self.tokenPath: [(200, tokenOK)]])
        let t = try await api(server).token(state: "S1", k: "K1")
        XCTAssertEqual(t.token, "a2")
        XCTAssertEqual(t.refresh, "r2")
        XCTAssertEqual(t.expiresIn, 3600)
        let req = try XCTUnwrap(server.requests(Self.tokenPath).first)
        XCTAssertEqual(req.url?.absoluteString, "\(AccountServer.base.absoluteString)/atlassian/token")
        XCTAssertEqual(req.httpMethod, "POST")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer sess")
        XCTAssertEqual(server.json(req), ["state": "S1", "k": "K1"])

        // no Tacie session: nothing is sent
        let none = Server([Self.tokenPath: [(200, tokenOK)]])
        do { _ = try await api(none, session: nil).token(state: "S1", k: "K1"); XCTFail() } catch {
            XCTAssertEqual(error as? AtlassianConnectError, .signedOut)
        }
        XCTAssertTrue(none.asked.isEmpty)

        let cases: [(Int, String, AtlassianConnectError)] = [
            (401, #"{"error":"signed_out"}"#, .signedOut), (400, #"{"error":"expired"}"#, .expired),
            (502, #"{"error":"failed"}"#, .failed), (503, #"{"error":"not_ready"}"#, .unready),
        ]
        for (status, body, want) in cases {
            let s = Server([Self.tokenPath: [(status, body)]])
            do { _ = try await api(s).token(state: "S1", k: "K1"); XCTFail("\(status)") } catch {
                XCTAssertEqual(error as? AtlassianConnectError, want, "\(status) \(body)")
            }
        }
        let down = Server()
        down.down = true
        do { _ = try await api(down).token(state: "S1", k: "K1"); XCTFail() } catch {
            XCTAssertEqual(error as? AtlassianConnectError, .network)
        }
    }

    func testRefreshRefusedVsFailed() async throws {
        let ok = Server([Self.refreshPath: [(200, tokenOK)]])
        let t = try await api(ok).refresh(conn())
        XCTAssertEqual(t.token, "a2")
        XCTAssertEqual(t.refresh, "r2")
        let req = try XCTUnwrap(ok.requests(Self.refreshPath).first)
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer sess")
        XCTAssertEqual(ok.json(req), ["refresh_token": "r1"])

        let cases: [(Int, String, AtlassianConnectError)] = [
            (401, #"{"error":"refused"}"#, .refused), (401, #"{"error":"signed_out"}"#, .signedOut),
            (502, #"{"error":"failed"}"#, .failed), (429, #"{"error":"busy"}"#, .notSent),
            (400, #"{"error":"bad_request"}"#, .notSent), (503, #"{"error":"not_ready"}"#, .unready),
        ]
        for (status, body, want) in cases {
            let s = Server([Self.refreshPath: [(status, body)]])
            do { _ = try await api(s).refresh(conn()); XCTFail("\(status)") } catch {
                XCTAssertEqual(error as? AtlassianConnectError, want, "\(status) \(body)")
            }
        }
    }

    @MainActor
    func testOnlyARefusalLosesTheConnection() async {
        // failed: the old pair stays and the connection is not lost
        let failed = Server([Self.refreshPath: [(502, #"{"error":"failed"}"#)]])
        let (a, boxA) = link(failed, saved: conn())
        await a.ensureFresh()
        XCTAssertEqual(a.saved?.token, "a1")
        XCTAssertFalse(a.lost)
        XCTAssertTrue(boxA.marker, "Atlassian may have rotated the pair: the next launch tries again")

        // signed out of Tacie: not lost either, the card asks for a sign-in
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
    }

    @MainActor
    func testAnswerThatNeverLeftTheRelayClearsTheMarker() async {
        for (status, body) in [(429, #"{"error":"busy"}"#), (400, #"{"error":"bad_request"}"#)] {
            let server = Server([Self.refreshPath: [(status, body)]])
            let (link, box) = link(server, saved: conn())
            await link.ensureFresh()
            XCTAssertFalse(box.marker, "\(status)")
            XCTAssertNil(link.retryTask)
            XCTAssertFalse(link.lost)
        }
    }

    /// A clock the retry wait moves forward.
    /// Holds a fake wait until the test opens it.
    private final class Gate: @unchecked Sendable {
        private var isOpen = false
        func open() { isOpen = true }
        func passed() async { while !isOpen { await Task.yield() } }
    }

    private final class Clock: @unchecked Sendable {
        var t: Date
        var waited: [TimeInterval] = []
        init(_ t: Date) { self.t = t }
    }

    @MainActor
    private func drainRetries(_ link: AtlassianLink) async {
        while let t = link.retryTask { await t.value }
    }

    @MainActor
    func testALostAnswerIsRetriedInsideTheReuseWindow() async {
        let server = Server([Self.refreshPath: [(502, #"{"error":"failed"}"#), (200, tokenOK)]])
        let (link, box) = link(server, saved: conn())
        let clock = Clock(soon)
        link.now = { clock.t }
        // the retry waits until the test has looked at the first try: on a slow runner it went before the checks below
        // and the first count read 2 (2026-11-26)
        let gate = Gate()
        link.wait = { s in
            await gate.passed()
            clock.waited.append(s); clock.t = clock.t.addingTimeInterval(s + 1); return true
        }
        await link.ensureFresh()
        XCTAssertEqual(server.requests(Self.refreshPath).count, 1)
        XCTAssertTrue(box.marker)
        XCTAssertNotNil(link.retryTask)
        gate.open()
        await drainRetries(link)
        XCTAssertEqual(clock.waited, [60])
        XCTAssertEqual(server.requests(Self.refreshPath).count, 2, "61 s later the same refresh token goes again")
        XCTAssertEqual(server.json(server.requests(Self.refreshPath)[1]), ["refresh_token": "r1"])
        XCTAssertEqual(link.saved?.token, "a2")
        XCTAssertFalse(box.marker)
        XCTAssertNil(link.retryTask)
    }

    @MainActor
    func testRetriesStopAtThreeAndOnDisconnect() async {
        let failed = (502, #"{"error":"failed"}"#)
        let server = Server([Self.refreshPath: Array(repeating: failed, count: 6)])
        let (link, box) = link(server, saved: conn())
        let clock = Clock(soon)
        link.now = { clock.t }
        link.wait = { s in clock.waited.append(s); clock.t = clock.t.addingTimeInterval(s + 1); return true }
        await link.ensureFresh()
        await drainRetries(link)
        XCTAssertEqual(server.requests(Self.refreshPath).count, 4, "the first try and three retries")
        XCTAssertLessThan(clock.t.timeIntervalSince(soon), 600, "all inside the ten minutes")
        XCTAssertTrue(box.marker, "the next launch still tries")

        let other = Server([Self.refreshPath: [failed, (200, tokenOK)]])
        let (l2, _) = self.link(other, saved: conn())
        // a real wait: only the disconnect's cancel ends it
        l2.wait = { _ in (try? await Task.sleep(for: .seconds(30))) != nil }
        await l2.ensureFresh()
        await l2.disconnect()
        await drainRetries(l2)
        XCTAssertEqual(other.requests(Self.refreshPath).count, 1)
    }

    func testResourcesMergeRowsOfOneSite() async throws {
        let server = Server([Self.resourcesPath: [(200, resourcesOK)]])
        let r = try await api(server).resources(token: "a2")
        XCTAssertEqual(r.cloud, "c1")
        XCTAssertEqual(r.site, "acme")
        XCTAssertEqual(r.siteURL, "https://acme.atlassian.net")
        let req = try XCTUnwrap(server.asked.first)
        XCTAssertEqual(req.url?.absoluteString, "https://api.atlassian.com/oauth/token/accessible-resources")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer a2")

        let empty = Server([Self.resourcesPath: [(200, "[]")]])
        do { _ = try await api(empty).resources(token: "a2"); XCTFail() } catch {
            XCTAssertEqual(error as? AtlassianConnectError, .failed)
        }
    }

    // MARK: the link

    @MainActor
    func testConnectSavesTheSiteThenPushes() async throws {
        let server = Server([Self.tokenPath: [(200, tokenOK)], Self.resourcesPath: [(200, resourcesOK)]])
        let (link, box) = link(server, saved: nil)
        var started: URL?
        link.authenticate = { start in
            started = start
            let state = URLComponents(url: start, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == "state" }?.value
            return URL(string: "tacie://atlassian?state=\(state ?? "")&k=K1")!
        }
        link.connect()
        XCTAssertEqual(link.phase, .waiting)
        await link.waitingTask?.value
        XCTAssertEqual(started?.path, "/atlassian/start")
        let state = try XCTUnwrap(URLComponents(url: started!, resolvingAgainstBaseURL: false)?.queryItems?.first?.value)
        XCTAssertTrue((32...128).contains(state.count))
        XCTAssertEqual(server.json(server.requests(Self.tokenPath)[0]), ["state": state, "k": "K1"])
        XCTAssertEqual(link.phase, .idle)
        let saved = try XCTUnwrap(link.saved)
        XCTAssertEqual(saved.token, "a2")
        XCTAssertEqual(saved.refreshToken, "r2")
        XCTAssertEqual(saved.expiresAt, soon.addingTimeInterval(3600))
        XCTAssertEqual(saved.cloud, "c1")
        XCTAssertEqual(saved.site, "acme")
        XCTAssertEqual(box.disk, saved)
        XCTAssertEqual(box.pushed.last??.token, "a2")
        XCTAssertEqual(box.connected, 1)
        XCTAssertEqual(link.site, "acme")
    }

    @MainActor
    func testRefreshKeepsTheSiteAndMutedAndRenewsBeforeTheTimer() async {
        let server = Server([Self.refreshPath: [(200, tokenOK)]])
        // more than the 40 minutes left: nothing to do
        let (fresh, _) = link(server, saved: conn(expires: 41 * 60))
        await fresh.ensureFresh()
        XCTAssertTrue(server.asked.isEmpty)

        let (link, box) = link(server, saved: conn(expires: 39 * 60, projects: ["P1"], spaces: ["S1"]))
        await link.ensureFresh()
        XCTAssertEqual(server.requests(Self.refreshPath).count, 1)
        XCTAssertEqual(link.saved?.token, "a2")
        XCTAssertEqual(link.saved?.refreshToken, "r2")
        XCTAssertEqual(link.saved?.mutedProjects, ["P1"])
        XCTAssertEqual(link.saved?.mutedSpaces, ["S1"])
        XCTAssertEqual(link.saved?.cloud, "c1")
        XCTAssertEqual(box.disk?.token, "a2")
        XCTAssertFalse(box.marker)
    }

    @MainActor
    func testRenewIsAtMostOnceAMinute() async {
        let ok = #"{"token":"a2","refresh_token":"r2","expires_in":3600}"#
        let server = Server([Self.refreshPath: [(200, ok), (200, ok)]])
        let (link, _) = link(server, saved: conn(expires: 3000))
        await link.renew()
        await link.renew()
        XCTAssertEqual(server.requests(Self.refreshPath).count, 1)
        link.now = { [soon] in soon.addingTimeInterval(61) }
        await link.renew()
        XCTAssertEqual(server.requests(Self.refreshPath).count, 2)
    }

    @MainActor
    func testRefreshUnderLockAdoptsANewerPairFromDisk() async {
        let server = Server([Self.refreshPath: [(200, tokenOK)]])
        let newer = conn("a9", refresh: "r9", expires: 3500)
        let (link, box) = link(server, saved: conn(), disk: newer)
        await link.ensureFresh()
        XCTAssertTrue(server.asked.isEmpty)
        XCTAssertEqual(link.saved, newer)
        XCTAssertEqual(box.pushed.last??.token, "a9")

        // another site's pair on disk is not taken
        let other = Server([Self.refreshPath: [(200, tokenOK)]])
        let (l2, _) = self.link(other, saved: conn(), disk: conn("a9", refresh: "r9", expires: 3500, cloud: "c2"))
        await l2.ensureFresh()
        XCTAssertEqual(other.requests(Self.refreshPath).count, 1)
        XCTAssertEqual(l2.saved?.token, "a2")
    }

    @MainActor
    func testMarkerMakesTheNextLaunchRefresh() async {
        let server = Server([Self.refreshPath: [(200, tokenOK)]])
        let (link, box) = link(server, saved: conn(expires: 3500))
        box.marker = true
        await link.ensureFresh()
        XCTAssertEqual(server.requests(Self.refreshPath).count, 1)
        XCTAssertEqual(link.saved?.token, "a2")
        XCTAssertFalse(box.marker)
    }

    @MainActor
    func testDisconnectForgetsAndPushesNil() async {
        let server = Server()
        let (link, box) = link(server, saved: conn())
        box.marker = true
        await link.disconnect()
        XCTAssertTrue(box.forgot)
        XCTAssertEqual(box.pushed.count, 1)
        XCTAssertEqual(box.pushed.last.map { $0 == nil }, true)
        XCTAssertFalse(link.connected)
        XCTAssertFalse(box.marker)
        XCTAssertTrue(server.asked.isEmpty, "Atlassian has no revoke: nothing is sent")
        XCTAssertNil(AtlassianConnection.engineBody(nil)["token"] as? String)
    }

    @MainActor
    func testMutingKeepsTheTokenAndHandsItOn() async {
        let server = Server()
        let (link, box) = link(server, saved: conn())
        await link.setMuted(projects: ["P2", "P1"], spaces: ["S1"])
        XCTAssertEqual(link.saved?.mutedProjects, ["P1", "P2"])
        XCTAssertEqual(box.disk?.mutedSpaces, ["S1"])
        let body = AtlassianConnection.engineBody(box.pushed.last ?? nil)
        XCTAssertEqual(body["token"] as? String, "a1")
        XCTAssertEqual(body["cloud"] as? String, "c1")
        XCTAssertEqual(body["site"] as? String, "acme")
        XCTAssertEqual(body["site_url"] as? String, "https://acme.atlassian.net")
        XCTAssertEqual(body["muted_projects"] as? [String], ["P1", "P2"])
        XCTAssertEqual(body["muted_spaces"] as? [String], ["S1"])
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
        XCTAssertNil(try AtlassianStore.load(from: BrokenStore(item: nil)))
        let saved = String(decoding: try JSONEncoder().encode(conn()), as: UTF8.self)
        XCTAssertEqual(try AtlassianStore.load(from: BrokenStore(item: saved)), conn())
        XCTAssertThrowsError(try AtlassianStore.load(from: BrokenStore(item: saved, locked: true)))
    }

    @MainActor
    func testUnreadableKeychainHandsNothingOverAtLaunch() async {
        let (link, box) = link(Server(), saved: nil)
        link.load = { throw BrokenStore.Locked() }
        link.reload()
        XCTAssertTrue(link.unreadable)
        XCTAssertFalse(link.connected)
        var sent: [AtlassianConnection?] = []
        await link.handOverAtLaunch { sent.append($0) }
        // nil would be a disconnect and the engine would drop the found items
        XCTAssertTrue(sent.isEmpty)
        XCTAssertTrue(box.pushed.isEmpty)
    }

    @MainActor
    func testUnreadableKeychainIsReadAgainOnTheTimer() async {
        let (link, box) = link(Server(), saved: nil)
        link.load = { throw BrokenStore.Locked() }
        link.reload()
        // still locked: nothing changes, nothing goes to the engine
        await link.retryUnreadable()
        XCTAssertTrue(link.unreadable)
        XCTAssertTrue(box.pushed.isEmpty)
        // unlocked: the connection is back and goes to the engine
        let back = conn()
        link.load = { back }
        await link.retryUnreadable()
        XCTAssertFalse(link.unreadable)
        XCTAssertEqual(link.saved, conn())
        XCTAssertEqual(box.pushed, [conn()])
        // a readable keychain is not read again
        link.load = { throw BrokenStore.Locked() }
        await link.retryUnreadable()
        XCTAssertFalse(link.unreadable)
        XCTAssertEqual(box.pushed.count, 1)
    }

    @MainActor
    func testUnreadableCardSaysSoInsteadOfOff() {
        let (link, _) = link(Server(), saved: nil)
        link.load = { throw BrokenStore.Locked() }
        link.reload()
        let status = AtlassianStatus(link: link, info: nil)
        XCTAssertEqual(status.state, .unreadable)
        XCTAssertEqual(status.line, "키체인에서 Jira · Confluence 연결을 읽지 못했어요")
    }

    @MainActor
    func testReadableKeychainHandsItsConnectionOverAtLaunch() async {
        let (none, _) = link(Server(), saved: nil, disk: nil)
        none.reload()
        XCTAssertFalse(none.unreadable)
        var sent: [AtlassianConnection?] = []
        await none.handOverAtLaunch { sent.append($0) }
        XCTAssertEqual(sent.count, 1)
        XCTAssertNil(sent[0] ?? nil)
        let (other, _) = link(Server(), saved: conn())
        other.reload()
        var got: [AtlassianConnection?] = []
        await other.handOverAtLaunch { got.append($0) }
        XCTAssertEqual(got, [conn()])
    }

    // MARK: 프로젝트·스페이스 고르기

    @MainActor
    func testSavingScopesKeepsMutedIdsTheListDoesNotShow() {
        // P9 was turned off before and is not in the list now: it stays off; P1 is turned back on, P2 off
        XCTAssertEqual(AtlassianLink.muted(shown: ["P1", "P2"], on: ["P1"], before: ["P1", "P9"]), ["P2", "P9"])
        // an empty list (the engine could not ask Atlassian) keeps everything as it was
        XCTAssertEqual(AtlassianLink.muted(shown: [], on: [], before: ["S1"]), ["S1"])
    }

    // 오픈소스판 3단계: an email and API token, pasted — never refreshed through the relay

    private let found = PastedTokens.Found(name: "acme.atlassian.net", cloud: "c1", siteURL: "https://acme.atlassian.net")

    @MainActor
    func testConnectPastedSavesAndPushes() async {
        let (link, box) = link(Server([:]), saved: nil)
        await link.connectPasted(token: "tok", email: "a@b.c", found: found)
        XCTAssertEqual(link.saved?.token, "tok")
        XCTAssertEqual(link.saved?.email, "a@b.c")
        XCTAssertEqual(link.saved?.cloud, "c1")
        XCTAssertEqual(link.saved?.siteURL, "https://acme.atlassian.net")
        XCTAssertEqual(link.saved?.pasted, true)
        XCTAssertEqual(box.pushed.last??.token, "tok")
        XCTAssertEqual(box.connected, 1)
        let body = AtlassianConnection.engineBody(link.saved)
        XCTAssertEqual(body["email"] as? String, "a@b.c")
        XCTAssertEqual(body["cloud"] as? String, "c1")
    }

    func testEngineBodyHasNoEmailForOAuth() {
        let c = AtlassianConnection(token: "a", refreshToken: "r", expiresAt: Date(), cloud: "c1", site: "acme",
                                    siteURL: "https://acme.atlassian.net")
        XCTAssertNil(AtlassianConnection.engineBody(c)["email"])
    }

    @MainActor
    func testPastedRenewMarksLostWithoutCalling() async {
        let server = Server([:])
        let (link, _) = link(server, saved: nil, session: nil)
        link.checkPasted = { _ in .rejected }
        await link.connectPasted(token: "tok", email: "a@b.c", found: found)
        await link.renew()
        XCTAssertTrue(link.lost)
        XCTAssertFalse(link.signedOut)
        XCTAssertTrue(server.asked.isEmpty)
    }

    @MainActor
    func testPastedEnsureFreshDoesNothing() async {
        let server = Server([:])
        let (link, box) = link(server, saved: nil, session: nil)
        await link.connectPasted(token: "tok", email: "a@b.c", found: found)
        box.marker = true
        await link.ensureFresh()
        XCTAssertTrue(server.asked.isEmpty)
        XCTAssertFalse(link.signedOut)
    }

    @MainActor
    func testPastedSameCloudKeepsMuted() async {
        let (link, _) = link(Server([:]), saved: conn(cloud: "c1", projects: ["P"], spaces: ["S"]))
        await link.connectPasted(token: "tok", email: "a@b.c", found: found)
        XCTAssertEqual(link.saved?.mutedProjects, ["P"])
        XCTAssertEqual(link.saved?.mutedSpaces, ["S"])
    }

    func testOldConnectionWithoutPastedDecodes() throws {
        let json = #"{"token":"a","refreshToken":"r","expiresAt":0,"cloud":"c1","site":"acme","siteURL":"https://acme.atlassian.net","mutedProjects":[],"mutedSpaces":[]}"#
        let c = try JSONDecoder().decode(AtlassianConnection.self, from: Data(json.utf8))
        XCTAssertNil(c.pasted)
        XCTAssertNil(c.email)
    }

    @MainActor
    func testPastedRenewKeepsATokenThatStillWorks() async {
        let (link, _) = link(Server([:]), saved: nil)
        link.checkPasted = { _ in .ok(self.found) }
        await link.connectPasted(token: "tok", email: "a@b.c", found: found)
        await link.renew()
        XCTAssertFalse(link.lost)
    }
}
