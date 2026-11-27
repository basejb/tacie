import XCTest
@testable import Tacie

final class AccountSpaceTests: XCTestCase {
    private var base: URL!
    private var defaults: UserDefaults!
    private var suite: String!
    private var keychain: FakeKeychain!

    /// Keychain items as (service, account) → secret.
    private final class FakeKeychain {
        var items: [String: String] = [:]
        var locked = false
        struct Locked: Error {}
        struct Item: TokenStore {
            let box: FakeKeychain
            let key: String
            func read() -> String? { box.locked ? nil : box.items[key] }
            func readChecked() throws -> String? {
                if box.locked { throw Locked() }
                return box.items[key]
            }
            func save(_ token: String) throws { box.items[key] = token }
            func delete() throws { box.items[key] = nil }
        }
        func store(_ service: String, _ account: String) -> any TokenStore { Item(box: self, key: "\(service)/\(account)") }
    }

    override func setUpWithError() throws {
        base = FileManager.default.temporaryDirectory.appendingPathComponent("space-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        suite = "AccountSpaceTests-\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suite)
        keychain = FakeKeychain()
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: base)
        defaults.removePersistentDomain(forName: suite)
    }

    private var adoption: SpaceAdoption {
        SpaceAdoption(base: base, defaults: defaults, store: keychain.store)
    }

    private func write(_ path: String, _ text: String = "x") throws {
        let url = base.appendingPathComponent(path)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(text.utf8).write(to: url)
    }

    private func exists(_ path: String) -> Bool { FileManager.default.fileExists(atPath: base.appendingPathComponent(path).path) }

    func testSignedOutIsTheOldLayoutAndAnAccountHasItsOwnFolderAndNames() {
        XCTAssertEqual(AccountSpace.signedOut.folder(base: base), base)
        XCTAssertEqual(AccountSpace.signedOut.key("connection"), "connection")
        let a = AccountSpace(id: "abc123")
        XCTAssertEqual(a.folder(base: base).path, base.appendingPathComponent("accounts/abc123").path)
        XCTAssertEqual(a.key("connection"), "connection@abc123")
    }

    func testOnlyPlainIdsBecomeFolders() {
        XCTAssertNil(AccountSpace(id: "../x").id)
        XCTAssertNil(AccountSpace(id: "a/b").id)
        XCTAssertNil(AccountSpace(id: "").id)
        XCTAssertEqual(AccountSpace(id: "0f3a9c").id, "0f3a9c")
    }

    func testTheFirstAccountTakesWhatTheSignedOutSpaceHeld() throws {
        try write("meetings/20261101-100000.json")
        try write("meetings/20261101-100000.caf")
        try write("people/people.json")
        try write("index/meta.json")
        try write("data/sources/repos.json")
        try write("models/bge-m3-onnx/model.onnx")
        try keychain.store("ai.tacie.notion", "connection").save("n")
        try keychain.store("ai.tacie.github", "token").save("g")
        defaults.set(true, forKey: GitHubCredentials.disconnectedKey)

        adoption.merge(into: "abc")

        for path in ["meetings/20261101-100000.json", "meetings/20261101-100000.caf", "people/people.json", "index/meta.json",
                     "data/sources/repos.json"] {
            XCTAssertTrue(exists("accounts/abc/\(path)"), path)
            XCTAssertFalse(exists(path), path)
        }
        XCTAssertTrue(exists("models/bge-m3-onnx/model.onnx"), "the models stay shared")
        XCTAssertEqual(keychain.items, ["ai.tacie.notion/connection@abc": "n", "ai.tacie.github/token@abc": "g"])
        XCTAssertTrue(defaults.bool(forKey: "githubDisconnected@abc"))
        XCTAssertNil(defaults.object(forKey: GitHubCredentials.disconnectedKey))
        XCTAssertNil(defaults.string(forKey: SpaceAdoption.pendingKey))
    }

    /// The case behind this change: A signs out, B signs in. B starts empty and A's data waits in A's space.
    func testAnotherAccountSignedInLaterGetsNothingOfTheFirst() throws {
        try write("meetings/a.json")
        try keychain.store("ai.tacie.linear", "connection").save("a")
        adoption.merge(into: "aaa")
        adoption.merge(into: "bbb")
        XCTAssertTrue(exists("accounts/aaa/meetings/a.json"))
        XCTAssertFalse(exists("accounts/bbb/meetings/a.json"))
        XCTAssertTrue(exists("accounts/bbb"))
        XCTAssertEqual(keychain.items, ["ai.tacie.linear/connection@aaa": "a"])
    }

    /// Signing in again to an account this Mac has a space for: meetings made while signed out join it (#56).
    func testMergeIntoExistingSpaceKeepsBoth() throws {
        try write("accounts/aaa/meetings/a.json", "account")
        try write("meetings/a.json", "signed out")
        try write("meetings/b.json")
        let moved = adoption.merge(into: "aaa")
        XCTAssertTrue(moved)
        XCTAssertEqual(try String(contentsOf: base.appendingPathComponent("accounts/aaa/meetings/a.json"), encoding: .utf8), "account")
        XCTAssertTrue(exists("accounts/aaa/meetings/b.json"))
        XCTAssertTrue(exists("meetings/a.json"), "a clash leaves the signed-out file where it was")
        XCTAssertFalse(exists("meetings/b.json"))
        XCTAssertNil(defaults.string(forKey: SpaceAdoption.pendingKey), "a clash is not a failed move")
    }

    /// The account's search data wins; the signed-out copy is dropped instead of being mixed in.
    func testMergeDropsSignedOutIndexWhenAccountHasOne() throws {
        try write("accounts/aaa/index/meta.json", "account")
        try write("index/meta.json", "signed out")
        try write("index.new/meta.json", "signed out")
        try write("data/sources/repos.json")
        adoption.merge(into: "aaa")
        XCTAssertEqual(try String(contentsOf: base.appendingPathComponent("accounts/aaa/index/meta.json"), encoding: .utf8), "account")
        XCTAssertFalse(exists("accounts/aaa/index.new"), "never the signed-out swap place into the account's index")
        XCTAssertFalse(exists("index"))
        XCTAssertFalse(exists("index.new"))
        XCTAssertTrue(exists("accounts/aaa/data/sources/repos.json"), "the account had no data: it moves")
    }

    /// The account's own connection stays; the signed-out one stays signed out.
    func testMergeKeepsAccountConnection() throws {
        try write("accounts/aaa/meetings/x.json")
        try keychain.store("ai.tacie.notion", "connection").save("old")
        try keychain.store("ai.tacie.notion", "connection@aaa").save("mine")
        try keychain.store("ai.tacie.linear", "connection").save("lin")
        defaults.set(true, forKey: "githubDisconnected@aaa")
        defaults.set(false, forKey: GitHubCredentials.disconnectedKey)
        adoption.merge(into: "aaa")
        XCTAssertEqual(keychain.items, ["ai.tacie.notion/connection@aaa": "mine", "ai.tacie.notion/connection": "old",
                                        "ai.tacie.linear/connection@aaa": "lin"])
        XCTAssertTrue(defaults.bool(forKey: "githubDisconnected@aaa"))
        XCTAssertEqual(defaults.object(forKey: GitHubCredentials.disconnectedKey) as? Bool, false)
    }

    func testMergeWithNothingSignedOutMovesNothing() throws {
        try write("accounts/aaa/meetings/x.json")
        XCTAssertFalse(adoption.merge(into: "aaa"))
    }

    /// The app quit half way: the next try moves the rest into the folder already there.
    func testAMoveThatStoppedHalfWayFinishes() throws {
        try write("accounts/aaa/meetings/first.json")
        try write("meetings/second.json")
        try write("people/people.json")
        defaults.set("aaa", forKey: SpaceAdoption.pendingKey)
        adoption.merge(into: "aaa")
        XCTAssertTrue(exists("accounts/aaa/meetings/first.json"))
        XCTAssertTrue(exists("accounts/aaa/meetings/second.json"))
        XCTAssertTrue(exists("accounts/aaa/people/people.json"))
        XCTAssertFalse(exists("meetings"))
        XCTAssertNil(defaults.string(forKey: SpaceAdoption.pendingKey))
    }

    /// A connection the account already has is kept; the signed-out one stays signed out.
    func testTheAccountsOwnConnectionWins() throws {
        try FileManager.default.createDirectory(at: base.appendingPathComponent("accounts/aaa"), withIntermediateDirectories: true)
        defaults.set("aaa", forKey: SpaceAdoption.pendingKey)
        try keychain.store("ai.tacie.notion", "connection").save("old")
        try keychain.store("ai.tacie.notion", "connection@aaa").save("mine")
        adoption.merge(into: "aaa")
        XCTAssertEqual(keychain.items, ["ai.tacie.notion/connection@aaa": "mine", "ai.tacie.notion/connection": "old"])
    }

    func testTheProductEngineRunsInTheAccountsSpace() {
        let home = URL(fileURLWithPath: "/Users/me")
        let support = URL(fileURLWithPath: "/Users/me/Library/Application Support")
        XCTAssertEqual(EngineLaunch.root(env: [:], isDev: false, open: false, saved: nil, home: home, appSupport: support, space: AccountSpace(id: "abc")).path,
                       support.appendingPathComponent("Tacie/accounts/abc").path)
        XCTAssertEqual(EngineLaunch.root(env: [:], isDev: false, open: false, saved: nil, home: home, appSupport: support, space: .signedOut),
                       support.appendingPathComponent("Tacie"))
        // a development build keeps its checkout for every account
        XCTAssertEqual(EngineLaunch.root(env: [:], isDev: true, open: false, saved: nil, home: home, appSupport: support, space: AccountSpace(id: "abc")),
                       home.appendingPathComponent("Repositories/tacie"))
    }

    /// A merge into A that could not finish: what is left stays signed out and goes to the next sign-in, whoever it is
    /// (one locked keychain item must not hold every later meeting back for A).
    func testLeftoversOfAnUnfinishedMergeGoToTheNextSignIn() throws {
        try keychain.store("ai.tacie.notion", "connection").save("a")
        try write("meetings/a.json")
        keychain.locked = true
        adoption.merge(into: "aaa")
        XCTAssertEqual(defaults.string(forKey: SpaceAdoption.pendingKey), "aaa", "a locked keychain is not an empty one")
        keychain.locked = false
        try write("meetings/later.json")
        adoption.merge(into: "ccc")
        XCTAssertTrue(exists("accounts/ccc/meetings/later.json"))
        XCTAssertEqual(keychain.items, ["ai.tacie.notion/connection@ccc": "a"])
        XCTAssertNil(defaults.string(forKey: SpaceAdoption.pendingKey))
    }

    /// A deleted account's space goes whole: files, connections, settings. Other accounts and the shared models stay.
    func testADeletedAccountsSpaceIsErased() throws {
        try write("accounts/aaa/meetings/a.json")
        try write("accounts/aaa/index/meta.json")
        try write("accounts/bbb/meetings/b.json")
        try write("models/bge-m3-onnx/model.onnx")
        try keychain.store("ai.tacie.notion", "connection@aaa").save("a")
        try keychain.store("ai.tacie.notion", "connection@bbb").save("b")
        defaults.set(true, forKey: "githubDisconnected@aaa")
        adoption.requestErase("aaa")
        adoption.eraseRequested(keeping: AccountSpace(id: "aaa"))
        XCTAssertTrue(exists("accounts/aaa"), "never the space in use")
        adoption.eraseRequested(keeping: .signedOut)
        XCTAssertFalse(exists("accounts/aaa"))
        XCTAssertTrue(exists("accounts/bbb/meetings/b.json"))
        XCTAssertTrue(exists("models/bge-m3-onnx/model.onnx"))
        XCTAssertEqual(keychain.items, ["ai.tacie.notion/connection@bbb": "b"])
        XCTAssertNil(defaults.object(forKey: "githubDisconnected@aaa"))
        XCTAssertNil(defaults.stringArray(forKey: SpaceAdoption.eraseKey))
    }
}
