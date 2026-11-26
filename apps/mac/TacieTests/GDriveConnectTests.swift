import XCTest
@testable import Tacie

final class GDriveConnectTests: XCTestCase {
    /// Answers from a queue of (status, JSON) and records what it was asked.
    private final class Server: @unchecked Sendable {
        var answers: [(Int, String)]
        var asked: [URLRequest] = []
        var down = false
        init(_ answers: [(Int, String)]) { self.answers = answers }
        func body(_ i: Int) -> String { String(decoding: asked[i].httpBody ?? Data(), as: UTF8.self) }
    }

    private let client = GDriveClient(id: "cid.apps.googleusercontent.com", secret: "sec")
    private let soon = Date(timeIntervalSince1970: 1_000_000)

    private func api(_ server: Server) -> GDriveConnect {
        var api = GDriveConnect()
        api.client = client
        api.now = { [soon] in soon }
        api.send = { req in
            server.asked.append(req)
            if server.down { throw URLError(.notConnectedToInternet) }
            let (status, body) = server.answers.isEmpty ? (500, "{}") : server.answers.removeFirst()
            return (Data(body.utf8), status)
        }
        return api
    }

    private func conn(_ token: String = "a1", expires: TimeInterval = 600, muted: [String] = []) -> GDriveConnection {
        GDriveConnection(token: token, refreshToken: "r1", expiresAt: soon.addingTimeInterval(expires),
                         account: "me@example.com", accountID: "p1", muted: muted)
    }

    private final class Box: @unchecked Sendable {
        var kept: [GDriveConnection] = []
        var pushed: [GDriveConnection?] = []
        var forgot = false
        var opened: [URL] = []
    }

    @MainActor
    private func link(_ server: Server, saved: GDriveConnection?,
                      reply: LoopbackReply? = .code("code1")) -> (GDriveLink, Box) {
        let box = Box()
        let link = GDriveLink(load: false)
        link.api = api(server)
        link.now = { [soon] in soon }
        link.keep = { box.kept.append($0) }
        link.forget = { box.forgot = true }
        link.push = { box.pushed.append($0) }
        link.open = { box.opened.append($0) }
        link.listen = { _ in
            ("http://127.0.0.1:5555", {
                if let reply { return reply }
                try await Task.sleep(for: .seconds(3600))
                throw CancellationError()
            })
        }
        link.preview(saved)
        return (link, box)
    }

    private let tokenOK = #"{"access_token":"a2","refresh_token":"r2","expires_in":3599,"scope":"https://www.googleapis.com/auth/drive.readonly"}"#
    private let aboutOK = #"{"user":{"emailAddress":"me@example.com","permissionId":"p1"}}"#

    @MainActor
    private func settle(_ link: GDriveLink) async {
        await link.waitingTask?.value
    }

    func testAuthorizeURLAsksForOfflineDriveReadWithPKCE() throws {
        let url = try XCTUnwrap(api(Server([])).authorizeURL(redirect: "http://127.0.0.1:5555", state: "s", challenge: "c"))
        let q = Dictionary(uniqueKeysWithValues: URLComponents(url: url, resolvingAgainstBaseURL: false)!.queryItems!.map { ($0.name, $0.value!) })
        XCTAssertEqual(q["scope"], "https://www.googleapis.com/auth/drive.readonly")
        XCTAssertEqual(q["redirect_uri"], "http://127.0.0.1:5555")
        XCTAssertEqual(q["access_type"], "offline")
        XCTAssertEqual(q["prompt"], "consent")
        XCTAssertEqual(q["code_challenge_method"], "S256")
        XCTAssertNil(q["client_secret"], "the secret never goes to the browser")
    }

    func testDropboxPathAndStateOnly() {
        let r = { (line: String) in LoopbackReply.read(requestLine: line, state: "s1", path: "/dropbox") }
        XCTAssertEqual(r("GET /dropbox?code=c1&state=s1 HTTP/1.1"), .code("c1"))
        XCTAssertEqual(r("GET /dropbox?error=access_denied&error_description=x&state=s1 HTTP/1.1"), .error("access_denied"))
        XCTAssertNil(r("GET /other?code=c1&state=s1 HTTP/1.1"))
        XCTAssertNil(r("GET /?code=c1&state=s1 HTTP/1.1"))
        XCTAssertNil(r("GET /favicon.ico HTTP/1.1"))
        XCTAssertNil(r("GET /dropbox?code=c1&state=other HTTP/1.1"))
        // Drive keeps taking its redirect at any path
        XCTAssertEqual(LoopbackReply.read(requestLine: "GET /?code=c1&state=s1 HTTP/1.1", state: "s1"), .code("c1"))
    }

