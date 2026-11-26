import XCTest
@testable import Tacie

final class IslandSizingTests: XCTestCase {
    func testGrowingResizesAtOnce() {
        let s = IslandSizing.steps(from: CGSize(width: 200, height: 60), to: CGSize(width: 560, height: 300))
        XCTAssertEqual(s.now, CGSize(width: 560, height: 300))
        XCTAssertNil(s.later)
    }

    func testShrinkingWaitsForTheFold() {
        let s = IslandSizing.steps(from: CGSize(width: 560, height: 300), to: CGSize(width: 200, height: 60))
        XCTAssertEqual(s.now, CGSize(width: 560, height: 300))
        XCTAssertEqual(s.later, CGSize(width: 200, height: 60))
    }

    // the ask field opening from the chooser: the pill widens while the card folds. The width may not wait.
    func testWiderButShorterGrowsWidthAtOnce() {
        let s = IslandSizing.steps(from: CGSize(width: 560, height: 300), to: CGSize(width: 800, height: 200))
        XCTAssertEqual(s.now, CGSize(width: 800, height: 300))
        XCTAssertEqual(s.later, CGSize(width: 800, height: 200))
    }

    func testDelayCoversTheExit() {
        XCTAssertGreaterThanOrEqual(IslandSizing.shrinkDelay, Theme.exitSeconds)
        XCTAssertLessThanOrEqual(IslandSizing.shrinkDelay, 0.4)  // never slower than the settle spring it waits for
    }

    // the pill row keeps the width it had before the pointer came only while hovering really widens it
    func testOnlyTheRunningPillWidensOnHover() {
        XCTAssertTrue(IslandSizing.widensOnHover(asking: false, closing: false, hasQuestion: false, failed: false, running: true))
        // typing in the ask field with the pointer on the pill: the field grows, so the width must follow
        XCTAssertFalse(IslandSizing.widensOnHover(asking: true, closing: false, hasQuestion: false, failed: false, running: true))
        XCTAssertFalse(IslandSizing.widensOnHover(asking: false, closing: false, hasQuestion: true, failed: false, running: true))
        XCTAssertFalse(IslandSizing.widensOnHover(asking: false, closing: true, hasQuestion: false, failed: false, running: true))
        XCTAssertFalse(IslandSizing.widensOnHover(asking: false, closing: false, hasQuestion: false, failed: true, running: true))
        XCTAssertFalse(IslandSizing.widensOnHover(asking: false, closing: false, hasQuestion: false, failed: false, running: false))
    }

    // two screens side by side: left 0–1440, right 1440–3360
    private let left = CGRect(x: 0, y: 0, width: 1440, height: 875)
    private let right = CGRect(x: 1440, y: 0, width: 1920, height: 1080)

    func testStaysPutWhenThereIsRoom() {
        let d = IslandSizing.push(CGRect(x: 500, y: 800, width: 400, height: 40), into: left, margin: 8)
        XCTAssertEqual(d, CGVector(dx: 0, dy: 0))
    }

    func testPushedLeftAtTheRightEdgeInsteadOfSpillingOntoTheNextScreen() {
        // the pill near the right edge grows to 600 wide: it ends 8 pt inside the edge
        let d = IslandSizing.push(CGRect(x: 1100, y: 800, width: 600, height: 40), into: left, margin: 8)
        XCTAssertEqual(d.dx, 1432 - 1700)
    }

    func testPushedRightAtTheLeftEdge() {
        let d = IslandSizing.push(CGRect(x: 1400, y: 800, width: 600, height: 40), into: right, margin: 8)
        XCTAssertEqual(d.dx, 48)
    }

    // the open button coming in 10 pt right of a pill resting at the edge: the island makes room for it
    func testOpenButtonMakesRoomAtTheEdge() {
        let resting = CGRect(x: 1200, y: 800, width: 232, height: 38)
        XCTAssertEqual(IslandSizing.push(resting, into: left, margin: 8).dx, 0)
        let hovered = CGRect(x: 1200, y: 800, width: 232 + 64 + 10 + 38, height: 38)
        XCTAssertEqual(IslandSizing.push(hovered, into: left, margin: 8).dx, -112)
    }

    func testDraggedUnderTheMenuBarComesBack() {
        let d = IslandSizing.push(CGRect(x: 500, y: 860, width: 300, height: 38), into: left, margin: 8)
        XCTAssertEqual(d.dy, 867 - 898)
    }

    func testWiderThanTheScreenCentresOnIt() {
        let d = IslandSizing.push(CGRect(x: 0, y: 400, width: 1600, height: 40), into: left, margin: 8)
        XCTAssertEqual(d.dx, -80)
    }

    // dragging past an edge: it follows less and less, never more than the limit
    func testDragPastTheEdgeGivesALittle() {
        let near = IslandSizing.stretch(CGRect(x: 1142, y: 400, width: 300, height: 40), into: left, margin: 8, limit: 24)
        XCTAssertEqual(near.dx, -3)  // 10 past: shows 7 of it
        let far = IslandSizing.stretch(CGRect(x: 2132, y: 400, width: 300, height: 40), into: left, margin: 8, limit: 24)
        XCTAssertEqual(far.dx, -1000 + 23)  // 1000 past: shows 23
    }

    func testRubberNeverReachesTheLimit() {
        XCTAssertEqual(IslandSizing.rubber(0, limit: 24), 0)
        XCTAssertLessThan(IslandSizing.rubber(100_000, limit: 24), 24.5)
        XCTAssertLessThan(IslandSizing.rubber(10, limit: 24), 10)
    }

    func testDragStaysInsideWithoutGive() {
        let d = IslandSizing.stretch(CGRect(x: 500, y: 400, width: 300, height: 40), into: left, margin: 8, limit: 24)
        XCTAssertEqual(d, CGVector(dx: 0, dy: 0))
    }
}
