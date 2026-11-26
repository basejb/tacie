import XCTest
@testable import Tacie

final class DropboxConnectTests: XCTestCase {
    /// Answers from a queue of (status, JSON) and records what it was asked.
    private final class Server: @unchecked Sendable {
        var answers: [(Int, String)]
        var asked: [URLRequest] = []
        var down = false
        init(_ answers: [(Int, String)] = []) { self.answers = answers }
        func body(_ i: Int) -> String { String(decoding: asked[i].httpBody ?? Data(), as: UTF8.self) }
        /// The form fields of request `i`, decoded.
        func fields(_ i: Int) -> [String: String] {
            var c = URLComponents()
            c.percentEncodedQuery = body(i)
            return Dictionary(uniqueKeysWithValues: (c.queryItems ?? []).map { ($0.name, $0.value ?? "") })
        }
    }

    private let soon = Date(timeIntervalSince1970: 1_000_000)

    private func api(_ server: Server) -> DropboxConnect {
        var api = DropboxConnect(appKey: "test-app-key")
        api.now = { [soon] in soon }
        api.send = { req in
            server.asked.append(req)
            if server.down { throw URLError(.notConnectedToInternet) }
            let (status, body) = server.answers.isEmpty ? (500, "{}") : server.answers.removeFirst()
            return (Data(body.utf8), status)
        }
        return api
    }

    private func conn(_ token: String = "a1", expires: TimeInterval = 600, muted: [String] = [],
                      accountID: String = "dbid:A1") -> DropboxConnection {
        DropboxConnection(token: token, refreshToken: "r1", expiresAt: soon.addingTimeInterval(expires),
                          accountID: accountID, account: "me@example.com", mutedFolders: muted)
    }

    private let exchangeOK = #"{"access_token":"a1","token_type":"bearer","expires_in":14400,"refresh_token":"r1","scope":"account_info.read files.content.read files.metadata.read","uid":"12","account_id":"dbid:A1"}"#

    func testAuthorizeURLAsksForOfflineAccessWithPKCE() throws {
        let url = DropboxConnect(appKey: "test-app-key").authorizeURL(redirect: "http://127.0.0.1:5555/dropbox", state: "s1", challenge: "c1", korean: true)
        let q = Dictionary(uniqueKeysWithValues: URLComponents(url: url, resolvingAgainstBaseURL: false)!.queryItems!.map { ($0.name, $0.value!) })
        XCTAssertEqual(url.host, "www.dropbox.com")
        XCTAssertEqual(url.path, "/oauth2/authorize")
        XCTAssertEqual(q, ["client_id": "test-app-key", "response_type": "code", "redirect_uri": "http://127.0.0.1:5555/dropbox",
                           "code_challenge": "c1", "code_challenge_method": "S256", "token_access_type": "offline",
                           "state": "s1", "locale": "ko"])
        let en = DropboxConnect(appKey: "test-app-key").authorizeURL(redirect: "http://127.0.0.1:5555/dropbox", state: "s1", challenge: "c1", korean: false)
        XCTAssertTrue(en.absoluteString.contains("locale=en"))
    }

    func testExchangeSendsTheVerifierWithoutASecret() async throws {
        let server = Server([(200, exchangeOK)])
        let got = try await api(server).exchange(code: "k1", verifier: "v1", redirect: "http://127.0.0.1:5555/dropbox")
        XCTAssertEqual(got.token, "a1")
        XCTAssertEqual(got.refresh, "r1")
        XCTAssertEqual(got.expiresIn, 14400)
        XCTAssertEqual(got.accountID, "dbid:A1")
        let req = server.asked[0]
        XCTAssertEqual(req.url, DropboxConnect.tokenURL)
        XCTAssertEqual(req.httpMethod, "POST")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Content-Type"), "application/x-www-form-urlencoded")
        XCTAssertEqual(server.fields(0), ["code": "k1", "code_verifier": "v1", "client_id": "test-app-key",
                                          "grant_type": "authorization_code", "redirect_uri": "http://127.0.0.1:5555/dropbox"])
        let body = server.body(0)
        XCTAssertFalse(body.contains("client_secret"))
    }

