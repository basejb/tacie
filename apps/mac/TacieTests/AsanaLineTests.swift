import XCTest
@testable import Tacie

/// The Asana card's lines (docs/32 5절, 5-1절) from the engine's `asana` block.
final class AsanaLineTests: XCTestCase {
    typealias Info = IndexState.AsanaInfo
    private let now = ISO8601DateFormatter().date(from: "2026-11-12T05:00:00Z")!  // 오후 2시 in Seoul

    private func info(_ json: String) -> Info {
        try! EngineClient.engineDecoder.decode(Info.self, from: Data(json.utf8))
    }

    private func line(_ json: String?, lost: Bool = false, lostAt: Date? = nil, signedOut: Bool = false) -> String {
        SettingsText.asanaLine(json.map(info), lost: lost, lostAt: lostAt, signedOut: signedOut, now: now)
    }

    private var pair: AsanaConnection {
        AsanaConnection(token: "t", refreshToken: "r", expiresAt: .distantFuture, userGid: "u1", userName: "Kim")
    }

    func testNeverConnectedBlockDecodes() {
        let i = info(#"{"connected": false, "refused": false, "workspaces": null, "tasks": null, "fetched_at": null, "initial_done": null, "error_kind": null, "lost_since": null, "blocked_workspaces": null}"#)
        XCTAssertFalse(i.connected)
        XCTAssertEqual(i.refused, false)
        XCTAssertNil(i.tasks)
        XCTAssertNil(i.blockedWorkspaces)
    }

    func testConnectedShowsWorkspacesAndTasks() {
        XCTAssertEqual(line(#"{"connected": true, "workspaces": 2, "tasks": 1240, "initial_done": true}"#),
                       "연결됨 · 워크스페이스 2개 · task 1,240개")
        XCTAssertEqual(line(nil), "연결됨")
    }

    func testFirstFetchSaysWhatComesNext() {
        XCTAssertEqual(line(#"{"connected": true, "workspaces": 1, "tasks": 300, "initial_done": false}"#),
                       "받은 task부터 찾아요 · 나머지는 이어서 받아요")
    }

    func testNothingToFetch() {
        XCTAssertEqual(line(#"{"connected": true, "workspaces": 1, "tasks": 0, "initial_done": true}"#), "받을 task가 없어요")
    }

    func testUnreachedBeforeAnythingFound() {
        XCTAssertEqual(line(#"{"connected": true, "tasks": null, "error_kind": "network"}"#),
                       "Asana에 닿지 못했어요. 다음 동기화에 다시 받아요")
    }

    func testBlockedWorkspacesLine() {
        XCTAssertNil(SettingsText.asanaBlockedLine(info(#"{"connected": true, "blocked_workspaces": []}"#)))
        XCTAssertNil(SettingsText.asanaBlockedLine(nil))
        XCTAssertEqual(SettingsText.asanaBlockedLine(info(#"{"connected": true, "blocked_workspaces": ["Acme"]}"#)),
                       "Acme 워크스페이스는 관리자가 막아 두어 받지 않아요")
        XCTAssertEqual(SettingsText.asanaBlockedLine(info(#"{"connected": true, "blocked_workspaces": ["Acme", "Beta"]}"#)),
                       "Acme, Beta 워크스페이스는 관리자가 막아 두어 받지 않아요")
    }

    func testLostKeepsSevenDaysThenStops() {
        // the engine's day comes first
        XCTAssertEqual(line(#"{"connected": true, "tasks": 3, "lost_since": "2026-11-10T03:00:00Z"}"#, lost: true),
                       "Asana 연결이 끊겼어요 · 다시 연결하지 않으면 11월 17일부터 찾지 않아요")
        XCTAssertEqual(line(#"{"connected": true, "tasks": 3, "lost_since": "2026-11-01T03:00:00Z"}"#, lost: true),
                       "Asana 연결이 끊겨서 찾지 않아요")
        // the engine has no day yet: the time the app saw the refusal
        let seen = ISO8601DateFormatter().date(from: "2026-11-11T03:00:00Z")!
        XCTAssertEqual(line(#"{"connected": true, "tasks": 3, "lost_since": null}"#, lost: true, lostAt: seen),
                       "Asana 연결이 끊겼어요 · 다시 연결하지 않으면 11월 18일부터 찾지 않아요")
    }

    func testLostWithoutAnyDaySaysNoDay() {
        XCTAssertEqual(line(#"{"connected": true, "tasks": 0, "lost_since": null}"#, lost: true), "Asana 연결이 끊겼어요")
        XCTAssertEqual(line(nil, lost: true), "Asana 연결이 끊겼어요")
    }

    func testEngineLostSinceAloneSaysLost() {
        // the engine's lost_since counts too, as on the other cards (the engine hides it until a new token is tried)
        XCTAssertEqual(line(#"{"connected": true, "workspaces": 1, "tasks": 3, "initial_done": true, "lost_since": "2026-11-10T03:00:00Z"}"#),
                       "Asana 연결이 끊겼어요 · 다시 연결하지 않으면 11월 17일부터 찾지 않아요")
    }

    @MainActor
    func testCardStateFollowsTheLink() {
        let link = AsanaLink(load: false)
        link.preview(pair)
        let lostSince = info(#"{"connected": true, "tasks": 3, "initial_done": true, "lost_since": "2026-11-10T03:00:00Z"}"#)
        XCTAssertEqual(AsanaStatus(link: link, info: lostSince, now: now).state, .lost)
        let fine = info(#"{"connected": true, "tasks": 3, "initial_done": true, "lost_since": null}"#)
        XCTAssertEqual(AsanaStatus(link: link, info: fine, now: now).state, .connected)
        link.preview(pair, lost: true)
        XCTAssertEqual(AsanaStatus(link: link, info: lostSince, now: now).state, .lost)
        XCTAssertNotNil(link.lostAt)
        link.preview(pair)
        XCTAssertNil(link.lostAt)
        let first = info(#"{"connected": true, "tasks": null}"#)
        XCTAssertEqual(AsanaStatus(link: link, info: first, stepLine: "Asana task 받는 중 · 30%", now: now).state,
                       .syncing("Asana task 받는 중 · 30%"))
        link.preview(nil, phase: .waiting)
        XCTAssertEqual(AsanaStatus(link: link, info: nil, now: now).state, .waiting)
        link.preview(nil)
        XCTAssertEqual(AsanaStatus(link: link, info: nil, now: now).state, .off)
    }

    @MainActor
    func testWordsAndSignedOut() {
        XCTAssertEqual(line(#"{"connected": true, "tasks": 3}"#, signedOut: true), "Tacie에 로그인한 뒤에 연결할 수 있어요")
        XCTAssertEqual(AsanaLink.words(.blocked), "회사 Asana 관리자가 Tacie를 허용해야 연결돼요")
        XCTAssertEqual(AsanaLink.words(.denied), "연결하지 않았어요")
        XCTAssertEqual(AsanaLink.words(.expired), "시간이 지났어요")
        XCTAssertEqual(SettingsText.step("asana"), "Asana task 받는 중")
    }

    @MainActor
    func testCountsAndLastFetched() {
        let link = AsanaLink(load: false)
        link.preview(pair)
        let offline = info(#"{"connected": true, "workspaces": 2, "tasks": 1240, "initial_done": true, "error_kind": "network", "fetched_at": "2026-11-12T02:00:00Z"}"#)
        let s = AsanaStatus(link: link, info: offline, now: now)
        XCTAssertEqual(s.counts, "워크스페이스 2개 · task 1,240개")
        XCTAssertNotNil(s.lastFetched)
        XCTAssertNil(AsanaStatus(link: link, info: info(#"{"connected": true, "workspaces": 2, "tasks": 1240, "initial_done": true}"#), now: now).lastFetched)
    }

    /// `GET /asana/scopes` says which workspace an admin blocks; an engine that leaves it out means not blocked.
    func testScopesDecodeBlocked() throws {
        let json = #"{"workspaces": [{"gid": "w1", "name": "Acme", "tasks": 3, "blocked": true}, {"gid": "w2", "name": "Beta", "tasks": 1}], "projects": []}"#
        let s = try EngineClient.engineDecoder.decode(AsanaScopes.self, from: Data(json.utf8))
        XCTAssertEqual(s.workspaces.map(\.blocked), [true, false])
    }

    /// A blocked workspace is not a pick: with only it left on, saving still waits; with it the only one, it does not.
    func testNoneOnIgnoresBlockedWorkspaces() {
        typealias W = AsanaScopes.Workspace
        let scopes = AsanaScopes(workspaces: [W(gid: "w1", name: "Acme", tasks: 3, blocked: true),
                                              W(gid: "w2", name: "Beta", tasks: 1)], projects: [])
        XCTAssertTrue(AsanaScopesBody.noneOn(scopes, onWorkspaces: ["w1"]))
        XCTAssertFalse(AsanaScopesBody.noneOn(scopes, onWorkspaces: ["w1", "w2"]))
        let onlyBlocked = AsanaScopes(workspaces: [W(gid: "w1", name: "Acme", tasks: 3, blocked: true)], projects: [])
        XCTAssertFalse(AsanaScopesBody.noneOn(onlyBlocked, onWorkspaces: []))
        XCTAssertFalse(AsanaScopesBody.noneOn(nil, onWorkspaces: []))
    }
}
