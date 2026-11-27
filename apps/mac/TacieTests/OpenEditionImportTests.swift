import XCTest
@testable import Tacie

final class OpenEditionImportTests: XCTestCase {
    private var root: URL!
    private var base: URL { root.appendingPathComponent("Tacie") }
    private var open: URL { root.appendingPathComponent("Tacie Open") }
    private var ours: UserDefaults!
    private var theirs: UserDefaults!
    private var suites: [String] = []

    override func setUpWithError() throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent("open-import-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        suites = ["OpenImportOurs-\(UUID().uuidString)", "OpenImportTheirs-\(UUID().uuidString)"]
        ours = UserDefaults(suiteName: suites[0])
        theirs = UserDefaults(suiteName: suites[1])
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: root)
        for s in suites { UserDefaults().removePersistentDomain(forName: s) }
    }

    private func write(_ url: URL, _ text: String = "x") throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(text.utf8).write(to: url)
    }

    private func importNow() -> Bool {
        OpenEditionImport.run(base: base, from: open, defaults: ours, openDefaults: theirs)
    }

    func testImportMovesMeetingsOnce() throws {
        try write(open.appendingPathComponent("meetings/a.json"))
        try write(open.appendingPathComponent("people/people.json"))
        try write(open.appendingPathComponent("models/gemma/model.gguf"))
        try write(base.appendingPathComponent("meetings/mine.json"))
        XCTAssertTrue(importNow())
        XCTAssertTrue(FileManager.default.fileExists(atPath: base.appendingPathComponent("meetings/a.json").path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: base.appendingPathComponent("meetings/mine.json").path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: base.appendingPathComponent("people/people.json").path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: base.appendingPathComponent("models/gemma/model.gguf").path))
        XCTAssertTrue(ours.bool(forKey: OpenEditionImport.markerKey))
        XCTAssertTrue(ours.bool(forKey: OpenEditionImport.noticeKey))
        // a meeting made in the open build later does not come over a second time
        try write(open.appendingPathComponent("meetings/b.json"))
        XCTAssertFalse(importNow())
        XCTAssertFalse(FileManager.default.fileExists(atPath: base.appendingPathComponent("meetings/b.json").path))
    }

    func testAClashKeepsOurFile() throws {
        try write(open.appendingPathComponent("meetings/a.json"), "open")
        try write(base.appendingPathComponent("meetings/a.json"), "ours")
        _ = importNow()
        XCTAssertEqual(try String(contentsOf: base.appendingPathComponent("meetings/a.json"), encoding: .utf8), "ours")
        XCTAssertTrue(FileManager.default.fileExists(atPath: open.appendingPathComponent("meetings/a.json").path))
    }

    /// Keys stay in the open build's keychain: only plain settings are read, and none of them is a key.
    func testImportNeverReadsKeychain() throws {
        theirs.set("sk-or-fake", forKey: "apiKey")
        theirs.set("key", forKey: AIMode.defaultsKey)
        _ = importNow()
        XCTAssertNil(ours.object(forKey: "apiKey"))
        for key in OpenEditionImport.settingsKeys {
            XCTAssertFalse(key.lowercased().contains("apikey"), key)
            XCTAssertFalse(key.hasPrefix("ai.tacie."), key)
        }
        XCTAssertEqual(AIMode.saved(ours), .key)
    }

    func testImportKeepsOurMode() throws {
        AIMode.save(.tacie, ours)
        theirs.set("local", forKey: AIMode.defaultsKey)
        theirs.set(true, forKey: LocalSTT.key)
        _ = importNow()
        XCTAssertEqual(AIMode.saved(ours), .tacie)
        XCTAssertTrue(ours.bool(forKey: LocalSTT.key), "unset settings still come over")
    }

    /// Someone who already used this app (meetings here or signed in) keeps their AI, even one never saved (nil).
    func testAnAppUsedBeforeKeepsItsSettings() throws {
        try write(base.appendingPathComponent("meetings/mine.json"))
        try write(open.appendingPathComponent("meetings/a.json"))
        theirs.set("local", forKey: AIMode.defaultsKey)
        XCTAssertTrue(importNow())
        XCTAssertNil(AIMode.saved(ours))
        XCTAssertFalse(OpenEditionImport.run(base: root.appendingPathComponent("Other"), from: open, defaults: UserDefaults(suiteName: suites[0] + "x")!,
                                             openDefaults: theirs, signedIn: true))
        UserDefaults().removePersistentDomain(forName: suites[0] + "x")
    }

    /// The home line speaks of meetings only when meetings came, and of the key only when the open build used one.
    func testNoticeFollowsWhatCame() throws {
        try write(open.appendingPathComponent("models/gemma/model.gguf"))
        _ = importNow()
        XCTAssertFalse(ours.bool(forKey: OpenEditionImport.noticeKey), "models alone are no meetings")
        let other = UserDefaults(suiteName: suites[0] + "n")!
        defer { UserDefaults().removePersistentDomain(forName: suites[0] + "n") }
        try write(open.appendingPathComponent("meetings/a.json"))
        theirs.set("key", forKey: AIMode.defaultsKey)
        AIMode.save(.tacie, other)  // ours already chosen: the open build's key still has to be pasted again
        _ = OpenEditionImport.run(base: base, from: open, defaults: other, openDefaults: theirs)
        XCTAssertTrue(other.bool(forKey: OpenEditionImport.noticeKey))
        XCTAssertTrue(OpenEditionImport.usedKey(other))
    }

    func testNothingThereMarksDoneWithoutANotice() {
        XCTAssertFalse(OpenEditionImport.run(base: base, from: open, defaults: ours, openDefaults: nil))
        XCTAssertTrue(ours.bool(forKey: OpenEditionImport.markerKey))
        XCTAssertFalse(ours.bool(forKey: OpenEditionImport.noticeKey))
    }
}
