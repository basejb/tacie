import XCTest
@testable import Tacie

final class SyncRulesTests: XCTestCase {
    func testAutoSyncRules() {
        let now = DateComponents(calendar: .init(identifier: .gregorian), year: 2026, month: 10, day: 23, hour: 12).date!
        let old = now.addingTimeInterval(-21 * 3600), fresh = now.addingTimeInterval(-3600)
        func rule(_ enabled: Bool, _ connected: Bool, _ cleared: Bool, _ at: Date?, _ busy: Bool) -> Bool {
            SyncRules.autoSync(enabled: enabled, connected: connected, cleared: cleared, builtAt: at, busy: busy, now: now)
        }
        XCTAssertTrue(rule(true, true, false, old, false))
        XCTAssertFalse(rule(true, true, false, fresh, false))
        XCTAssertFalse(rule(false, true, false, old, false))
        XCTAssertFalse(rule(true, false, false, old, false))
        XCTAssertFalse(rule(true, true, true, nil, false))
        XCTAssertFalse(rule(true, true, false, old, true))
        XCTAssertTrue(rule(true, true, false, nil, false))
    }

    func testShouldCheck() {
        XCTAssertTrue(SyncRules.shouldCheck(enabled: true, connected: true, cleared: false, busy: false))
        XCTAssertFalse(SyncRules.shouldCheck(enabled: false, connected: true, cleared: false, busy: false))
        XCTAssertFalse(SyncRules.shouldCheck(enabled: true, connected: false, cleared: false, busy: false))
        XCTAssertFalse(SyncRules.shouldCheck(enabled: true, connected: true, cleared: true, busy: false))
        XCTAssertFalse(SyncRules.shouldCheck(enabled: true, connected: true, cleared: false, busy: true))
    }

    func testAfterCheckFallsBackToAgeRule() {
        let now = DateComponents(calendar: .init(identifier: .gregorian), year: 2026, month: 10, day: 24, hour: 12).date!
        let old = now.addingTimeInterval(-21 * 3600), fresh = now.addingTimeInterval(-3600)
        let changed = IndexCheck(code: true, github: nil, changed: true)
        let same = IndexCheck(code: false, github: false, changed: false)
        func rule(_ c: IndexCheck?, _ at: Date?, enabled: Bool = true, busy: Bool = false) -> Bool {
            SyncRules.afterCheck(c, enabled: enabled, connected: true, cleared: false, builtAt: at, busy: busy, now: now)
        }
        XCTAssertTrue(rule(changed, fresh))
        XCTAssertFalse(rule(changed, fresh, enabled: false))
        XCTAssertFalse(rule(changed, fresh, busy: true))
        XCTAssertTrue(rule(nil, old))  // the check failed: the 20-hour rule
        XCTAssertFalse(rule(nil, fresh))
        XCTAssertTrue(rule(same, old))
        XCTAssertFalse(rule(same, fresh))
        XCTAssertFalse(rule(same, old, busy: true))
    }

    func testRowsShowEachSourceOfTheSync() {
        XCTAssertNil(SyncRules.rows(step: "repo", progress: nil, repo: "o/r", notion: false))
        XCTAssertNil(SyncRules.rows(step: "notion", progress: nil, repo: nil, notion: true))
        let gh = SyncRules.rows(step: "sources", progress: nil, repo: "o/r", notion: true)!
        XCTAssertEqual(gh.map(\.state), [.running, .waiting, .waiting])
        XCTAssertEqual(gh[0].detail, "r")
        let notion = SyncRules.rows(step: "notion", progress: (120, 303), repo: "o/r", notion: true)!
        XCTAssertEqual(notion.map(\.state), [.done, .running, .waiting])
        XCTAssertEqual(notion[1].detail, "페이지 120 / 303")
        let embed = SyncRules.rows(step: "embed", progress: (40, 900), repo: "o/r", notion: true)!
        XCTAssertEqual(embed.map(\.state), [.done, .done, .running])
        XCTAssertNil(embed[1].detail)
        // just asked for: nothing has started
        XCTAssertEqual(SyncRules.rows(step: nil, progress: nil, repo: "o/r", notion: true)!.map(\.state),
                       [.waiting, .waiting, .waiting])
    }

    func testLinearHasItsOwnRowAndShare() {
        let rows = SyncRules.rows(step: "linear", progress: (2, 4), repo: "o/r", notion: true, linear: true)!
        XCTAssertEqual(rows.map(\.name), ["GitHub", "노션", "Linear", "검색 준비"])
        XCTAssertEqual(rows.map(\.state), [.done, .done, .running, .waiting])
        XCTAssertNotNil(SyncRules.rows(step: "repo", progress: nil, repo: "o/r", notion: false, linear: true))
        let before = SyncRules.fraction(step: "linear", progress: 0, notion: false, linear: true)
        let after = SyncRules.fraction(step: "embed", progress: 0, notion: false, linear: true)
        XCTAssertLessThan(before, after)
        XCTAssertEqual(SettingsText.step("linear"), "Linear 이슈 받는 중")
    }

