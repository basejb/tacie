import SwiftUI

/// The sidebar folded into a bar along the top, when the window is narrow (MainView.compactBelow): the chosen meeting's
/// title opens the list as a drop-down; Start meeting and settings stay one click away.
struct CompactBar: View {
    static let height: CGFloat = 52
    /// Clear of the traffic lights, with the same gap they keep between themselves.
    static let besideLights: CGFloat = 86

    let title: String
    let open: Bool
    let running: Bool
    /// A sync going on or 남은 일 waiting: a dot on the list button, since the rows that say so are in the drop-down.
    let attention: Bool
    let toggle: () -> Void
    let actions: MainActions
    @Bindable var selection: MainSelection

    var body: some View {
        HStack(spacing: 8) {
            Button(action: toggle) {
                HStack(spacing: 7) {
                    Image(systemName: "sidebar.left").font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.ink2)
                    Text(title).uiFont(13, weight: .semibold).lineLimit(1)
                    Image(systemName: "chevron.down").font(.system(size: 9.5, weight: .bold)).foregroundStyle(Theme.ink3)
                        .rotationEffect(.degrees(open ? 180 : 0))
                    if attention { Circle().fill(Theme.warn).frame(width: 6, height: 6) }
                }
                .padding(.horizontal, 10)
                .frame(height: 30)
                .background(open ? Theme.fillHover : Theme.fill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .contentShape(Rectangle())
            }
            .buttonStyle(PressStyle())
            .help("회의 목록")
            .layoutPriority(-1)

            Spacer(minLength: 8)

            StartMeetingButton(start: actions.startMode, disabled: running)
            Button { selection.settings = .account } label: { IconButton(symbol: "gearshape") }
                .buttonStyle(.plain)
                .help("설정 (⌘,)")
        }
        // the traffic lights sit on the left of this row
        .padding(.leading, Self.besideLights)
        .padding(.trailing, 12)
        .frame(height: Self.height)
        .overlay(alignment: .bottom) { Rectangle().fill(Theme.hairline).frame(height: 1) }
        .zIndex(1)
    }
}
