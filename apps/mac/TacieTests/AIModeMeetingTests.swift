import XCTest
@testable import Tacie

/// Open edition 2-D: behavior during a meeting. With nothing chosen it says what it will do, and in On-device
/// processing with no local server it does not send the meeting notes. In Choose providers,
/// answer cards set to "Off" mean recording only.
final class AIModeMeetingTests: XCTestCase {
    func testNoModeKeyLine() {
        XCTAssertEqual(OpenAIMeeting.troubleLine(kind: "key", open: true, mode: nil), "받아쓸 방법을 골라 주세요")
    }

    func testKeyModeKeepsKeyError() {
        XCTAssertNil(OpenAIMeeting.troubleLine(kind: "key", open: true, mode: .key))
        XCTAssertNil(OpenAIMeeting.troubleLine(kind: "down", open: true, mode: nil))
    }

    func testOfficialIgnoresMode() {
        XCTAssertNil(OpenAIMeeting.troubleLine(kind: "key", open: false, mode: nil))
        XCTAssertTrue(OpenAIMeeting.notesGo(open: false, mode: .local, server: nil))
        XCTAssertFalse(OpenAIMeeting.cardsOff(open: false, mode: .custom, cards: .off))
    }

    private let ollama = LocalServer(name: "Ollama", url: "http://127.0.0.1:11434/v1", model: "gemma4:e4b")

    func testLocalWithoutServerSkipsNotes() {
        XCTAssertFalse(OpenAIMeeting.notesGo(open: true, mode: .local, server: nil))
        XCTAssertEqual(OpenAIMeeting.noServerNotes, "회의 정리는 설정 → AI에서 모델을 받으면 돼요")
    }

    func testLocalWithServerGoes() {
        XCTAssertTrue(OpenAIMeeting.notesGo(open: true, mode: .local, server: ollama))
    }

    func testKeyModeGoes() {
        XCTAssertTrue(OpenAIMeeting.notesGo(open: true, mode: .key, server: nil))
        XCTAssertTrue(OpenAIMeeting.notesGo(open: true, mode: nil, server: nil))
    }

    func testCardsOffStartsRecordOnly() {
        XCTAssertTrue(OpenAIMeeting.cardsOff(open: true, mode: .custom, cards: .off))
        XCTAssertFalse(OpenAIMeeting.cardsOff(open: true, mode: .key, cards: .off))
        XCTAssertTrue(CardsGate.startsRecordOnly(practice: false, setting: false, open: true, jev: true, cardsOff: true))
        XCTAssertFalse(CardsGate.startsRecordOnly(practice: true, setting: false, open: true, jev: true, cardsOff: true))
    }

    func testCardsOffCannotTurnOn() {
        XCTAssertFalse(CardsGate.canTurnCardsOn(open: true, jev: true, cardsOff: true))
        XCTAssertTrue(CardsGate.canTurnCardsOn(open: true, jev: true, cardsOff: false))
    }

    // final review: "Mac 안에서만" while the speech model downloads has no key to fail on; say what it waits for
    func testLocalKeyFailureSaysModel() {
        XCTAssertEqual(OpenAIMeeting.troubleLine(kind: "key", open: true, mode: .local), "음성 인식 모델을 받으면 받아써요")
        XCTAssertNil(OpenAIMeeting.troubleLine(kind: "key", open: false, mode: .local))
    }
    // A typed question with no TypeSafe or OpenRouter key still gets search cards; the line says what the answer
    // needs, not "choose how to transcribe" or "the speech model is downloading" (those are about meeting audio).
    func testTypedAskKeyLine() {
        XCTAssertEqual(OpenAIMeeting.askTroubleLine(kind: "key", open: true, mode: nil), CardsGate.needsKey)
        XCTAssertEqual(OpenAIMeeting.askTroubleLine(kind: "key", open: true, mode: .local), CardsGate.needsKey)
        XCTAssertNil(OpenAIMeeting.askTroubleLine(kind: "key", open: true, mode: .key))
        XCTAssertNil(OpenAIMeeting.askTroubleLine(kind: "key", open: true, mode: .custom))
        XCTAssertNil(OpenAIMeeting.askTroubleLine(kind: "down", open: true, mode: .local))
        XCTAssertNil(OpenAIMeeting.askTroubleLine(kind: "key", open: false, mode: nil))
    }

    // 오픈소스판 e2e (2026-11-25): the main window AI said "서비스 키를 확인해 주세요" with nothing chosen, and "외부 서비스가
    // 응답하지 않아요" when this Mac's Ollama failed in on-device mode.
    func testFailureLineInTheOpenEdition() {
        XCTAssertEqual(OpenAIMeeting.failureLine(kind: "key", open: true, mode: nil), OpenAIMeeting.chooseInSettings)
        XCTAssertEqual(OpenAIMeeting.failureLine(kind: "key", open: true, mode: .key), "OpenRouter 키를 확인해 주세요")
        // no key at all in API key mode: add one, not "check" it (2026-11-26)
        XCTAssertEqual(OpenAIMeeting.failureLine(kind: "key", open: true, mode: .key, keySaved: false), OpenAIMeeting.addKeyInSettings)
        XCTAssertEqual(OpenAIMeeting.failureLine(kind: "down", open: true, mode: .local), "이 Mac의 회의 정리 모델이 응답하지 않아요")
        XCTAssertEqual(OpenAIMeeting.failureLine(kind: "busy", open: true, mode: .local), "이 Mac의 회의 정리 모델이 응답하지 않아요")
        XCTAssertNil(OpenAIMeeting.failureLine(kind: "down", open: true, mode: .key))
        XCTAssertNil(OpenAIMeeting.failureLine(kind: "key", open: true, mode: .custom))
        XCTAssertNil(OpenAIMeeting.failureLine(kind: "model", open: true, mode: .local))
        XCTAssertNil(OpenAIMeeting.failureLine(kind: "key", open: false, mode: nil))
    }

    // the 정리 tab was blank when the notes came back empty
    func testEmptyNotesLine() {
        XCTAssertTrue(EmptyNotes.line(transcribed: false, open: true, mode: nil).contains("설정 → AI"))
        XCTAssertEqual(EmptyNotes.line(transcribed: false, open: true, mode: .local), "이 회의에서는 받아쓴 말이 없어서 요약을 쓰지 못했어요.")
        XCTAssertEqual(EmptyNotes.line(transcribed: false, open: false, mode: nil), "이 회의에서는 받아쓴 말이 없어서 요약을 쓰지 못했어요.")
        XCTAssertEqual(EmptyNotes.line(transcribed: true, open: true, mode: nil), "정리할 내용을 찾지 못했어요.")
    }
}
