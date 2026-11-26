import XCTest
@testable import Tacie

/// The engine's `dropbox` block, folder picker rows and check key, the Dropbox keychain item in the account space, and the card's lines (docs/33).
final class DropboxLineTests: XCTestCase {
    typealias Info = IndexState.DropboxInfo

    private func info(_ json: String) -> Info {
        try! EngineClient.engineDecoder.decode(Info.self, from: Data(json.utf8))
    }

    func testNeverConnectedBlockDecodes() {  // the contract's exact block
        let i = info(#"{"connected":false,"account":null,"files":null,"fetched_at":null,"initial_done":null,"refused":false,"error_kind":null,"lost_since":null}"#)
        XCTAssertFalse(i.connected)
        XCTAssertNil(i.files)
        XCTAssertEqual(i.refused, false)
    }

    func testFoldersAndCheckDecode() throws {
        let f = try EngineClient.engineDecoder.decode([String: [DropboxFolder]].self,
            from: Data(#"{"folders":[{"id":"f1","name":"영업","path":"/영업","files":3,"team_folder":true}]}"#.utf8))["folders"]!
        XCTAssertEqual(f, [DropboxFolder(id: "f1", name: "영업", path: "/영업", files: 3, teamFolder: true)])
        let c = try EngineClient.engineDecoder.decode(IndexCheck.self, from: Data(#"{"code":null,"github":null,"dropbox":true,"changed":true}"#.utf8))
        XCTAssertEqual(c.dropbox, true)
    }

    func testCardMarkAndOpensIn() {
        let c = Card(id: "1", path: "dropbox/id:x", kind: "dropbox", source: "dropbox", byline: "Dropbox · 영업 / 제안서 · 10월 27일 수정",
                     url: "https://www.dropbox.com/home/영업?preview=a.pdf", lineStart: 1, lineEnd: 1, title: "a.pdf", snippet: "",
                     score: nil, githubUrl: nil, localPath: nil, chunk: nil, repeat: nil)
        XCTAssertEqual(c.logo, "ServiceDropbox"); XCTAssertEqual(c.symbol, "doc.richtext"); XCTAssertEqual(c.opensIn, "Dropbox")
    }

    func testSpaceAdoptionMovesTheDropboxItem() {
        XCTAssertTrue(SpaceAdoption.keychain.contains { $0 == ("ai.tacie.dropbox", "connection") })
    }

    private let now = ISO8601DateFormatter().date(from: "2026-11-12T05:00:00Z")!

    private func line(_ json: String?, lost: Bool = false, lostAt: Date? = nil) -> String {
        SettingsText.dropboxLine(json.map(info), lost: lost, lostAt: lostAt, now: now)
    }

    func testConnectedShowsAccountAndFiles() {
        XCTAssertEqual(line(#"{"connected": true, "account": "me@example.com", "files": 1240, "initial_done": true}"#),
                       "연결됨 · me@example.com · 파일 1,240개")
        XCTAssertEqual(line(nil), "연결됨")
    }

    func testFirstFetchSaysMoreComing() {
        XCTAssertEqual(line(#"{"connected": true, "account": "me@example.com", "files": 300, "initial_done": false}"#),
                       "연결됨 · me@example.com · 파일 300개 · 더 받는 중")
    }

    func testNothingToFetch() {
        XCTAssertEqual(line(#"{"connected": true, "files": 0, "initial_done": true}"#), "받을 파일이 없어요")
    }

    func testUnreachedBeforeAnythingFound() {
        XCTAssertEqual(line(#"{"connected": true, "files": null, "error_kind": "network"}"#),
                       "Dropbox에 닿지 못했어요. 다음 동기화에 다시 받아요")
        // a refused token is the app's to renew (`refused`, renew): the card stays connected, so the line does too
        XCTAssertEqual(line(#"{"connected": true, "files": null, "error_kind": "key"}"#), "연결됨")
        XCTAssertEqual(line(#"{"connected": true, "account": "me@example.com", "files": null, "initial_done": false, "error_kind": "key"}"#),
                       "연결됨 · me@example.com · 더 받는 중")
    }

    func testLostKeepsSevenDaysThenStops() {
        XCTAssertTrue(line(#"{"connected": true, "files": 3, "lost_since": "2026-11-11T05:00:00Z"}"#, lost: true).contains("11월 18일"))
        XCTAssertEqual(line(#"{"connected": true, "files": 3, "lost_since": "2026-11-01T05:00:00Z"}"#, lost: true),
                       "Dropbox 연결이 끊긴 지 7일이 지나 이제 찾지 않아요")
        let seen = ISO8601DateFormatter().date(from: "2026-11-11T03:00:00Z")!
        XCTAssertEqual(line(#"{"connected": true, "files": 3, "lost_since": null}"#, lost: true, lostAt: seen),
                       "Dropbox 연결이 끊겼어요 · 다시 연결하지 않으면 11월 18일부터 찾지 않아요")
    }

    func testLostWithoutAnyDaySaysNoDay() {
        XCTAssertEqual(line(#"{"connected": true, "files": null, "lost_since": null}"#, lost: true), "Dropbox 연결이 끊겼어요")
        XCTAssertEqual(line(nil, lost: true), "Dropbox 연결이 끊겼어요")
    }

    func testUnreadableKeychainLine() {
        XCTAssertEqual(SettingsText.dropboxUnreadable, "키체인에서 Dropbox 연결을 읽지 못했어요")
    }

    private var pair: DropboxConnection {
        DropboxConnection(token: "t", refreshToken: "r", expiresAt: .distantFuture, accountID: "dbid:A1", account: "me@example.com")
    }

    @MainActor
    func testCardStateFollowsTheLink() {
        let link = DropboxLink(load: false)
        link.preview(nil, phase: .waiting)
        XCTAssertEqual(DropboxStatus(link: link, info: nil, now: now).state, .waiting)
        link.preview(nil, phase: .failed("시간이 지났어요"))
        XCTAssertEqual(DropboxStatus(link: link, info: nil, now: now).state, .failed("시간이 지났어요"))
        link.preview(nil, unreadable: true)
        XCTAssertEqual(DropboxStatus(link: link, info: nil, now: now).state, .unreadable)
        XCTAssertEqual(DropboxStatus(link: link, info: nil, now: now).line, "키체인에서 Dropbox 연결을 읽지 못했어요")
        link.preview(nil)
        XCTAssertEqual(DropboxStatus(link: link, info: nil, now: now).state, .off)
        link.preview(pair)
        let first = info(#"{"connected": true, "files": null}"#)
        XCTAssertEqual(DropboxStatus(link: link, info: first, stepLine: "Dropbox 파일 받는 중 · 25%", now: now).state,
                       .syncing("Dropbox 파일 받는 중 · 25%"))
        link.preview(pair, lost: true)
        XCTAssertEqual(DropboxStatus(link: link, info: nil, now: now).state, .lost)
        link.preview(pair)
        let lostSince = info(#"{"connected": true, "files": 3, "initial_done": true, "lost_since": "2026-11-10T03:00:00Z"}"#)
        XCTAssertEqual(DropboxStatus(link: link, info: lostSince, now: now).state, .lost)
        let fine = info(#"{"connected": true, "account": "me@example.com", "files": 10, "initial_done": true}"#)
        XCTAssertEqual(DropboxStatus(link: link, info: fine, now: now).state, .connected)
        XCTAssertEqual(DropboxStatus(link: link, info: fine, now: now).name, "me@example.com")
    }

    @MainActor
    func testCountsAndLastFetched() {
        let link = DropboxLink(load: false)
        link.preview(pair)
        let offline = info(#"{"connected": true, "files": 1240, "initial_done": true, "error_kind": "network", "fetched_at": "2026-11-12T02:00:00Z"}"#)
        let s = DropboxStatus(link: link, info: offline, now: now)
        XCTAssertEqual(s.counts, "파일 1,240개")
        XCTAssertFalse(s.growing)
        XCTAssertTrue(s.lastFetched?.hasPrefix("마지막으로 받은 때:") == true)
        let fine = info(#"{"connected": true, "files": 1240, "initial_done": false}"#)
        XCTAssertNil(DropboxStatus(link: link, info: fine, now: now).lastFetched)
        XCTAssertTrue(DropboxStatus(link: link, info: fine, now: now).growing)
        XCTAssertNil(DropboxStatus(link: link, info: info(#"{"connected": true, "files": 0, "initial_done": true}"#), now: now).counts)
        link.preview(DropboxConnection(token: "t", refreshToken: "r", expiresAt: .distantFuture, accountID: "dbid:A1", account: nil))
        XCTAssertEqual(DropboxStatus(link: link, info: nil, now: now).name, "연결됨")
    }
}
