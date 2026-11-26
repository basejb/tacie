import XCTest
@testable import Tacie

final class AIKeysTests: XCTestCase {
    func testCleanTrimsAndRejectsWhatCannotBeAKey() {
        XCTAssertEqual(AIKeys.clean("  sk-or-v1-abc \n"), "sk-or-v1-abc")
        XCTAssertNil(AIKeys.clean("   "))
        XCTAssertNil(AIKeys.clean("sk abc"))  // a space inside: two things pasted
        XCTAssertNil(AIKeys.clean("키"))
        XCTAssertNil(AIKeys.clean(String(repeating: "a", count: 4097)))
    }

    func testSavedNamesEveryKeySoTheEngineDropsARemovedOne() {
        let saved = AIKeys.saved { $0 == .typesafe ? " ts-key " : nil }
        XCTAssertEqual(saved.count, 2)
        XCTAssertEqual(saved[.typesafe] ?? nil, "ts-key")
        XCTAssertEqual(saved[.openrouter].map { $0 == nil }, true)  // present, and nil
    }

    func testKeychainServicesDoNotShareTheGitHubToken() {
        let services = AIKeys.Name.allCases.map { AIKeys.store($0).service }
        XCTAssertEqual(services, ["ai.tacie.openrouter", "ai.tacie.typesafe"])
        XCTAssertEqual(KeychainTokenStore().service, "ai.tacie.github")
        // a development build (tests run as one) keeps its own items next to the installed app's
        // (the open edition's development build adds `.open` the same way; BuildTests checks all four)
        XCTAssertEqual(Build.keychainSuffix, Build.isOpen ? ".open.dev" : ".dev")
        XCTAssertEqual(Build.logFolder, Build.isOpen ? "tacie-open-dev" : "tacie-dev")
        XCTAssertEqual(Build.supportFolder, Build.isOpen ? "Tacie Open Dev" : "Tacie Dev")
    }
}

final class AIKeysSaveTests: XCTestCase {
    private final class MemoryStore: TokenStore {
        var value: String?
        func read() -> String? { value }
        func save(_ token: String) throws { value = token }
        func delete() throws { value = nil }
    }

    func testSaveCleansThePasteAndRefusesWhatIsNotAKey() throws {
        let store = MemoryStore()
        try AIKeys.save("  sk-or-v1-abc\n", as: .openrouter, in: store)
        XCTAssertEqual(store.value, "sk-or-v1-abc")
        XCTAssertThrowsError(try AIKeys.save("two keys", as: .openrouter, in: store)) {
            XCTAssertEqual($0 as? AIKeys.SaveError, .notAKey)
        }
        XCTAssertEqual(store.value, "sk-or-v1-abc")
        try AIKeys.remove(.openrouter, in: store)
        XCTAssertNil(store.value)
    }

    func testKeychainItemsCarryALabel() {
        XCTAssertEqual(AIKeys.store(.openrouter).label, "Tacie OpenRouter API key")
        XCTAssertEqual(AIKeys.store(.typesafe).label, "Tacie TypeSafe API key")
    }
}
