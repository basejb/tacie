import XCTest
@testable import Tacie

final class NetHealthTests: XCTestCase {
    private let t0 = Date(timeIntervalSince1970: 1_000_000)

    func testFineByDefault() {
        XCTAssertEqual(NetHealth().level(now: t0, waiting: []), .ok)
    }

    func testPathDownShowsOnlyAfterThreeSeconds() {
        var h = NetHealth()
        h.path(online: false, at: t0)
        XCTAssertEqual(h.level(now: t0.addingTimeInterval(2), waiting: []), .ok)
        XCTAssertEqual(h.level(now: t0.addingTimeInterval(3), waiting: []), .offline)
        h.path(online: true, at: t0.addingTimeInterval(4))
        XCTAssertEqual(h.level(now: t0.addingTimeInterval(4), waiting: []), .ok)
    }

    func testPathDownKeepsFirstMoment() {
        var h = NetHealth()
        h.path(online: false, at: t0)
        h.path(online: false, at: t0.addingTimeInterval(2))  // a second update while still down
        XCTAssertEqual(h.level(now: t0.addingTimeInterval(3), waiting: []), .offline)
        XCTAssertEqual(h.downFor(now: t0.addingTimeInterval(42)), 42)
    }

    func testOneNetworkFailureIsAHiccupTwoAreOffline() {
        var h = NetHealth()
        h.heard(seconds: 20, network: true)
        XCTAssertEqual(h.level(now: t0, waiting: []), .ok)
        h.heard(seconds: 20, network: true)
        XCTAssertEqual(h.level(now: t0, waiting: []), .offline)
        h.heard(seconds: 1, network: false)
        XCTAssertEqual(h.level(now: t0, waiting: []), .ok)
    }

    func testTwoSlowRepliesInARowAreSlow() {
        var h = NetHealth()
        h.heard(seconds: 7, network: false)
        XCTAssertEqual(h.level(now: t0, waiting: []), .ok)
        h.heard(seconds: 1, network: false)
        h.heard(seconds: 7, network: false)
        XCTAssertEqual(h.level(now: t0, waiting: []), .ok, "not in a row")
        h.heard(seconds: 8, network: false)
        XCTAssertEqual(h.level(now: t0, waiting: []), .slow)
    }

    func testSlowClearsAfterThreeFastReplies() {
        var h = NetHealth()
        h.heard(seconds: 7, network: false)
        h.heard(seconds: 7, network: false)
        h.heard(seconds: 1, network: false)
        h.heard(seconds: 2, network: false)
        XCTAssertEqual(h.level(now: t0, waiting: []), .slow)
        h.heard(seconds: 4, network: false)  // neither slow nor fast: starts the count over
        h.heard(seconds: 1, network: false)
        h.heard(seconds: 1, network: false)
        XCTAssertEqual(h.level(now: t0, waiting: []), .slow)
        h.heard(seconds: 1, network: false)
        XCTAssertEqual(h.level(now: t0, waiting: []), .ok)
    }

    func testHangingUtteranceIsSlow() {
        let h = NetHealth()
        XCTAssertEqual(h.level(now: t0.addingTimeInterval(7), waiting: [t0]), .ok)
        XCTAssertEqual(h.level(now: t0.addingTimeInterval(8), waiting: [t0]), .slow)
    }

    func testPileOfWaitingUtterancesIsSlow() {
        let h = NetHealth()
        XCTAssertEqual(h.level(now: t0, waiting: [t0, t0]), .ok)
        XCTAssertEqual(h.level(now: t0, waiting: [t0, t0, t0]), .slow)
    }

    func testOfflineOutranksSlow() {
        var h = NetHealth()
        h.path(online: false, at: t0)
        XCTAssertEqual(h.level(now: t0.addingTimeInterval(10), waiting: [t0, t0, t0]), .offline)
    }

    func testWords() {
        XCTAssertEqual(NetHealth.notice(recorded: true, localSTT: false), "인터넷이 끊겼어요 · 녹음은 계속돼요")
        XCTAssertEqual(NetHealth.notice(recorded: false, localSTT: false), "인터넷이 끊겼어요 · 이 동안은 받아쓰지 못해요")
        XCTAssertEqual(NetHealth.notice(recorded: true, localSTT: true), "인터넷이 끊겼어요 · 받아쓰기는 계속돼요")
        XCTAssertTrue(NetHealth.help(.offline, recorded: true, localSTT: false, downFor: 42).hasPrefix("인터넷이 끊긴 지 0:42예요."))
        XCTAssertEqual(NetHealth.help(.ok, recorded: true, localSTT: false, downFor: nil), "")
        XCTAssertEqual(NetHealth.clock(125), "2:05")
    }
}
