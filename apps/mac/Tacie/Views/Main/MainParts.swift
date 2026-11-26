import SwiftUI

/// Small pieces of the main window, in the island's ink (values from the approved mock, design notes).
struct StrongButton: View {
    let label: String
    var icon: String? = nil
    /// A drop-down mark after the label: the button opens a choice instead of acting at once.
    var chevron = false
    var body: some View {
        HStack(spacing: 5) {
            if let icon { Image(systemName: icon).uiFont(10.5, weight: .bold) }
            Text(label).uiFont(12.5, weight: .semibold)
            if chevron { Image(systemName: "chevron.down").uiFont(8.5, weight: .bold).opacity(0.6) }
        }
        .foregroundStyle(Theme.onStrong)
        .padding(.horizontal, 13)
        .frame(height: 28)
        .background(Theme.ink, in: Capsule())
        .contentShape(Capsule())
    }
}

struct OutlineButton: View {
    let label: String
    var icon: String? = nil
    var tint: Color = Theme.ink
    var body: some View {
        HStack(spacing: 5) {
            if let icon { Image(systemName: icon).uiFont(11, weight: .semibold).contentTransition(.symbolEffect(.replace)) }
            if !label.isEmpty { Text(label).uiFont(12.5, weight: .medium) }
        }
        .foregroundStyle(tint)
        .padding(.horizontal, label.isEmpty ? 9 : 12)
        .frame(height: 28)
        .overlay(Capsule().stroke(Theme.ink.opacity(0.14)))
        .contentShape(Capsule())
    }
}

struct IconButton: View {
    let symbol: String
    var body: some View {
        Image(systemName: symbol).uiFont(13, weight: .medium).foregroundStyle(Theme.ink2)
            .frame(width: 28, height: 28)
            .contentShape(Rectangle())
    }
}

/// The AI 기본법 mark on what the model wrote: one per page, on the date line.
struct AITag: View {
    var body: some View {
        HStack(spacing: 3) {
            Image(systemName: "sparkles").uiFont(9, weight: .semibold)
            Text("AI 작성").uiFont(11, weight: .semibold)
        }
        .foregroundStyle(Theme.ink3)
        .padding(.horizontal, 7)
        .frame(height: 19)
        .background(Theme.fill, in: Capsule())
    }
}

struct LiveDot: View {
    /// Red while something is wrong (the pill's dot does the same).
    var warn = false
    var body: some View { Circle().fill(warn ? Theme.warn : Theme.live).frame(width: 7, height: 7) }
}

/// Tacie's round avatar. Small ones get the filled template (docs/03: under 26px), big ones the color logo.
struct Avatar: View {
    var size: CGFloat = 26
    var body: some View {
        ZStack {
            Circle().fill(Theme.paper)
            Circle().stroke(Theme.ink.opacity(0.1))
            if size >= 40 {
                BrandMark(style: .l1(dark: false), height: size * 0.42)
            } else {
                BrandMark(style: .template, height: size * 0.48).foregroundStyle(Theme.ink)
            }
        }
        .frame(width: size, height: size)
    }
}

/// A section title on the meeting page, with an optional count and something on the right.
struct DocHeading<Trailing: View>: View {
    let title: String
    var count: Int? = nil
    /// The title drawn by the page instead (a meeting page's selectable text, `MeetingPage.heading`).
    var titleView: AnyView? = nil
    @ViewBuilder var trailing: Trailing
    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            if let titleView {
                titleView
            } else {
                Text(title).uiFont(17, weight: .semibold)
            }
            if let count { Text(verbatim: "\(count)").uiFont(17, weight: .semibold).foregroundStyle(Theme.ink4).monospacedDigit() }
            Spacer()
            trailing
        }
        .padding(.bottom, 12)
    }
}

extension DocHeading where Trailing == EmptyView {
    init(title: String, count: Int? = nil, titleView: AnyView? = nil) {
        self.init(title: title, count: count, titleView: titleView) { EmptyView() }
    }
}

struct Bullet: View {
    let text: String
    /// The words drawn by the page instead (a meeting page's selectable text, `MeetingPage.bullet`).
    var words: AnyView? = nil
    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Circle().fill(Theme.ink.opacity(0.55)).frame(width: 4.5, height: 4.5).offset(y: -3.5)
            if let words {
                words
            } else {
                Text(text).uiFont(15).lineSpacing(4).fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
            }
        }
    }
}

/// A file a found answer stands on: its number in the answer line, what kind it is, its name.
struct FileChip: View {
    let n: Int
    let card: Card
    var body: some View {
        HStack(spacing: 5) {
            Text(verbatim: "\(n)").uiFont(10.5, weight: .semibold).foregroundStyle(Theme.ink3)
            SourceMark(card: card, size: 11)
            Text(card.name).uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink2).lineLimit(1)
        }
        .padding(.horizontal, 10)
        .frame(height: 26)
        .background(Theme.fill, in: Capsule())
        .contentShape(Capsule())
    }
}

/// A grey bar where text is still being written. Bars pulse one beat after the one above (`pulseDelay`).
struct Skeleton: View {
    let width: CGFloat
    var pulseDelay: Double = 0
    @State private var on = false
    @Environment(\.reducesMotion) private var reduceMotion
    var body: some View {
        Capsule().fill(Theme.ink.opacity(on ? 0.09 : 0.045)).frame(maxWidth: width).frame(height: 11)
            .onAppear {
                guard !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true).delay(pulseDelay)) { on = true }
            }
    }
}

/// A status line with a band of light passing over it while something is being written (Arc text-shimmer).
/// Runs only while on screen; 움직임 줄이기 shows plain grey text.
struct ShimmerText: View {
    let text: String
    var period: Double = 1.6
    var size: CGFloat = 12.5
    var weight: Font.Weight = .regular
    var ink: AnyShapeStyle = AnyShapeStyle(Theme.ink3)
    @Environment(\.reducesMotion) private var reduceMotion

    var body: some View {
        let base = Text(text).uiFont(size, weight: weight).foregroundStyle(ink)
            .lineLimit(2).lineSpacing(1.5).fixedSize(horizontal: false, vertical: true)
        if reduceMotion {
            base
        } else {
            TimelineView(.animation) { ctx in
                base.overlay {
                    GeometryReader { g in
                        let band = g.size.width * 0.35
                        let x = Shimmer.bandOffset(phase: Shimmer.phase(at: ctx.date.timeIntervalSinceReferenceDate, period: period),
                                                   width: g.size.width, band: band)
                        Text(text).uiFont(size, weight: weight).foregroundStyle(Theme.ink)
                            .lineLimit(2).lineSpacing(1.5).fixedSize(horizontal: false, vertical: true)
                            // leading-anchored, so the offset math above holds (the default mask alignment is centre)
                            .mask(alignment: .leading) {
                                LinearGradient(stops: [.init(color: .clear, location: 0), .init(color: .black, location: 0.5), .init(color: .clear, location: 1)],
                                               startPoint: .leading, endPoint: .trailing)
                                    .frame(width: band).offset(x: x)
                            }
                    }
                }
            }
        }
    }
}
