import XCTest
@testable import Tacie

final class WhatsNewTests: XCTestCase {
    private func entry(_ v: String) -> WhatsNewEntry {
        WhatsNewEntry(version: v, scene: .liveTranscript, title: "title \(v)", detail: "", more: ["more \(v)"])
    }

    private lazy var entries = [entry("0.1.32"), entry("0.1.35")]

    func testShowsTheVersionJustInstalled() {
        XCTAssertEqual(WhatsNew.pick(seen: "0.1.31", installed: "0.1.32", entries: entries)?.version, "0.1.32")
    }

    func testAnUpdateWithNothingNewShowsNothing() {
        XCTAssertNil(WhatsNew.pick(seen: "0.1.32", installed: "0.1.33", entries: entries))
    }

    func testOnceSeenItDoesNotComeBack() {
        XCTAssertNil(WhatsNew.pick(seen: "0.1.32", installed: "0.1.32", entries: entries))
    }

    func testAnInstallFromBeforeTheSheetGetsTheFeaturesSinceThen() {
        // no seen version: the install had 0.1.31's features already
        XCTAssertEqual(WhatsNew.pick(seen: nil, installed: "0.1.33", entries: entries)?.version, "0.1.32")
    }

    func testSkippedVersionsShowTheNewestWithTheOthersUnderIt() {
        let e = WhatsNew.pick(seen: "0.1.31", installed: "0.1.36", entries: entries)
        XCTAssertEqual(e?.version, "0.1.35")
        XCTAssertEqual(e?.more, ["more 0.1.35", "title 0.1.32"])
    }

    func testAnEntryNewerThanTheInstalledVersionWaits() {
        XCTAssertEqual(WhatsNew.pick(seen: "0.1.31", installed: "0.1.34", entries: entries)?.version, "0.1.32")
    }

    func testVersionsCompareAsNumbers() {
        XCTAssertEqual(WhatsNew.pick(seen: "0.1.9", installed: "0.1.32", entries: entries)?.version, "0.1.32")
    }
}
