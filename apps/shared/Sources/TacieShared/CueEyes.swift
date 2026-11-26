import SwiftUI

/// Tacie's face on the island: the round head at the pill's left end, two warm lights inside it (2026-10-26; until
/// then the lights sat in the pill on their own). No eyeballs, no mouth; the lights' size, brightness and position
/// say what the app is doing, and the head tilts and squashes a little behind them. `EyeRig` decides the values,
/// this view only draws them. Island only: the main window and avatars draw the still logo (`BrandMark`) instead.
struct CueEyes: View {
    let mood: EyeMood
    var level: Float = 0
    var remote: Bool = false
    var tint: EyeTint = .warm
    /// The head's height; the view is 1.5 × as wide (`BrandGeometry.aspect`).
    var height: CGFloat = 24
    /// Where the lights look, -1...1 each way; nil looks ahead (`EyeRig.gaze`).
    var gaze: CGPoint? = nil

    // The rig lives in a box, not in view state: stepping it inside the timeline body must not invalidate
    // the view, or every frame re-runs the body in a loop (one core at 100%, springs stepped 400× a frame).
    @State private var box = RigBox()
    /// The mood the face came to rest in; while it is still the mood, the timeline stops (nothing moves).
    @State private var stillIn: Still?
    @Environment(\.reducesMotion) private var reduceMotion

    private struct Still: Equatable { let mood: EyeMood; let remote: Bool }

    var body: some View {
        let now = Still(mood: mood, remote: remote)
        TimelineView(.animation(minimumInterval: Self.interval(for: mood), paused: reduceMotion || stillIn == now)) { ctx in
            let channels = box.advance(to: ctx.date.timeIntervalSinceReferenceDate,
                                       mood: mood, level: Double(level), remote: remote, tint: tint,
                                       reduceMotion: reduceMotion, gaze: gaze.map { (Double($0.x), Double($0.y)) })
            // state can't change mid-update: note the rest on the next turn, if the face is still there
            let _ = box.settled && stillIn != now ? DispatchQueue.main.async { if box.settled { stillIn = now } } : ()
            LitHead(channels: channels, tint: tint, height: height)
        }
        .frame(width: CGFloat(BrandGeometry.width(forHeight: Double(height))), height: height)
        .onChange(of: mood) { stillIn = nil }
        .onChange(of: remote) { stillIn = nil }
        .onChange(of: gaze) { stillIn = nil }
        .accessibilityHidden(true)
    }

    /// Asleep and starting only breathe (4 s and 6.3 s periods, a few percent of the light's size): 30 fps
    /// looks the same there. Every other mood draws at the display's rate.
    static func interval(for mood: EyeMood) -> Double? {
        switch mood {
        case .asleep, .starting: return 1.0 / 30
        default: return nil
        }
    }
}

/// Holds one `EyeRig` and steps it at most once per timestamp.
final class RigBox {
    private var rig = EyeRig()
    private var last: TimeInterval?

    func advance(to now: TimeInterval, mood: EyeMood, level: Double, remote: Bool, tint: EyeTint,
                 reduceMotion: Bool, gaze: (x: Double, y: Double)? = nil) -> EyeChannels {
        rig.mood = mood
        rig.gaze = gaze
        rig.level = level
        rig.remote = remote
        rig.tint = tint
        rig.reduceMotion = reduceMotion
        if let last, now <= last { return rig.current }     // same frame asked twice: draw what we have
        // long gaps (sleep, hidden, a paused timeline) count as one frame; 1/15 leaves room for 30 fps jitter
        // (4 substeps keep each spring step at 1/60 s or less)
        let dt = last.map { min(now - $0, 1.0 / 15) } ?? 1.0 / 60
        rig.step(dt: dt, time: now)
        last = now
        return rig.current
    }

    /// The last step left the face at rest (see `EyeRig.settled`).
    var settled: Bool { rig.settled }
}

/// The island's head for one set of channel values: frosted cream, lit from the upper left, shaded toward the
/// base, a soft contact shadow, and the two lights inside on the widest line. The head turns about its base.
/// Still; `CueEyes` animates it, `--export-eyes` renders it. Colors are fixed: on the glass, system colors turn
/// dark over dark windows.
struct LitHead: View {
    let channels: EyeChannels
    var tint: EyeTint = .warm
    var height: CGFloat

    private var width: CGFloat { CGFloat(BrandGeometry.width(forHeight: Double(height))) }

