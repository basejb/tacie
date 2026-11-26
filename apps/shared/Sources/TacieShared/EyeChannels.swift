import Foundation

/// One of the two lights inside the pill: how big, how bright, and how far off its resting spot.
struct EyeSide: Equatable {
    var size: Double = 1     // radius multiplier, 0.5...1.5
    var glow: Double = 1     // opacity, 0...1
    var x: Double = 0        // offset in eye gaps, -0.3...0.3
    var y: Double = 0        // offset in pill heights, -0.3...0.3

    static let rest = EyeSide()

    static func lerp(_ a: EyeSide, _ b: EyeSide, _ t: Double) -> EyeSide {
        EyeSide(size: a.size + (b.size - a.size) * t,
                glow: a.glow + (b.glow - a.glow) * t,
                x: a.x + (b.x - a.x) * t,
                y: a.y + (b.y - a.y) * t)
    }

    var isFinite: Bool { size.isFinite && glow.isFinite && x.isFinite && y.isFinite }
}

/// The 11 rig values the island's face is drawn from. The rig writes them, the view only reads.
struct EyeChannels: Equatable {
    var left: EyeSide = .rest
    var right: EyeSide = .rest
    var tilt: Double = 0       // degrees, -6...6
    var squash: Double = 1     // vertical scale, 0.9...1.1 (horizontal is the inverse)
    var frost: Double = 0      // 0...1, how hazy the lights look (remote meetings)

    static let rest = EyeChannels()

    static func lerp(_ a: EyeChannels, _ b: EyeChannels, _ t: Double) -> EyeChannels {
        EyeChannels(left: .lerp(a.left, b.left, t),
                    right: .lerp(a.right, b.right, t),
                    tilt: a.tilt + (b.tilt - a.tilt) * t,
                    squash: a.squash + (b.squash - a.squash) * t,
                    frost: a.frost + (b.frost - a.frost) * t)
    }

    var isFinite: Bool { left.isFinite && right.isFinite && tilt.isFinite && squash.isFinite && frost.isFinite }
}

/// What the island is doing, as the face shows it.
enum EyeMood: Equatable {
    case asleep      // no meeting
    case starting    // meeting starting up
    case listening   // lights alternate with the voice
    case heard       // a question was caught
    case thinking    // searching
    case happy       // found it
    case unsure      // nothing certain (below 0.8)
    case paused      // paused, or no microphone
    case failed      // an outside service failed
}
