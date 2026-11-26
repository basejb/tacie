import XCTest
@testable import Tacie

final class EyeRigTests: XCTestCase {
    private var clock = 0.0

    private func run(_ rig: inout EyeRig, seconds: Double, dt: Double = 1.0 / 60) {
        let end = clock + seconds
        while clock < end {
            rig.step(dt: dt, time: clock)
            clock += dt
        }
    }

    // MARK: targets (spec 3-4)

    func testHeardTargetFromSpec() {
        let c = EyeRig.target(for: .heard, remote: false)
        XCTAssertEqual(c.left.size, 1.25, accuracy: 0.001)
        XCTAssertEqual(c.right.size, 1.25, accuracy: 0.001)
        XCTAssertEqual(c.left.glow, 1.0, accuracy: 0.001)
    }

    func testPausedIsOneSided() {
        let c = EyeRig.target(for: .paused, remote: false)
        XCTAssertEqual(c.left.size, 0.7, accuracy: 0.001)
        XCTAssertEqual(c.left.glow, 0.35, accuracy: 0.001)
        XCTAssertEqual(c.right.size, 0.6, accuracy: 0.001)
        XCTAssertEqual(c.right.glow, 0.10, accuracy: 0.001)
    }

    func testUnsureIsSymmetric() {
        let c = EyeRig.target(for: .unsure, remote: false)
        XCTAssertEqual(c.left, c.right)
        XCTAssertEqual(c.left.size, 0.8, accuracy: 0.001)
        XCTAssertEqual(c.left.glow, 0.6, accuracy: 0.001)
    }

    func testHappyPullsEyesInward() {
        let c = EyeRig.target(for: .happy, remote: false)
        XCTAssertEqual(c.left.x, 0.15, accuracy: 0.001)
        XCTAssertEqual(c.right.x, -0.15, accuracy: 0.001)
    }

    func testRemoteAddsFrost() {
        XCTAssertEqual(EyeRig.target(for: .listening, remote: true).frost, 0.6, accuracy: 0.001)
    }

    // MARK: motion

    func testListeningAlternatesOutOfPhase() {
        // 1.4 s period: at t = 0.35 the left light grows while the right shrinks
        let m = EyeRig.motion(for: .listening, level: 1, at: 0.35)
        XCTAssertGreaterThan(m.left.size, 0)
        XCTAssertLessThan(m.right.size, 0)
        XCTAssertEqual(m.left.size, -m.right.size, accuracy: 0.001)
    }

    func testListeningAmplitudeClamped() {
        let lo = EyeRig.motion(for: .listening, level: 0, at: 0.35).left.size
        let hi = EyeRig.motion(for: .listening, level: 1, at: 0.35).left.size
        let over = EyeRig.motion(for: .listening, level: 7, at: 0.35).left.size
        let under = EyeRig.motion(for: .listening, level: -3, at: 0.35).left.size
        XCTAssertEqual(lo, 0.1, accuracy: 0.01)
        XCTAssertEqual(hi, 0.3, accuracy: 0.01)
        XCTAssertEqual(over, hi, accuracy: 0.001)
        XCTAssertEqual(under, lo, accuracy: 0.001)
    }

    func testThinkingLeftLeadsRightBy200ms() {
        // 1 s period: left peaks near t = 0.1, right near t = 0.3
        let l = EyeRig.motion(for: .thinking, level: 0, at: 0.1)
        let r = EyeRig.motion(for: .thinking, level: 0, at: 0.3)
        XCTAssertLessThan(l.left.y, -0.1)
        XCTAssertEqual(l.right.y, 0, accuracy: 0.02)
        XCTAssertLessThan(r.right.y, -0.1)
        XCTAssertEqual(r.left.y, 0, accuracy: 0.02)
    }

    // MARK: spring

    func testSpringReaches85PercentAtAttack() {
        // unsure (0.8, no motion) → heard (1.25, no motion): away from rest, so the 0.2 s attack applies
        var rig = EyeRig()
        rig.reduceMotion = false
        rig.mood = .unsure
        run(&rig, seconds: 2)
        rig.mood = .heard
        let start = rig.current.left.size
        run(&rig, seconds: 0.2)
        let frac = (rig.current.left.size - start) / (1.25 - start)
        XCTAssertEqual(frac, 0.9, accuracy: 0.08)   // ζ .85 at ωt = 3.36 lands near 92%
    }

    func testReleaseIsSlowerThanAttack() {
        // heard (1.25) → unsure (0.8): back toward rest, so attack × 2.2 → about halfway at 0.2 s
        var rig = EyeRig()
        rig.reduceMotion = false
        rig.mood = .heard
        run(&rig, seconds: 2)
        rig.mood = .unsure
        let start = rig.current.left.size
        run(&rig, seconds: 0.2)
        let frac = (start - rig.current.left.size) / (start - 0.8)
        XCTAssertEqual(frac, 0.45, accuracy: 0.1)
    }

    func testReduceMotionSnapsToTarget() {
        var rig = EyeRig()
        rig.reduceMotion = true
        rig.mood = .heard
        rig.step(dt: 1.0 / 60, time: 0)
        XCTAssertEqual(rig.current, EyeRig.target(for: .heard, remote: false))
    }

    func testStillMoodSettlesAtItsTarget() {
        // heard has no motion: the springs come to rest, and the rig says so (CueEyes stops redrawing)
        var rig = EyeRig()
        rig.reduceMotion = false
        rig.mood = .heard
        run(&rig, seconds: 0.3)
        XCTAssertFalse(rig.settled)
        run(&rig, seconds: 4)
        XCTAssertTrue(rig.settled)
        XCTAssertEqual(rig.current.left.size, 1.25, accuracy: EyeRig.stillness)
        rig.mood = .thinking
        run(&rig, seconds: 1.0 / 60)
        XCTAssertFalse(rig.settled)
    }

    func testBreathingNeverSettles() {
        var rig = EyeRig()
        rig.reduceMotion = false
        rig.mood = .asleep
        for _ in 0..<600 {
            run(&rig, seconds: 1.0 / 60)
            XCTAssertFalse(rig.settled)
        }
    }

    func testRapidMoodChangesStayFinite() {
        var rig = EyeRig()
        rig.reduceMotion = false
        for (i, m) in [EyeMood.listening, .heard, .thinking, .happy, .listening].enumerated() {
            rig.mood = m
            run(&rig, seconds: 0.1)
            XCTAssertTrue(rig.current.isFinite, "step \(i)")
        }
        XCTAssertLessThan(rig.current.left.size, 2.0)
        XCTAssertGreaterThan(rig.current.left.size, 0.3)
    }

    func testBodyFollowsEyesLagged() {
        var rig = EyeRig()
        rig.reduceMotion = false
        rig.mood = .thinking
        var peak = 0.0
        for _ in 0..<180 {
            run(&rig, seconds: 1.0 / 60)
            peak = max(peak, abs(rig.current.tilt))
        }
        XCTAssertLessThanOrEqual(peak, 2.0 + 0.05)    // spec: thinking tilts ±2°
        XCTAssertGreaterThan(peak, 1.0)               // it must follow, even late
        rig.mood = .heard
        run(&rig, seconds: 1)
        XCTAssertEqual(rig.current.squash, 1.04, accuracy: 0.01)
    }
}
