import XCTest
@testable import Tacie

final class LaunchWindowTests: XCTestCase {
    private let app = "/Applications/Tacie.app/Contents/MacOS/Tacie"

    func testOpeningTheAppShowsTheMainWindow() {
        XCTAssertTrue(LaunchWindow.showsMain(arguments: [app], loginItem: false))
    }

    func testStartingAtLoginShowsOnlyTheIsland() {
        XCTAssertFalse(LaunchWindow.showsMain(arguments: [app], loginItem: true))
    }

    func testScriptedLaunchesShowOnlyTheIsland() {
        for flag in ["--debug-control", "--ask", "--replay", "--listen", "--export-eyes", "--export-brand"] {
            XCTAssertFalse(LaunchWindow.showsMain(arguments: [app, flag], loginItem: false), flag)
        }
    }

    func testCaptureAndFixtureFlagsStillShowTheMainWindow() {
        XCTAssertTrue(LaunchWindow.showsMain(arguments: [app, "--allow-capture", "--meetings-dir", "/tmp/m"], loginItem: false))
    }
}