    func testExchangeWithoutFileReadScopeFails() async {
        let answer = #"{"access_token":"a1","expires_in":14400,"refresh_token":"r1","scope":"account_info.read files.metadata.read","account_id":"dbid:A1"}"#
        do {
            _ = try await api(Server([(200, answer)])).exchange(code: "k1", verifier: "v1", redirect: "http://127.0.0.1:5555/dropbox")
            XCTFail("expected .scope")
        } catch { XCTAssertEqual(error as? DropboxConnectError, .scope) }
    }

    func testExchangeWithoutAccountIDFails() async {
        let answer = #"{"access_token":"a1","expires_in":14400,"refresh_token":"r1","scope":"account_info.read files.content.read files.metadata.read"}"#
        do {
            _ = try await api(Server([(200, answer)])).exchange(code: "k1", verifier: "v1", redirect: "http://127.0.0.1:5555/dropbox")
            XCTFail("expected .failed")
        } catch { XCTAssertEqual(error as? DropboxConnectError, .failed) }
    }

    func testExchangeWithoutExpiresInTakesFourHours() async throws {
        let answer = #"{"access_token":"a1","refresh_token":"r1","scope":"files.content.read files.metadata.read","account_id":"dbid:A1"}"#
        let got = try await api(Server([(200, answer)])).exchange(code: "k1", verifier: "v1", redirect: "http://127.0.0.1:5555/dropbox")
        XCTAssertEqual(got.expiresIn, 14400)
    }

