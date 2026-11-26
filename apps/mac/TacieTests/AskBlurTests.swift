import XCTest
@testable import Tacie

final class AskBlurTests: XCTestCase {
    func testAskBoxAnswerClosesOutsideAMeeting() {
        XCTAssertTrue(AskBlur.closes(asking: true, typedAnswer: true, inMeeting: false, held: false))
        XCTAssertTrue(AskBlur.closes(asking: false, typedAnswer: true, inMeeting: false, held: false))
    }

    // the field open with nothing asked yet goes too, as Spotlight's does
    func testEmptyAskFieldCloses() {
        XCTAssertTrue(AskBlur.closes(asking: true, typedAnswer: false, inMeeting: false, held: false))
    }

    func testMeetingAnswersStay() {
        // a heard answer, and a typed one asked during the meeting
        XCTAssertFalse(AskBlur.closes(asking: false, typedAnswer: false, inMeeting: true, held: false))
        XCTAssertFalse(AskBlur.closes(asking: true, typedAnswer: true, inMeeting: true, held: false))
    }

    // a Slack answer, or a heard one left on the island after the meeting ended
    func testOtherAnswersStay() {
        XCTAssertFalse(AskBlur.closes(asking: false, typedAnswer: false, inMeeting: false, held: false))
    }

    func testOnboardingHoldsTheAsk() {
        XCTAssertFalse(AskBlur.closes(asking: true, typedAnswer: true, inMeeting: false, held: true))
    }
}
