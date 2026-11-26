import XCTest
@testable import Tacie

final class MeetingStartWatchTests: XCTestCase {
    private let all = Set(MeetingApp.allCases)
    private let t0 = Date(timeIntervalSince1970: 1_000_000)

    private func at(_ s: TimeInterval) -> Date { t0 + s }

    func testBundleIDsMapToApps() {
        XCTAssertEqual(MeetingApp.of(bundleID: "us.zoom.xos"), .zoom)
        XCTAssertEqual(MeetingApp.of(bundleID: "com.google.Chrome.helper"), .browser)
        XCTAssertEqual(MeetingApp.of(bundleID: "com.apple.WebKit.GPU"), .browser)
        XCTAssertEqual(MeetingApp.of(bundleID: "com.tinyspeck.slackmacgap.helper"), .slack)
        XCTAssertEqual(MeetingApp.of(bundleID: "com.microsoft.teams2"), .teams)
        XCTAssertNil(MeetingApp.of(bundleID: "ai.tacie.app"))
        XCTAssertNil(MeetingApp.of(bundleID: "com.google.Chromecast"))  // a prefix without the dot is another app
        XCTAssertNil(MeetingApp.of(bundleID: "com.apple.CoreSpeech"))
    }

    func testAsksOnceAfterTheMicrophoneIsHeld() {
        var w = MeetingStartWatch()
        XCTAssertEqual(w.update(holding: [.zoom], now: at(0), listening: false, remote: false, allowed: all), [])
        XCTAssertEqual(w.nextCheck(now: at(0)), at(MeetingStartWatch.hold))
        XCTAssertEqual(w.update(holding: [.zoom], now: at(2), listening: false, remote: false, allowed: all), [])
        XCTAssertEqual(w.update(holding: [.zoom], now: at(3), listening: false, remote: false, allowed: all), [.ask(.zoom)])
        XCTAssertEqual(w.update(holding: [.zoom], now: at(60), listening: false, remote: false, allowed: all), [])
    }

    func testABlipIsNotAMeeting() {
        var w = MeetingStartWatch()
        _ = w.update(holding: [.slack], now: at(0), listening: false, remote: false, allowed: all)
        XCTAssertEqual(w.update(holding: [], now: at(1), listening: false, remote: false, allowed: all), [])
        XCTAssertEqual(w.update(holding: [], now: at(5), listening: false, remote: false, allowed: all), [])
    }

    func testAMuteDoesNotAskAgainButANewMeetingDoes() {
        var w = MeetingStartWatch()
        _ = w.update(holding: [.zoom], now: at(0), listening: false, remote: false, allowed: all)
        XCTAssertEqual(w.update(holding: [.zoom], now: at(4), listening: false, remote: false, allowed: all), [.ask(.zoom)])
        _ = w.update(holding: [], now: at(10), listening: false, remote: false, allowed: all)
        XCTAssertEqual(w.update(holding: [.zoom], now: at(20), listening: false, remote: false, allowed: all), [])
        XCTAssertEqual(w.update(holding: [.zoom], now: at(30), listening: false, remote: false, allowed: all), [])
        // let go for longer than `gone`: that meeting is over, the next one is asked about
        _ = w.update(holding: [], now: at(40), listening: false, remote: false, allowed: all)
        _ = w.update(holding: [], now: at(40 + MeetingStartWatch.gone), listening: false, remote: false, allowed: all)
        _ = w.update(holding: [.zoom], now: at(100), listening: false, remote: false, allowed: all)
        XCTAssertEqual(w.update(holding: [.zoom], now: at(104), listening: false, remote: false, allowed: all), [.ask(.zoom)])
    }

    func testAppsTurnedOffAreNotAsked() {
        var w = MeetingStartWatch()
        _ = w.update(holding: [.browser], now: at(0), listening: false, remote: false, allowed: all.subtracting([.browser]))
        XCTAssertEqual(w.update(holding: [.browser], now: at(5), listening: false, remote: false, allowed: all.subtracting([.browser])), [])
        XCTAssertEqual(w.update(holding: [.browser], now: at(6), listening: false, remote: false, allowed: []), [])
    }

