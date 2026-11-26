import SwiftUI

/// The kind the notes were written as, on the meeting page's date line beside "Written by AI" (docs/23 3-5, mockup A):
/// "Interview ▾". It opens a popover like Start meeting's: each kind with what its notes hold; picking another writes
/// the notes again as that kind. Lines the person changed go with the old sections, so it asks first when there are
/// any.
struct NotesKindButton: View {
    let kind: NotesKind
    let hasEdits: Bool
    let choose: (NotesKind) -> Void

    @State private var open = false
    @State private var asking: NotesKind?
    @State private var hover = false

    var body: some View {
        // plain, not PressStyle: the popover anchors to the button and a squeezed button moves it
        Button { open.toggle() } label: {
            HStack(spacing: 4) {
                Text(kind.button).uiFont(11, weight: .semibold)
                Image(systemName: "chevron.down").uiFont(8, weight: .semibold)
            }
            .foregroundStyle(Theme.ink3)
            .padding(.horizontal, 7)
            .frame(height: 19)
            .background(hover || open ? Theme.fillHover : Theme.fill, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .help("어떤 녹음인지 바꾸면 그 형식으로 다시 써요")
        .popover(isPresented: $open, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 12) {
                Text("어떤 녹음인가요?").uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink2)
                VStack(spacing: 6) {
                    ForEach(NotesKind.allCases, id: \.self) { k in
                        KindRow(kind: k, current: k == kind) {
                            open = false
                            guard k != kind else { return }
                            if hasEdits { asking = k } else { choose(k) }
                        }
                    }
                }
            }
            .padding(16)
            .frame(width: 340, alignment: .leading)
            .appearanceScheme()
        }
        .confirmationDialog(asking.map { String(localized: "\($0.asTarget) 다시 쓸까요?") } ?? "",
                            isPresented: Binding(get: { asking != nil }, set: { if !$0 { asking = nil } })) {
            Button("다시 쓰기") { if let k = asking { choose(k) }; asking = nil }
            Button("취소", role: .cancel) { asking = nil }
        } message: {
            if let k = asking { Text("요약과 아래 목록을 \(k.noteName) 형식으로 새로 써요. 직접 고친 내용은 사라져요.") }
        }
    }
}

/// One kind in the popover, drawn like 회의 시작's tiles: its symbol in a white circle, its name, what its notes hold;
/// the kind shown now has a check.
private struct KindRow: View {
    let kind: NotesKind
    let current: Bool
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Image(systemName: kind.symbol).font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.ink)
                    .frame(width: 30, height: 30).background(Theme.paper, in: Circle())
                VStack(alignment: .leading, spacing: 2) {
                    Text(kind.menuTitle).font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.ink)
                    Text(kind.menuDetail).font(.system(size: 12)).foregroundStyle(Theme.ink3)
                }
                Spacer(minLength: 8)
                if current {
                    Image(systemName: "checkmark").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.ink)
                }
            }
            .padding(.horizontal, 12)
            .frame(maxWidth: .infinity, minHeight: 56, alignment: .leading)
            .background(hover || current ? Theme.fillHover : Theme.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        }
        .buttonStyle(PressStyle())
        .onHover { hover = $0 }
    }
}
