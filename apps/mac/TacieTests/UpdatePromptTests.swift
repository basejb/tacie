import XCTest

final class UpdatePromptTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 2_000_000_000)
    private let day: TimeInterval = 86400

    private func shows(prompted: String? = nil, ago: TimeInterval? = nil, busy: Bool = false,
                       meetingAgo: TimeInterval? = nil) -> Bool {
        UpdatePrompt.shows(version: "0.1.23", now: now, promptedVersion: prompted,
                           promptedAt: ago.map { now.addingTimeInterval(-$0) }, busy: busy,
                           meetingEndedAt: meetingAgo.map { now.addingTimeInterval(-$0) })
    }

    func testFirstTimeForAVersionShows() {
        XCTAssertTrue(shows())
        XCTAssertTrue(shows(prompted: "0.1.22", ago: 60))
    }

    func testSameVersionWaitsSevenDays() {
        XCTAssertFalse(shows(prompted: "0.1.23", ago: 6 * day))
        XCTAssertTrue(shows(prompted: "0.1.23", ago: 7 * day))
    }

    func testNeverDuringOrJustAfterAMeeting() {
        XCTAssertFalse(shows(busy: true))
        XCTAssertFalse(shows(meetingAgo: 10 * 60))
        XCTAssertTrue(shows(meetingAgo: 31 * 60))
    }
}

final class IdleUpdateTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 2_000_000_000)

    private func installs(idle: TimeInterval = 11 * 60, busy: Bool = false, holding: Bool = false,
                          meetingAgo: TimeInterval? = nil) -> Bool {
        IdleUpdate.installs(now: now, idle: idle, busy: busy, holding: holding,
                            meetingEndedAt: meetingAgo.map { now.addingTimeInterval(-$0) })
    }

    func testInstallsAfterTenIdleMinutes() {
        XCTAssertFalse(installs(idle: 9 * 60))
        XCTAssertTrue(installs(idle: 10 * 60))
    }

    func testNeverDuringOrJustAfterAMeetingOrTheOnboarding() {
        XCTAssertFalse(installs(busy: true))
        XCTAssertFalse(installs(holding: true))
        XCTAssertFalse(installs(meetingAgo: 20 * 60))
        XCTAssertTrue(installs(meetingAgo: 31 * 60))
    }

    func testRelaunchKeepsTheMainWindowAsItWas() {
        XCTAssertEqual(IdleUpdate.reopensMain(askedAt: now.addingTimeInterval(-20), mainWasOpen: false, now: now), false)
        XCTAssertEqual(IdleUpdate.reopensMain(askedAt: now.addingTimeInterval(-20), mainWasOpen: true, now: now), true)
    }

    func testAnOrdinaryOrLateStartIsNotQuiet() {
        XCTAssertNil(IdleUpdate.reopensMain(askedAt: nil, mainWasOpen: false, now: now))
        XCTAssertNil(IdleUpdate.reopensMain(askedAt: now.addingTimeInterval(-6 * 60), mainWasOpen: false, now: now))
    }
}
