import XCTest
@testable import Tacie

/// Open edition 2-D: one mode (API key · On-device processing · Choose providers; 키 하나로 · Mac 안에서만 · 직접 고르기 before
/// their renaming) decides the values passed to the engine. Saved Choose providers values are overwritten, not deleted.
final class AIModeTests: XCTestCase {
    private let address = AIAddress(url: "https://api.openai.com/v1", model: "gpt-5-mini")
    private let soniox = SttProviderChoice(name: .soniox, region: "jp")
    private let ollama = LocalServer(name: "Ollama", url: "http://127.0.0.1:11434/v1", model: "gemma4:e4b")
    private let keys: [AIKeys.Name: String?] = [.openrouter: "or-fake", .typesafe: "ts-fake"]

    private func values(_ mode: AIMode?, relay: Bool = false, server: LocalServer? = nil,
                        cards: CardsChoice = .auto, localSwitch: Bool = false) -> AIMode.EngineValues {
        AIMode.effective(mode: mode, relay: relay, keys: keys, address: address, addressKey: "oa-fake",
                         provider: soniox, providerKey: "sx-fake", localSwitch: localSwitch, server: server, cards: cards)
    }

    /// Tacie AI with a session is today's official payload, the same as nothing chosen (#56).
    func testTacieRelayIsTodaysPayload() {
        let official = values(.tacie, relay: true, server: ollama, localSwitch: true)
        let relayNone = values(nil, relay: true, localSwitch: true)
        XCTAssertEqual(official.keys, relayNone.keys)
        XCTAssertEqual(official.chat as? [String: String], relayNone.chat as? [String: String])
        XCTAssertEqual(official.stt as? [String: String], relayNone.stt as? [String: String])
        XCTAssertEqual(official.sttPath, relayNone.sttPath)
        XCTAssertEqual(official.localStt, true)
        let none = values(nil, localSwitch: true)
        // and nil is today's payload
        XCTAssertEqual(none.keys, keys)
        XCTAssertEqual(none.chat as? [String: String], ["url": "https://api.openai.com/v1", "model": "gpt-5-mini", "key": "oa-fake"])
        XCTAssertEqual(none.stt as? [String: String], ["provider": "soniox", "key": "sx-fake", "region": "jp"])
        XCTAssertEqual(none.sttPath, "soniox")
    }

    func testKeyMode() {
        let v = values(.key, localSwitch: true)
        XCTAssertEqual(v.keys, [.openrouter: "or-fake", .typesafe: nil])
        XCTAssertTrue(v.chat is NSNull)
        XCTAssertTrue(v.stt is NSNull)
        XCTAssertNil(v.sttPath)
        XCTAssertFalse(v.localStt)
    }

    func testLocalMode() {
        let v = values(.local, server: ollama)
        XCTAssertEqual(v.keys, [.openrouter: nil, .typesafe: nil])
        XCTAssertEqual(v.chat as? [String: String], ["url": "http://127.0.0.1:11434/v1", "model": "gemma4:e4b"])
        XCTAssertTrue(v.stt is NSNull)
        XCTAssertTrue(v.localStt)
    }

    func testLocalIgnoresSavedProvider() {
        let v = values(.local, server: ollama)
        XCTAssertTrue(v.stt is NSNull)
        XCTAssertNil(v.sttPath)
    }

    func testLocalWithoutServer() {
        XCTAssertTrue(values(.local).chat is NSNull)
    }

    func testCustomCardsOpenRouterDropsTypeSafe() {
        let v = values(.custom, cards: .openrouter)
        XCTAssertEqual(v.keys[.typesafe] ?? "x", nil)
        XCTAssertEqual(v.keys[.openrouter] ?? nil, "or-fake")
        XCTAssertEqual(values(.custom).keys, keys)
    }

    func testRelayWins() {
        let v = values(.local, relay: true, server: ollama)
        XCTAssertTrue(v.chat is NSNull)
        XCTAssertTrue(v.stt is NSNull)
    }

