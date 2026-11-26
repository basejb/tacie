import XCTest
@testable import Tacie

final class DevSettingsTests: XCTestCase {
    private func suite() -> UserDefaults { UserDefaults(suiteName: UUID().uuidString)! }

    private func filled() -> UserDefaults {
        let d = suite()
        d.set("  x/stt ", forKey: DevSettings.sttModel)
        d.set("", forKey: DevSettings.answerModel)
        d.set("0,65", forKey: DevSettings.gate)
        d.set("2", forKey: DevSettings.show)
        d.set(true, forKey: DevSettings.debugText)
        return d
    }

    func testThresholdParsing() {
        XCTAssertEqual(DevSettings.threshold("0.75"), 0.75)
        XCTAssertEqual(DevSettings.threshold("0,75"), 0.75)
        XCTAssertEqual(DevSettings.threshold(" 0.6 "), 0.6)
        XCTAssertEqual(DevSettings.threshold("1"), 1)
        XCTAssertNil(DevSettings.threshold("1.5"))
        XCTAssertNil(DevSettings.threshold("-0.1"))
        XCTAssertNil(DevSettings.threshold("abc"))
        XCTAssertNil(DevSettings.threshold(""))
    }

    func testEnvironmentSkipsInvalidValues() {
        XCTAssertEqual(DevSettings.environment(filled(), dev: true),
                       ["TACIE_STT_MODEL": "x/stt", "TACIE_GATE_THR": "0.65", "TACIE_LOG_TEXT": "1"])
    }

    /// Without a pick on the developer page the region comes from where the Mac is (`SonioxRegion`), as in a shipped app.
    func testEnvironmentLeavesTheRegionToTheMac() {
        XCTAssertEqual(DevSettings.environment(suite(), dev: true), [:])
    }

    func testEnvironmentPassesTheUSShadowRegion() {
        let d = suite()
        d.set("us", forKey: DevSettings.sonioxRegion)
        XCTAssertEqual(DevSettings.environment(d, dev: true), ["TACIE_SONIOX_REGION": "us"])
    }

    func testEnvironmentOnlyInDevBuilds() {
        XCTAssertEqual(DevSettings.environment(filled(), dev: false), [:])
    }

    func testSlackMergeKeepsOtherKeys() throws {
        let old = Data(#"{"me":"U1","bot_token":"old"}"#.utf8)
        let merged = try SlackConfig.merged(old, token: " xoxb-new\n")
        let obj = try JSONSerialization.jsonObject(with: merged) as? [String: String]
        XCTAssertEqual(obj, ["me": "U1", "bot_token": "xoxb-new"])
        let fresh = try SlackConfig.merged(nil, token: "t")
        XCTAssertEqual(try JSONSerialization.jsonObject(with: fresh) as? [String: String], ["bot_token": "t"])
        XCTAssertThrowsError(try SlackConfig.merged(Data("{".utf8), token: "t"))
    }

    func testSaveWritesOwnerOnlyFile() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent("slack/config.json")
        try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(#"{"me":"U1"}"#.utf8).write(to: file)
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: file.path)
        try SlackConfig.save(token: " xoxb-t ", to: file)
        XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: file.path)[.posixPermissions] as? Int, 0o600)
        XCTAssertEqual(try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: String],
                       ["me": "U1", "bot_token": "xoxb-t"])

        let fresh = dir.appendingPathComponent("new/config.json")
        try SlackConfig.save(token: "t", to: fresh)
        XCTAssertEqual(try FileManager.default.attributesOfItem(atPath: fresh.path)[.posixPermissions] as? Int, 0o600)
    }
}
