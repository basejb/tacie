import XCTest
@testable import Tacie

/// One app (#56): what used to follow the edition now follows whether Tacie AI runs (`AIMode.ownAI`).
final class OwnAITests: XCTestCase {
    func testOwnAIIsEverythingButTheRelay() {
        XCTAssertFalse(AIMode.ownAI(mode: .tacie, signedIn: true))
        XCTAssertTrue(AIMode.ownAI(mode: .tacie, signedIn: false))
        XCTAssertTrue(AIMode.ownAI(mode: .local, signedIn: true))
        XCTAssertTrue(AIMode.ownAI(mode: nil, signedIn: true), "nothing chosen is not Tacie AI")
    }

    func testTacieSignedOutAsksToSignIn() {
        XCTAssertEqual(OpenAIMeeting.troubleLine(kind: "key", open: true, mode: .tacie), OpenAIMeeting.signInLine)
    }

    func testLocalNeverStartsCards() {
        XCTAssertTrue(CardsGate.startsRecordOnly(practice: false, setting: false, open: AIMode.ownAI(mode: .local, signedIn: true), jev: false))
        XCTAssertFalse(CardsGate.startsRecordOnly(practice: false, setting: false, open: AIMode.ownAI(mode: .tacie, signedIn: true), jev: false))
    }
}
