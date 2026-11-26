import XCTest
@testable import Tacie

/// Open edition stage 3: a pasted token is checked right away. Tested with fake responses only (no real tokens).
final class PastedTokensTests: XCTestCase {
    private final class Seen: @unchecked Sendable { var requests: [URLRequest] = [] }

    private func respond(_ table: [String: (Int, String)], seen: Seen = Seen()) -> AIChecks.Get {
        { req in
            seen.requests.append(req)
            guard let (status, body) = table[req.url!.absoluteString] else { throw URLError(.cannotConnectToHost) }
            return (Data(body.utf8), HTTPURLResponse(url: req.url!, statusCode: status, httpVersion: nil, headerFields: nil)!)
        }
    }

    private let github = "https://api.github.com/user"
    private let notion = "https://api.notion.com/v1/users/me"
    private let linear = "https://api.linear.app/graphql"
    private let asana = "https://app.asana.com/api/1.0/users/me?opt_fields=gid,name"
    private let tenant = "https://myteam.atlassian.net/_edge/tenant_info"
    private let myself = "https://myteam.atlassian.net/rest/api/3/myself"

    func testGitHubOk() async {
        let r = await PastedTokens.check(.github, token: "tok", get: respond([github: (200, #"{"login":"me"}"#)]))
        XCTAssertEqual(r, .ok(PastedTokens.Found(name: "me")))
    }

    func testRejected401() async {
        let r = await PastedTokens.check(.github, token: "tok", get: respond([github: (401, "")]))
        XCTAssertEqual(r, .rejected)
    }

    func testRejected403() async {
        let r = await PastedTokens.check(.github, token: "tok", get: respond([github: (403, "")]))
        XCTAssertEqual(r, .rejected)
    }

    func testUnknownOn500() async {
        let r = await PastedTokens.check(.github, token: "tok", get: respond([github: (500, "")]))
        XCTAssertEqual(r, .unknown)
    }

    func testUnknownOnThrow() async {
        let r = await PastedTokens.check(.github, token: "tok", get: respond([:]))
        XCTAssertEqual(r, .unknown)
    }

    func testTokenTrimmed() async {
        let seen = Seen()
        _ = await PastedTokens.check(.github, token: "  tok\n", get: respond([github: (200, #"{"login":"me"}"#)], seen: seen))
        XCTAssertEqual(seen.requests.first?.value(forHTTPHeaderField: "Authorization"), "Bearer tok")
    }

    func testNotionReadsWorkspaceName() async {
        let body = #"{"object":"user","type":"bot","bot":{"workspace_name":"팀 위키"}}"#
        let r = await PastedTokens.check(.notion, token: "ntn_x", get: respond([notion: (200, body)]))
        XCTAssertEqual(r, .ok(PastedTokens.Found(name: "팀 위키")))
    }

    func testNotionSendsVersionHeader() async {
        let seen = Seen()
        _ = await PastedTokens.check(.notion, token: "ntn_x", get: respond([notion: (200, "{}")], seen: seen))
        XCTAssertEqual(seen.requests.first?.value(forHTTPHeaderField: "Notion-Version"), "2022-06-28")
        XCTAssertEqual(seen.requests.first?.value(forHTTPHeaderField: "Authorization"), "Bearer ntn_x")
    }

    func testLinearPersonalKeyRawHeader() async {
        let seen = Seen()
        let body = #"{"data":{"organization":{"id":"o1","name":"Acme"}}}"#
        let r = await PastedTokens.check(.linear, token: "lin_api_x", get: respond([linear: (200, body)], seen: seen))
        XCTAssertEqual(r, .ok(PastedTokens.Found(name: "Acme", id: "o1")))
        XCTAssertEqual(seen.requests.first?.value(forHTTPHeaderField: "Authorization"), "lin_api_x")
        XCTAssertEqual(seen.requests.first?.httpMethod, "POST")
    }

    func testLinearGraphQLErrorIsRejected() async {
        let body = #"{"errors":[{"message":"Authentication required"}]}"#
        let r = await PastedTokens.check(.linear, token: "lin_api_x", get: respond([linear: (200, body)]))
        XCTAssertEqual(r, .rejected)
    }

    func testAsanaReadsGid() async {
        let body = #"{"data":{"gid":"123","name":"Kim"}}"#
        let r = await PastedTokens.check(.asana, token: "2/x", get: respond([asana: (200, body)]))
        XCTAssertEqual(r, .ok(PastedTokens.Found(name: "Kim", id: "123")))
    }

    func testAtlassianOk() async {
        let seen = Seen()
        let r = await PastedTokens.check(.atlassian, token: "tok", email: "a@b.c", site: "myteam",
                                         get: respond([tenant: (200, #"{"cloudId":"c1"}"#), myself: (200, "{}")], seen: seen))
        XCTAssertEqual(r, .ok(PastedTokens.Found(name: "myteam.atlassian.net", cloud: "c1", siteURL: "https://myteam.atlassian.net")))
        let auth = seen.requests.last?.value(forHTTPHeaderField: "Authorization")
        XCTAssertEqual(auth, "Basic " + Data("a@b.c:tok".utf8).base64EncodedString())
    }

    func testAtlassianNoSite() async {
        let r = await PastedTokens.check(.atlassian, token: "tok", email: "a@b.c", site: "myteam",
                                         get: respond([tenant: (404, "")]))
        XCTAssertEqual(r, .noSite)
    }

    func testAtlassianRejected() async {
        let r = await PastedTokens.check(.atlassian, token: "tok", email: "a@b.c", site: "myteam",
                                         get: respond([tenant: (200, #"{"cloudId":"c1"}"#), myself: (401, "")]))
        XCTAssertEqual(r, .rejected)
    }

    func testSiteURL() {
        for typed in ["myteam", "myteam.atlassian.net", "https://myteam.atlassian.net/", " http://myteam.atlassian.net "] {
            XCTAssertEqual(PastedTokens.siteURL(typed), "https://myteam.atlassian.net", typed)
        }
        XCTAssertNil(PastedTokens.siteURL(""))
        XCTAssertNil(PastedTokens.siteURL("   "))
    }

    func testGitHubMakeURLPrefills() {
        let q = PastedTokens.makeURL(.github).query ?? ""
        for part in ["name=Tacie", "contents=read", "issues=read", "pull_requests=read"] {
            XCTAssertTrue(q.contains(part), part)
        }
    }

    func testOfferedOnlyInOpenEdition() {
        XCTAssertEqual(PastedTokens.offered, Build.isOpen)
    }

    // final review: Linear answers a bad key with 400 and AUTHENTICATION_ERROR
    func testLinear400AuthErrorIsRejected() async {
        let body = #"{"errors":[{"message":"x","extensions":{"code":"AUTHENTICATION_ERROR"}}]}"#
        let r = await PastedTokens.check(.linear, token: "lin_api_x", get: respond([linear: (400, body)]))
        XCTAssertEqual(r, .rejected)
    }

    private let wiki = "https://myteam.atlassian.net/wiki/api/v2/spaces?limit=1"

    // final review: a Confluence-only site (no Jira) still connects
    func testAtlassianConfluenceOnlyIsOk() async {
        let r = await PastedTokens.check(.atlassian, token: "tok", email: "a@b.c", site: "myteam",
                                         get: respond([tenant: (200, #"{"cloudId":"c1"}"#), myself: (404, ""), wiki: (200, "{}")]))
        XCTAssertEqual(r, .ok(PastedTokens.Found(name: "myteam.atlassian.net", cloud: "c1", siteURL: "https://myteam.atlassian.net")))
    }

    func testAtlassianNoJiraAccessButConfluenceIsOk() async {
        let r = await PastedTokens.check(.atlassian, token: "tok", email: "a@b.c", site: "myteam",
                                         get: respond([tenant: (200, #"{"cloudId":"c1"}"#), myself: (403, ""), wiki: (200, "{}")]))
        if case .ok = r {} else { XCTFail("\(r)") }
    }

    func testSiteURLDropsAPath() {
        XCTAssertEqual(PastedTokens.siteURL("https://myteam.atlassian.net/jira/your-work"), "https://myteam.atlassian.net")
    }
}
