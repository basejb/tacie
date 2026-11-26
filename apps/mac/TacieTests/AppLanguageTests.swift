import XCTest
@testable import Tacie

/// Settings → General → Language: stored as the app's own AppleLanguages, and a restart only when what is shown
/// changes.
final class AppLanguageTests: XCTestCase {
    func testSaveWritesTheAppsOwnLanguagesAndSystemRemovesThem() {
        let d = UserDefaults(suiteName: "AppLanguageTests")!
        defer { d.removePersistentDomain(forName: "AppLanguageTests") }
        AppLanguage.english.save(d)
        XCTAssertEqual(d.stringArray(forKey: AppLanguage.key), ["en"])
        AppLanguage.korean.save(d)
        XCTAssertEqual(d.stringArray(forKey: AppLanguage.key), ["ko"])
        AppLanguage.system.save(d)
        XCTAssertNil(d.persistentDomain(forName: "AppLanguageTests")?[AppLanguage.key])
    }

    func testShowsKoreanFollowsTheMacForAuto() {
        XCTAssertTrue(AppLanguage.korean.showsKorean(macLanguages: ["en-US"]))
        XCTAssertFalse(AppLanguage.english.showsKorean(macLanguages: ["ko-KR"]))
        XCTAssertTrue(AppLanguage.system.showsKorean(macLanguages: ["ko-KR", "en-US"]))
        XCTAssertFalse(AppLanguage.system.showsKorean(macLanguages: ["en-GB", "ko-KR"]))
        // a Mac in a language the app does not have falls to the next it does
        XCTAssertTrue(AppLanguage.system.showsKorean(macLanguages: ["ja-JP", "ko-KR"]))
    }
}
