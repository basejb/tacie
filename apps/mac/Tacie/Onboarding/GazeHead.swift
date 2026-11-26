import AppKit
import SwiftUI

/// The welcome's face, looking toward the pointer (the landing's face does the same, site/src/face-rig.ts). The mood's
/// own motion comes from `EyeRig` as on the island; on top of it both lights move toward the pointer and the head turns
/// a little late. Holds still with Reduce Motion.
struct GazeHead: View {
    let mood: EyeMood
    var height: CGFloat = 78
    @State private var rig = RigBox()
    @State private var gaze = Gaze()
    @Environment(\.reducesMotion) private var reduce

    var body: some View {
        GeometryReader { geo in
            TimelineView(.animation(paused: reduce)) { ctx in
                let now = ctx.date.timeIntervalSinceReferenceDate
                let base = rig.advance(to: now, mood: mood, level: 0, remote: false, tint: .warm, reduceMotion: reduce)
                let look = reduce ? (0, 0, 0) : gaze.advance(to: now, target: Self.target(face: geo.frame(in: .global)))
                LitHead(channels: Self.looking(base, x: look.0, y: look.1, turn: look.2), height: height)
            }
        }
        .frame(width: CGFloat(BrandGeometry.width(forHeight: Double(height))), height: height)
        .accessibilityHidden(true)
    }

    /// Where the pointer is, as a gaze in −1…1 on each axis, from the face's middle (window coordinates → screen).
    private static func target(face: CGRect) -> (Double, Double) {
        guard let window = NSApp.keyWindow ?? NSApp.windows.first(where: { $0.title == OnboardingWindowController.title }) else { return (0, 0) }
        let mouse = NSEvent.mouseLocation
        let cx = window.frame.minX + face.midX
        let cy = window.frame.maxY - face.minY - face.height * 0.48
        return (tanh((mouse.x - cx) / 340), tanh((cy - mouse.y) / 300))
    }

    /// The rig's channels with the gaze on top: lights move up to 0.16 eye gaps across and 0.07 heights up or down,
    /// the far light shrinks a touch, and the head tilts up to 2°.
    static func looking(_ c: EyeChannels, x: Double, y: Double, turn: Double) -> EyeChannels {
        var c = c
        c.left.x += x * 0.16
        c.right.x += x * 0.16
        c.left.y += y * 0.07
        c.right.y += y * 0.07
        c.left.size *= 1 + 0.05 * x
        c.right.size *= 1 - 0.05 * x
        c.tilt += turn * 2
        return c
    }
}

/// Two springs for the gaze and a slower one for the head's turn (the landing's numbers: attack 0.2 s, ζ .85,
/// back to rest 2.2 × slower, the body 0.34 s). Not view state: stepping it every frame must not redraw the body.
final class Gaze {
    private var x = (p: 0.0, v: 0.0), y = (p: 0.0, v: 0.0), turn = (p: 0.0, v: 0.0)
    private var last: TimeInterval?

    func advance(to now: TimeInterval, target: (Double, Double)) -> (Double, Double, Double) {
        let dt = min(1.0 / 30, now - (last ?? now))
        last = now
        guard dt > 0 else { return (x.p, y.p, turn.p) }
        for _ in 0..<4 {
            let h = dt / 4
            Self.step(&x, toward: target.0, attack: abs(target.0) < abs(x.p) ? 0.44 : 0.2, h)
            Self.step(&y, toward: target.1, attack: abs(target.1) < abs(y.p) ? 0.44 : 0.2, h)
            Self.step(&turn, toward: x.p, attack: 0.34, h)
        }
        return (x.p, y.p, turn.p)
    }

    private static func step(_ s: inout (p: Double, v: Double), toward target: Double, attack: Double, _ dt: Double) {
        let w = 3.36 / attack
        s.v += (w * w * (target - s.p) - 2 * 0.85 * w * s.v) * dt
        s.p += s.v * dt
    }
}
