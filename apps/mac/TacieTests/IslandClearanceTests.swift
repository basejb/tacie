import XCTest
@testable import Tacie

final class IslandClearanceTests: XCTestCase {
    private let screen = NSRect(x: 0, y: 0, width: 1512, height: 944)
    private let island = NSRect(x: 553, y: 860, width: 406, height: 64)

    func testAWindowUnderTheIslandMovesBelowIt() {
        let w = NSRect(x: 486, y: 240, width: 540, height: 652)
        let o = IslandClearance.origin(clearOf: island, window: w, screen: screen)
        XCTAssertEqual(o.x, 486)
        XCTAssertEqual(o.y + w.height, island.minY - IslandClearance.gap)
    }

    func testAWindowAwayFromTheIslandStays() {
        let w = NSRect(x: 486, y: 100, width: 540, height: 652)
        XCTAssertEqual(IslandClearance.origin(clearOf: island, window: w, screen: screen), w.origin)
        XCTAssertEqual(IslandClearance.origin(clearOf: nil, window: NSRect(x: 486, y: 240, width: 540, height: 652), screen: screen),
                       NSPoint(x: 486, y: 240))
    }

    func testAShortScreenKeepsTheWindowOnIt() {
        let small = NSRect(x: 0, y: 0, width: 1280, height: 700)
        let w = NSRect(x: 370, y: 20, width: 540, height: 652)
        let o = IslandClearance.origin(clearOf: NSRect(x: 437, y: 620, width: 406, height: 64), window: w, screen: small)
        XCTAssertEqual(o.y, 8)
    }
}
