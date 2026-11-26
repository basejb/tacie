import XCTest
@testable import Tacie

/// The open edition's in-meeting cards (2-C): without a key to call Jev, a meeting starts as recording only and cannot
/// switch to answer cards.
final class CardsGateTests: XCTestCase {
    func testOfficialUnchanged() {
        // 공식 앱은 키가 없어도(로그인 전) 지금처럼 설정을 따라요
        XCTAssertFalse(CardsGate.startsRecordOnly(practice: false, setting: false, open: false, jev: false))
        XCTAssertTrue(CardsGate.startsRecordOnly(practice: false, setting: true, open: false, jev: true))
        XCTAssertTrue(CardsGate.canTurnCardsOn(open: false, jev: false))
    }

    func testOpenWithoutKeyRecordsOnly() {
        XCTAssertTrue(CardsGate.startsRecordOnly(practice: false, setting: false, open: true, jev: false))
        XCTAssertFalse(CardsGate.canTurnCardsOn(open: true, jev: false))
    }

    func testOpenWithKeyFollowsSetting() {
        XCTAssertFalse(CardsGate.startsRecordOnly(practice: false, setting: false, open: true, jev: true))
        XCTAssertTrue(CardsGate.startsRecordOnly(practice: false, setting: true, open: true, jev: true))
        XCTAssertTrue(CardsGate.canTurnCardsOn(open: true, jev: true))
    }

    func testPracticeShowsCards() {
        XCTAssertFalse(CardsGate.startsRecordOnly(practice: true, setting: true, open: true, jev: false))
    }
}
