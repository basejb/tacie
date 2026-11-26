import Foundation

/// Light color: warm for everything, reddish when an outside service failed.
enum EyeTint: Equatable { case warm, error }

/// Drives the island's face: a state picks target values, springs chase them,
/// and the view draws whatever the springs are at. The body (tilt, squash) follows the eyes a little late.
struct EyeRig {
    struct Tuning {
        var attack: Double = 0.2             // s to reach a target when moving away from rest
        var zeta: Double = 0.85
        var releaseFactor: Double = 2.2      // attack × this when moving back toward rest
        var substeps: Int = 4
        var tiltPerAsymmetry: Double = 13.3  // degrees per (left.y - right.y); thinking peaks at 2°
        var squashPerSize: Double = 0.16     // per avg(size - 1); heard lands on 1.04
        var bodyLag: Double = 0.09           // s
        var breathPeriods: (Double, Double) = (4.0, 6.3)
        var gazeX: Double = 0.3              // light offset (× the gap between them) at a full look sideways
        var gazeY: Double = 0.12             // light offset (× head height) at a full look up or down
        static let `default` = Tuning()
    }

    var tuning: Tuning
    var mood: EyeMood = .asleep
    var level: Double = 0          // 0...1, voice level while listening
    var remote: Bool = false       // remote meeting: lights go hazy
    var tint: EyeTint = .warm
    var reduceMotion: Bool = false
    /// Where the face looks, -1...1 each way (x right, y down); nil looks ahead. The main window's feedback card
    /// follows the pointer with it; the island never sets it.
    var gaze: (x: Double, y: Double)? = nil
    private(set) var current: EyeChannels = .rest
    /// The face has come to rest: the mood has no motion of its own and every spring is within `stillness`
    /// of its goal, so further steps change nothing visible. `CueEyes` pauses its timeline on it.
    private(set) var settled = false

    // Per-channel state kept in channel-shaped structs (not keypath dictionaries: hashing a KeyPath
    // 40 times a step was most of the idle cost). A channel read before it is set is 0, as before.
    private var velocity = EyeChannels.zero
    private var bodyHistory: [(time: Double, tilt: Double, squash: Double)] = []
    private var speedFor = EyeChannels.zero    // attack seconds per channel, fixed when the mood changes
    private var speedMood: (EyeMood, Bool)?

    init(tuning: Tuning = .default) {
        self.tuning = tuning
    }

    // MARK: targets (spec 3-4)

    /// Where each mood rests, without the alternation and breathing on top.
    static func target(for mood: EyeMood, remote: Bool) -> EyeChannels {
        var c = EyeChannels.rest
        switch mood {
        case .asleep:
            c.left = EyeSide(size: 0.7, glow: 0.35); c.right = EyeSide(size: 0.78, glow: 0.45)
        case .starting:
            c.left = EyeSide(size: 0.7, glow: 0.5); c.right = EyeSide(size: 0.78, glow: 0.6)
        case .listening:
            c.left = EyeSide(size: 0.86, glow: 0.78); c.right = c.left
        case .heard:
            c.left = EyeSide(size: 1.25, glow: 1); c.right = c.left
        case .thinking:
            c.left = EyeSide(size: 1, glow: 1); c.right = c.left
        case .happy:
            c.left = EyeSide(size: 0.9, glow: 1, x: 0.15); c.right = EyeSide(size: 0.9, glow: 1, x: -0.15)
        case .unsure:
            c.left = EyeSide(size: 0.8, glow: 0.6); c.right = c.left
        case .paused, .failed:
            c.left = EyeSide(size: 0.7, glow: 0.35); c.right = EyeSide(size: 0.6, glow: 0.10)
        }
        c.frost = remote ? 0.6 : 0
        return c
    }

    /// What moves on top of the target at time t: alternation, breathing, the thinking hops.
    static func motion(for mood: EyeMood, level: Double, at t: Double, tuning: Tuning = .default) -> EyeChannels {
        var m = EyeChannels.zero
        switch mood {
        case .listening:
            let amp = 0.1 + 0.2 * min(max(level, 0), 1)
            let s = amp * sin(2 * .pi * t / 1.4)
            m.left.size = s; m.right.size = -s
            m.left.glow = 1.5 * s; m.right.glow = -1.5 * s
        case .asleep, .starting:
            let s = 0.04 * sin(2 * .pi * t / tuning.breathPeriods.0)
            let b = 0.03 * sin(2 * .pi * t / tuning.breathPeriods.1)
            m.left.size = s; m.right.size = -s
            m.left.glow = 0.05 * sin(2 * .pi * t / tuning.breathPeriods.0) + b
            m.right.glow = -0.05 * sin(2 * .pi * t / tuning.breathPeriods.0) + b
        case .thinking:
            let p = t - floor(t)   // 1 s period: left hops in [0, 0.2], right in [0.2, 0.4]
            if p < 0.2 {
                let s = sin(.pi * p / 0.2)
                m.left.y = -0.15 * s; m.left.size = 0.1 * s; m.left.glow = 0.5 * s
            } else if p < 0.4 {
                let s = sin(.pi * (p - 0.2) / 0.2)
                m.right.y = -0.15 * s; m.right.size = 0.1 * s; m.right.glow = 0.5 * s
            }
        case .heard, .happy, .unsure, .paused, .failed:
            break
        }
        return m
    }

    // MARK: stepping

