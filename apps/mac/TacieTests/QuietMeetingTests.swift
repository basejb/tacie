import XCTest
@testable import Tacie

final class QuietMeetingTests: XCTestCase {
    func testSilentOnlyAfterTwoMinutesWithoutALine() {
        let start = Date(timeIntervalSince1970: 1_000)
        var q = QuietMeeting(start: start)
        XCTAssertFalse(q.silent(now: start.addingTimeInterval(119)))
        XCTAssertTrue(q.silent(now: start.addingTimeInterval(120)))
        q.heard(at: start.addingTimeInterval(100))
        XCTAssertFalse(q.silent(now: start.addingTimeInterval(200)))
        XCTAssertTrue(q.silent(now: start.addingTimeInterval(220)))
    }

    func testOverOnlyAfterThreeMinutesWithoutWords() {
        let start = Date(timeIntervalSince1970: 1_000)
        var q = QuietMeeting(start: start)
        XCTAssertFalse(q.over(now: start.addingTimeInterval(179)))
        XCTAssertTrue(q.over(now: start.addingTimeInterval(180)))
        q.heard(at: start.addingTimeInterval(100), text: "그건 다음 주에 보죠")
        XCTAssertFalse(q.over(now: start.addingTimeInterval(279)))
        XCTAssertTrue(q.over(now: start.addingTimeInterval(280)))
    }

    func testSoundMarksAreNotTalk() {
        let start = Date(timeIntervalSince1970: 1_000)
        var q = QuietMeeting(start: start)
        q.heard(at: start.addingTimeInterval(170), text: "[효과음]")
        XCTAssertTrue(q.over(now: start.addingTimeInterval(180)))  // a mark does not hold the end off
        XCTAssertFalse(q.silent(now: start.addingTimeInterval(180)))  // but the mic does hear something
        q.heard(at: start.addingTimeInterval(200))  // resumed, or kept going: counts from here
        XCTAssertFalse(q.over(now: start.addingTimeInterval(379)))
        XCTAssertFalse(QuietMeeting.isSpeech("[클로징 음악] [pause]"))
        XCTAssertTrue(QuietMeeting.isSpeech("[웃음] 네 좋아요"))
        XCTAssertTrue(QuietMeeting.isSpeech("OK"))
    }

    func testEmptyPageSaysWhyAndPointsAtTheRecording() {
        XCTAssertTrue(QuietMeeting.emptyPage(inPerson: true, recorded: false).contains("원격 회의"))
        XCTAssertTrue(QuietMeeting.emptyPage(inPerson: true, recorded: true).hasSuffix("들어 볼 수 있어요."))
        XCTAssertFalse(QuietMeeting.emptyPage(inPerson: false, recorded: false).contains("녹음"))
    }
}
