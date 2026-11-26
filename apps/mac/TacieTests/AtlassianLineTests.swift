import XCTest
@testable import Tacie

/// The Jira · Confluence card's line (docs/31 5절, 5-1절) from the engine's `atlassian` block.
final class AtlassianLineTests: XCTestCase {
    typealias Info = IndexState.AtlassianInfo
    private let now = ISO8601DateFormatter().date(from: "2026-11-12T05:00:00Z")!  // 오후 2시 in Seoul
    private var seoul: Calendar {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "Asia/Seoul")!
        return c
    }

    private func info(_ json: String) -> Info {
        try! EngineClient.engineDecoder.decode(Info.self, from: Data(json.utf8))
    }

    private func line(_ json: String?, lost: Bool = false, signedOut: Bool = false) -> String {
        SettingsText.atlassianLine(json.map(info), lost: lost, signedOut: signedOut, now: now, calendar: seoul)
    }

    func testNeverConnectedBlockDecodes() {
        let i = info(#"{"connected": false, "site": null, "refused": false, "issues": null, "pages": null, "fetched_at": null, "initial_done": null, "error_kind": null, "lost_since": null, "paused_until": null}"#)
        XCTAssertFalse(i.connected)
        XCTAssertEqual(i.refused, false)
        XCTAssertNil(i.issues)
    }

    func testConnectedShowsSiteAndCounts() {
        XCTAssertEqual(line(#"{"connected": true, "site": "acme", "issues": 1240, "pages": 380, "initial_done": true}"#),
                       "연결됨 · acme · 이슈 1,240개 · 페이지 380개")
        XCTAssertEqual(line(nil), "연결됨")
    }

    func testFirstFetchSaysRecentFirst() {
        XCTAssertEqual(line(#"{"connected": true, "site": "acme", "issues": 100, "pages": 3, "initial_done": false}"#),
                       "최근 자료부터 받는 중 · 나머지는 이어서 받아요")
    }

    func testPausedSaysWhenItCarriesOn() {
        XCTAssertEqual(line(#"{"connected": true, "site": "acme", "issues": 100, "initial_done": false, "paused_until": "2026-11-12T06:00:00Z"}"#),
                       "Atlassian 한도에 걸려 잠시 멈췄어요 · 오후 3시에 이어서 받아요")
        XCTAssertEqual(line(#"{"connected": true, "issues": 100, "initial_done": false, "paused_until": "2026-11-12T06:07:00Z"}"#),
                       "Atlassian 한도에 걸려 잠시 멈췄어요 · 오후 3시 7분에 이어서 받아요")
        // a pause that is over says nothing about it
        XCTAssertEqual(line(#"{"connected": true, "site": "acme", "issues": 1, "pages": 1, "initial_done": true, "paused_until": "2026-11-12T04:00:00Z"}"#),
                       "연결됨 · acme · 이슈 1개 · 페이지 1개")
    }

    func testLostKeepsSevenDaysThenStops() {
        XCTAssertEqual(line(#"{"connected": true, "issues": 3, "lost_since": "2026-11-10T03:00:00Z"}"#, lost: true),
                       "Jira · Confluence 연결이 끊겼어요 · 다시 연결하지 않으면 11월 17일부터 찾지 않아요")
        XCTAssertEqual(line(#"{"connected": true, "issues": 3, "lost_since": "2026-11-01T03:00:00Z"}"#, lost: true),
                       "Jira · Confluence 연결이 끊겨서 찾지 않아요")
    }

    func testLostWithoutDateOrDataSaysNoDay() {
        // the engine already dropped the found items and forgot the day: no day that slides with the clock
        XCTAssertEqual(line(#"{"connected": true, "issues": 0, "pages": 0, "lost_since": null}"#, lost: true),
                       "Jira · Confluence 연결이 끊겼어요")
        XCTAssertEqual(line(#"{"connected": true, "issues": null, "pages": null}"#, lost: true),
                       "Jira · Confluence 연결이 끊겼어요")
    }

    func testEngineLostSinceShowsLostEvenIfAppDoesNot() {
        XCTAssertEqual(line(#"{"connected": true, "issues": 3, "lost_since": "2026-11-10T03:00:00Z"}"#),
                       "Jira · Confluence 연결이 끊겼어요 · 다시 연결하지 않으면 11월 17일부터 찾지 않아요")
    }

    @MainActor
    func testEngineLostSinceMakesCardLost() {
        let link = AtlassianLink(load: false)
        link.preview(AtlassianConnection(token: "t", refreshToken: "r", expiresAt: .distantFuture, cloud: "c", site: "acme",
                                         siteURL: "https://acme.atlassian.net"))
        let i = info(#"{"connected": true, "issues": 3, "lost_since": "2026-11-10T03:00:00Z"}"#)
        XCTAssertEqual(AtlassianStatus(link: link, info: i, now: now).state, .lost)
    }

    @MainActor
    func testSignedOutBlockedAndEmpty() {
        XCTAssertEqual(line(#"{"connected": true, "issues": 3}"#, signedOut: true), "Tacie에 로그인한 뒤에 연결할 수 있어요")
        XCTAssertEqual(SettingsText.atlassianBlocked, "Atlassian 조직 관리자가 바깥 앱 연결을 막아 두었어요")
        XCTAssertEqual(AtlassianLink.words(.blocked), SettingsText.atlassianBlocked)
        XCTAssertEqual(AtlassianLink.words(.signedOut), SettingsText.atlassianSignedOut)
        XCTAssertEqual(line(#"{"connected": true, "issues": 0, "pages": 0, "initial_done": true}"#), "받을 이슈와 페이지가 없어요")
        XCTAssertEqual(line(#"{"connected": true, "issues": null, "error_kind": "network"}"#),
                       "Atlassian에 닿지 못했어요. 다음 동기화에 다시 받아요")
    }
}