    /// Advance dt seconds. Eyes chase target + motion through springs; the body chases the eyes, 90 ms late.
    mutating func step(dt: Double, time t: Double) {
        let base = Self.target(for: mood, remote: remote)
        if reduceMotion {
            current = base
            velocity = .zero
            bodyHistory.removeAll()
            settled = Self.isStill(mood)
            return
        }
        if speedMood == nil || speedMood! != (mood, remote) {
            speedMood = (mood, remote)
            setSpeeds(toward: base)
        }
        var goal = base + Self.motion(for: mood, level: level, at: t, tuning: tuning)
        if let g = gaze {
            let x = min(max(g.x, -1), 1) * tuning.gazeX, y = min(max(g.y, -1), 1) * tuning.gazeY
            goal.left.x += x; goal.right.x += x
            goal.left.y += y; goal.right.y += y
        }
        let h = dt / Double(tuning.substeps)
        for _ in 0..<tuning.substeps {
            chase(\.left.size, toward: goal.left.size, rest: 1, h: h)
            chase(\.left.glow, toward: goal.left.glow, rest: 1, h: h)
            chase(\.left.x, toward: goal.left.x, rest: 0, h: h)
            chase(\.left.y, toward: goal.left.y, rest: 0, h: h)
            chase(\.right.size, toward: goal.right.size, rest: 1, h: h)
            chase(\.right.glow, toward: goal.right.glow, rest: 1, h: h)
            chase(\.right.x, toward: goal.right.x, rest: 0, h: h)
            chase(\.right.y, toward: goal.right.y, rest: 0, h: h)
        }
        current.frost = goal.frost

        // the body follows what the eyes were doing bodyLag ago
        let tiltRaw = tuning.tiltPerAsymmetry * (current.left.y - current.right.y)
        let squashRaw = 1 + tuning.squashPerSize * ((current.left.size - 1) + (current.right.size - 1)) / 2
        bodyHistory.append((t, tiltRaw, squashRaw))
        let lagged = bodyHistory.last { $0.time <= t - tuning.bodyLag } ?? bodyHistory[0]
        bodyHistory.removeAll { $0.time < t - tuning.bodyLag - dt }
        for _ in 0..<tuning.substeps {
            chase(\.tilt, toward: lagged.tilt, rest: 0, h: h)
            chase(\.squash, toward: lagged.squash, rest: 1, h: h)
        }
        settled = Self.isStill(mood) && Self.near(current, goal, tilt: tiltRaw, squash: squashRaw)
            && Self.near(velocity, .zero, tilt: 0, squash: 0)
    }

    /// Moods whose `motion` is always zero: their springs settle to a fixed pose.
    static func isStill(_ mood: EyeMood) -> Bool {
        switch mood {
        case .heard, .happy, .unsure, .paused, .failed: return true
        case .listening, .asleep, .starting, .thinking: return false
        }
    }

    /// Far below a pixel at the island's size (sizes and offsets are fractions of a 24 pt head).
    static let stillness = 1e-4

    private static func near(_ a: EyeChannels, _ b: EyeChannels, tilt: Double, squash: Double) -> Bool {
        func n(_ x: Double, _ y: Double) -> Bool { abs(x - y) < stillness }
        return n(a.left.size, b.left.size) && n(a.left.glow, b.left.glow) && n(a.left.x, b.left.x) && n(a.left.y, b.left.y)
            && n(a.right.size, b.right.size) && n(a.right.glow, b.right.glow) && n(a.right.x, b.right.x)
            && n(a.right.y, b.right.y) && n(a.tilt, tilt) && n(a.squash, squash)
    }

    /// Decided once per mood change: a channel heading back toward rest moves slower than one leaving it.
    private mutating func setSpeeds(toward base: EyeChannels) {
        let rests: [(WritableKeyPath<EyeChannels, Double>, Double)] = [
            (\.left.size, 1), (\.left.glow, 1), (\.left.x, 0), (\.left.y, 0),
            (\.right.size, 1), (\.right.glow, 1), (\.right.x, 0), (\.right.y, 0),
            (\.tilt, 0), (\.squash, 1),
        ]
        for (key, rest) in rests {
            let returning = abs(base[keyPath: key] - rest) < abs(current[keyPath: key] - rest)
            speedFor[keyPath: key] = returning ? tuning.attack * tuning.releaseFactor : tuning.attack
        }
    }

    /// One spring step on one channel.
    private mutating func chase(_ key: WritableKeyPath<EyeChannels, Double>, toward target: Double, rest: Double, h: Double) {
        let x = current[keyPath: key]
        let attack = speedFor[keyPath: key] == 0 ? tuning.attack : speedFor[keyPath: key]
        let omega = 3.36 / attack
        var v = velocity[keyPath: key]
        let a = omega * omega * (target - x) - 2 * tuning.zeta * omega * v
        v += a * h
        velocity[keyPath: key] = v
        current[keyPath: key] = x + v * h
    }
}

extension EyeChannels {
    /// All zeros: the additive identity for motion offsets.
    static let zero = EyeChannels(left: EyeSide(size: 0, glow: 0, x: 0, y: 0),
                                  right: EyeSide(size: 0, glow: 0, x: 0, y: 0),
                                  tilt: 0, squash: 0, frost: 0)

    static func + (a: EyeChannels, b: EyeChannels) -> EyeChannels {
        EyeChannels(left: EyeSide(size: a.left.size + b.left.size, glow: a.left.glow + b.left.glow,
                                  x: a.left.x + b.left.x, y: a.left.y + b.left.y),
                    right: EyeSide(size: a.right.size + b.right.size, glow: a.right.glow + b.right.glow,
                                   x: a.right.x + b.right.x, y: a.right.y + b.right.y),
                    tilt: a.tilt + b.tilt, squash: a.squash + b.squash, frost: a.frost + b.frost)
    }
}
