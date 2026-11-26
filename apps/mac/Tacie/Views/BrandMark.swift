import SwiftUI

/// The still logo: the same round head and lights as the character, for places that don't listen (main window,
/// avatars, app icon, exports). Three versions per docs/03: L1 in color, L4 one-color, and the filled
/// template for anything under 26px.
struct BrandMark: View {
    enum Style: Equatable {
        case l1(dark: Bool)   // frosted cream head with the two lights
        case l4               // outline + dots + glow rings, in the current foreground color
        case template         // filled head with the eyes punched out, for menu bars and favicons
    }

    let style: Style
    var height: CGFloat

    private var width: CGFloat { CGFloat(BrandGeometry.width(forHeight: Double(height))) }
    private var h: Double { Double(height) }

    var body: some View {
        Group {
            switch style {
            case .l1(let dark): l1(dark: dark)
            case .l4: l4
            case .template: template
            }
        }
        .frame(width: width, height: height)
        .accessibilityLabel("Tacie")
    }

    // MARK: L1

    private func l1(dark: Bool) -> some View {
        ZStack {
            BrandOutline().fill(LinearGradient(colors: dark ? [Color(hex: 0xF3EEE4), Color(hex: 0xD9D1C0)]
                                                            : [Color(hex: 0xFBF8F1), Color(hex: 0xEEE7D9)],
                                               startPoint: .top, endPoint: .bottom))
            if !dark { BrandOutline().strokeBorder(Color(hex: 0xE2DACB), lineWidth: max(1, height * 0.013)) }
            EyeLights(channels: .rest, tint: .warm, height: height)
                .position(x: width / 2, y: height * BrandGeometry.eyeLineRatio)
        }
    }

    // MARK: L4

    private var l4: some View {
        Canvas { gc, size in
            let stroke = h * BrandGeometry.strokeRatio
            let dot = h * 0.075
            let ring = h * BrandGeometry.ringRatio
            let box = CGRect(origin: .zero, size: size).insetBy(dx: stroke / 2, dy: stroke / 2)
            gc.stroke(Path(BrandGeometry.outline(in: box)), with: .foreground, lineWidth: stroke)
            let centers = BrandGeometry.eyeCenters(in: size)
            for c in [centers.left, centers.right] {
                gc.fill(Path(ellipseIn: CGRect(x: c.x - dot, y: c.y - dot, width: dot * 2, height: dot * 2)), with: .foreground)
                var faint = gc
                faint.opacity = 0.28
                faint.stroke(Path(ellipseIn: CGRect(x: c.x - ring, y: c.y - ring, width: ring * 2, height: ring * 2)),
                             with: .foreground, lineWidth: h * 0.04)
            }
        }
    }

    // MARK: template

    private var template: some View {
        Canvas { gc, size in
            let hole = BrandGeometry.holeRadius(height: h)
            var body = Path(BrandGeometry.outline(in: CGRect(origin: .zero, size: size)))
            let centers = BrandGeometry.eyeCenters(in: size)
            for c in [centers.left, centers.right] {
                body.addEllipse(in: CGRect(x: c.x - hole, y: c.y - hole, width: hole * 2, height: hole * 2))
            }
            gc.fill(body, with: .foreground, style: FillStyle(eoFill: true))
        }
    }
}