    func testListenerIgnoresStrayRequests() async throws {
        let l = try LoopbackListener(state: "s1", path: "/dropbox", pages: .dropbox)
        let port = try await l.start()
        let base = "http://127.0.0.1:\(port)"
        let (_, stray) = try await URLSession.shared.data(from: URL(string: "\(base)/favicon.ico")!)
        XCTAssertEqual((stray as? HTTPURLResponse)?.statusCode, 404)
        let (body, ok) = try await URLSession.shared.data(from: URL(string: "\(base)/dropbox?code=c1&state=s1")!)
        XCTAssertEqual((ok as? HTTPURLResponse)?.statusCode, 200)
        XCTAssertTrue(String(decoding: body, as: UTF8.self).contains(LoopbackPages.dropbox.connected))
        let got = try await l.reply()
        XCTAssertEqual(got, .code("c1"))
    }

    func testLoopbackReplyReadsOnlyTheRightState() {
        XCTAssertEqual(LoopbackReply.read(requestLine: "GET /?state=s&code=4/abc HTTP/1.1", state: "s"), LoopbackReply.code("4/abc"))
        XCTAssertEqual(LoopbackReply.read(requestLine: "GET /?error=access_denied&state=s HTTP/1.1", state: "s"), LoopbackReply.error("access_denied"))
        XCTAssertNil(LoopbackReply.read(requestLine: "GET /?state=x&code=c HTTP/1.1", state: "s"))
        XCTAssertNil(LoopbackReply.read(requestLine: "GET /favicon.ico HTTP/1.1", state: "s"))
        XCTAssertNil(LoopbackReply.read(requestLine: "POST /?state=s&code=c HTTP/1.1", state: "s"))
    }

    func testExchangeSendsTheSecretAndVerifier() async throws {
        let server = Server([(200, tokenOK)])
        let c = try await api(server).exchange(code: "c1", verifier: "v1", redirect: "http://127.0.0.1:5555")
        XCTAssertEqual(c.token, "a2")
        XCTAssertEqual(c.refreshToken, "r2")
        XCTAssertEqual(c.expiresAt, soon.addingTimeInterval(3599))
        let body = server.body(0)
        for part in ["client_secret=sec", "code_verifier=v1", "grant_type=authorization_code"] {
            XCTAssertTrue(body.contains(part), part)
        }
    }

