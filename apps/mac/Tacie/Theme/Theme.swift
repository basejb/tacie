import AppKit
import SwiftUI

/// Light, quiet surfaces: one ink at opacity steps (1 · .66 · .5 · .3), round pills, soft layered shadows,
/// and a single living accent for "listening". (docs/03)
enum Theme {
    static let ink = Color.dynamic(light: 0x0B0B0C, dark: 0xEDEDEB)
    static let ink2 = ink.opacity(0.66)
    /// 50% on Retina; 62% on 1x monitors, where small grey Korean at 50% reads as a smudge (3.7:1 on white).
    static let ink3 = DisplayInk(retina: 0.5, oneX: 0.62)
    static let ink4 = ink.opacity(0.3)
    static let hairline = ink.opacity(0.08)
    static let fill = ink.opacity(0.05)
    static let fillHover = ink.opacity(0.08)
    static let paper = Color.dynamic(light: 0xFFFFFF, dark: 0x1A1A1B)
    /// a surface lifted over the page (settings, menus, the player bar): the page itself in light mode, a step lighter in dark
    static let raised = Color.dynamic(light: 0xFFFFFF, dark: 0x232325)
    /// behind a service's own logo: light in both modes, since the official marks are drawn for a light ground
    static let logoTile = Color.dynamic(light: 0xFFFFFF, dark: 0xF2F2F0)
    /// the chosen chip of a segmented choice: white on the grey track, a lighter grey in dark mode
    static let chipOn = Color.dynamic(light: 0xFFFFFF, dark: 0x3C3C3F)
    static let live = Color(hex: 0x1FB85A)
    static let warn = Color(hex: 0xE5484D)
    /// A slow connection: the one lit bar of the pill's signal bars (`NetBars`).
    static let slow = Color(hex: 0xE39B2D)
    /// A suggestion waiting for a yes (화자 추천): the dotted line under a name and its row in the namer.
    static let suggest = Color(hex: 0xE0A03A)
    static let suggestTint = Color.dynamic(light: 0xFBF6EE, dark: 0x2E2719)
    static let settingsSidebar = Color.dynamic(light: 0xF8F8F7, dark: 0x202021)
    static let scrim = Color.dynamic(light: 0x000000, dark: 0x000000, lightOpacity: 0.28, darkOpacity: 0.55)
    /// the light veil under the compact window's meeting list
    static let scrimSoft = Color.dynamic(light: 0x000000, dark: 0x000000, lightOpacity: 0.06, darkOpacity: 0.35)
    static let modalShadow = Color.dynamic(light: 0x000000, dark: 0x000000, lightOpacity: 0.18, darkOpacity: 0.5)
    /// a thin light edge on a modal: in dark mode the shadow alone does not lift it off the page
    static let modalEdge = Color.dynamic(light: 0x000000, dark: 0xFFFFFF, lightOpacity: 0, darkOpacity: 0.1)
    static let knob = Color.dynamic(light: 0xFFFFFF, dark: 0x1A1A1B)
    /// the knob of a toggle that is off: on dark paper a dark knob would vanish into the grey track
    static let knobOff = Color.dynamic(light: 0xFFFFFF, dark: 0xB4B4B1)
    static let knobShadow = Color.black.opacity(0.18)
    static let cardShadow = Color.dynamic(light: 0x000000, dark: 0x000000, lightOpacity: 0.03, darkOpacity: 0.3)
    /// text on the black (dark mode: near-white) or red buttons
    static let onStrong = Color.dynamic(light: 0xFFFFFF, dark: 0x111112)
    /// the two lights inside the island (docs/03): warm, with a brighter center; reddish when a service failed
    static let eyeGlow = Color(hex: 0xFFC654)
    static let eyeCore = Color(hex: 0xFFF6D8)
    static let eyeError = Color(hex: 0xE8846A)
    /// one colour per meeting speaker (SavedMeeting.speakerColors): a light tint for the dot, the letter in the colour itself
    static let speakers: [Color] = [0x4F7BEF, 0x1F9E74, 0xE0803A, 0x9A6AE8, 0xD9577F, 0x2A9DB4].map { Color(hex: $0) }
    static func speaker(_ index: Int) -> Color { speakers[((index % speakers.count) + speakers.count) % speakers.count] }

