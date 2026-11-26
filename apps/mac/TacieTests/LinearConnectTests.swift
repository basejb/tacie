import XCTest
@testable import Tacie

final class LinearConnectTests: XCTestCase {
    /// Answers from a queue of (status, JSON) and records what it was asked. `slow` makes each answer wait a turn so
    /// two callers overlap.
    private final class Server: @unchecked Sendable {
        var answers: [(Int, String)]
        var asked: [URLRequest] = []
        var down = false
        init(_ answers: [(Int, String)]) { self.answers = answers }
        func body(_ i: Int) -> String { String(decoding: asked[i].httpBody ?? Data(), as: UTF8.self) }
        var tokenRequests: Int { asked.filter { $0.url?.path == "/oauth/token" }.count }
    }

    private func api(_ server: Server) -> LinearConnect {
        var api = LinearConnect(clientID: "cid")
        api.session = { "session" }
        api.send = { req in
            server.asked.append(req)
            await Task.yield()
            if server.down { throw URLError(.notConnectedToInternet) }
            let (status, body) = server.answers.isEmpty ? (202, #"{"pending":true}"#) : server.answers.removeFirst()
            return (Data(body.utf8), status)
        }
        return api
    }

    private let soon = Date(timeIntervalSince1970: 1_000_000)
    private func conn(_ token: String = "a1", refresh: String = "r1", expires: TimeInterval = 600, muted: [String] = []) -> LinearConnection {
        LinearConnection(token: token, refreshToken: refresh, expiresAt: soon.addingTimeInterval(expires),
                         workspace: "팀", workspaceID: "w1", muted: muted)
    }

    @MainActor
    private func link(_ server: Server, saved: LinearConnection?, disk: LinearConnection? = nil) -> (LinearLink, Box) {
        let box = Box()
        box.disk = disk ?? saved
        let link = LinearLink(load: false)
        link.api = api(server)
        link.now = { [soon] in soon }
        link.load = { box.disk }
        link.keep = { c in
            if box.keepFails { throw KeychainTokenStore.Failure(status: -25308) }
            box.disk = c
            box.kept.append(c)
        }
        link.forget = { box.disk = nil; box.forgot = true }
        link.marker = .init(set: { box.marker = $0 }, isSet: { box.marker })
        link.lock = .none
        link.push = { box.pushed.append($0) }
        // the sign-in window ends at this sign-in's return with a key
        link.authenticate = { url in
            let state = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == "state" }?.value ?? ""
            return URL(string: "tacie://linear?state=\(state)&k=K")!
        }
        link.setForTesting(saved)
        return (link, box)
    }

    private final class Box: @unchecked Sendable {
        var disk: LinearConnection?
        var kept: [LinearConnection] = []
        var pushed: [LinearConnection?] = []
        var keepFails = false
        var forgot = false
        var marker = false
    }

    private let tokenOK = #"{"access_token":"a2","refresh_token":"r2","expires_in":86399}"#

    @MainActor
    func testConnectWithoutAClientIDSaysNotReadyAndOpensNothing() {
        let (link, _) = link(Server([]), saved: nil)
        link.api.clientID = ""
        var opened: [URL] = []
        link.authenticate = { opened.append($0); return $0 }
        link.connect()
        XCTAssertEqual(opened, [])
        XCTAssertEqual(link.phase, .failed("Linear 연결은 아직 준비 중이에요"))
        XCTAssertNil(link.waitingTask)
    }

