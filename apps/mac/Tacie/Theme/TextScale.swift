import SwiftUI

/// The main window's text size, ⌘+ / ⌘- / ⌘0 and Settings → General: the meeting list, the meeting page and the
/// settings modal.
/// The island and the onboarding keep their sizes (their layouts are measured in points).
enum TextScale {
    static let key = "textScale"
    static let steps: [Double] = [0.85, 0.9, 1.0, 1.1, 1.2, 1.3, 1.45, 1.6]
    static let standard = 1.0

    /// The nearest step, so a hand-edited default never leaves the ladder.
    static func clamp(_ v: Double) -> Double {
        steps.min { abs($0 - v) < abs($1 - v) } ?? standard
    }

    static func larger(_ v: Double) -> Double { steps.first { $0 > clamp(v) + 0.001 } ?? steps.last! }
    static func smaller(_ v: Double) -> Double { steps.last { $0 < clamp(v) - 0.001 } ?? steps.first! }

    static func percent(_ v: Double) -> String { "\(Int((clamp(v) * 100).rounded()))%" }
}

private struct TextScaleKey: EnvironmentKey { static let defaultValue = 1.0 }

extension EnvironmentValues {
    var textScale: Double {
        get { self[TextScaleKey.self] }
        set { self[TextScaleKey.self] = newValue }
    }
}

/// `.system(size:)` times the window's text size.
private struct ScaledFont: ViewModifier {
    let size: CGFloat
    var weight: Font.Weight = .regular
    var design: Font.Design = .default
    var digits = false
    @Environment(\.textScale) private var scale

    func body(content: Content) -> some View {
        let f = Font.system(size: size * scale, weight: weight, design: design)
        content.font(digits ? f.monospacedDigit() : f)
    }
}

extension View {
    /// A font that follows the main window's text size (⌘+ / ⌘-).
    func uiFont(_ size: CGFloat, weight: Font.Weight = .regular, design: Font.Design = .default, digits: Bool = false) -> some View {
        modifier(ScaledFont(size: size, weight: weight, design: design, digits: digits))
    }
}

/// Negative tracking only on Retina: on 1x monitors glyphs land on whole pixels, so pulling them 0.4–0.6 pt
/// closer makes the gaps uneven and the title looks muddy.
private struct RetinaTracking: ViewModifier {
    let amount: CGFloat
    @Environment(\.displayScale) private var displayScale

    func body(content: Content) -> some View {
        content.tracking(displayScale >= 2 ? amount : 0)
    }
}

extension View {
    func retinaTracking(_ amount: CGFloat) -> some View {
        modifier(RetinaTracking(amount: amount))
    }
}
