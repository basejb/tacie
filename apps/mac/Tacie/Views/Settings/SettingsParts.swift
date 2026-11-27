import SwiftUI

/// The settings list (here and not in SettingsView.swift: Search settings and its tests use it).
enum SettingsPane: String, CaseIterable, Identifiable {
    case account, general, meeting, ai, connections, privacy, developer
    var id: String { rawValue }
    var title: String {
        switch self {
        case .account: String(localized: "계정")
        case .general: String(localized: "일반")
        case .meeting: String(localized: "settings.meeting", defaultValue: "회의")
        case .ai: "AI"
        case .connections: String(localized: "settings.connections", defaultValue: "연결")
        case .privacy: String(localized: "개인정보")
        case .developer: String(localized: "개발자")
        }
    }
    var icon: String {
        switch self {
        case .account: "person.crop.circle"
        case .general: "gearshape"
        case .meeting: "mic"
        case .ai: "sparkles"
        case .connections: "arrow.up.right.square"
        case .privacy: "lock"
        case .developer: "hammer"
        }
    }
    /// The panes in the sidebar; 개발자 comes last and only in a development build. AI shows only while the app uses its
    /// own API keys (a development build that is not signed in): with Tacie AI there is nothing to choose there, and a
    /// problem shows on 계정.
    @MainActor static var main: [SettingsPane] { panes(hasAccount: Build.hasAccount) }
    /// The pane actually shown: ⌘, and the settings buttons ask for 계정, which the open edition does not have.
    static func resolve(_ pane: SettingsPane, open: Bool = Build.isOpen) -> SettingsPane {
        open && pane == .account ? .general : pane
    }
    /// AI is always there: it is where Tacie AI, a key or this Mac is chosen (#56). A source build has no 계정.
    static func panes(hasAccount: Bool) -> [SettingsPane] {
        let all: [SettingsPane] = [.account, .general, .meeting, .ai, .connections, .privacy]
        return hasAccount ? all : all.filter { $0 != .account }
    }
}

/// Pieces of the settings pages (values from the approved mock, design notes).
enum SettingsStyle {
    static let sidebar = Theme.ink.opacity(0.03)
    static let selected = Theme.ink.opacity(0.065)
    static let pageTitle = Font.system(size: 22, weight: .semibold)
    static let pageDesc = Font.system(size: 13.5)
    static let section = Font.system(size: 14, weight: .semibold)
    static let rowTitle = Font.system(size: 13.5, weight: .medium)
    static let rowDesc = Font.system(size: 12.5)
    static let control = Font.system(size: 12.5, weight: .medium)
}

struct PageHeader: View {
    let title: String
    var desc: String? = nil
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).uiFont(22, weight: .semibold).foregroundStyle(Theme.ink)
            if let desc {
                Text(desc).uiFont(13.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.bottom, 30)
    }
}

struct SettingsSection<Content: View>: View {
    let title: String
    var desc: String? = nil
    /// 회의's steps: the number before the title, filled while the page's scene shows this step.
    var step: Int? = nil
    var lit = false
    @ViewBuilder var content: Content
    @Environment(\.reducesMotion) private var reduceMotion
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    if let step {
                        StepDot(n: step, on: lit)
                            .animation(Theme.anim(Theme.fast, reduce: reduceMotion), value: lit)
                    }
                    Text(title).uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                }
                if let desc {
                    Text(desc).uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.bottom, 10)
            Rectangle().fill(Theme.hairline).frame(height: 1).padding(.bottom, 4)
            content
        }
        .padding(.bottom, 34)
    }
}

struct SettingsRow<Trailing: View>: View {
    let title: String
    var desc: String? = nil
    var titleColor: Color = Theme.ink
    var descColor = AnyShapeStyle(Theme.ink3)
    /// A thin progress bar under the description (a download in progress).
    var bar: Double? = nil
    @ViewBuilder var trailing: Trailing
    var body: some View {
        HStack(alignment: .center, spacing: 24) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title).uiFont(13.5, weight: .medium).foregroundStyle(titleColor)
                if let desc {
                    Text(desc).uiFont(12.5).foregroundStyle(descColor).fixedSize(horizontal: false, vertical: true)
                }
                if let bar { SyncBar(fraction: bar).padding(.top, 3) }
            }
            Spacer(minLength: 0)
            trailing
        }
        .padding(.vertical, 11)
        // 설정 검색 scrolls to a row by its title and marks it for a moment
        .modifier(SettingsTargetMark(title: title))
        .id(title)
    }
}

struct InkToggle: View {
    @Binding var on: Bool
    @Environment(\.isEnabled) private var enabled
    @Environment(\.reducesMotion) private var reduceMotion
    var body: some View {
        ZStack(alignment: on ? .trailing : .leading) {
            Capsule().fill(on ? Theme.ink : Theme.ink.opacity(0.14))
            Circle().fill(on ? Theme.knob : Theme.knobOff).frame(width: 14, height: 14).padding(2)
                .shadow(color: Theme.knobShadow, radius: 1, y: 0.5)
        }
        .frame(width: 30, height: 18)
        .opacity(enabled ? 1 : 0.4)
        .contentShape(Capsule())
        .onTapGesture { if enabled { withAnimation(Theme.anim(Theme.snappy, reduce: reduceMotion)) { on.toggle() } } }
    }
}