    static let meta = Font.system(size: 12.5)
    static let question = Font.system(size: 13.5, weight: .medium)
    static let answer = Font.system(size: 16, weight: .medium)
    static let answerLarge = Font.system(size: 26, weight: .medium)
    static let chip = Font.system(size: 12.5, weight: .medium)
    static let code = Font.system(size: 11.5, design: .monospaced)
    static let button = Font.system(size: 13, weight: .semibold)

    static let pillHeight: CGFloat = 38
    static let cardWidth: CGFloat = 520
    static let cardWidthLarge: CGFloat = 760
    static let radiusCard: CGFloat = 26
    static let radiusInner: CGFloat = 14
    /// room around the island for its shadow; the window is this much larger than what it shows
    static let shadowRoom: CGFloat = 22

    /// Liquid Glass tinted white, with a thin white veil on top: still glass (edge refraction, a little colour
    /// through), yet cream over dark windows instead of grey. Tried 2026-10-25: more tint only turns greyer;
    /// a veil of .5 hid the glass, .3 keeps it.
    static let glassTint = 0.75
    static let glassVeil = 0.3
    /// what the glass is tinted with: white, or a warm near-black in dark mode
    static let glassBase = Color.dynamic(light: 0xFFFFFF, dark: 0x1C1C1E)

    /// iOS sheet curve for moves
    static let move = Animation.timingCurve(0.32, 0.72, 0, 1, duration: 0.42)
    /// a slight overshoot, for appearance only: a card or a trail coming in
    static let grow = Animation.spring(response: 0.42, dampingFraction: 0.78)
    /// sizes that report state (card height, pill width, 크게 보기) settle without overshoot
    static let settle = Animation.spring(response: 0.4, dampingFraction: 1)
    /// small feedback (press, toggle, chevron, a count rolling): fast, no overshoot
    static let snappy = Animation.spring(response: 0.26, dampingFraction: 1)
    /// one highlight gliding between rows or tabs. Quicker than the panels and with a hint of bounce: at 0.42s
    /// critically damped it trailed the click (2026-10-25 hands-on), which read as lag rather than calm.
    static let glide = Animation.spring(duration: 0.32, bounce: 0.1)
    /// something coming onto the screen on a curve, not a spring
    static let enter = Animation.timingCurve(0.16, 1, 0.3, 1, duration: 0.24)
    /// delay for the n-th item of a list that arrives one after another; keep the total under about 0.4s
    static func stagger(_ index: Int, step: Double = 0.035) -> Double { Double(index) * step }
    /// hovering a trail tab shows its answer: a quick, plain ease (not tied to the exit timing)
    static let peek = Animation.easeOut(duration: 0.18)
    /// a small control going away (pause, stop, the open button when the pointer leaves the pill): gone fast from the
    /// first frame. `exit`'s ease-in holds a faint copy on screen, then snaps, which reads as lag on something this small
    static let vanish = Animation.easeOut(duration: 0.12)
    /// leaving is quicker than arriving and never bounces
    static let exitSeconds = 0.18
    static let exit = Animation.timingCurve(0.7, 0, 0.84, 0, duration: exitSeconds)
    static let fast = Animation.easeOut(duration: 0.14)
    /// the old page when a meeting page or settings pane is swapped: off in a couple of frames, so it never sits
    /// under the new one. `exit`'s ease-in held it near full opacity while the new page rose in (2026-10-31 hands-on)
    static let swap = Animation.easeOut(duration: 0.07)
    /// 움직임 줄이기: no travel, scale or blur, only a short fade
    static let reduced = Animation.easeOut(duration: 0.12)
    static func anim(_ a: Animation, reduce: Bool) -> Animation { reduce ? reduced : a }

    /// A page or block of text coming in: it fades in and moves `x`/`y` points into place, on Retina only. On a 1x
    /// monitor the move leaves the text half a pixel off the grid after it ends, until the window is resized, and
    /// every line looks smudged (measured 2026-11-03 on a 1080p monitor: hard top/bottom stroke edges 971 → 32 on a
    /// summary line). There, and with 움직임 줄이기, it only fades.
    static func arrive(x: CGFloat = 0, y: CGFloat = 0, reduce: Bool, scale: CGFloat) -> AnyTransition {
        reduce || scale < 2 ? .opacity : .opacity.combined(with: .offset(x: x, y: y)).animation(enter)
    }
}

