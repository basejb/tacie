import Sparkle
import XCTest

/// Sparkle's window (Check for updates) gets the notes link with ?since= too, so it shows every skipped version.
final class UpdateNotesTests: XCTestCase {
    private let page = URL(string: "https://models.tacie.app/updates/Tacie-0.1.23.html")!

    func testSinceIsAddedOnce() {
        let once = UpdateNotes.since(page, installed: "0.1.20")
        XCTAssertEqual(once?.absoluteString, "https://models.tacie.app/updates/Tacie-0.1.23.html?since=0.1.20")
        XCTAssertEqual(UpdateNotes.since(once, installed: "0.1.21"), once)
        XCTAssertEqual(UpdateNotes.since(page, installed: nil), page)
    }

    /// The item as Sparkle parses it from our appcast (publish-update.sh), rebuilt with the new link.
    func testAppcastItemKeepsEverythingButTheLink() throws {
        let dict: [String: Any] = [
            "title": "0.1.23",
            "sparkle:version": "24",
            "sparkle:shortVersionString": "0.1.23",
            "sparkle:minimumSystemVersion": "14.0",
            "sparkle:releaseNotesLink": ["content": page.absoluteString],
            "enclosure": ["url": "https://models.tacie.app/updates/Tacie-0.1.23.zip", "length": "58323640",
                          "type": "application/octet-stream", "sparkle:edSignature": "abc"],
        ]
        let item = try XCTUnwrap(SUAppcastItem(dictionary: dict))
        XCTAssertEqual(item.releaseNotesURL, page)
        let with = try XCTUnwrap(UpdateNotes.item(item, installed: "0.1.20"))
        XCTAssertEqual(with.releaseNotesURL?.absoluteString, page.absoluteString + "?since=0.1.20")
        XCTAssertEqual(with.versionString, "24")
        XCTAssertEqual(with.displayVersionString, "0.1.23")
        XCTAssertEqual(with.fileURL, item.fileURL)
        XCTAssertEqual(with.contentLength, item.contentLength)
        XCTAssertEqual(with.minimumSystemVersion, "14.0")
        XCTAssertEqual((with.propertiesDictionary["enclosure"] as? [String: Any])?["sparkle:edSignature"] as? String, "abc")
    }

    /// From 0.1.36 a version has Tacie-<v>.ko.html and .en.html; Tacie opens the one in its own language.
    func testLocalizedPicksTheAppsLanguage() {
        let ko = URL(string: "https://models.tacie.app/updates/Tacie-0.1.36.ko.html")!
        let en = URL(string: "https://models.tacie.app/updates/Tacie-0.1.36.en.html")!
        XCTAssertEqual(UpdateNotes.localized(ko, language: "en"), en)
        XCTAssertEqual(UpdateNotes.localized(en, language: "ko"), ko)
        XCTAssertEqual(UpdateNotes.localized(ko, language: "ko"), ko)
        // an older Korean-only page stays as it is
        XCTAssertEqual(UpdateNotes.localized(page, language: "en"), page)
        XCTAssertEqual(UpdateNotes.since(UpdateNotes.localized(ko, language: "en"), installed: "0.1.30")?.absoluteString,
                       "https://models.tacie.app/updates/Tacie-0.1.36.en.html?since=0.1.30")
        XCTAssertEqual(UpdateNotes.page(version: "0.1.36", language: "en"), en)
        XCTAssertEqual(UpdateNotes.page(version: "0.1.36", language: "ko"), ko)
    }

    /// Sparkle took the Korean link (a Korean Mac) while Tacie runs in English: the item gets the English one.
    func testAppcastItemGetsTheAppsLanguage() throws {
        let item = try XCTUnwrap(SUAppcastItem(dictionary: [
            "sparkle:version": "37",
            "sparkle:shortVersionString": "0.1.36",
            "sparkle:releaseNotesLink": ["content": "https://models.tacie.app/updates/Tacie-0.1.36.ko.html", "xml:lang": "ko"],
            "enclosure": ["url": "https://models.tacie.app/updates/Tacie-0.1.36.zip", "length": "1"],
        ]))
        let with = try XCTUnwrap(UpdateNotes.item(item, installed: "0.1.35", language: "en"))
        XCTAssertEqual(with.releaseNotesURL?.absoluteString,
                       "https://models.tacie.app/updates/Tacie-0.1.36.en.html?since=0.1.35")
    }

    func testNoLinkKeepsSparklesItem() throws {
        let item = try XCTUnwrap(SUAppcastItem(dictionary: [
            "sparkle:version": "24",
            "enclosure": ["url": "https://models.tacie.app/updates/Tacie-0.1.23.zip", "length": "1"],
        ]))
        XCTAssertNil(UpdateNotes.item(item, installed: "0.1.20"))
    }
}
