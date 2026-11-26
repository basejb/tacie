import SwiftUI

/// A row's done state: the check and its words, then a beat later the words slip away and only the check stays.
/// The check draws itself (a ring closes, fills green, the tick is drawn in), so a wait shown as `SyncRing` in the
/// same place reads as closing into its mark.
struct DoneMark: View {
    let text: String
    @State private var settled = false
    @Environment(\.reducesMotion) private var reduce

    var body: some View {
        HStack(spacing: 5) {
            if !settled {
                Text(text).font(.system(size: 12.5, weight: .medium)).foregroundStyle(Theme.ink2)
                    .transition(.opacity.combined(with: .offset(x: 6)))
            }
            DrawnCheck(size: settled ? 18 : 12)
        }
        .frame(height: 30)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(text)
        .task {
            try? await Task.sleep(for: .seconds(1.4))
            if reduce { settled = true } else { withAnimation(.spring(duration: 0.45, bounce: 0.3)) { settled = true } }
        }
    }
}

/// The green check, drawn in on appear: the ring closes, the disc fills, then the tick. With 움직임 줄이기 it is just there.
struct DrawnCheck: View {
    let size: CGFloat
    @State private var ring: CGFloat = 0
    @State private var filled = false
    @State private var tick: CGFloat = 0
    @Environment(\.reducesMotion) private var reduce

    var body: some View {
        ZStack {
            Circle().trim(from: 0, to: ring)
                .stroke(Theme.live, style: StrokeStyle(lineWidth: max(1.2, size * 0.12), lineCap: .round))
                .rotationEffect(.degrees(-90))
                .padding(size * 0.06)
            Circle().fill(Theme.live).scaleEffect(filled ? 1 : 0.001)
            CheckTick().trim(from: 0, to: tick)
                .stroke(.white, style: StrokeStyle(lineWidth: max(1.3, size * 0.13), lineCap: .round, lineJoin: .round))
        }
        .frame(width: size, height: size)
        .task {
            if reduce { ring = 1; filled = true; tick = 1; return }
            withAnimation(.easeOut(duration: 0.22)) { ring = 1 }
            try? await Task.sleep(for: .seconds(0.18))
            withAnimation(Theme.grow) { filled = true }
            try? await Task.sleep(for: .seconds(0.1))
            withAnimation(.easeOut(duration: 0.22)) { tick = 1 }
        }
    }
}

/// The tick inside `DrawnCheck`, in the proportions of SF Symbols' checkmark.circle.fill.
private struct CheckTick: Shape {
    func path(in r: CGRect) -> Path {
        var p = Path()
        p.move(to: CGPoint(x: r.minX + r.width * 0.29, y: r.minY + r.height * 0.52))
        p.addLine(to: CGPoint(x: r.minX + r.width * 0.43, y: r.minY + r.height * 0.66))
        p.addLine(to: CGPoint(x: r.minX + r.width * 0.71, y: r.minY + r.height * 0.36))
        return p
    }
}

/// A first sync's wait, where `DoneMark` will come: a small grey ring that fills with the percent in the status
/// line, or a short arc going round when the line has none. 움직임 줄이기 keeps the arc still.
struct SyncRing: View {
    let fraction: Double?
    @Environment(\.reducesMotion) private var reduce

    /// The percent in a status line such as "노션 페이지 받는 중 40%".
    static func fraction(in line: String) -> Double? {
        guard let m = line.firstMatch(of: /(\d{1,3})\s*%/), let n = Double(m.1) else { return nil }
        return min(1, max(0, n / 100))
    }

    var body: some View {
        let size: CGFloat = 12
        let style = StrokeStyle(lineWidth: 1.8, lineCap: .round)
        ZStack {
            Circle().stroke(Theme.hairline, lineWidth: 1.8)
            if let fraction {
                Circle().trim(from: 0, to: max(0.04, fraction)).stroke(Theme.ink3, style: style)
                    .rotationEffect(.degrees(-90))
                    .animation(.easeOut(duration: 0.3), value: fraction)
            } else if reduce {
                Circle().trim(from: 0, to: 0.25).stroke(Theme.ink3, style: style).rotationEffect(.degrees(-90))
            } else {
                TimelineView(.animation) { ctx in
                    let turn = ctx.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 1.1) / 1.1
                    Circle().trim(from: 0, to: 0.25).stroke(Theme.ink3, style: style).rotationEffect(.degrees(turn * 360))
                }
            }
        }
        .padding(1)
        .frame(width: size, height: size)
        .frame(height: 30)
        .accessibilityHidden(true)
    }
}
