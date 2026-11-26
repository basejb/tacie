import CoreGraphics
import Foundation

/// The one shape behind the logo, the app icon, the menu bar icon and the island's lights: a round head
/// (2026-10-25, the A1 render), 1.5:1, a broad dome on top and a flat base with soft corners. Two lights sit
/// on the widest line, at 0.3 and 0.7 of the width. Every ratio is of the head's height unless it says width.
enum BrandGeometry {
    static let aspect: Double = 1.5
    static let eyeLineRatio: Double = 0.58    // the widest line, measured from the top; the lights sit on it
    static let eyeXRatios: (left: Double, right: Double) = (0.3, 0.7)   // of the width
    static let eyeCoreRatio: Double = 0.07    // bright center
    static let eyeHaloRatio: Double = 0.23    // soft spread, fades to clear
    static let holeRatio: Double = 0.107      // punched eye in the filled template
    static let strokeRatio: Double = 0.055    // L4 outline
    static let ringRatio: Double = 0.15       // L4 glow ring
    static let clearRatio: Double = 0.28      // clear space around the mark

    static func width(forHeight h: Double) -> Double { h * aspect }

    /// Distance between the two lights, for drawing them without the head (the island).
    static func eyeSpacing(forHeight h: Double) -> Double { width(forHeight: h) * (eyeXRatios.right - eyeXRatios.left) }

    static func eyeCenters(in size: CGSize) -> (left: CGPoint, right: CGPoint) {
        let y = size.height * eyeLineRatio
        return (CGPoint(x: size.width * eyeXRatios.left, y: y), CGPoint(x: size.width * eyeXRatios.right, y: y))
    }

    static func coreRadius(height h: Double) -> Double { h * eyeCoreRatio }
    static func haloRadius(height h: Double) -> Double { h * eyeHaloRatio }
    static func holeRadius(height h: Double) -> Double { h * holeRatio }

    // The outline, fitted to the A1 render (design notes). In a unit box
    // (x = half-widths from the center, y = heights from the top) each side is two cubic curves:
    //   top:    (0,0) → (1, 0.58)  handles (0.88, 0) and (1, 0.58·(1−0.34))
    //   bottom: (1, 0.58) → (0.44, 1)  handles (1, 0.58+0.46·0.42) and (0.88, 1); the base is flat between ±0.44.
    static let topHandleX: Double = 0.88
    static let topHandleY: Double = 0.34
    static let bottomHandleY: Double = 0.46
    static let baseHalfWidth: Double = 0.44
    static let bottomHandleX: Double = 0.44

    /// The head's outline in `rect`, top of the dome at the top edge.
    static func outline(in rect: CGRect) -> CGPath {
        let cx = rect.midX, hw = rect.width / 2, h = rect.height, top = rect.minY
        func pt(_ x: Double, _ y: Double) -> CGPoint { CGPoint(x: cx + hw * x, y: top + h * y) }
        let yc = eyeLineRatio
        let path = CGMutablePath()
        path.move(to: pt(0, 0))
        path.addCurve(to: pt(1, yc), control1: pt(topHandleX, 0), control2: pt(1, yc * (1 - topHandleY)))
        path.addCurve(to: pt(baseHalfWidth, 1), control1: pt(1, yc + bottomHandleY * (1 - yc)), control2: pt(baseHalfWidth + bottomHandleX, 1))
        path.addLine(to: pt(-baseHalfWidth, 1))
        path.addCurve(to: pt(-1, yc), control1: pt(-baseHalfWidth - bottomHandleX, 1), control2: pt(-1, yc + bottomHandleY * (1 - yc)))
        path.addCurve(to: pt(0, 0), control1: pt(-1, yc * (1 - topHandleY)), control2: pt(-topHandleX, 0))
        path.closeSubpath()
        return path
    }
}