/// The look of a text drop-down; wrap it as a Menu label.
struct MenuText: View {
    let label: String
    var body: some View {
        HStack(spacing: 5) {
            Text(label).uiFont(12.5, weight: .medium)
            Image(systemName: "chevron.down").uiFont(8.5, weight: .bold)
        }
        .foregroundStyle(Theme.ink2)
        .padding(.horizontal, 9)
        .frame(height: 26)
        .background(Theme.ink.opacity(0.045), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
    }
}

/// A drop-down of fixed choices in the house look (`MenuText`). It hugs its label and sits at the row's trailing edge,
/// so the menus of a page line up on the right; a system Picker in a fixed frame drew at its own width, centred.
/// `onChange` runs after the person picks a different item.
struct ChoiceMenu<T: Hashable>: View {
    @Binding var value: T
    let items: [(T, String)]
    var onChange: () -> Void = {}
    var body: some View {
        Menu {
            ForEach(items, id: \.0) { item in
                Button(item.1) {
                    guard value != item.0 else { return }
                    value = item.0
                    onChange()
                }
            }
        } label: {
            MenuText(label: items.first { $0.0 == value }?.1 ?? "")
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
    }
}

struct Chevron: View {
    var body: some View { Image(systemName: "chevron.right").uiFont(11, weight: .semibold).foregroundStyle(Theme.ink4) }
}

struct SoonTag: View {
    var text = String(localized: "settings.soonTag", defaultValue: "준비 중")
    var body: some View {
        Text(text).uiFont(11, weight: .semibold).foregroundStyle(Theme.ink3)
            .padding(.horizontal, 7).frame(height: 19)
            .background(Theme.ink.opacity(0.05), in: Capsule())
    }
}

struct Meter: View {
    let value: Double
    var tint: Color = Theme.ink
    var body: some View {
        GeometryReader { g in
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.ink.opacity(0.08))
                Capsule().fill(tint).frame(width: g.size.width * min(max(value, 0), 1))
            }
        }
        .frame(height: 6)
    }
}

struct LogoTile: View {
    var symbol: String? = nil
    /// A service's own mark from the asset catalog (official file, drawn as is: never recoloured or reshaped).
    var logo: String? = nil
    var size: CGFloat = 34
    var body: some View {
        Group {
            if let logo {
                Image(logo).resizable().scaledToFit().frame(width: size * 0.52, height: size * 0.52)
            } else if let symbol {
                Image(systemName: symbol)
                    .font(.system(size: size * 0.42, weight: .medium))
                    .foregroundStyle(Theme.ink)
            }
        }
        .frame(width: size, height: size)
        .background(logo != nil ? Theme.logoTile : Theme.paper, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).stroke(Theme.ink.opacity(0.1)))
    }
}

/// The back row of a page inside the modal: "‹ 일반".
struct DetailHeader: View {
    let back: String
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            Text("‹ \(back)").uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink2)
        }
        .buttonStyle(.plain)
        .padding(.bottom, 20)
    }
}

/// The bottom of a page: what people need once in a long while, as one wrapping line of quiet links.
struct FooterLinks<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        Wrap(spacing: 16) { content }
            .padding(.top, 4)
            .id(SettingsSearch.footerID)
    }
}

struct FooterLink: View {
    let label: String
    var tint: Color = Theme.ink2
    let action: () -> Void
    @Environment(\.isEnabled) private var enabled
    var body: some View {
        Button(action: action) {
            Text(label).uiFont(12.5).foregroundStyle(tint)
                .underline(color: tint.opacity(0.35))
        }
        .buttonStyle(.plain)
        .opacity(enabled ? 1 : 0.4)
    }
}

/// A few choices side by side; the chosen one sits on a raised paper chip that glides between them.
struct ChoiceStrip<Value: Hashable>: View {
    let choices: [(Value, String)]
    @Binding var value: Value
    @Namespace private var ns
    @Environment(\.reducesMotion) private var reduceMotion
    var body: some View {
        HStack(spacing: 2) {
            ForEach(choices, id: \.0) { choice in
                let on = choice.0 == value
                Text(choice.1).uiFont(12.5, weight: .medium)
                    .foregroundStyle(on ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3))
                    .padding(.horizontal, 11)
                    .frame(height: 24)
                    .background {
                        if on {
                            RoundedRectangle(cornerRadius: 6, style: .continuous).fill(Theme.chipOn)
                                .shadow(color: Theme.knobShadow, radius: 1, y: 0.5)
                                .matchedGeometryEffect(id: "choice-on", in: ns)
                        }
                    }
                    .contentShape(Rectangle())
                    .onTapGesture { withAnimation(Theme.anim(Theme.glide, reduce: reduceMotion)) { value = choice.0 } }
            }
        }
        .padding(2)
        .background(Theme.ink.opacity(0.06), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}
