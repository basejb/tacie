import SwiftUI

/// The Settings → Meetings scene for the "Speech recognition" row (2026-11-18): where meeting sound goes, drawn like
/// the landing's sources beam. Meeting audio on the left, Tacie on this Mac in the middle, the speech recognition
/// server on the right. A light (the eye glow) runs from the sound to Tacie, and with Server on to the server and back;
/// then a line of the transcript shows under Tacie. With On-device the light stops at Tacie, the line to the server is
/// dashed and the server card is empty. Decorative: nothing in it is pressed. Reduced motion: no light, the transcript
/// line stays.
struct STTPathScene: View {
    /// The place on the right: the speech recognition server here, OpenRouter on the open edition's AI page.
    struct Far {
        let title: String
        let note: String
        let symbol: String
        /// The note when nothing goes there.
        var offNote = String(localized: "보내지 않아요")
        static var stt: Far { Far(title: String(localized: "음성 인식 서버"), note: String(localized: "음성 인식 업체"), symbol: "server.rack") }
    }

    let onDevice: Bool
    var far: Far = .stt
    /// The transcript line under Tacie; off when nothing is transcribed yet (the AI page without a key).
    var showsSaid = true
    @Environment(\.reducesMotion) private var reduceMotion

    private static let cycle: Double = 3.6

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30, paused: reduceMotion)) { ctx in
            let f = reduceMotion ? -1 : ctx.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: Self.cycle) / Self.cycle
            VStack(spacing: 0) {
                HStack(spacing: 0) {
                    soundNode(lit: f >= 0 && f < 0.08)
                    Wire(light: leg(f, 0, 0.3))
                    face
                    Wire(light: onDevice ? nil : leg(f, 0.3, 0.52) ?? leg(f, 0.55, 0.74).map { 1 - $0 }, dashed: onDevice)
                    serverNode(lit: !onDevice && f > 0.5 && f < 0.58)
                }
                .padding(.horizontal, 26)
                said(shown: showsSaid && (f < 0 || (onDevice ? (f > 0.46 && f < 0.94) : (f > 0.76 && f < 0.97))))
                    .padding(.top, 30)
            }
            .padding(.top, 44)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        }
    }

    /// Where the light is on a leg that runs from `start` to `end` of the cycle (0...1), nil when it is not on it.
    private func leg(_ f: Double, _ start: Double, _ end: Double) -> Double? {
        guard f >= start, f <= end else { return nil }
        return (f - start) / (end - start)
    }

    private var face: some View {
        LitHead(channels: EyeRig.target(for: .listening, remote: false), height: 38)
            .overlay(alignment: .bottom) {
                HStack(spacing: 4) {
                    Image(systemName: "laptopcomputer").uiFont(9.5, weight: .semibold)
                    Text("이 Mac").uiFont(11, weight: .semibold)
                }
                .foregroundStyle(Theme.ink2)
                .fixedSize()
                .offset(y: 19)
            }
            .padding(.horizontal, 4)
    }

    private func soundNode(lit: Bool) -> some View {
        PathNode(title: String(localized: "회의 소리"), note: String(localized: "마이크 · 회의 앱"), lit: lit, off: false, trailing: false) {
            SceneLevel(phase: 1)
        }
    }

    private func serverNode(lit: Bool) -> some View {
        PathNode(title: far.title,
                 note: onDevice ? far.offNote : far.note,
                 lit: lit, off: onDevice, trailing: true) {
            Image(systemName: far.symbol).uiFont(12, weight: .semibold)
                .foregroundStyle(onDevice ? AnyShapeStyle(Theme.ink4) : AnyShapeStyle(Theme.ink))
        }
    }

    private func said(shown: Bool) -> some View {
        HStack(spacing: 6) {
            Text("화자 2").uiFont(11, weight: .semibold).foregroundStyle(Theme.ink3)
            Text("재시도는 세 번으로 두죠").uiFont(11.5).foregroundStyle(Theme.ink)
        }
        .padding(.horizontal, 10).frame(height: 24)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.raised))
        .shadow(color: .black.opacity(0.06), radius: 5, y: 2)
        .opacity(shown ? 1 : 0)
        .offset(y: shown ? 0 : 3)
        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: shown)
    }
}

/// One place on the path: an icon tile and two lines of text on a raised card; `off` leaves only its outline.
private struct PathNode<Icon: View>: View {
    let title: String
    let note: String
    let lit: Bool
    let off: Bool
    let trailing: Bool
    @ViewBuilder var icon: Icon

    var body: some View {
        HStack(spacing: 9) {
            if !trailing { tile }
            VStack(alignment: trailing ? .trailing : .leading, spacing: 1) {
                Text(title).uiFont(12.5, weight: .semibold).foregroundStyle(off ? AnyShapeStyle(Theme.ink3) : AnyShapeStyle(Theme.ink))
                Text(note).uiFont(11).foregroundStyle(Theme.ink3)
            }
            .fixedSize()
            if trailing { tile }
        }
        .padding(.leading, trailing ? 11 : 6).padding(.trailing, trailing ? 6 : 11).padding(.vertical, 6)
        .background {
            let shape = RoundedRectangle(cornerRadius: 13, style: .continuous)
            if off {
                shape.strokeBorder(Theme.ink.opacity(0.14))
            } else {
                shape.fill(Theme.raised)
                    .shadow(color: .black.opacity(0.05), radius: 1, y: 1)
                    .shadow(color: .black.opacity(0.05), radius: 8, y: 4)
            }
        }
        .overlay {
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .strokeBorder(Theme.eyeGlow.opacity(lit ? 0.6 : 0), lineWidth: 2)
                .animation(.easeOut(duration: 0.25), value: lit)
        }
    }

    private var tile: some View {
        icon
            .frame(width: 28, height: 28)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(off ? Color.clear : Theme.fill))
    }
}

/// The line between two places, with the light on it at `light` (0 = left end, 1 = right end).
/// A straight line with the eye glow running along it (`light` 0...1), shared by the path scenes.
struct Wire: View {
    let light: Double?
    var dashed = false

    var body: some View {
        Canvas { ctx, size in
            let y = size.height / 2
            var base = Path()
            base.move(to: CGPoint(x: 4, y: y))
            base.addLine(to: CGPoint(x: size.width - 4, y: y))
            ctx.stroke(base, with: .color(Theme.ink.opacity(dashed ? 0.1 : 0.14)),
                       style: StrokeStyle(lineWidth: 1.2, dash: dashed ? [3, 4] : []))
            guard let light else { return }
            let seg: CGFloat = 22
            let head = 4 + (size.width - 8 + seg) * light
            var p = Path()
            p.move(to: CGPoint(x: max(4, head - seg), y: y))
            p.addLine(to: CGPoint(x: min(size.width - 4, head), y: y))
            ctx.stroke(p, with: .color(Theme.eyeGlow.opacity(0.28)), style: StrokeStyle(lineWidth: 8, lineCap: .round))
            ctx.stroke(p, with: .color(Theme.eyeGlow), style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
        }
        .frame(minWidth: 24, maxWidth: .infinity)
        .frame(height: 12)
    }
}
