import XCTest
@testable import Tacie

/// One app (#56): signed out, Tacie sends nothing to its servers except model downloads.
final class SignedInTrafficTests: XCTestCase {
    func testAnalyticsOffWhenSignedOut() {
        XCTAssertFalse(Analytics.allowed(signedIn: false, hasToken: true))
        XCTAssertTrue(Analytics.allowed(signedIn: true, hasToken: true))
        XCTAssertFalse(Analytics.allowed(signedIn: true, hasToken: false))
    }

    func testFeedbackNeedsSignIn() {
        XCTAssertFalse(FeedbackSurvey.offered(signedIn: false))
        XCTAssertEqual(FeedbackSurvey.offered(signedIn: true), Build.hasAccount)
    }

    func testWhatsNewFollowsTheOfficialFeed() {
        XCTAssertEqual(WhatsNew.offered, Build.hasAccount)
    }
}
