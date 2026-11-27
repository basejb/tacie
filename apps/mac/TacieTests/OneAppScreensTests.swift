import XCTest
@testable import Tacie

/// One app (#56): screens follow the account and the AI choice, not the edition.
final class OneAppScreensTests: XCTestCase {
    func testPastedTokensSplit() {
        // a source build pastes every token
        XCTAssertTrue(PastedTokens.pasted(.notion, signedIn: false, hasAccount: false))
        // Atlassian and Asana need a Tacie session for their one-click connect; without one, a token
        XCTAssertTrue(PastedTokens.pasted(.atlassian, signedIn: false, hasAccount: true))
        XCTAssertTrue(PastedTokens.pasted(.asana, signedIn: false, hasAccount: true))
        XCTAssertFalse(PastedTokens.pasted(.atlassian, signedIn: true, hasAccount: true))
        // the rest connect in one click with the build's values, signed in or not
        XCTAssertTrue(PastedTokens.pasted(.notion, signedIn: false, hasAccount: true), "Notion's token exchange needs a session")
        XCTAssertFalse(PastedTokens.pasted(.notion, signedIn: true, hasAccount: true))
        XCTAssertFalse(PastedTokens.pasted(.github, signedIn: false, hasAccount: true))
        XCTAssertFalse(PastedTokens.pasted(.gdrive, signedIn: false, hasAccount: true))
        XCTAssertFalse(PastedTokens.pasted(.dropbox, signedIn: false, hasAccount: true))
        XCTAssertTrue(PastedTokens.pasted(.linear, signedIn: false, hasAccount: true), "Linear's code is handed over with a session")
        XCTAssertFalse(PastedTokens.pasted(.linear, signedIn: true, hasAccount: true))
    }

    func testAIPaneAlwaysThere() {
        XCTAssertEqual(SettingsPane.panes(hasAccount: true), [.account, .general, .meeting, .ai, .connections, .privacy])
        XCTAssertEqual(SettingsPane.panes(hasAccount: false), [.general, .meeting, .ai, .connections, .privacy])
    }

    func testTacieChosenOnlyWithSession() {
        XCTAssertTrue(OnboardingFlow.aiChosen(mode: .tacie, signedIn: true))
        XCTAssertFalse(OnboardingFlow.aiChosen(mode: .tacie, signedIn: false))
        XCTAssertTrue(OnboardingFlow.aiChosen(mode: .local, signedIn: false))
        XCTAssertFalse(OnboardingFlow.aiChosen(mode: nil, signedIn: true))
    }
}

final class SignedOutNoticeTests: XCTestCase {
    func testShowsOnlySignedOutWithAnAccountLeftOnThisMac() {
        XCTAssertTrue(SignedOutNotice.shows(signedIn: false, hasAccount: true, accountsHere: true))
        XCTAssertFalse(SignedOutNotice.shows(signedIn: true, hasAccount: true, accountsHere: true))
        XCTAssertFalse(SignedOutNotice.shows(signedIn: false, hasAccount: true, accountsHere: false))
        XCTAssertFalse(SignedOutNotice.shows(signedIn: false, hasAccount: false, accountsHere: true))
    }

    func testAccountsHereReadsTheAccountsFolder() throws {
        let base = FileManager.default.temporaryDirectory.appendingPathComponent("notice-\(UUID())")
        XCTAssertFalse(AccountSpace.anyAccount(base: base))
        try FileManager.default.createDirectory(at: AccountSpace(id: "a1b2c3d4").folder(base: base), withIntermediateDirectories: true)
        XCTAssertTrue(AccountSpace.anyAccount(base: base))
    }
}

final class OneAppNoticeTests: XCTestCase {
    /// Only the open build that updates itself (the GitHub release DMG) tells its users to move; a source build never.
    func testOnlyTheSelfUpdatingOpenBuildShowsIt() {
        let feed = URL(string: "https://github.com/basejb/tacie/releases/latest/download/appcast.xml")
        XCTAssertTrue(OneAppNotice.shows(open: true, feed: feed))
        XCTAssertFalse(OneAppNotice.shows(open: true, feed: nil))
        XCTAssertFalse(OneAppNotice.shows(open: false, feed: URL(string: "https://example.com/appcast.xml")))
    }
}

final class OnDeviceDownloadCopyTests: XCTestCase {
    /// 온디바이스 처리 has no server to fall back to while the model downloads: the copy must not say there is one.
    func testOnDeviceProcessingNeverPromisesAServer() {
        let phase = LocalSTT.Phase.downloading(done: 100_000_000, total: 600_000_000)
        XCTAssertTrue(LocalSTTCopy.detail(phase).contains("서버"))
        XCTAssertFalse(LocalSTTCopy.detail(phase, serverFallback: false).contains("서버"))
        XCTAssertFalse(LocalSTTCopy.meetingLine(phase, serverFallback: false)!.contains("서버"))
        XCTAssertFalse(LocalSTTCopy.meetingLine(.waiting, serverFallback: false)!.contains("서버"))
    }

    /// The token window says why a token is pasted, without edition names.
    func testTokenReasonNamesNoEdition() {
        for hasAccount in [true, false] {
            let why = TokenCopy.why(.notion, hasAccount: hasAccount)
            XCTAssertFalse(why.contains("오픈소스"), why)
        }
        XCTAssertTrue(TokenCopy.why(.notion, hasAccount: true).contains("로그인"))
    }
}