/// 화면 모드 (설정 > 일반): follow the system, or keep light or dark. Theme colours read `isDark` themselves
/// instead of the drawing appearance, so the island's glass taking a dark look over a dark window never flips them.
@Observable final class AppearanceMode {
    enum Choice: String, CaseIterable { case system, light, dark }
    static let shared = AppearanceMode()
    static let key = "appearance"
    /// read by the colour providers, which may run off the main thread
    nonisolated(unsafe) static var isDark = false

    var choice: Choice { didSet { UserDefaults.standard.set(choice.rawValue, forKey: Self.key); apply() } }
    private(set) var systemDark = false
    var dark: Bool { choice == .system ? systemDark : choice == .dark }
    var scheme: ColorScheme { dark ? .dark : .light }
    var nsAppearance: NSAppearance? { NSAppearance(named: dark ? .darkAqua : .aqua) }

    private init() {
        choice = Choice(rawValue: UserDefaults.standard.string(forKey: Self.key) ?? "") ?? .system
        systemDark = Self.readSystem()
        Self.isDark = dark
        DistributedNotificationCenter.default().addObserver(forName: Notification.Name("AppleInterfaceThemeChangedNotification"),
                                                            object: nil, queue: .main) { [weak self] _ in
            guard let self else { return }
            self.systemDark = Self.readSystem()
            self.apply()
        }
    }

    private static func readSystem() -> Bool {
        UserDefaults.standard.string(forKey: "AppleInterfaceStyle") == "Dark"
    }

    /// Repaints every window: their appearance changes, so dynamic colours resolve again.
    func apply() {
        Self.isDark = dark
        for w in NSApp.windows { w.appearance = nsAppearance; w.invalidateShadow(); w.contentView?.needsDisplay = true }
        for w in NSApp.windows where w.backgroundColor != .clear { w.backgroundColor = NSColor(Theme.paper) }
    }
}

extension View {
    /// The colour scheme of the chosen 화면 모드; views read it so a switch redraws them.
    func appearanceScheme() -> some View { modifier(AppearanceScheme()) }
}

private struct AppearanceScheme: ViewModifier {
    @State private var mode = AppearanceMode.shared
    func body(content: Content) -> some View { content.environment(\.colorScheme, mode.scheme) }
}

/// A service's official mark for the current 화면 모드: Linear's dark mark vanishes on dark paper, so dark mode uses its
/// official light mark. The rest are drawn for either ground.
enum ServiceLogo {
    static func name(_ logo: String, _ scheme: ColorScheme) -> String {
        scheme == .dark && logo == "ServiceLinear" ? "ServiceLinearLight" : logo
    }
}

extension Color {
    /// One colour per 화면 모드, chosen by `AppearanceMode.isDark` at draw time.
    static func dynamic(light: UInt32, dark: UInt32, lightOpacity: Double = 1, darkOpacity: Double = 1) -> Color {
        func ns(_ hex: UInt32, _ a: Double) -> NSColor {
            NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                    blue: CGFloat(hex & 0xFF) / 255, alpha: a)
        }
        let l = ns(light, lightOpacity), d = ns(dark, darkOpacity)
        return Color(nsColor: NSColor(name: nil) { _ in AppearanceMode.isDark ? d : l })
    }

    init(hex: UInt32, opacity: Double = 1) {
        self.init(.sRGB,
                  red: Double((hex >> 16) & 0xFF) / 255,
                  green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255,
                  opacity: opacity)
    }
}

extension View {
    /// The island's material: white-tinted Liquid Glass under a thin white veil on macOS 26 (`Theme.glassTint`,
    /// `Theme.glassVeil`); near-opaque paper over frosted blur before it. Always light, so ink text reads over
    /// dark windows too.
    @ViewBuilder func surface<S: Shape>(_ shape: S) -> some View {
        if #available(macOS 26.0, *) {
            self.background(Theme.glassBase.opacity(Theme.glassVeil), in: shape)
                .glassEffect(.regular.tint(Theme.glassBase.opacity(Theme.glassTint)), in: shape)
        } else {
            self.background(Theme.glassBase.opacity(0.92), in: shape)
                .background(VisualEffect().clipShape(shape))
        }
    }

    /// Several soft layers instead of one hard shadow.
    func softShadow() -> some View {
        self.shadow(color: .black.opacity(0.05), radius: 1, y: 1)
            .shadow(color: .black.opacity(0.07), radius: 10, y: 6)
            .shadow(color: .black.opacity(0.05), radius: 24, y: 14)
    }
}