    func testChallengeIsBase64URLSHA256() {
        XCTAssertEqual(LinearConnect.challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
                       "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
        let v = LinearConnect.newVerifier()
        XCTAssertTrue((43...128).contains(v.count))
        XCTAssertNil(v.rangeOfCharacter(from: CharacterSet(charactersIn: "+/=")))
    }

    func testAuthorizeURLAsksForReadWithPKCE() {
        let url = LinearConnect(clientID: "cid").authorizeURL(state: "S", challenge: "C")
        let items = Dictionary(uniqueKeysWithValues: URLComponents(url: url, resolvingAgainstBaseURL: false)!
            .queryItems!.map { ($0.name, $0.value ?? "") })
        XCTAssertEqual(url.host, "linear.app")
        XCTAssertEqual(items["client_id"], "cid")
        XCTAssertEqual(items["redirect_uri"], LinearConnect.redirect)
        XCTAssertEqual(items["scope"], "read")
        XCTAssertEqual(items["state"], "S")
        XCTAssertEqual(items["code_challenge"], "C")
        XCTAssertEqual(items["code_challenge_method"], "S256")
        XCTAssertEqual(items["prompt"], "consent")
        XCTAssertEqual(items["actor"], "user")
    }

    func testASignInStateCarriesTheKeyedMark() {
        let s = LinearConnect.newSignInState()
        XCTAssertTrue(s.hasPrefix("k2_"))
        XCTAssertTrue((32...128).contains(s.count))
    }

    func testOnlyThisSignInsReturnIsTaken() throws {
        let api = LinearConnect(clientID: "cid")
        XCTAssertEqual(try api.parseReturn(URL(string: "tacie://linear?state=S&k=K")!, state: "S"), "K")
        XCTAssertThrowsError(try api.parseReturn(URL(string: "tacie://linear?state=X&k=K")!, state: "S")) {
            XCTAssertTrue($0 is CancellationError)
        }
        XCTAssertThrowsError(try api.parseReturn(URL(string: "tacie://linear?state=S&error=denied")!, state: "S")) {
            XCTAssertEqual($0 as? LinearConnectError, .denied)
        }
    }

    func testTheCodeIsPickedUpWithTheKeyAndTheSession() async throws {
        let server = Server([(200, #"{"code":"c1"}"#)])
        let code = try await api(server).code(state: "S", k: "K")
        XCTAssertEqual(code, "c1")
        XCTAssertEqual(server.asked[0].url?.path, "/linear/code")
        XCTAssertEqual(server.asked[0].httpMethod, "POST")
        XCTAssertEqual(server.asked[0].value(forHTTPHeaderField: "Authorization"), "Bearer session")
        XCTAssertEqual(try JSONSerialization.jsonObject(with: server.asked[0].httpBody ?? Data()) as? [String: String],
                       ["state": "S", "k": "K"])
    }

    func testCodeErrors() async {
        do {
            _ = try await api(Server([(400, #"{"error":"expired"}"#)])).code(state: "S", k: "K")
            XCTFail("no code")
        } catch { XCTAssertEqual(error as? LinearConnectError, .expired) }
        var signedOut = api(Server([]))
        signedOut.session = { nil }
        do {
            _ = try await signedOut.code(state: "S", k: "K")
            XCTFail("no code")
        } catch { XCTAssertEqual(error as? LinearConnectError, .signedOut) }
    }

    @MainActor
    func testAReturnForAnotherSignInIsIgnored() async {
        let server = Server([])
        let (link, box) = link(server, saved: nil)
        link.authenticate = { _ in URL(string: "tacie://linear?state=k2_someone-else&k=K")! }
        link.connect()
        await link.waitingTask?.value
        XCTAssertFalse(link.connected)
        XCTAssertEqual(link.phase, .idle)
        XCTAssertTrue(server.asked.isEmpty)
        XCTAssertTrue(box.kept.isEmpty)
    }

    func testExchangeSendsTheVerifierAndNoSecret() async throws {
        let server = Server([(200, tokenOK)])
        let c = try await api(server).exchange(code: "c1", verifier: "V")
        XCTAssertEqual(c.token, "a2")
        XCTAssertEqual(c.refreshToken, "r2")
        let body = server.body(0)
        XCTAssertTrue(body.contains("code_verifier=V"))
        XCTAssertTrue(body.contains("grant_type=authorization_code"))
        XCTAssertFalse(body.contains("client_secret"))
        XCTAssertEqual(server.asked[0].url?.absoluteString, "https://api.linear.app/oauth/token")
    }

    @MainActor
    func testTokensAreSavedBeforeTheWorkspaceQuery() async throws {
        let server = Server([(200, #"{"code":"c1"}"#), (200, tokenOK), (500, "{}")])
        let (link, box) = link(server, saved: nil)
        link.connect()
        await link.waitingTask?.value
        XCTAssertEqual(box.kept.first?.token, "a2")
        XCTAssertTrue(link.connected)
        XCTAssertEqual(box.pushed.count, 1)
    }

    func testRefreshKeepsWorkspaceAndMuted() async throws {
        let server = Server([(200, tokenOK)])
        let c = try await api(server).refresh(conn(muted: ["t2"]))
        XCTAssertEqual(c.token, "a2")
        XCTAssertEqual(c.workspaceID, "w1")
        XCTAssertEqual(c.muted, ["t2"])
        XCTAssertTrue(server.body(0).contains("grant_type=refresh_token"))
        XCTAssertFalse(server.body(0).contains("client_secret"))
    }

    func testOnlyInvalidGrantMeansConnectAgain() async {
        do {
            _ = try await api(Server([(400, #"{"error":"invalid_grant"}"#)])).refresh(conn())
            XCTFail("refused")
        } catch { XCTAssertEqual(error as? LinearConnectError, .refused) }
        do {
            _ = try await api(Server([(400, #"{"error":"invalid_request"}"#)])).refresh(conn())
            XCTFail("failed")
        } catch { XCTAssertEqual(error as? LinearConnectError, .failed) }
    }

    @MainActor
    func testTwoCallersShareOneRefresh() async {
        let server = Server([(200, tokenOK)])
        let (link, _) = link(server, saved: conn())
        async let a: Void = link.ensureFresh()
        async let b: Void = link.ensureFresh()
        _ = await (a, b)
        XCTAssertEqual(server.tokenRequests, 1)
        XCTAssertEqual(link.saved?.token, "a2")
    }

    @MainActor
    func testANewerPairInTheKeychainIsAdoptedNotRefreshed() async {
        let server = Server([(200, tokenOK)])
        let newer = conn("a9", refresh: "r9", expires: 80_000)
        let (link, box) = link(server, saved: conn(), disk: newer)
        await link.ensureFresh()
        XCTAssertEqual(server.tokenRequests, 0)
        XCTAssertEqual(link.saved, newer)
        XCTAssertEqual(box.pushed.last??.token, "a9")
    }

    @MainActor
    func testARefreshFinishingAfterReconnectIsDropped() async {
        let server = Server([(200, tokenOK)])
        let (link, box) = link(server, saved: conn())
        let other = conn("other", refresh: "rx", expires: 80_000)
        let inner = link.api.send
        link.api.send = { req in
            // a reconnect lands while the token request is out
            await MainActor.run { link.setForTesting(other); box.disk = other }
            return try await inner(req)
        }
        await link.ensureFresh()
        XCTAssertEqual(server.tokenRequests, 1)
        XCTAssertEqual(link.saved?.token, "other")
        XCTAssertFalse(box.kept.contains { $0.token == "a2" })
    }

    @MainActor
    func testAnOlderPairInTheKeychainIsNotTaken() async {
        let server = Server([(200, tokenOK)])
        let (link, _) = link(server, saved: conn("a5", refresh: "r5", expires: 80_000), disk: conn())
        await link.ensureFresh()
        XCTAssertEqual(link.saved?.token, "a5")
        XCTAssertEqual(server.tokenRequests, 0)
    }

    @MainActor
    func testAKeychainWriteFailureKeepsTheNewPairInMemory() async {
        let server = Server([(200, tokenOK)])
        let (link, box) = link(server, saved: conn())
        box.keepFails = true
        await link.ensureFresh()
        XCTAssertEqual(link.saved?.token, "a2")
        XCTAssertTrue(box.marker, "the pending marker stays until the pair is written")
        box.keepFails = false
        await link.ensureFresh()  // retries the write, no new token request
        XCTAssertEqual(box.disk?.token, "a2")
        XCTAssertEqual(server.tokenRequests, 1)
        XCTAssertFalse(box.marker)
    }

    @MainActor
    func testAPendingMarkerAtLaunchRefreshesAtOnce() async {
        let server = Server([(200, tokenOK)])
        let (link, box) = link(server, saved: conn(expires: 80_000))
        box.marker = true
        await link.ensureFresh()
        XCTAssertEqual(server.tokenRequests, 1)
        XCTAssertFalse(box.marker)
    }

    @MainActor
    func testANetworkFailureDuringRefreshKeepsTheOldToken() async {
        let server = Server([])
        server.down = true
        let (link, _) = link(server, saved: conn())
        await link.ensureFresh()
        XCTAssertEqual(link.saved?.token, "a1")
        XCTAssertFalse(link.lost)
    }

    @MainActor
    func testNoRefreshWhenMoreThanAnHourIsLeft() async {
        let server = Server([(200, tokenOK)])
        let (link, _) = link(server, saved: conn(expires: 7200))
        await link.ensureFresh()
        XCTAssertEqual(server.tokenRequests, 0)
    }

    @MainActor
    func testRenewIsThrottledToTenMinutes() async {
        let server = Server([(200, tokenOK), (200, tokenOK)])
        let (link, _) = link(server, saved: conn(expires: 80_000))
        await link.renew()
        await link.renew()
        XCTAssertEqual(server.tokenRequests, 1)
    }

    @MainActor
    func testDisconnectForgetsFirstThenRevokesBothTokens() async {
        let server = Server([(200, "{}"), (200, "{}")])
        let (link, box) = link(server, saved: conn())
        await link.disconnect()
        XCTAssertTrue(box.forgot)
        XCTAssertEqual(box.pushed.last.map { $0 == nil }, true)
        let revokes = server.asked.filter { $0.url?.path == "/oauth/revoke" }
        XCTAssertEqual(revokes.count, 2)
        XCTAssertTrue(String(decoding: revokes[0].httpBody ?? Data(), as: UTF8.self).contains("token_type_hint=refresh_token"))
        XCTAssertFalse(link.connected)
    }

    @MainActor
    func testANewWorkspaceStartsWithNoMutedTeams() async {
        let server = Server([(200, #"{"code":"c1"}"#), (200, tokenOK),
                             (200, #"{"data":{"organization":{"id":"w2","name":"다른 팀"}}}"#)])
        let (link, box) = link(server, saved: conn(muted: ["t2"]))
        link.connect()
        await link.waitingTask?.value
        XCTAssertEqual(link.saved?.workspaceID, "w2")
        XCTAssertEqual(link.saved?.muted, [])
        XCTAssertEqual(box.pushed.last??.workspace, "다른 팀")
    }

    @MainActor
    func testMutingDuringARefreshIsKept() async {
        let server = Server([(200, tokenOK)])
        let (link, box) = link(server, saved: conn())
        let inner = link.api.send
        link.api.send = { req in
            if req.url?.path == "/oauth/token" { await link.setMuted(["t2"]) }
            return try await inner(req)
        }
        await link.ensureFresh()
        XCTAssertEqual(link.saved?.token, "a2")
        XCTAssertEqual(link.saved?.muted, ["t2"])
        XCTAssertEqual(box.disk?.muted, ["t2"])
        XCTAssertEqual(box.pushed.last??.muted, ["t2"])
    }

    // 오픈소스판 3단계: a pasted personal key is saved as it is and never refreshed or revoked through OAuth

    @MainActor
    func testConnectPastedSavesAndPushes() async {
        let (link, box) = link(Server([]), saved: nil)
        var connected = 0
        link.onConnected = { connected += 1 }
        await link.connectPasted(token: "lin_api_x", org: PastedTokens.Found(name: "Acme", id: "o1"))
        XCTAssertEqual(link.saved?.token, "lin_api_x")
        XCTAssertEqual(link.saved?.pasted, true)
        XCTAssertEqual(link.saved?.workspace, "Acme")
        XCTAssertEqual(link.saved?.workspaceID, "o1")
        XCTAssertEqual(box.disk?.token, "lin_api_x")
        XCTAssertEqual(box.pushed.last??.token, "lin_api_x")
        XCTAssertEqual(connected, 1)
        XCTAssertFalse(link.lost)
    }

    @MainActor
    func testPastedRenewMarksLostWithoutCalling() async {
        let server = Server([])
        let (link, _) = link(server, saved: nil)
        link.checkPasted = { _ in .rejected }
        await link.connectPasted(token: "lin_api_x", org: PastedTokens.Found(name: "Acme", id: "o1"))
        await link.renew()
        XCTAssertTrue(link.lost)
        XCTAssertEqual(server.asked.count, 0)
    }

    @MainActor
    func testPastedEnsureFreshDoesNothing() async {
        let server = Server([])
        let (link, box) = link(server, saved: nil)
        await link.connectPasted(token: "lin_api_x", org: PastedTokens.Found(name: "Acme", id: "o1"))
        box.marker = true  // even a marker left over does not start a refresh
        await link.ensureFresh()
        XCTAssertEqual(server.asked.count, 0)
        XCTAssertFalse(link.lost)
    }

    @MainActor
    func testPastedSameWorkspaceKeepsMuted() async {
        let (link, _) = link(Server([]), saved: conn(muted: ["t1"]))
        await link.connectPasted(token: "lin_api_x", org: PastedTokens.Found(name: "팀", id: "w1"))
        XCTAssertEqual(link.saved?.muted, ["t1"])
        await link.connectPasted(token: "lin_api_y", org: PastedTokens.Found(name: "Other", id: "w2"))
        XCTAssertEqual(link.saved?.muted, [])
    }

    @MainActor
    func testPastedDisconnectDoesNotRevoke() async {
        let server = Server([])
        let (link, box) = link(server, saved: nil)
        await link.connectPasted(token: "lin_api_x", org: PastedTokens.Found(name: "Acme", id: "o1"))
        await link.disconnect()
        XCTAssertTrue(box.forgot)
        XCTAssertEqual(server.asked.count, 0)
    }

    func testOldConnectionWithoutPastedDecodes() throws {
        let json = #"{"token":"a","refreshToken":"r","expiresAt":0,"workspace":"팀","muted":[]}"#
        let c = try JSONDecoder().decode(LinearConnection.self, from: Data(json.utf8))
        XCTAssertNil(c.pasted)
        XCTAssertEqual(c.refreshToken, "r")
    }

    // final review: one refusal re-checks the pasted key before asking for a new one
    @MainActor
    func testPastedRenewKeepsAKeyThatStillWorks() async {
        let (link, _) = link(Server([]), saved: nil)
        link.checkPasted = { _ in .ok(PastedTokens.Found(name: "Acme", id: "o1")) }
        await link.connectPasted(token: "lin_api_x", org: PastedTokens.Found(name: "Acme", id: "o1"))
        await link.renew()
        XCTAssertFalse(link.lost)
    }

    // final review: no connection is saved while the account is changing
    @MainActor
    func testConnectPastedWaitsOutAnAccountChange() async {
        let (link, box) = link(Server([]), saved: nil)
        await link.settle()
        await link.connectPasted(token: "lin_api_x", org: PastedTokens.Found(name: "Acme", id: "o1"))
        XCTAssertNil(link.saved)
        XCTAssertTrue(box.kept.isEmpty)
    }
}