    func testAtlassianHasItsOwnRowAndShare() {
        let rows = SyncRules.rows(step: "atlassian", progress: (120, 120), repo: "o/r", notion: false, atlassian: true)!
        XCTAssertEqual(rows.map(\.name), ["GitHub", "Jira · Confluence", "검색 준비"])
        XCTAssertEqual(rows[1], SyncRules.Row(name: "Jira · Confluence", detail: "이슈·페이지 120", state: .running))
        XCTAssertNotNil(SyncRules.rows(step: "repo", progress: nil, repo: "o/r", notion: false, atlassian: true))
        XCTAssertEqual(SyncRules.fraction(step: "atlassian", progress: 0, notion: false, atlassian: true), 0.35, accuracy: 1e-9)
        XCTAssertEqual(SyncRules.fraction(step: "embed", progress: 0, notion: false, atlassian: true), 0.5, accuracy: 1e-9)
        XCTAssertEqual(SyncRules.fraction(step: "embed", progress: 0, notion: false, gdrive: true, atlassian: true), 0.65, accuracy: 1e-9)
        XCTAssertEqual(SettingsText.step("atlassian"), "Jira 이슈와 Confluence 페이지 받는 중")
        XCTAssertEqual(SettingsText.syncWarning(["atlassian"]), "Jira · Confluence는 지난 데이터로 찾아요")
    }

    func testDropboxShareAndRow() {
        XCTAssertEqual(SettingsText.step("dropbox"), "Dropbox 파일 받는 중")
        XCTAssertEqual(SettingsText.syncWarning(["dropbox"]), "Dropbox는 지난 데이터로 찾아요")
        let rows = SyncRules.rows(step: "dropbox", progress: (3, 10), repo: "o/r", notion: false, dropbox: true)!
        XCTAssertEqual(rows.map(\.name), ["GitHub", "Dropbox", "검색 준비"])
        XCTAssertEqual(rows[1], .init(name: "Dropbox", detail: "파일 3 / 10", state: .running))
        let at = { (s: String) in SyncRules.fraction(step: s, progress: 0, notion: false, asana: true, dropbox: true) }
        XCTAssertLessThan(at("asana"), at("dropbox")); XCTAssertLessThan(at("dropbox"), at("embed"))
        XCTAssertEqual(at("embed"), SyncRules.fraction(step: "embed", progress: 0, notion: false, asana: true) + 0.15, accuracy: 1e-9)
    }

    func testAsanaHasItsOwnRowAndShare() {
        let rows = SyncRules.rows(step: "asana", progress: (30, 120), repo: "o/r", notion: false, asana: true)!
        XCTAssertEqual(rows.map(\.name), ["GitHub", "Asana", "검색 준비"])
        XCTAssertEqual(rows[1], SyncRules.Row(name: "Asana", detail: "task 30 / 120", state: .running))
        XCTAssertNotNil(SyncRules.rows(step: "repo", progress: nil, repo: "o/r", notion: false, asana: true))
        XCTAssertEqual(SyncRules.fraction(step: "asana", progress: 0, notion: false, asana: true), 0.35, accuracy: 1e-9)
        XCTAssertEqual(SyncRules.fraction(step: "embed", progress: 0, notion: false, asana: true), 0.5, accuracy: 1e-9)
        XCTAssertEqual(SettingsText.syncWarning(["asana"]), "Asana는 지난 데이터로 찾아요")
    }

    func testEverySourceStillLeavesRoomForEmbedding() {
        // all six connected: the sources' shares shrink together so embedding keeps its tenth and the bar never goes back
        let all = { (step: String) in
            SyncRules.fraction(step: step, progress: 0, notion: true, linear: true, gdrive: true, atlassian: true, asana: true)
        }
        let order = ["repo", "sources", "notion", "linear", "gdrive", "atlassian", "asana", "embed", "swap"].map(all)
        XCTAssertEqual(order, order.sorted())
        XCTAssertEqual(all("swap") - all("embed"), 0.1, accuracy: 1e-9)
        XCTAssertEqual(SyncRules.fraction(step: "swap", progress: 1, notion: true, linear: true, gdrive: true, atlassian: true,
                                          asana: true), 1, accuracy: 1e-9)
    }

