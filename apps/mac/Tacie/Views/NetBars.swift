import SwiftUI

/// The pill's signal bars, beside the time only while the connection is not fine (`NetHealth`): slow lights the
/// first bar amber, offline greys them all under a red slash.
struct NetBars: View {
    let level: NetHealth.Level

    var body: some View {
        HStack(alignment: .bottom, spacing: 2) {
            ForEach([4.0, 7.0, 11.0].indices, id: \.self) { i in
                RoundedRectangle(cornerRadius: 1, style: .continuous)
                    .fill(level == .slow && i == 0 ? Theme.slow : Theme.ink4)
                    .frame(width: 3, height: [4.0, 7.0, 11.0][i])
            }
        }
        .frame(width: 13, height: 11, alignment: .bottomLeading)
        .overlay {
            if level == .offline {
                Capsule().fill(Theme.warn).frame(width: 15, height: 1.5).rotationEffect(.degrees(-35))
            }
        }
        .padding(.vertical, 6)  // a taller target for the help
        .contentShape(Rectangle())
        .accessibilityElement()
        .accessibilityLabel(level == .slow ? String(localized: "연결이 느려요") : String(localized: "인터넷이 끊겼어요"))
    }
}