/// Behind-window blur for macOS 14–15.
struct VisualEffect: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let v = NSVisualEffectView()
        v.material = .popover
        v.blendingMode = .behindWindow
        v.state = .active
        return v
    }

    func updateNSView(_ nsView: NSVisualEffectView, context: Context) {}
}

/// Appear from a blur, the way an answer comes into focus.
/// A band of light sweeping over a line that is still being written. One pass per `period` seconds.
enum Shimmer {
    static func phase(at t: Double, period: Double = 1.6) -> Double {
        t.truncatingRemainder(dividingBy: period) / period
    }
    /// Leading-edge offset of a `band`-wide highlight: fully off the left at phase 0, fully off the right at 1.
    static func bandOffset(phase: Double, width: Double, band: Double) -> Double {
        phase * (width + band) - band
    }
}

/// Opacity leads (0.18s) and the blur clears after it (0.3s), so the text is readable before it is fully sharp.
/// Timed here, not by the surrounding layout animation.
struct BlurIn: ViewModifier {
    let active: Bool
    var radius: Double = 8
    func body(content: Content) -> some View {
        content
            .blur(radius: active ? radius : 0)
            .animation(.easeOut(duration: 0.3), value: active)
            .opacity(active ? 0 : 1)
            .animation(.easeOut(duration: 0.18), value: active)
    }
}

extension AnyTransition {
    static var blurIn: AnyTransition {
        .modifier(active: BlurIn(active: true), identity: BlurIn(active: false))
    }
}

/// Keycap: small, round, quiet.
struct Keycap: View {
    let text: String
    var body: some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(Theme.ink3)
            .padding(.horizontal, 6)
            .frame(height: 20)
            .background(Theme.fill, in: Capsule())
    }
}

/// Round black button, the one strong action in a view.
struct InkButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Theme.button)
            .foregroundStyle(Theme.onStrong)
            .padding(.horizontal, 14)
            .frame(height: 32)
            .background(Theme.ink.opacity(configuration.isPressed ? 0.8 : 1), in: Capsule())
            .pressScale(configuration.isPressed)
    }
}

/// A press squeezes the whole button to 0.97 and lets go on the snappy spring. Not for anything that opens a
/// popup anchored to itself (the popup positions against the pressed size): answer those with colour only.
struct PressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.pressScale(configuration.isPressed)
    }
}

private struct PressScale: ViewModifier {
    let pressed: Bool
    @Environment(\.reducesMotion) private var reduceMotion
    func body(content: Content) -> some View {
        content.scaleEffect(pressed && !reduceMotion ? 0.97 : 1)
            .animation(Theme.anim(Theme.snappy, reduce: reduceMotion), value: pressed)
    }
}

extension View {
    func pressScale(_ pressed: Bool) -> some View { modifier(PressScale(pressed: pressed)) }
}

/// Quiet round button: a tint of ink, darker on hover.
struct GhostButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        GhostBody(configuration: configuration)
    }

    private struct GhostBody: View {
        let configuration: Configuration
        @State private var hover = false
        var body: some View {
            configuration.label
                .font(Theme.chip)
                .foregroundStyle(Theme.ink2)
                .padding(.horizontal, 12)
                .frame(height: 28)
                .background(hover || configuration.isPressed ? Theme.fillHover : Theme.fill, in: Capsule())
                .pressScale(configuration.isPressed)
                .onHover { hover = $0 }
        }
    }
}

/// The ink at one opacity on Retina and a darker one on 1x monitors (`displayScale` < 2).
struct DisplayInk: ShapeStyle {
    let retina: Double
    let oneX: Double

    func resolve(in environment: EnvironmentValues) -> Color {
        Theme.ink.opacity(environment.displayScale >= 2 ? retina : oneX)
    }
}
