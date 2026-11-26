import XCTest
@testable import Tacie

/// Open edition stage 3: the token window's flow (paste → check → save → close). Checking and saving are fake.
@MainActor
final class TokenConnectModelTests: XCTestCase {
    private final class Box {
        var checks = 0
        var connected: [(PastedTokens.Found, String, String)] = []
        var closed = 0
    }

    private func model(_ source: ConnectionsScene.Source, answers: [PastedTokens.Check], box: Box,
                       slowFirst: Bool = false) -> TokenConnectModel {
        var queue = answers
        let m = TokenConnectModel(
            source: source,
            check: { _, _, _, _ in
                box.checks += 1
                let n = box.checks
                let answer = queue.isEmpty ? PastedTokens.Check.unknown : queue.removeFirst()
                if slowFirst && n == 1 { try? await Task.sleep(for: .milliseconds(200)) }
                return answer
            },
            connect: { found, token, email in box.connected.append((found, token, email)) },
            done: { box.closed += 1 })
        m.delay = .zero
        m.closeAfter = .zero
        return m
    }

    private func settle(_ m: TokenConnectModel) async {
        for _ in 0..<50 where m.state == .checking { try? await Task.sleep(for: .milliseconds(10)) }
        try? await Task.sleep(for: .milliseconds(30))
    }

    func testOpenShowsFiveTokenSources() {
        XCTAssertEqual(ConnectionsScene.Source.available(open: true, dev: false),
                       [.github, .notion, .linear, .atlassian, .asana, .notes])
        XCTAssertEqual(ConnectionsScene.Source.available(open: true, dev: true),
                       [.github, .notion, .linear, .atlassian, .asana, .notes])
        XCTAssertEqual(ConnectionsScene.Source.available(open: false, dev: false), ConnectionsScene.Source.allCases)
    }

    func testOkConnectsAndCloses() async {
        let box = Box()
        let m = model(.linear, answers: [.ok(PastedTokens.Found(name: "Acme", id: "o1"))], box: box)
        m.token = " lin_api_x\n"
        m.changed()
        await settle(m)
        XCTAssertEqual(m.state, .ok)
        XCTAssertEqual(box.connected.count, 1)
        XCTAssertEqual(box.connected.first?.1, "lin_api_x")  // trimmed before saving
        XCTAssertEqual(box.closed, 1)
    }

    func testRejectedDoesNotSave() async {
        let box = Box()
        let m = model(.github, answers: [.rejected], box: box)
        m.token = "bad"
        m.changed()
        await settle(m)
        XCTAssertEqual(m.state, .rejected)
        XCTAssertTrue(box.connected.isEmpty)
        XCTAssertEqual(box.closed, 0)
    }

    func testUnknownDoesNotSaveAndRecheckRetries() async {
        let box = Box()
        let m = model(.asana, answers: [.unknown, .ok(PastedTokens.Found(name: "Kim", id: "u1"))], box: box)
        m.token = "2/x"
        m.changed()
        await settle(m)
        XCTAssertEqual(m.state, .unknown)
        XCTAssertTrue(box.connected.isEmpty)
        m.recheck()
        await settle(m)
        XCTAssertEqual(m.state, .ok)
        XCTAssertEqual(box.connected.count, 1)
    }

    func testNotionOkDoesNotClose() async {
        let box = Box()
        let m = model(.notion, answers: [.ok(PastedTokens.Found(name: "팀"))], box: box)
        m.token = "ntn_x"
        m.changed()
        await settle(m)
        XCTAssertEqual(m.state, .ok)
        XCTAssertEqual(box.connected.count, 1)
        XCTAssertEqual(box.closed, 0)  // step 3 is still to do; 완료 closes
    }

    func testAtlassianWaitsForThreeFields() async {
        let box = Box()
        let m = model(.atlassian, answers: [.noSite], box: box)
        m.token = "tok"
        m.changed()
        await settle(m)
        XCTAssertEqual(box.checks, 0)
        XCTAssertEqual(m.state, .empty)
        m.email = "a@b.c"
        m.site = "myteam"
        m.changed()
        await settle(m)
        XCTAssertEqual(box.checks, 1)
        XCTAssertEqual(m.state, .noSite)
    }

    func testNewTypingCancelsOldCheck() async {
        let box = Box()
        let m = model(.github, answers: [.ok(PastedTokens.Found(name: "old")), .rejected], box: box, slowFirst: true)
        m.token = "first"
        m.changed()
        try? await Task.sleep(for: .milliseconds(20))
        m.token = "second"
        m.changed()
        try? await Task.sleep(for: .milliseconds(400))
        XCTAssertEqual(m.state, .rejected)
        XCTAssertTrue(box.connected.isEmpty)  // the first check's late ok is dropped
    }

    func testEmptyTokenGoesBackToEmpty() async {
        let box = Box()
        let m = model(.github, answers: [], box: box)
        m.token = "   "
        m.changed()
        await settle(m)
        XCTAssertEqual(m.state, .empty)
        XCTAssertEqual(box.checks, 0)
    }

    // final review: 취소 before the check ends saves nothing
    func testCancelBeforeCheckEndsSavesNothing() async {
        let box = Box()
        let m = model(.linear, answers: [.ok(PastedTokens.Found(name: "Acme", id: "o1"))], box: box)
        m.delay = .milliseconds(50)
        m.token = "lin_api_x"
        m.changed()
        m.cancel()
        try? await Task.sleep(for: .milliseconds(150))
        XCTAssertTrue(box.connected.isEmpty)
        XCTAssertEqual(box.closed, 0)
    }

    // final review: the GitHub card offers 연결 in the open edition (no Tacie app, a pasted token instead)
    func testGitHubCardCanConnect() {
        XCTAssertTrue(GitHubConnectOffer.canConnect(clientID: "", pasted: true))
        XCTAssertFalse(GitHubConnectOffer.canConnect(clientID: "", pasted: false))
        XCTAssertTrue(GitHubConnectOffer.canConnect(clientID: "cid", pasted: false))
    }
}
