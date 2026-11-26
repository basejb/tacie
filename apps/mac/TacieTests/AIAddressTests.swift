import XCTest
@testable import Tacie

/// The open edition's AI address (2-A): the OpenAI-compatible address, model and key that the app saves and passes
/// to the engine.
final class AIAddressTests: XCTestCase {
    private let local = AIAddress(url: "http://127.0.0.1:11434/v1", model: "gemma4:e4b")

    func testPayloadNullWithoutAddress() {
        XCTAssertTrue(AIAddress.payload(address: nil, key: nil, relay: false) is NSNull)
    }

    func testRelayWinsOverAddress() {
        XCTAssertTrue(AIAddress.payload(address: local, key: "k", relay: true) is NSNull)
    }

    func testPayloadHasUrlModelKey() {
        XCTAssertEqual(AIAddress.payload(address: local, key: "k", relay: false) as? [String: String],
                       ["url": "http://127.0.0.1:11434/v1", "model": "gemma4:e4b", "key": "k"])
        XCTAssertEqual(AIAddress.payload(address: local, key: nil, relay: false) as? [String: String],
                       ["url": "http://127.0.0.1:11434/v1", "model": "gemma4:e4b"])
    }

    // A saved key goes only to the address it was saved for: switching 회의 정리 to OpenAI or to another address with
    // the key field empty must not send the old provider's key to the new one.
    func testSavedKeyStaysWithItsAddress() {
        XCTAssertTrue(AIAddress.keepsKey(saved: local, newURL: "http://127.0.0.1:11434/v1"))
        XCTAssertTrue(AIAddress.keepsKey(saved: local, newURL: " http://127.0.0.1:11434/v1 "))
        XCTAssertFalse(AIAddress.keepsKey(saved: local, newURL: "https://api.openai.com/v1"))
        XCTAssertFalse(AIAddress.keepsKey(saved: AIAddress(url: "https://api.openai.com/v1", model: "m"), newURL: "https://example.com/v1"))
        XCTAssertFalse(AIAddress.keepsKey(saved: nil, newURL: "https://api.openai.com/v1"))
    }

    func testWaitUnchangedWithoutAddress() {
        XCTAssertEqual(EngineClient.wait(15, addressSet: false), 15)
        XCTAssertEqual(EngineClient.wait(300, ledger: true, addressSet: false), 300)
    }

    func testWaitLongerWithAddress() {
        XCTAssertEqual(EngineClient.wait(15, addressSet: true), 330)
        XCTAssertEqual(EngineClient.wait(600, addressSet: true), 600)
        XCTAssertEqual(EngineClient.wait(300, ledger: true, addressSet: true), 1600)
    }
}