    func testExchangeWithoutDriveScopeFails() async {
        let server = Server([(200, #"{"access_token":"a","refresh_token":"r","scope":"openid"}"#)])
        do {
            _ = try await api(server).exchange(code: "c", verifier: "v", redirect: "r")
            XCTFail("no Drive scope")
        } catch {
            XCTAssertEqual(error as? GDriveConnectError, .scope)
        }
    }

    func testRefreshKeepsTheRefreshTokenAndInvalidGrantIsRefused() async throws {
        let server = Server([(200, #"{"access_token":"a9","expires_in":3599}"#), (400, #"{"error":"invalid_grant"}"#)])
        let next = try await api(server).refresh(conn())
        XCTAssertEqual(next.token, "a9")
        XCTAssertEqual(next.refreshToken, "r1")
        XCTAssertEqual(next.muted, conn().muted)
        do {
            _ = try await api(server).refresh(conn())
            XCTFail("refused")
        } catch {
            XCTAssertEqual(error as? GDriveConnectError, .refused)
        }
    }

    @MainActor
    func testConnectSavesTheAccountAndHandsItOver() async {
        let server = Server([(200, tokenOK), (200, aboutOK)])
        let (link, box) = link(server, saved: nil)
        link.connect()
        await settle(link)
        XCTAssertEqual(box.opened.count, 1)
        XCTAssertEqual(link.phase, .idle)
        XCTAssertEqual(link.saved?.account, "me@example.com")
        XCTAssertEqual(link.saved?.accountID, "p1")
        XCTAssertEqual(box.kept.last?.accountID, "p1")
        XCTAssertEqual(box.pushed.count, 1)
        XCTAssertEqual(server.asked[1].value(forHTTPHeaderField: "Authorization"), "Bearer a2")
    }

    @MainActor
    func testDenyingSaysSoAndSavesNothing() async {
        let (link, box) = link(Server([]), saved: nil, reply: .error("access_denied"))
        link.connect()
        await settle(link)
        XCTAssertEqual(link.phase, .failed("연결하지 않았어요"))
        XCTAssertTrue(box.kept.isEmpty)
        XCTAssertNil(link.saved)
    }

    @MainActor
    func testWithoutABundledClientItSaysNotReady() {
        let (link, box) = link(Server([]), saved: nil)
        link.api.client = nil
        link.connect()
        XCTAssertEqual(link.phase, .failed("Google Drive 연결은 아직 준비 중이에요"))
        XCTAssertTrue(box.opened.isEmpty)
    }

    @MainActor
    func testCancelStopsWaiting() async {
        let (link, box) = link(Server([]), saved: nil, reply: nil)
        link.connect()
        try? await Task.sleep(for: .milliseconds(50))
        link.cancel()
        await settle(link)
        XCTAssertEqual(link.phase, .idle)
        XCTAssertTrue(box.kept.isEmpty)
    }

    @MainActor
    func testEnsureFreshRefreshesOnlyNearTheEnd() async {
        let server = Server([(200, #"{"access_token":"a9","expires_in":3599}"#)])
        let (link, box) = link(server, saved: conn(expires: 3 * 3600))
        await link.ensureFresh()
        XCTAssertTrue(server.asked.isEmpty)
        link.preview(conn(expires: 30 * 60, muted: ["d1"]))
        await link.ensureFresh()
        XCTAssertEqual(link.saved?.token, "a9")
        XCTAssertEqual(link.saved?.muted, ["d1"], "only the token fields change")
        XCTAssertEqual(box.pushed.last??.token, "a9")
    }

    @MainActor
    func testRenewForcesARefreshAtMostOnceAMinute() async {
        let ok = #"{"access_token":"a9","expires_in":3599}"#
        let server = Server([(200, ok), (200, ok)])
        let (link, _) = link(server, saved: conn(expires: 3 * 3600))
        await link.renew()
        await link.renew()
        XCTAssertEqual(server.asked.count, 1)
    }

    @MainActor
    func testARefusedRefreshMarksTheConnectionLost() async {
        let server = Server([(400, #"{"error":"invalid_grant"}"#)])
        let (link, _) = link(server, saved: conn(expires: 60))
        await link.ensureFresh()
        XCTAssertTrue(link.lost)
        XCTAssertTrue(link.connected, "found files stay searchable until the user acts")
    }

    @MainActor
    func testDisconnectForgetsHandsOverNilAndRevokes() async {
        let server = Server([(200, "{}")])
        let (link, box) = link(server, saved: conn())
        await link.disconnect()
        XCTAssertTrue(box.forgot)
        XCTAssertEqual(box.pushed.count, 1)
        XCTAssertNil(box.pushed[0])
        XCTAssertEqual(server.asked.first?.url?.host, "oauth2.googleapis.com")
        XCTAssertTrue(server.body(0).contains("token=r1"))
        XCTAssertFalse(link.connected)
    }

    @MainActor
    func testSetMutedKeepsTheTokenAndHandsItOver() async {
        let (link, box) = link(Server([]), saved: conn())
        await link.setMuted(["d2", "d1"])
        XCTAssertEqual(link.saved?.muted, ["d1", "d2"])
        XCTAssertEqual(box.pushed.last??.muted, ["d1", "d2"])
    }

    func testCardLine() {
        typealias Info = IndexState.GDriveInfo
        let now = Date(timeIntervalSince1970: 1_793_000_000)
        func info(_ files: Int?, done: Bool? = true, kind: String? = nil) -> Info {
            Info(connected: true, account: "me@example.com", refused: nil, files: files, drives: 0, fetchedAt: nil,
                 initialDone: done, errorKind: kind, lostSince: nil)
        }
        XCTAssertEqual(SettingsText.gdriveLine(info(1240), lost: false, now: now), "me@example.com · 문서 1,240개")
        XCTAssertEqual(SettingsText.gdriveLine(info(500, done: false), lost: false, now: now), "me@example.com · 문서 500개 · 더 받는 중")
        XCTAssertEqual(SettingsText.gdriveLine(info(0), lost: false, now: now), "받을 문서가 없어요")
        XCTAssertEqual(SettingsText.gdriveLine(info(nil, kind: "network"), lost: false, now: now),
                       "Google Drive에 닿지 못했어요. 다음 동기화에 다시 받아요")
        XCTAssertTrue(SettingsText.gdriveLine(info(3), lost: true, now: now).hasPrefix("Google Drive 연결이 끊겼어요 · "))
        // no day from the engine and nothing found: no day that slides with the clock
        XCTAssertEqual(SettingsText.gdriveLine(info(nil), lost: true, now: now), "Google Drive 연결이 끊겼어요")
        XCTAssertEqual(SettingsText.gdriveLine(nil, lost: true, now: now), "Google Drive 연결이 끊겼어요")
        // the engine knows before the app does: the lost line, counted from the engine's day
        let since = now.addingTimeInterval(-86_400)
        let engineLost = Info(connected: true, account: "me@example.com", refused: true, files: 3, drives: 0, fetchedAt: nil,
                              initialDone: true, errorKind: "key", lostSince: ISO8601DateFormatter().string(from: since))
        XCTAssertEqual(SettingsText.gdriveLine(engineLost, lost: false, now: now),
                       SettingsText.gdriveLine(engineLost, lost: true, now: now))
        XCTAssertTrue(SettingsText.gdriveLine(engineLost, lost: false, now: now).hasPrefix("Google Drive 연결이 끊겼어요 · "))
        XCTAssertEqual(SettingsText.gdriveLine(engineLost, lost: false, now: now.addingTimeInterval(3_600)),
                       SettingsText.gdriveLine(engineLost, lost: false, now: now))
    }

    func testSyncShareAndRows() {
        XCTAssertEqual(SyncRules.fraction(step: "gdrive", progress: 0, notion: false, gdrive: true), 0.35, accuracy: 1e-9)
        XCTAssertEqual(SyncRules.fraction(step: "embed", progress: 0, notion: false, gdrive: true), 0.5, accuracy: 1e-9)
        let rows = SyncRules.rows(step: "gdrive", progress: (3, 10), repo: "o/n", notion: false, gdrive: true)
        XCTAssertEqual(rows?.map(\.name), ["GitHub", "Google Drive", "검색 준비"])
        XCTAssertEqual(rows?[1], SyncRules.Row(name: "Google Drive", detail: "문서 3 / 10", state: .running))
    }

    func testCardSymbolFollowsTheFileKind() {
        func card(_ byline: String) -> Card {
            Card(id: "1", path: "gdrive/x", kind: "gdrive", source: "gdrive", byline: byline, url: nil, lineStart: 1, lineEnd: 1,
                 title: "t", snippet: "", score: nil, githubUrl: nil, localPath: nil, chunk: nil, repeat: nil)
        }
        XCTAssertEqual(card("Google Sheets · 내 드라이브").symbol, "tablecells")
        XCTAssertEqual(card("Google Slides · 기획팀").symbol, "rectangle.on.rectangle")
        XCTAssertEqual(card("Google Docs · 내 드라이브").symbol, "doc.richtext")
        XCTAssertEqual(card("Google Docs").opensIn, "Google Drive")
    }

    func testDriveIsOfferedInFirstStepsNotLeftovers() {
        var p = OnboardingProgress()
        p.notify = ["drive", "slack"]
        var f = OnboardingFacts()
        f.hasSource = true
        f.micGranted = true
        f.callAudio = .granted
        // connecting is the main window's 처음 써 보기 now (docs/11 14절)
        let items = OnboardingFlow.leftovers(p, f)
        XCTAssertFalse(items.contains(.connectDrive))
        XCTAssertTrue(items.contains(.notify(["slack"])))
    }
}