    func testAsanaCards() {
        let c = try! JSONDecoder().decode(Card.self, from: Data(#"{"id":"c","path":"asana/task/1","kind":"asana","source":"asana","lineStart":1,"lineEnd":2,"title":"결제 개편","snippet":"s"}"#.utf8))
        XCTAssertEqual(c.opensIn, "Asana")
        XCTAssertEqual(c.logo, "ServiceAsana")
        XCTAssertEqual(c.symbol, "checklist")
    }

    func testJiraAndConfluenceCards() {
        func card(_ kind: String) -> Card {
            try! JSONDecoder().decode(Card.self, from: Data(#"{"id":"c","path":"\#(kind)/c1/1","kind":"\#(kind)","source":"\#(kind)","lineStart":1,"lineEnd":2,"title":"ENG-1 · x","snippet":"s"}"#.utf8))
        }
        XCTAssertEqual(card("jira").opensIn, "Jira")
        XCTAssertEqual(card("confluence").opensIn, "Confluence")
        XCTAssertEqual(card("jira").logo, "ServiceJira")
        XCTAssertEqual(card("confluence").logo, "ServiceConfluence")
    }

    func testLinearCardOpensInLinear() {
        let c = try! JSONDecoder().decode(Card.self, from: Data(#"{"id":"c","path":"linear/issue/1","kind":"linear","source":"linear","lineStart":1,"lineEnd":2,"title":"COR-7 · x","snippet":"s"}"#.utf8))
        XCTAssertEqual(c.opensIn, "Linear")
        XCTAssertEqual(c.symbol, "checklist")
    }

    func testLinearStatusCopy() {
        let now = ISO8601DateFormatter().date(from: "2026-11-01T03:00:00Z")!
        func info(_ json: String) -> IndexState.LinearInfo {
            let d = JSONDecoder()
            d.keyDecodingStrategy = .convertFromSnakeCase
            return try! d.decode(IndexState.LinearInfo.self, from: Data(json.utf8))
        }
        XCTAssertEqual(SettingsText.linearLine(info(#"{"connected":true,"workspace":"팀","issues":1240}"#), lost: false, now: now),
                       "팀 · 이슈 1,240개")
        XCTAssertEqual(SettingsText.linearLine(info(#"{"connected":true,"issues":0,"error_kind":"key"}"#), lost: false, now: now),
                       "워크스페이스 관리자가 연결을 허용해야 해요")
        XCTAssertEqual(SettingsText.linearLine(info(#"{"connected":true,"issues":0,"error_kind":"network"}"#), lost: false, now: now),
                       "Linear에 닿지 못했어요. 다음 동기화에 다시 받아요")
        XCTAssertEqual(SettingsText.linearLine(info(#"{"connected":true,"workspace":"팀","issues":0}"#), lost: false, now: now),
                       "받을 이슈가 없어요")
        XCTAssertEqual(SettingsText.linearLine(info(#"{"connected":true,"error_kind":"key"}"#), lost: false, now: now),
                       "워크스페이스 관리자가 연결을 허용해야 해요")
        XCTAssertEqual(SettingsText.linearLine(info(#"{"connected":true,"issues":3,"lost_since":"2026-10-30T03:00:00Z"}"#), lost: true, now: now),
                       "Linear 연결이 끊겼어요 · 다시 연결하지 않으면 11월 6일부터 Linear 자료는 찾지 않아요")
        XCTAssertEqual(SettingsText.linearLine(info(#"{"connected":true,"issues":3,"lost_since":"2026-10-20T03:00:00Z"}"#), lost: true, now: now),
                       "Linear 연결이 끊겨서 Linear 자료는 찾지 않아요")
        // no day from the engine and nothing found: no day that slides with the clock
        XCTAssertEqual(SettingsText.linearLine(info(#"{"connected":true,"issues":0}"#), lost: true, now: now), "Linear 연결이 끊겼어요")
        XCTAssertEqual(SettingsText.linearLine(nil, lost: true, now: now), "Linear 연결이 끊겼어요")
        // the engine knows before the app does: the lost line from the engine's day
        XCTAssertEqual(SettingsText.linearLine(info(#"{"connected":true,"issues":3,"lost_since":"2026-10-30T03:00:00Z"}"#), lost: false, now: now),
                       "Linear 연결이 끊겼어요 · 다시 연결하지 않으면 11월 6일부터 Linear 자료는 찾지 않아요")
    }

    /// app.log gets a fixed word from the engine's error (its real messages), never the message.
    func testGitHubReasonPicksAFixedWord() {
        XCTAssertEqual(SyncRules.githubReason("GitHub is not connected"), "not-connected")
        XCTAssertEqual(SyncRules.githubReason("GitHub answered HTTP 401 (bad credentials)"), "401")
        XCTAssertEqual(SyncRules.githubReason("GitHub answered 401 for the repo"), "401")
        XCTAssertEqual(SyncRules.githubReason("GitHub answered HTTP 403 (SAML enforcement)"), "403")
        XCTAssertEqual(SyncRules.githubReason("GitHub answered HTTP 410"), "404")
        XCTAssertEqual(SyncRules.githubReason("gh is not logged in"), "gh")
        XCTAssertEqual(SyncRules.githubReason("something else about owner/repo"), "other")
        XCTAssertEqual(SyncRules.githubReason(nil), "other")
    }
}
