import XCTest
@testable import Tacie

final class NotionConnectTests: XCTestCase {
    /// A server that answers from a queue of (status, JSON) and records what it was asked.
    private final class Server: @unchecked Sendable {
        var answers: [(Int, String)]
        var asked: [URLRequest] = []
        init(_ answers: [(Int, String)]) { self.answers = answers }
    }

    private func api(_ server: Server) -> NotionConnect {
        var api = NotionConnect()
        api.session = { "session" }
        api.send = { req in
            server.asked.append(req)
            let (status, body) = server.answers.isEmpty ? (202, #"{"pending":true}"#) : server.answers.removeFirst()
            return (Data(body.utf8), status)
        }
        return api
    }

    func testStateIsLongAndURLSafe() {
        let s = NotionConnect.newState()
        XCTAssertGreaterThanOrEqual(s.count, 32)
        XCTAssertNil(s.rangeOfCharacter(from: CharacterSet(charactersIn: "+/=")))
        XCTAssertNotEqual(s, NotionConnect.newState())
    }

    func testASignInStateCarriesTheKeyedMark() {
        XCTAssertTrue(NotionConnect.newSignInState().hasPrefix("k2_"))
    }

    func testOnlyThisSignInsReturnIsTaken() throws {
        let api = NotionConnect()
        XCTAssertEqual(try api.parseReturn(URL(string: "tacie://notion?state=S&k=K")!, state: "S"), "K")
        XCTAssertThrowsError(try api.parseReturn(URL(string: "tacie://notion?state=X&k=K")!, state: "S")) {
            XCTAssertTrue($0 is CancellationError)
        }
        XCTAssertThrowsError(try api.parseReturn(URL(string: "tacie://linear?state=S&k=K")!, state: "S")) {
            XCTAssertTrue($0 is CancellationError)
        }
        XCTAssertThrowsError(try api.parseReturn(URL(string: "tacie://notion?state=S&error=denied")!, state: "S")) {
            XCTAssertEqual($0 as? NotionConnectError, .denied)
        }
        XCTAssertThrowsError(try api.parseReturn(URL(string: "tacie://notion?state=S")!, state: "S")) {
            XCTAssertEqual($0 as? NotionConnectError, .failed)
        }
    }

    func testTheTokenIsPickedUpWithTheKeyAndTheSession() async throws {
        let server = Server([(200, #"{"token":"t","refresh_token":"r","workspace":"팀"}"#)])
        let c = try await api(server).token(state: "S", k: "K")
        XCTAssertEqual(c, NotionConnection(token: "t", refreshToken: "r", workspace: "팀"))
        let req = server.asked[0]
        XCTAssertEqual(req.httpMethod, "POST")
        XCTAssertEqual(req.url?.path, "/notion/token")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer session")
        let body = try JSONSerialization.jsonObject(with: req.httpBody ?? Data()) as? [String: String]
        XCTAssertEqual(body, ["state": "S", "k": "K"])
    }

    func testTokenErrors() async {
        for (answer, want) in [((400, #"{"error":"expired"}"#), NotionConnectError.expired),
                               ((401, #"{"error":"signed_out"}"#), .signedOut),
                               ((502, #"{"error":"failed"}"#), .failed)] {
            do {
                _ = try await api(Server([answer])).token(state: "S", k: "K")
                XCTFail("no connection")
            } catch { XCTAssertEqual(error as? NotionConnectError, want) }
        }
        var signedOut = api(Server([]))
        signedOut.session = { nil }
        do {
            _ = try await signedOut.token(state: "S", k: "K")
            XCTFail("no connection")
        } catch { XCTAssertEqual(error as? NotionConnectError, .signedOut) }
    }

    @MainActor
    func testConnectTakesTheReturnAndKeepsTheConnection() async {
        let link = NotionLink(load: false)
        var kept: [NotionConnection?] = []
        link.keep = { kept.append($0) }
        link.api = api(Server([(200, #"{"token":"t","refresh_token":"r","workspace":"팀"}"#)]))
        var shown: [URL] = []
        link.authenticate = { url in
            shown.append(url)
            let state = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == "state" }?.value ?? ""
            return URL(string: "tacie://notion?state=\(state)&k=K")!
        }
        link.connect()
        await link.waitingForTesting?.value
        XCTAssertEqual(shown.first?.path, "/notion/start")
        XCTAssertEqual(kept, [NotionConnection(token: "t", refreshToken: "r", workspace: "팀")])
        XCTAssertTrue(link.connected)
        XCTAssertEqual(link.phase, .idle)
    }

    @MainActor
    func testAReturnForAnotherSignInIsIgnored() async {
        let link = NotionLink(load: false)
        link.keep = { _ in XCTFail("nothing kept") }
        let server = Server([])
        link.api = api(server)
        link.authenticate = { _ in URL(string: "tacie://notion?state=k2_someone-else&k=K")! }
        link.connect()
        await link.waitingForTesting?.value
        XCTAssertFalse(link.connected)
        XCTAssertEqual(link.phase, .idle)
        XCTAssertTrue(server.asked.isEmpty)
    }

    func testRefreshRefusedMeansConnectAgain() async throws {
        let server = Server([(401, #"{"error":"refused"}"#)])
        do {
            _ = try await api(server).refresh("r")
            XCTFail("refused")
        } catch {
            XCTAssertEqual(error as? NotionConnectError, .refused)
        }
        let ok = Server([(200, #"{"token":"t2","refresh_token":null,"workspace":null}"#)])
        let c = try await api(ok).refresh("r")
        XCTAssertEqual(c.token, "t2")
        XCTAssertEqual(ok.asked[0].httpMethod, "POST")
        XCTAssertEqual(ok.asked[0].value(forHTTPHeaderField: "Authorization"), "Bearer session")
        // no Tacie session: not a lost connection
        do {
            _ = try await api(Server([(403, #"{"error":"signed_out"}"#)])).refresh("r")
            XCTFail("signed out")
        } catch { XCTAssertEqual(error as? NotionConnectError, .signedOut) }
    }

    @MainActor
    func testRenewKeepsTheRefreshTokenAndWorkspace() async {
        let link = NotionLink(load: false)
        link.keep = { _ in }  // never the real keychain
        let server = Server([(200, #"{"token":"t2","refresh_token":null,"workspace":null}"#)])
        link.api = api(server)
        var pushed: [NotionConnection?] = []
        link.push = { pushed.append($0) }
        link.setForTesting(NotionConnection(token: "t1", refreshToken: "r1", workspace: "팀"))
        await link.renew()
        XCTAssertEqual(pushed, [NotionConnection(token: "t2", refreshToken: "r1", workspace: "팀")])
        await link.renew()  // within ten minutes: no second try
        XCTAssertEqual(server.asked.count, 1)
        XCTAssertFalse(link.lost)
    }

    @MainActor
    func testRenewWithoutARefreshTokenIsLost() async {
        let link = NotionLink(load: false)
        link.keep = { _ in }  // never the real keychain
        link.setForTesting(NotionConnection(token: "t1", refreshToken: nil, workspace: nil))
        await link.renew()
        XCTAssertTrue(link.lost)
    }

    // 오픈소스판 3단계: an internal integration token, pasted

    @MainActor
    func testConnectPastedSavesAndPushes() async {
        let link = NotionLink(load: false)
        var kept: [NotionConnection?] = []
        link.keep = { kept.append($0) }
        var pushed: [NotionConnection?] = []
        link.push = { pushed.append($0) }
        await link.connectPasted(token: "ntn_x", workspace: "팀 위키")
        XCTAssertEqual(link.saved, NotionConnection(token: "ntn_x", refreshToken: nil, workspace: "팀 위키"))
        XCTAssertEqual(kept.last??.token, "ntn_x")
        XCTAssertEqual(pushed.last??.token, "ntn_x")
        XCTAssertFalse(link.lost)
    }

    @MainActor
    func testPastedRenewMarksLostWithoutCalling() async {
        let link = NotionLink(load: false)
        link.keep = { _ in }
        let server = Server([])
        link.api = api(server)
        await link.connectPasted(token: "ntn_x", workspace: nil)
        await link.renew()
        XCTAssertTrue(link.lost)
        XCTAssertTrue(server.asked.isEmpty)
    }
}
