import XCTest
@testable import Tacie

final class PillTapTests: XCTestCase {
    private func tap(asking: Bool = false, closing: Bool = false, current: String? = nil, running: Bool = false) -> PillTap {
        PillTap.action(asking: asking, closing: closing, current: current, running: running)
    }

    /// Also after a meeting failed to start: that state is not running, and the chooser shows the reason.
    func testBeforeAMeetingOpensTheChooser() {
        XCTAssertEqual(tap(), .chooser)
    }

    func testDuringAMeetingOpensTheMainWindow() {
        XCTAssertEqual(tap(running: true), .main(question: nil))
    }

    func testOverAnAnswerOpensTheMainWindowAtThatQuestion() {
        XCTAssertEqual(tap(current: "s1u4", running: true), .main(question: "s1u4"))
        XCTAssertEqual(tap(current: "s1u4"), .main(question: "s1u4"))
    }

    func testTheAskFieldAndTheEndCountdownTakeNoPillClick() {
        XCTAssertEqual(tap(asking: true, running: true), .nothing)
        XCTAssertEqual(tap(closing: true, current: "s1u4", running: true), .nothing)
    }
}
