import SwiftUI

/// A connected tile's status in two lines (onboarding "Where should Tacie look for answers?", 2026-11-01): where it is
/// connected (the workspace or account, ink), then the numbers (pages, issues, documents, lighter). One line held both
/// and a long name cut the numbers off. Either line shows its whole text on hover; an email keeps its domain (cut in
/// the middle).
struct TileStatusLines: View {
    let name: String
    var detail: String? = nil
    /// The numbers are still growing (a big first fetch comes in batches): a small spinner before them.
    var growing = false
    /// A must-do pick (nothing picked yet) at the end of the second line, so the tile keeps its height.
    var pick: (label: String, action: () -> Void)? = nil

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: "checkmark.circle.fill").uiFont(11).foregroundStyle(Theme.live)
            VStack(alignment: .leading, spacing: 2) {
                Text(name).uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                    .lineLimit(1).truncationMode(name.contains("@") ? .middle : .tail)
                    .help(name)
                if let detail {
                    HStack(spacing: 5) {
                        if growing { ProgressView().controlSize(.mini).scaleEffect(0.8) }
                        Text(detail).uiFont(11).foregroundStyle(Theme.ink3).lineLimit(1).help(detail)
                        if let pick {
                            Spacer(minLength: 6)
                            Button(pick.label, action: pick.action).buttonStyle(.plain)
                                .uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink).fixedSize()
                        }
                    }
                }
            }
            Spacer(minLength: 0)
        }
    }
}

/// The ⋯ at a connected tile's top right: the same place and look as 설정 → 연결's cards.
struct TileMenu<Items: View>: View {
    @ViewBuilder let items: () -> Items

    var body: some View {
        Menu { items() } label: {
            Image(systemName: "ellipsis").uiFont(13, weight: .semibold).foregroundStyle(Theme.ink3)
                .frame(width: 24, height: 20)
                .contentShape(Rectangle())
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
    }
}
