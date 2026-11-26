import XCTest
@testable import Tacie

/// Settings → Meetings → Notes language (2026-11-19): the engine gets the app language only when App language is
/// picked.
final class NotesLanguageTests: XCTestCase {
    func testTargetIsTheAppLanguageOnlyForApp() throws {
        let d = try XCTUnwrap(UserDefaults(suiteName: "NotesLanguageTests"))
        d.removePersistentDomain(forName: "NotesLanguageTests")
        XCTAssertNil(NotesLanguage.target(d))
        d.set(NotesLanguage.meeting, forKey: NotesLanguage.key)
        XCTAssertNil(NotesLanguage.target(d))
        d.set(NotesLanguage.app, forKey: NotesLanguage.key)
        // the tests run as Korean (AppLocale: no English strings in the xctest bundle)
        XCTAssertEqual(NotesLanguage.target(d), "ko")
        d.removePersistentDomain(forName: "NotesLanguageTests")
    }
}