    var body: some View {
        ZStack {
            BrandOutline()
                .fill(EllipticalGradient(stops: [.init(color: .white, location: 0),
                                                 .init(color: FaceColors.hex(0xF4EDE0), location: 0.45),
                                                 .init(color: FaceColors.hex(0xD9CAAE), location: 0.85),
                                                 .init(color: FaceColors.hex(0xC9B794), location: 1)],
                                         center: UnitPoint(x: 0.42, y: 0.18), startRadiusFraction: 0, endRadiusFraction: 1))
                .shadow(color: Self.shade.opacity(0.28), radius: height * 0.025, y: height * 0.06)
                .shadow(color: Self.shade.opacity(0.12), radius: height * 0.1, y: height * 0.12)
            // the glint where the light lands, toward the top of the dome
            Ellipse()
                .fill(EllipticalGradient(colors: [.white.opacity(0.9), .white.opacity(0)], center: .center,
                                         startRadiusFraction: 0, endRadiusFraction: 0.5))
                .frame(width: width * 0.45, height: height * 0.24)
                .position(x: width * 0.41, y: height * 0.2)
            // the lights are a touch larger than the logo's so they still read at 24 pt; kept inside the head
            EyeLights(channels: channels, tint: tint, height: height, lightScale: 1.2)
                .position(x: width / 2, y: height * BrandGeometry.eyeLineRatio)
                .frame(width: width, height: height)
                .clipShape(BrandOutline())
        }
        .frame(width: width, height: height)
        .rotationEffect(.degrees(channels.tilt), anchor: .bottom)
        .scaleEffect(x: 1 / channels.squash, y: channels.squash, anchor: .bottom)
    }

    private static let shade = FaceColors.hex(0x503C1E)
}

/// The two lights for one set of channel values, centered on the view; the head (`LitHead`) and the still logo
/// (`BrandMark`) place them on the eye line.
struct EyeLights: View {
    let channels: EyeChannels
    var tint: EyeTint = .warm
    var height: CGFloat
    /// Light size against the brand ratios (core 0.07 h, halo 0.23 h); the gap between them stays the brand's.
    var lightScale: Double = 1

    var body: some View {
        Canvas { gc, size in draw(in: gc, size: size) }
            .frame(width: height * 1.2, height: height)   // wide enough for both halos at their resting size
    }

    private func draw(in gc: GraphicsContext, size: CGSize) {
        let c = channels
        let h = Double(height)
        let gap = BrandGeometry.eyeSpacing(forHeight: h)        // distance between the two lights
        let mid = CGPoint(x: size.width / 2, y: size.height / 2)
        let glow = tint == .error ? FaceColors.error : FaceColors.glow
        for (side, dir) in [(c.left, -1.0), (c.right, 1.0)] {
            let center = CGPoint(x: mid.x + dir * gap / 2 + side.x * gap, y: mid.y + side.y * h)
            let halo = BrandGeometry.haloRadius(height: h) * lightScale * side.size * (1 + 0.5 * c.frost)
            let core = BrandGeometry.coreRadius(height: h) * lightScale * side.size
            let alpha = min(max(side.glow, 0), 1) * (1 - 0.5 * c.frost)
            // On the island's white paper `.plusLighter` adds nothing, so the halo is a plain warm tint.
            var soft = gc
            soft.opacity = alpha * 0.8
            soft.fill(Path(ellipseIn: CGRect(x: center.x - halo, y: center.y - halo, width: halo * 2, height: halo * 2)),
                      with: .radialGradient(Gradient(stops: [.init(color: glow, location: 0),
                                                             .init(color: glow.opacity(0.55), location: 0.4),
                                                             .init(color: glow.opacity(0), location: 1)]),
                                            center: center, startRadius: 0, endRadius: halo))
            var bright = gc
            bright.opacity = alpha
            bright.fill(Path(ellipseIn: CGRect(x: center.x - core, y: center.y - core, width: core * 2, height: core * 2)),
                        with: .radialGradient(Gradient(stops: [.init(color: FaceColors.core, location: 0),
                                                               .init(color: glow, location: 0.45),
                                                               .init(color: glow, location: 1)]),
                                              center: center, startRadius: 0, endRadius: core))
        }
    }
}

/// The head's outline as a SwiftUI shape, so fills, strokes and clips all use `BrandGeometry.outline`.
struct BrandOutline: InsettableShape {
    var inset: CGFloat = 0
    func path(in rect: CGRect) -> Path { Path(BrandGeometry.outline(in: rect.insetBy(dx: inset, dy: inset))) }
    func inset(by amount: CGFloat) -> BrandOutline { BrandOutline(inset: inset + amount) }
}

/// The face's own colours, the same on the Mac and the iPhone (the Mac's `Theme.eyeGlow`, `eyeCore`, `eyeError`), and
/// fixed: on the island's glass, system colours turn dark over dark windows.
enum FaceColors {
    static let glow = hex(0xFFC654)
    static let core = hex(0xFFF6D8)
    static let error = hex(0xE8846A)

    static func hex(_ v: UInt32) -> Color {
        Color(.sRGB, red: Double((v >> 16) & 0xFF) / 255, green: Double((v >> 8) & 0xFF) / 255,
              blue: Double(v & 0xFF) / 255, opacity: 1)
    }
}

/// `--reduce-motion` sets this on the island root; views OR it with the system's accessibilityReduceMotion,
/// which cannot be written into the environment.
private struct ForcedReduceMotionKey: EnvironmentKey { static let defaultValue = false }

extension EnvironmentValues {
    var forcedReduceMotion: Bool {
        get { self[ForcedReduceMotionKey.self] }
        set { self[ForcedReduceMotionKey.self] = newValue }
    }
    /// What views read: the system setting or the debug flag. One accessor, so no view forgets the flag.
    var reducesMotion: Bool { accessibilityReduceMotion || forcedReduceMotion }
}