    func testModeSwitchKeepsCustomValues() throws {
        let d = UserDefaults(suiteName: "AIModeTests-\(UUID())")!
        d.set(address.url, forKey: AIAddress.urlKey)
        d.set(address.model, forKey: AIAddress.modelKey)
        d.set("soniox", forKey: SttProviderChoice.providerKey)
        d.set("jp", forKey: SttProviderChoice.regionKey)
        AIMode.save(.custom, d)
        AIMode.save(.key, d)
        XCTAssertEqual(AIMode.saved(d), .key)
        AIMode.save(.custom, d)
        XCTAssertEqual(AIMode.saved(d), .custom)
        XCTAssertEqual(AIAddress.saved(d), address)
        XCTAssertEqual(SttProviderChoice.saved(d), soniox)
        AIMode.save(nil, d)
        XCTAssertNil(AIMode.saved(d))
    }

    func testLocalServerRoundTrip() {
        let d = UserDefaults(suiteName: "AIModeTests-\(UUID())")!
        XCTAssertNil(LocalServer.saved(d))
        LocalServer.save(ollama, d)
        XCTAssertEqual(LocalServer.saved(d), ollama)
        LocalServer.save(nil, d)
        XCTAssertNil(LocalServer.saved(d))
    }

    // a dev engine reads ~/.config key files; "Mac 안에서만" must not, even in a dev build
    func testKeyFilesOffInLocalDev() {
        XCTAssertFalse(AIModeApply.keyFiles(dev: true, mode: .local, open: true))
    }

    func testKeyFilesOnInKeyDev() {
        XCTAssertTrue(AIModeApply.keyFiles(dev: true, mode: .key, open: true))
        XCTAssertTrue(AIModeApply.keyFiles(dev: true, mode: nil, open: true))
        XCTAssertFalse(AIModeApply.keyFiles(dev: false, mode: .key, open: true))
    }

    func testKeyFilesOfficialDevUnchanged() {
        XCTAssertTrue(AIModeApply.keyFiles(dev: true, mode: .local, open: false))
        XCTAssertFalse(AIModeApply.keyFiles(dev: false, mode: .local, open: false))
    }

    func testRestartOnlyAcrossLocal() {
        XCTAssertTrue(AIModeApply.needsRestart(from: .key, to: .local, dev: true, open: true))
        XCTAssertTrue(AIModeApply.needsRestart(from: .local, to: .custom, dev: true, open: true))
        XCTAssertFalse(AIModeApply.needsRestart(from: .key, to: .custom, dev: true, open: true))
        XCTAssertFalse(AIModeApply.needsRestart(from: .key, to: .local, dev: false, open: true))
        XCTAssertFalse(AIModeApply.needsRestart(from: .key, to: .local, dev: true, open: false))
    }

    // final review: 직접 고르기 shows what is saved for custom, whatever mode runs now (changing one row must not
    // rewrite the others from the mode)
    func testCustomPicksIgnoreMode() {
        let d = UserDefaults(suiteName: "AIModeTests-\(UUID())")!
        d.set("https://api.openai.com/v1", forKey: AIAddress.urlKey)
        d.set("gpt-5-mini", forKey: AIAddress.modelKey)
        d.set("off", forKey: CardsChoice.defaultsKey)
        AIMode.save(.key, d)
        XCTAssertEqual(AICustomPicks.chat(d), .openai)
        XCTAssertEqual(AICustomPicks.cards(d, typesafeKey: false), .off)
        AIMode.save(.local, d)
        XCTAssertEqual(AICustomPicks.chat(d), .openai)
    }

    // final review: the engine-wait hook reads UserDefaults only (it runs on every engine request, off the main thread)
    func testAddressSetFromDefaults() {
        let d = UserDefaults(suiteName: "AIModeTests-\(UUID())")!
        XCTAssertFalse(AIMode.addressSet(d))
        LocalServer.save(ollamaServer, d)
        AIMode.save(.local, d)
        XCTAssertTrue(AIMode.addressSet(d))
        AIMode.save(.key, d)
        d.set("https://x/v1", forKey: AIAddress.urlKey); d.set("m", forKey: AIAddress.modelKey)
        XCTAssertFalse(AIMode.addressSet(d))
        AIMode.save(.tacie, d)
        XCTAssertTrue(AIMode.addressSet(d))  // Tacie AI: today's `AIAddress.isSet`
        AIMode.save(.custom, d)
        XCTAssertTrue(AIMode.addressSet(d))
    }

    private var ollamaServer: LocalServer { LocalServer(name: "Ollama", url: "http://127.0.0.1:11434/v1", model: "gemma4:e4b") }
}