    func testRefreshKeepsTheRefreshTokenAndOnlyInvalidGrantIsRefused() async throws {
        let server = Server([(200, #"{"access_token":"a3","token_type":"bearer","expires_in":14400}"#)])
        let got = try await api(server).refresh(conn())
        XCTAssertEqual(got.token, "a3")
        XCTAssertEqual(got.expiresIn, 14400)
        XCTAssertEqual(server.fields(0), ["grant_type": "refresh_token", "refresh_token": "r1", "client_id": "test-app-key"])

        for (answer, want) in [((400, #"{"error":"invalid_grant","error_description":"refresh token is invalid or revoked"}"#), DropboxConnectError.refused),
                               ((400, #"{"error":"invalid_request"}"#), .failed)] {
            do {
                _ = try await api(Server([answer])).refresh(conn())
                XCTFail("expected \(want)")
            } catch { XCTAssertEqual(error as? DropboxConnectError, want) }
        }
        let down = Server()
        down.down = true
        do {
            _ = try await api(down).refresh(conn())
            XCTFail("expected .network")
        } catch { XCTAssertEqual(error as? DropboxConnectError, .network) }
    }

    func testAccountSendsJSONNullAndReadsTeam() async throws {
        let server = Server([
            (200, #"{"account_id":"dbid:A1","email":"me@example.com","root_info":{".tag":"user"}}"#),
            (200, #"{"account_id":"dbid:A1","email":"me@example.com","team":{"id":"dbtid:1","name":"Acme"}}"#),
            (401, #"{"error_summary":"invalid_access_token/"}"#),
        ])
        let api = api(server)
        let user = try await api.account(token: "a1")
        XCTAssertEqual(user, DropboxAccount(id: "dbid:A1", email: "me@example.com", team: false))
        let req = server.asked[0]
        XCTAssertEqual(req.url, DropboxConnect.accountURL)
        XCTAssertEqual(req.httpMethod, "POST")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer a1")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Content-Type"), "application/json")
        XCTAssertEqual(server.body(0), "null")
        let team = try await api.account(token: "a1")
        XCTAssertTrue(team.team)
        do {
            _ = try await api.account(token: "a1")
            XCTFail("expected .failed")
        } catch { XCTAssertEqual(error as? DropboxConnectError, .failed) }
    }

    func testPaperLegacyFromFeatures() async {
        let enabled = #"{"values":[{".tag":"paper_as_files","paper_as_files":{".tag":"enabled","enabled":true}}]}"#
        let disabled = #"{"values":[{".tag":"paper_as_files","paper_as_files":{".tag":"enabled","enabled":false}}]}"#
        let server = Server([(200, enabled), (200, disabled), (400, "{}")])
        let api = api(server)
        let first = await api.paperLegacy(token: "a1")
        XCTAssertEqual(first, false)
        XCTAssertEqual(server.asked[0].url, DropboxConnect.featuresURL)
        XCTAssertEqual(server.asked[0].value(forHTTPHeaderField: "Content-Type"), "application/json")
        XCTAssertEqual(server.asked[0].value(forHTTPHeaderField: "Authorization"), "Bearer a1")
        let sent = (try? JSONSerialization.jsonObject(with: server.asked[0].httpBody ?? Data())) as? [String: Any]
        XCTAssertEqual((sent?["features"] as? [[String: String]])?.first?[".tag"], "paper_as_files")
        let second = await api.paperLegacy(token: "a1")
        XCTAssertEqual(second, true)
        let third = await api.paperLegacy(token: "a1")
        XCTAssertNil(third)
        let down = Server()
        down.down = true
        let fourth = await self.api(down).paperLegacy(token: "a1")
        XCTAssertNil(fourth)
    }

    func testRevokeRetriesOnceWithAFreshTokenAfter401() async {
        let server = Server([(401, "{}"), (200, #"{"access_token":"a9","expires_in":14400}"#), (200, "null")])
        await api(server).revoke(conn())
        XCTAssertEqual(server.asked.count, 3)
        for i in [0, 2] {
            XCTAssertEqual(server.asked[i].url, DropboxConnect.revokeURL)
            XCTAssertEqual(server.asked[i].httpMethod, "POST")
            XCTAssertEqual(server.body(i), "null")
            XCTAssertEqual(server.asked[i].value(forHTTPHeaderField: "Content-Type"), "application/json")
        }
        XCTAssertEqual(server.asked[0].value(forHTTPHeaderField: "Authorization"), "Bearer a1")
        XCTAssertEqual(server.asked[1].url, DropboxConnect.tokenURL)
        XCTAssertEqual(server.asked[2].value(forHTTPHeaderField: "Authorization"), "Bearer a9")

        let again = Server([(401, "{}"), (200, #"{"access_token":"a9","expires_in":14400}"#), (401, "{}")])
        await api(again).revoke(conn())
        XCTAssertEqual(again.asked.count, 3)

        let ok = Server([(200, "null")])
        await api(ok).revoke(conn())
        XCTAssertEqual(ok.asked.count, 1)
    }

    func testRevokeStopsWhenTheRefreshIsRefused() async {
        let server = Server([(401, "{}"), (400, #"{"error":"invalid_grant"}"#), (200, "null")])
        await api(server).revoke(conn())
        XCTAssertEqual(server.asked.count, 2)
        XCTAssertEqual(server.asked[1].url, DropboxConnect.tokenURL)
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
        XCTAssertEqual(DropboxStore.service, "ai.tacie.dropbox")
        XCTAssertEqual(DropboxStore.store.service, "ai.tacie.dropbox")
        XCTAssertNil(try DropboxStore.load(from: BrokenStore(item: nil)))
        let saved = String(decoding: try JSONEncoder().encode(conn(muted: ["f2"])), as: UTF8.self)
        XCTAssertEqual(try DropboxStore.load(from: BrokenStore(item: saved)), conn(muted: ["f2"]))
        XCTAssertThrowsError(try DropboxStore.load(from: BrokenStore(item: saved, locked: true)))
    }

    /// An item that does not decode is not "not connected": handing the engine nil would drop the fetched files.
    func testStoreThrowsOnAnItemThatDoesNotDecodeAndReadsOneWithoutMutes() throws {
        XCTAssertThrowsError(try DropboxStore.load(from: BrokenStore(item: "not json")))
        XCTAssertThrowsError(try DropboxStore.load(from: BrokenStore(item: #"{"token":"a1"}"#)))
        let old = #"{"token":"a1","refreshToken":"r1","expiresAt":0,"accountID":"dbid:A1","account":"me@example.com"}"#
        let c = try XCTUnwrap(try DropboxStore.load(from: BrokenStore(item: old)))
        XCTAssertEqual(c.mutedFolders, [])
        XCTAssertEqual(c.accountID, "dbid:A1")
    }

    func testEngineBody() {
        XCTAssertEqual(DropboxConnection.engineBody(nil)["token"] as? NSNull, NSNull())
        XCTAssertEqual(DropboxConnection.engineBody(nil)["muted_folders"] as? [String], [])
        let b = DropboxConnection.engineBody(conn(muted: ["f2"]))
        XCTAssertEqual(b["token"] as? String, "a1")
        XCTAssertEqual(b["account_id"] as? String, "dbid:A1")
        XCTAssertEqual(b["account"] as? String, "me@example.com")
        XCTAssertEqual(b["muted_folders"] as? [String], ["f2"])
    }

    // MARK: DropboxLink

    private final class Box: @unchecked Sendable {
        var kept: [DropboxConnection] = []
        var pushed: [DropboxConnection?] = []
        var forgot = false
        var reads = 0
        var opened: [URL] = []
        var connected: [DropboxAccount] = []
        var events: [String] = []
        var disk: DropboxConnection?
        var now: Date
        init(now: Date) { self.now = now }
    }

    @MainActor
    private func link(_ server: Server, saved: DropboxConnection?, disk: DropboxConnection? = nil,
                      reply: LoopbackReply? = .code("code1")) -> (DropboxLink, Box) {
        let box = Box(now: soon)
        box.disk = disk ?? saved
        let link = DropboxLink(load: false)
        var api = api(server)
        let send = api.send
        api.send = { req in
            if req.url == DropboxConnect.revokeURL { box.events.append("revoke") }
            return try await send(req)
        }
        link.api = api
        link.now = { box.now }
        link.lock = .none
        link.load = { box.disk }
        link.keep = { c in box.disk = c; box.kept.append(c) }
        link.forget = { box.disk = nil; box.forgot = true; box.events.append("forget") }
        link.push = { c in box.pushed.append(c); box.events.append(c == nil ? "push nil" : "push") }
        link.open = { box.opened.append($0) }
        link.onConnected = { box.connected.append($0) }
        link.listen = { _ in
            ("http://127.0.0.1:5555/dropbox", {
                if let reply { return reply }
                try await Task.sleep(for: .seconds(3600))
                throw CancellationError()
            })
        }
        link.seed(saved)
        return (link, box)
    }

    private func accountOK(_ id: String = "dbid:A1") -> String {
        #"{"account_id":"\#(id)","email":"me@example.com","root_info":{".tag":"user"}}"#
    }
    private let paperOK = #"{"values":[{".tag":"paper_as_files","paper_as_files":{".tag":"enabled","enabled":true}}]}"#

    private func query(_ url: URL) -> [String: String] {
        Dictionary(uniqueKeysWithValues: (URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []).map { ($0.name, $0.value ?? "") })
    }

    @MainActor
    func testConnectSavesTheAccountAndHandsItOver() async throws {
        let server = Server([(200, exchangeOK), (200, accountOK()), (200, paperOK)])
        let (link, box) = link(server, saved: nil)
        link.connect()
        XCTAssertEqual(link.phase, .waiting)
        await link.waitingTask?.value
        let q = query(try XCTUnwrap(box.opened.first))
        XCTAssertEqual(q["redirect_uri"], "http://127.0.0.1:5555/dropbox")
        // the same redirect goes to the exchange, and the verifier matches the challenge
        XCTAssertEqual(server.fields(0)["redirect_uri"], "http://127.0.0.1:5555/dropbox")
        XCTAssertEqual(server.fields(0)["code"], "code1")
        XCTAssertEqual(LinearConnect.challenge(server.fields(0)["code_verifier"] ?? ""), q["code_challenge"])
        let saved = try XCTUnwrap(box.kept.last)
        XCTAssertEqual(saved.accountID, "dbid:A1")
        XCTAssertEqual(saved, DropboxConnection(token: "a1", refreshToken: "r1", expiresAt: soon.addingTimeInterval(14400),
                                                accountID: "dbid:A1", account: "me@example.com", mutedFolders: []))
        XCTAssertEqual(link.saved, saved)
        XCTAssertEqual(box.pushed, [saved])
        XCTAssertEqual(box.connected, [DropboxAccount(id: "dbid:A1", email: "me@example.com", team: false, paperLegacy: false)])
        XCTAssertEqual(link.phase, .idle)
        XCTAssertTrue(link.connected)
        XCTAssertEqual(link.account, "me@example.com")
    }

    @MainActor
    func testDenyingSaysSoAndSavesNothing() async {
        let server = Server()
        let (link, box) = link(server, saved: nil, reply: .error("access_denied"))
        link.connect()
        await link.waitingTask?.value
        XCTAssertEqual(link.phase, .failed("연결하지 않았어요"))
        XCTAssertTrue(box.kept.isEmpty)
        XCTAssertTrue(box.pushed.isEmpty)
        XCTAssertTrue(server.asked.isEmpty)
    }

    @MainActor
    func testApprovalThatNeverReturnsTimesOut() async {
        let server = Server()
        let (link, box) = link(server, saved: nil, reply: nil)
        link.patience = 0.05
        link.connect()
        await link.waitingTask?.value
        XCTAssertEqual(link.phase, .failed("시간이 지났어요"))
        XCTAssertTrue(box.kept.isEmpty)
        XCTAssertTrue(box.pushed.isEmpty)
        XCTAssertTrue(server.asked.isEmpty)
    }

    /// A link whose Paper-flag request hangs until cancelled, with a flag that says it was asked.
    @MainActor
    private func linkStuckOnPaper() -> (DropboxLink, Box, () -> Bool) {
        let server = Server([(200, exchangeOK), (200, accountOK()), (200, "null")])
        let (link, box) = link(server, saved: nil)
        let send = link.api.send
        final class Flag: @unchecked Sendable { var asked = false }
        let flag = Flag()
        link.api.send = { req in
            if req.url == DropboxConnect.featuresURL {
                flag.asked = true
                try await Task.sleep(for: .seconds(3600))
            }
            return try await send(req)
        }
        return (link, box, { flag.asked })
    }

    @MainActor
    private func untilAsked(_ asked: () -> Bool) async {
        for _ in 0..<1000 where !asked() { try? await Task.sleep(for: .milliseconds(2)) }
        XCTAssertTrue(asked())
    }

    @MainActor
    func testCancelWhileThePaperFlagIsPendingSavesNothing() async {
        let (link, box, asked) = linkStuckOnPaper()
        link.connect()
        let task = link.waitingTask
        await untilAsked(asked)
        link.cancel()
        await task?.value
        XCTAssertTrue(box.kept.isEmpty)
        XCTAssertTrue(box.pushed.isEmpty)
        XCTAssertTrue(box.connected.isEmpty)
        XCTAssertNil(link.saved)
        XCTAssertEqual(link.phase, .idle)
    }

    @MainActor
    func testDisconnectWhileThePaperFlagIsPendingSavesNothing() async {
        let (link, box, asked) = linkStuckOnPaper()
        link.connect()
        let task = link.waitingTask
        await untilAsked(asked)
        await link.disconnect()
        await task?.value
        XCTAssertTrue(box.kept.isEmpty)
        XCTAssertEqual(box.pushed, [nil])
        XCTAssertTrue(box.connected.isEmpty)
        XCTAssertNil(link.saved)
    }

    @MainActor
    func testDisconnectDuringThePushDoesNotFireOnConnected() async {
        let server = Server([(200, exchangeOK), (200, accountOK()), (200, paperOK)])
        let (link, box) = link(server, saved: nil)
        final class Gate: @unchecked Sendable { var started = false; var open = false }
        let gate = Gate()
        let push = link.push
        link.push = { c in
            if c != nil {
                gate.started = true
                while !gate.open { try? await Task.sleep(for: .milliseconds(2)) }
            }
            await push(c)
        }
        link.connect()
        let task = link.waitingTask
        await untilAsked { gate.started }
        // the disconnect's nil waits for the push in flight, so it runs on its own while the gate opens
        let disconnecting = Task { await link.disconnect() }
        await untilAsked { link.saved == nil }
        gate.open = true
        await disconnecting.value
        await task?.value
        XCTAssertTrue(box.connected.isEmpty)
        XCTAssertNil(link.saved)
    }

    /// A disconnect while the connect's hand-over is still on its way to the engine: once that push lands, the
    /// disconnect's nil follows it, so the engine ends up with no connection, not the stale one.
    @MainActor
    func testDisconnectWhileThePushIsBlockedLeavesTheEngineWithNil() async {
        let server = Server([(200, exchangeOK), (200, accountOK()), (200, paperOK)])
        let (link, box) = link(server, saved: nil)
        final class Gate: @unchecked Sendable { var started = false; var open = false }
        let gate = Gate()
        let push = link.push
        link.push = { c in
            if c != nil {
                gate.started = true
                while !gate.open { try? await Task.sleep(for: .milliseconds(2)) }
            }
            await push(c)
        }
        link.connect()
        let task = link.waitingTask
        await untilAsked { gate.started }
        let disconnecting = Task { await link.disconnect() }
        await untilAsked { link.saved == nil }
        // give an unserialized nil every chance to land first
        try? await Task.sleep(for: .milliseconds(20))
        gate.open = true
        await disconnecting.value
        await task?.value
        XCTAssertEqual(box.pushed.count, 2)
        XCTAssertEqual(box.pushed.last, .some(nil))
    }

    /// A hand-over queued behind a push in flight is dropped once the connection changed under it.
    @MainActor
    func testAQueuedPushFromAnOlderConnectionIsDropped() async {
        let (link, box) = link(Server(), saved: conn())
        final class Gate: @unchecked Sendable { var started = false; var open = false }
        let gate = Gate()
        let push = link.push
        link.push = { c in
            if c?.mutedFolders == ["f1"] {
                gate.started = true
                while !gate.open { try? await Task.sleep(for: .milliseconds(2)) }
            }
            await push(c)
        }
        let first = Task { await link.setMuted(["f1"]) }
        await untilAsked { gate.started }
        let second = Task { await link.setMuted(["f2"]) }  // queued behind the first
        try? await Task.sleep(for: .milliseconds(10))
        let disconnecting = Task { await link.disconnect() }
        try? await Task.sleep(for: .milliseconds(10))
        gate.open = true
        await first.value; await second.value; await disconnecting.value
        XCTAssertEqual(box.pushed.map { $0?.mutedFolders }, [["f1"], nil])
    }

    @MainActor
    func testCancelStopsWaiting() async {
        let (link, box) = link(Server(), saved: nil, reply: nil)
        link.connect()
        let task = link.waitingTask
        link.cancel()
        await task?.value
        XCTAssertEqual(link.phase, .idle)
        XCTAssertNil(link.waitingTask)
        XCTAssertTrue(box.kept.isEmpty)
        XCTAssertTrue(box.pushed.isEmpty)
    }

    @MainActor
    func testReconnectKeepsMutesOnlyForTheSameAccount() async {
        let before = conn(muted: ["f1"])
        let (same, box) = link(Server([(200, exchangeOK), (200, accountOK()), (200, paperOK)]), saved: before)
        same.connect()
        await same.waitingTask?.value
        XCTAssertEqual(box.kept.last?.mutedFolders, ["f1"])

        let (other, box2) = link(Server([(200, exchangeOK), (200, accountOK("dbid:B2")), (200, paperOK)]), saved: before)
        other.connect()
        await other.waitingTask?.value
        XCTAssertEqual(box2.kept.last?.accountID, "dbid:B2")
        XCTAssertEqual(box2.kept.last?.mutedFolders, [])
    }

    @MainActor
    func testEnsureFreshRefreshesOnlyNearTheEnd() async {
        let near = Server([(200, #"{"access_token":"a2","expires_in":14400}"#)])
        let (link, box) = link(near, saved: conn(muted: ["f1"]))
        await link.ensureFresh()
        XCTAssertEqual(near.asked.count, 1)
        XCTAssertEqual(link.saved?.token, "a2")
        XCTAssertEqual(link.saved?.refreshToken, "r1")
        XCTAssertEqual(link.saved?.expiresAt, soon.addingTimeInterval(14400))
        XCTAssertEqual(link.saved?.mutedFolders, ["f1"])
        XCTAssertEqual(box.kept.last, link.saved)
        XCTAssertEqual(box.pushed, [link.saved])

        let far = Server()
        let (l2, b2) = self.link(far, saved: conn(expires: 3 * 3600))
        await l2.ensureFresh()
        XCTAssertTrue(far.asked.isEmpty)
        XCTAssertTrue(b2.pushed.isEmpty)
    }

    @MainActor
    func testRenewForcesARefreshAtMostOnceAMinute() async {
        let server = Server([(200, #"{"access_token":"a2","expires_in":14400}"#), (200, #"{"access_token":"a3","expires_in":14400}"#)])
        let (link, box) = link(server, saved: conn(expires: 3 * 3600))
        await link.renew()
        XCTAssertEqual(server.asked.count, 1)
        await link.renew()
        XCTAssertEqual(server.asked.count, 1)
        box.now = soon.addingTimeInterval(61)
        await link.renew()
        XCTAssertEqual(server.asked.count, 2)
        XCTAssertEqual(link.saved?.token, "a3")
    }

    @MainActor
    func testARefusedRefreshMarksTheConnectionLost() async {
        let server = Server([(400, #"{"error":"invalid_grant"}"#)])
        let (link, box) = link(server, saved: conn())
        await link.ensureFresh()
        XCTAssertTrue(link.lost)
        XCTAssertEqual(link.lostAt, soon)
        XCTAssertTrue(box.pushed.isEmpty)
        XCTAssertEqual(link.saved, conn())
        // a failed refresh that was not refused changes nothing
        let (l2, b2) = self.link(Server([(400, #"{"error":"invalid_request"}"#)]), saved: conn())
        await l2.ensureFresh()
        XCTAssertFalse(l2.lost)
        XCTAssertNil(l2.lostAt)
        XCTAssertEqual(l2.saved, conn())
        XCTAssertTrue(b2.pushed.isEmpty)
    }

    @MainActor
    func testRefreshUnderLockAdoptsANewerTokenFromDisk() async {
        let server = Server()
        let disk = conn("a7", expires: 3 * 3600)
        let (link, box) = link(server, saved: conn(), disk: disk)
        await link.ensureFresh()
        XCTAssertTrue(server.asked.isEmpty)
        XCTAssertEqual(link.saved?.token, "a7")
        XCTAssertEqual(box.pushed, [disk])
        // another account's token on disk is not taken
        let other = Server([(200, #"{"access_token":"a2","expires_in":14400}"#)])
        let (l2, _) = self.link(other, saved: conn(), disk: conn("a9", expires: 3 * 3600, accountID: "dbid:B2"))
        await l2.ensureFresh()
        XCTAssertEqual(other.asked.count, 1)
        XCTAssertEqual(l2.saved?.token, "a2")
        XCTAssertEqual(l2.saved?.accountID, "dbid:A1")
    }

    @MainActor
    func testDisconnectForgetsHandsOverNilThenRevokes() async {
        let server = Server([(200, "null")])
        let (link, box) = link(server, saved: conn())
        link.seed(conn(), lost: true)
        await link.disconnect()
        XCTAssertEqual(box.events, ["forget", "push nil", "revoke"])
        XCTAssertNil(link.saved)
        XCTAssertFalse(link.lost)
        XCTAssertNil(link.lostAt)
        XCTAssertEqual(server.asked.first?.value(forHTTPHeaderField: "Authorization"), "Bearer a1")
    }

    /// `dropbox-preview` shows a made-up connection: nothing done on the card while it shows reaches the keychain,
    /// the engine or Dropbox, so the real connection is never overwritten or wiped.
    @MainActor
    func testPreviewNeverTouchesTheRealConnection() async {
        let server = Server([(200, #"{"access_token":"a2","expires_in":14400}"#), (200, "null")])
        let (link, box) = link(server, saved: nil, disk: conn("real", expires: 3 * 3600), reply: nil)
        link.load = { box.reads += 1; return box.disk }
        link.preview(conn("preview", expires: 60), lost: false)
        XCTAssertTrue(link.previewing)
        await link.setMuted(["f1"])
        await link.ensureFresh()
        await link.renew()
        await link.refreshNow(force: true)
        await link.disconnect()
        XCTAssertTrue(box.kept.isEmpty)
        XCTAssertFalse(box.forgot)
        XCTAssertTrue(box.pushed.isEmpty)
        XCTAssertTrue(server.asked.isEmpty)
        XCTAssertEqual(box.disk?.token, "real")
        // the unreadable preview: the 30-minute timer reads nothing and hands nothing over
        link.preview(nil, unreadable: true)
        XCTAssertTrue(link.previewing)
        await link.retryUnreadable()
        var sent: [DropboxConnection?] = []
        await link.handOverAtLaunch { sent.append($0) }
        XCTAssertEqual(box.reads, 0)
        XCTAssertTrue(sent.isEmpty)
        XCTAssertTrue(box.pushed.isEmpty)
        // off, reload and connect end the preview
        link.preview(nil)
        XCTAssertFalse(link.previewing)
        link.preview(conn("preview"))
        link.reload()
        XCTAssertFalse(link.previewing)
        XCTAssertEqual(link.saved?.token, "real")
        link.preview(conn("preview"))
        link.connect()
        XCTAssertFalse(link.previewing)
        XCTAssertEqual(link.saved?.token, "real")
        link.cancel()
    }

    @MainActor
    func testUnreadableKeychainHandsNothingOverAtLaunch() async {
        let (link, box) = link(Server(), saved: nil)
        link.load = { throw BrokenStore.Locked() }
        link.reload()
        XCTAssertTrue(link.unreadable)
        XCTAssertNil(link.saved)
        var sent: [DropboxConnection?] = []
        await link.handOverAtLaunch { sent.append($0) }
        XCTAssertTrue(sent.isEmpty)
        XCTAssertTrue(box.pushed.isEmpty)
        // still locked on the timer: nothing goes
        await link.retryUnreadable()
        XCTAssertTrue(box.pushed.isEmpty)
        // unlocked on the timer: the connection goes to the engine, once
        link.load = { [self] in conn() }
        await link.retryUnreadable()
        await link.retryUnreadable()
        XCTAssertFalse(link.unreadable)
        XCTAssertEqual(box.pushed, [conn()])
        await link.handOverAtLaunch { sent.append($0) }
        XCTAssertEqual(sent, [conn()])
    }

    @MainActor
    func testSetMutedKeepsTheTokenAndHandsItOver() async {
        let (link, box) = link(Server(), saved: conn())
        await link.setMuted(["f2", "f1"])
        XCTAssertEqual(link.saved?.mutedFolders, ["f1", "f2"])
        XCTAssertEqual(link.saved?.token, "a1")
        XCTAssertEqual(box.kept, [conn(muted: ["f1", "f2"])])
        XCTAssertEqual(box.pushed, [conn(muted: ["f1", "f2"])])
    }

    func testSavingFoldersKeepsMutedIdsTheListDoesNotShow() {
        XCTAssertEqual(DropboxLink.muted(shown: ["f1", "f2"], on: ["f1"], before: ["f9"]), ["f2", "f9"])
    }

    @MainActor
    func testSettleStopsWaitingAndReloadForgetsLostState() async {
        let server = Server()
        let (link, box) = link(server, saved: nil, reply: nil)
        link.connect()
        await link.settle()
        XCTAssertNil(link.waitingTask)
        // held until reload: nothing starts
        link.connect()
        XCTAssertNil(link.waitingTask)
        link.seed(conn(), lost: true)
        await link.ensureFresh()
        XCTAssertTrue(server.asked.isEmpty)
        box.disk = conn("a5", expires: 3 * 3600)
        link.reload()
        XCTAssertFalse(link.lost)
        XCTAssertNil(link.lostAt)
        XCTAssertEqual(link.phase, .idle)
        XCTAssertEqual(link.saved?.token, "a5")
        XCTAssertFalse(link.unreadable)
    }

    @MainActor
    func testWords() {
        XCTAssertEqual(DropboxLink.words(.refused), "Dropbox 연결이 끊겼어요")
        XCTAssertEqual(DropboxLink.words(.scope), "파일 읽기를 허용해야 연결돼요")
        XCTAssertEqual(DropboxLink.words(.network), "연결하지 못했어요")
        XCTAssertEqual(DropboxLink.words(.failed), "연결하지 못했어요")
    }
}
