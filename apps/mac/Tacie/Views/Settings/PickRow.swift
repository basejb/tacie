import SwiftUI

/// One row of a pick-several sheet (Choose vaults, Choose calendars): an ink check, what is picked, and a grey second
/// line when there is one. `lead` sits between the check and the name (a calendar's colour dot); the name dims when
/// off.
struct PickRow<Lead: View>: View {
    let title: String
    var detail: String? = nil
    let on: Bool
    let toggle: () -> Void
    @ViewBuilder var lead: Lead

    var body: some View {
        Button(action: toggle) {
            HStack(spacing: 10) {
                PickCheck(on: on)
                lead
                VStack(alignment: .leading, spacing: 1) {
                    Text(verbatim: title).uiFont(13.5, weight: .medium)
                        .foregroundStyle(on ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3)).lineLimit(1)
                    if let detail {
                        Text(verbatim: detail).uiFont(11.5).foregroundStyle(Theme.ink3).lineLimit(1).truncationMode(.middle)
                    }
                }
                Spacer(minLength: 8)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, detail == nil ? 0 : 6)
            .frame(minHeight: detail == nil ? 32 : 34)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

extension PickRow where Lead == EmptyView {
    init(title: String, detail: String? = nil, on: Bool, toggle: @escaping () -> Void) {
        self.init(title: title, detail: detail, on: on, toggle: toggle) { EmptyView() }
    }
}

/// The ink check box of a pick row.
struct PickCheck: View {
    let on: Bool

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 5, style: .continuous).fill(on ? Theme.ink : .clear)
            RoundedRectangle(cornerRadius: 5, style: .continuous).strokeBorder(Theme.ink.opacity(on ? 0 : 0.25), lineWidth: 1.2)
            Image(systemName: "checkmark").uiFont(9.5, weight: .bold).foregroundStyle(Theme.onStrong).opacity(on ? 1 : 0)
        }
        .frame(width: 16, height: 16)
    }
}
