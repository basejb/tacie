import SwiftUI

/// The frame every settings scene sits in (2026-11-15): a light inner box under the page title with a drawn piece of
/// Tacie inside, one line at the bottom saying what the scene shows, and an optional mark in the top-left corner. Same
/// box as the scenes in What's new (docs/29 2절 item 5). Nothing in a scene is pressed.
struct SettingsSceneFrame<Content: View, Corner: View>: View {
    let caption: String
    @ViewBuilder var content: Content
    @ViewBuilder var corner: Corner
    /// A shorter frame without the caption line, where the words next to it already say the same (the onboarding).
    var short = false

    static var height: CGFloat { 196 }
    static var shortHeight: CGFloat { 164 }

    var body: some View {
        ZStack(alignment: .topLeading) {
            content
            corner.padding(.leading, 14).padding(.top, 12)
            if !short {
                Text(caption).uiFont(12).foregroundStyle(Theme.ink3).lineLimit(1)
                    .padding(.horizontal, 14).padding(.bottom, 11)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
                    .id(caption)
                    .transition(.opacity)
            }
        }
        .frame(height: short ? Self.shortHeight : Self.height)
        .frame(maxWidth: .infinity)
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous))
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
    }
}

extension SettingsSceneFrame where Corner == EmptyView {
    init(caption: String, short: Bool = false, @ViewBuilder content: () -> Content) {
        self.init(caption: caption, content: content, corner: { EmptyView() }, short: short)
    }
}

/// 화면 위 Tacie as drawn in a scene: the living face on the left, then whatever the state shows.
struct ScenePill<Content: View>: View {
    var mood: EyeMood = .listening
    @ViewBuilder var content: Content
    var body: some View {
        HStack(spacing: 8) {
            CueEyes(mood: mood, remote: false, height: 19)
            content
        }
        .padding(.leading, 9).padding(.trailing, 13).frame(height: 36)
        .background(Capsule().fill(Theme.raised))
        .overlay(Capsule().strokeBorder(Theme.hairline))
        .shadow(color: .black.opacity(0.08), radius: 8, y: 3)
    }
}

/// A card under the pill (답 카드, 회의 정리).
struct SceneCard<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 3) { content }
            .padding(.horizontal, 14).padding(.vertical, 10)
            .frame(width: 300, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.raised))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.hairline))
            .shadow(color: .black.opacity(0.07), radius: 8, y: 3)
    }
}

/// One line under the pill (녹음 알림, 음악 멈추기).
struct SceneLine<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        HStack(spacing: 7) { content }
            .padding(.horizontal, 12).frame(height: 30)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Theme.raised))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.hairline))
            .shadow(color: .black.opacity(0.06), radius: 6, y: 2)
    }
}

/// A small chip: a source with its logo, or a count.
struct SceneChip: View {
    var logo: String? = nil
    let label: String
    var body: some View {
        HStack(spacing: 4) {
            if let logo { Image(logo).resizable().scaledToFit().frame(width: 11, height: 11) }
            Text(label).uiFont(11, weight: .medium).foregroundStyle(Theme.ink2)
        }
        .padding(.horizontal, 7).frame(height: 20)
        .background(Capsule().fill(Theme.fill))
        .padding(.top, 3)
    }
}

/// A line of 대화 기록: the speaker, the words, and what a setting adds (a translation under it, a play mark, a name).
struct SceneTranscript: View {
    let who: String
    let text: String
    var under: String? = nil
    var play = false
    var renamed: String? = nil
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 5) {
                if let renamed {
                    Text(who).strikethrough().foregroundStyle(Theme.ink3)
                    Image(systemName: "arrow.right").uiFont(8.5, weight: .bold).foregroundStyle(Theme.ink3)
                    Text(renamed).foregroundStyle(Theme.speaker(1))
                } else {
                    Text(who).foregroundStyle(Theme.speaker(1))
                }
                if play { Image(systemName: "play.fill").uiFont(8.5).foregroundStyle(Theme.ink) }
            }
            .uiFont(11, weight: .semibold)
            Text(text).uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink)
            if let under { Text(under).uiFont(12).foregroundStyle(Theme.ink2) }
        }
        .frame(width: 330, alignment: .leading)
        .padding(.top, 4)
    }
}

/// The main window, small, open on a meeting.
struct SceneWindow: View {
    let title: String
    let meta: String
    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 4) { ForEach(0..<3, id: \.self) { _ in Circle().fill(Theme.ink4).frame(width: 6, height: 6) } }
            HStack(spacing: 5) {
                Text(title).uiFont(12, weight: .semibold).foregroundStyle(Theme.ink)
                Text("· \(meta)").uiFont(12).foregroundStyle(Theme.ink3)
            }
            Capsule().fill(Theme.fill).frame(width: 220, height: 5)
            Capsule().fill(Theme.fill).frame(width: 160, height: 5)
        }
        .padding(10)
        .frame(width: 300, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Theme.paper))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.hairline))
    }
}

/// A short list of meetings, name and day.
struct SceneList: View {
    let rows: [(String, String)]
    var body: some View {
        VStack(spacing: 0) {
            ForEach(rows.indices, id: \.self) { i in
                HStack {
                    Text(rows[i].0).uiFont(12, weight: .medium).foregroundStyle(Theme.ink)
                    Spacer()
                    Text(rows[i].1).uiFont(11.5).foregroundStyle(Theme.ink3)
                }
                .frame(height: 24)
                if i < rows.count - 1 { Rectangle().fill(Theme.hairline).frame(height: 1) }
            }
        }
        .frame(width: 300)
    }
}

/// Three bars moving like a sound level; still with 움직임 줄이기.
struct SceneLevel: View {
    var phase: Int = 0
    @Environment(\.reducesMotion) private var reduceMotion
    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 20, paused: reduceMotion)) { ctx in
            let t = ctx.date.timeIntervalSinceReferenceDate
            HStack(alignment: .bottom, spacing: 2) {
                ForEach(0..<3, id: \.self) { i in
                    let v = reduceMotion ? 0.6 : 0.5 + 0.5 * sin(t * 6 + Double(i * 2 + phase))
                    Capsule().fill(Theme.ink).frame(width: 3, height: 3 + 10 * v)
                }
            }
            .frame(height: 13, alignment: .bottom)
        }
    }
}

/// A step number in a ring; filled when its step is the one the scene shows.
struct StepDot: View {
    let n: Int
    let on: Bool
    var size: CGFloat = 18
    var body: some View {
        Text("\(n)").uiFont(size * 0.6, weight: .semibold, digits: true)
            .foregroundStyle(on ? AnyShapeStyle(Theme.onStrong) : AnyShapeStyle(Theme.ink2))
            .frame(width: size, height: size)
            .background(Circle().fill(on ? Theme.ink : .clear))
            .overlay(Circle().strokeBorder(on ? Theme.ink : Theme.ink4))
    }
}
