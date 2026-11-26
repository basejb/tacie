import XCTest
@testable import Tacie

final class BrandGeometryTests: XCTestCase {
    func testWidthIsAspectTimesHeight() {
        XCTAssertEqual(BrandGeometry.width(forHeight: 100), 150, accuracy: 0.001)
    }

    func testEyeCentersOnTheWidestLine() {
        let c = BrandGeometry.eyeCenters(in: CGSize(width: 150, height: 100))
        XCTAssertEqual(c.left.x, 45, accuracy: 0.001)
        XCTAssertEqual(c.right.x, 105, accuracy: 0.001)
        XCTAssertEqual(c.left.y, 58, accuracy: 0.001)
        XCTAssertEqual(c.right.y, 58, accuracy: 0.001)
        XCTAssertEqual(BrandGeometry.eyeSpacing(forHeight: 100), 60, accuracy: 0.001)
    }

    func testOutlineFillsItsBoxAndIsWidestOnTheEyeLine() {
        let box = CGRect(x: 0, y: 0, width: 150, height: 100)
        let b = BrandGeometry.outline(in: box).boundingBoxOfPath
        XCTAssertEqual(b.minX, 0, accuracy: 0.01); XCTAssertEqual(b.maxX, 150, accuracy: 0.01)
        XCTAssertEqual(b.minY, 0, accuracy: 0.01); XCTAssertEqual(b.maxY, 100, accuracy: 0.01)
        let path = BrandGeometry.outline(in: box)
        XCTAssertTrue(path.contains(CGPoint(x: 1, y: 58)))      // the sides touch the box on the eye line
        XCTAssertFalse(path.contains(CGPoint(x: 1, y: 20)))     // the dome is narrower above it
        XCTAssertTrue(path.contains(CGPoint(x: 75, y: 99)))     // the base is flat in the middle
        XCTAssertFalse(path.contains(CGPoint(x: 10, y: 99)))    // and rounded at the corners
    }

    func testRadiiMatchSpec() {
        XCTAssertEqual(BrandGeometry.coreRadius(height: 75), 5.25, accuracy: 0.01)
        XCTAssertEqual(BrandGeometry.haloRadius(height: 75), 17.25, accuracy: 0.01)
        XCTAssertEqual(BrandGeometry.holeRadius(height: 75), 8.025, accuracy: 0.01)
    }

    func testTemplateHoleGapAtAvatarSize() {
        // The 26pt avatar holds a 12.5-high head (18.75 wide): holes r 1.34, centers 7.5 apart.
        // The gap between the holes must be wider than a hole, or the two eyes blur into one.
        let h = 12.5
        let c = BrandGeometry.eyeCenters(in: CGSize(width: BrandGeometry.width(forHeight: h), height: h))
        let gap = (c.right.x - c.left.x) - 2 * BrandGeometry.holeRadius(height: h)
        XCTAssertGreaterThan(gap, 2 * BrandGeometry.holeRadius(height: h))
    }

    func testChannelsRestIsFinite() {
        XCTAssertTrue(EyeChannels.rest.isFinite)
    }

    func testLerpHalfway() {
        var b = EyeChannels.rest
        b.left.size = 2
        b.tilt = 6
        let m = EyeChannels.lerp(.rest, b, 0.5)
        XCTAssertEqual(m.left.size, 1.5, accuracy: 0.001)
        XCTAssertEqual(m.tilt, 3, accuracy: 0.001)
    }
}