    func testNotAskedWhileTacieListens() {
        var w = MeetingStartWatch()
        _ = w.update(holding: [.teams], now: at(0), listening: true, remote: false, allowed: all)
        XCTAssertEqual(w.update(holding: [.teams], now: at(5), listening: true, remote: false, allowed: all), [])
        // the meeting Tacie listened to ends; the call that began during it is not asked about afterwards
        XCTAssertEqual(w.update(holding: [.teams], now: at(9), listening: false, remote: false, allowed: all), [])
    }

    func testReleaseEndsTheRemoteMeetingListenedTo() {
        var w = MeetingStartWatch()
        _ = w.update(holding: [.zoom], now: at(0), listening: false, remote: false, allowed: all)
        _ = w.update(holding: [.zoom], now: at(4), listening: false, remote: false, allowed: all)  // asked, then 듣기 시작
        _ = w.update(holding: [.zoom], now: at(5), listening: true, remote: true, allowed: all)
        XCTAssertEqual(w.linked, .zoom)
        XCTAssertEqual(w.update(holding: [], now: at(10), listening: true, remote: true, allowed: all), [])
        XCTAssertEqual(w.nextCheck(now: at(10)), at(10 + MeetingStartWatch.release))
        XCTAssertEqual(w.update(holding: [], now: at(10 + MeetingStartWatch.release), listening: true, remote: true, allowed: all),
                       [.ended(.zoom)])
        // said once: 계속 듣기 keeps the meeting, nothing asks again
        XCTAssertEqual(w.update(holding: [], now: at(25), listening: true, remote: true, allowed: all), [])
    }

    func testAShortMuteDoesNotEndIt() {
        var w = MeetingStartWatch()
        _ = w.update(holding: [.zoom], now: at(0), listening: true, remote: true, allowed: all)
        _ = w.update(holding: [.zoom], now: at(4), listening: true, remote: true, allowed: all)
        _ = w.update(holding: [], now: at(5), listening: true, remote: true, allowed: all)
        XCTAssertEqual(w.update(holding: [.zoom], now: at(9), listening: true, remote: true, allowed: all), [])
        XCTAssertEqual(w.update(holding: [.zoom], now: at(30), listening: true, remote: true, allowed: all), [])
    }

    func testAnInPersonMeetingIsNotEndedByACall() {
        var w = MeetingStartWatch()
        _ = w.update(holding: [.facetime], now: at(0), listening: true, remote: false, allowed: all)
        _ = w.update(holding: [.facetime], now: at(4), listening: true, remote: false, allowed: all)
        _ = w.update(holding: [], now: at(5), listening: true, remote: false, allowed: all)
        XCTAssertEqual(w.update(holding: [], now: at(20), listening: true, remote: false, allowed: all), [])
        XCTAssertNil(w.linked)
    }

    func testTheQuestionGoesAwayWhenTheMeetingIsOver() {
        var w = MeetingStartWatch()
        _ = w.update(holding: [.slack], now: at(0), listening: false, remote: false, allowed: all)
        XCTAssertEqual(w.update(holding: [.slack], now: at(4), listening: false, remote: false, allowed: all), [.ask(.slack)])
        _ = w.update(holding: [], now: at(10), listening: false, remote: false, allowed: all)
        XCTAssertEqual(w.nextCheck(now: at(10)), at(10 + MeetingStartWatch.gone))
        XCTAssertEqual(w.update(holding: [], now: at(10 + MeetingStartWatch.gone), listening: false, remote: false, allowed: all),
                       [.over(.slack)])
        // an app never asked about says nothing when it lets go
        _ = w.update(holding: [.zoom], now: at(50), listening: false, remote: false, allowed: [])
        _ = w.update(holding: [], now: at(60), listening: false, remote: false, allowed: [])
        XCTAssertEqual(w.update(holding: [], now: at(60 + MeetingStartWatch.gone), listening: false, remote: false, allowed: []), [])
    }
}
