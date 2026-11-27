import XCTest
@testable import Tacie

/// One app (#56): the AI choice and sign-in are apart. The relay runs only for Tacie AI with a session; signing in never
/// changes the choice, and "On-device processing" never sends a key or reaches the relay.
final class RelayRuleTests: XCTestCase {
    func testRelayOnlyForTacieSignedIn() {
        XCTAssertTrue(Relay.on(mode: .tacie, signedIn: true))
        XCTAssertFalse(Relay.on(mode: .tacie, signedIn: false))
        XCTAssertFalse(Relay.on(mode: .local, signedIn: true))
        XCTAssertFalse(Relay.on(mode: .key, signedIn: true))
        XCTAssertFalse(Relay.on(mode: .custom, signedIn: true))
        // nothing chosen: signing in never chooses Tacie AI (#56)
        XCTAssertFalse(Relay.on(mode: nil, signedIn: true))
        XCTAssertFalse(Relay.on(mode: nil, signedIn: false))
    }

    func testLocalSignedInSendsNothing() {
        let v = AIMode.effective(mode: .local, relay: Relay.on(mode: .local, signedIn: true),
                                 keys: [.openrouter: "sk-or-fake", .typesafe: "ts-fake"], address: nil, addressKey: nil,
                                 provider: SttProviderChoice(name: .soniox, region: "jp"), providerKey: "sx-fake",
                                 localSwitch: false, server: nil, cards: .auto)
        XCTAssertEqual(v.keys, [.openrouter: nil, .typesafe: nil])
        XCTAssertTrue(v.stt is NSNull)
        XCTAssertTrue(v.localStt)
    }

    func testTacieSignedOutSendsNoKeys() {
        let v = AIMode.effective(mode: .tacie, relay: false, keys: [.openrouter: "sk-or-fake", .typesafe: "ts-fake"],
                                 address: nil, addressKey: nil, provider: nil, providerKey: nil, localSwitch: false,
                                 server: nil, cards: .auto)
        XCTAssertEqual(v.keys, [.openrouter: nil, .typesafe: nil])
        XCTAssertTrue(v.chat is NSNull)
        XCTAssertTrue(v.stt is NSNull)
    }

    func testFirstLaunchSignedInBecomesTacie() {
        XCTAssertEqual(AIMode.firstLaunch(signedIn: true, saved: nil), .tacie)
        XCTAssertNil(AIMode.firstLaunch(signedIn: false, saved: nil))
        // someone signed out who used the app before the update had Tacie AI, the only AI then
        XCTAssertEqual(AIMode.firstLaunch(signedIn: false, saved: nil, usedBefore: true), .tacie)
        XCTAssertEqual(AIMode.firstLaunch(signedIn: true, saved: .local), .local)
    }
}
