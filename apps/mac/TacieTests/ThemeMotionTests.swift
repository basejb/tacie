import XCTest
import SwiftUI
@testable import Tacie

final class ThemeMotionTests: XCTestCase {
    func testReduceSwapsAnyAnimationForTheFade() {
        XCTAssertEqual(Theme.anim(Theme.grow, reduce: true), Theme.reduced)
        XCTAssertEqual(Theme.anim(Theme.exit, reduce: true), Theme.reduced)
        XCTAssertEqual(Theme.anim(Theme.settle, reduce: false), Theme.settle)
    }

    func testTokensMatchTheSpec() {
        XCTAssertEqual(Theme.settle, .spring(response: 0.4, dampingFraction: 1))
        XCTAssertEqual(Theme.exit, .timingCurve(0.7, 0, 0.84, 0, duration: 0.18))
        XCTAssertEqual(Theme.exitSeconds, 0.18)
        XCTAssertEqual(Theme.reduced, .easeOut(duration: 0.12))
        XCTAssertEqual(Theme.grow, .spring(response: 0.42, dampingFraction: 0.78))
    }
}

final class ReducesMotionTests: XCTestCase {
    func testForcedFlagReadsAsReducedMotion() {
        var env = EnvironmentValues()
        XCTAssertFalse(env.reducesMotion)
        env.forcedReduceMotion = true
        XCTAssertTrue(env.reducesMotion)
    }
}

final class ThemeGlassTests: XCTestCase {
    func testGlassIsTintedWithAThinVeil() {
        XCTAssertEqual(Theme.glassTint, 0.75)
        XCTAssertEqual(Theme.glassVeil, 0.3)
    }
}

final class ThemePolishTokenTests: XCTestCase {
    func testTokensMatchTheSpec() {
        XCTAssertEqual(Theme.snappy, .spring(response: 0.26, dampingFraction: 1))
        XCTAssertEqual(Theme.glide, .spring(duration: 0.32, bounce: 0.1))
        XCTAssertEqual(Theme.enter, .timingCurve(0.16, 1, 0.3, 1, duration: 0.24))
    }
    func testStaggerStepsByItem() {
        XCTAssertEqual(Theme.stagger(0), 0)
        XCTAssertEqual(Theme.stagger(2), 0.07, accuracy: 1e-9)
        XCTAssertEqual(Theme.stagger(1, step: 0.08), 0.08, accuracy: 1e-9)
    }
}

final class ShimmerTests: XCTestCase {
    func testPhaseWrapsEveryPeriod() {
        XCTAssertEqual(Shimmer.phase(at: 0), 0)
        XCTAssertEqual(Shimmer.phase(at: 0.8), 0.5, accuracy: 1e-9)
        XCTAssertEqual(Shimmer.phase(at: 3.2), 0, accuracy: 1e-9)
        XCTAssertEqual(Shimmer.phase(at: 1, period: 4), 0.25, accuracy: 1e-9)
    }
}

final class ShimmerBandTests: XCTestCase {
    func testBandStartsOffTheLeadingEdgeAndEndsOffTheTrailingEdge() {
        XCTAssertEqual(Shimmer.bandOffset(phase: 0, width: 200, band: 70), -70)
        XCTAssertEqual(Shimmer.bandOffset(phase: 1, width: 200, band: 70), 200)
    }
    func testExitIsQuickerThanEnter() {
        XCTAssertLessThan(Theme.exitSeconds, 0.24)
    }
}
