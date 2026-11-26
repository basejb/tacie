import SwiftUI

/// Continue meeting at the foot of an ended meeting's page, beside Play recording (docs/35 5-4): the same pill, a green
/// dot for listening. Shown while the meeting can still be taken up again and nothing else is being listened to.
struct ResumeCapsule: View {
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 7) {
                Circle().fill(Theme.live).frame(width: 7, height: 7)
                Text("회의 이어 가기").uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink)
            }
            .padding(.horizontal, 14).frame(height: 40)
            .contentShape(Capsule())
        }
        .buttonStyle(PressStyle())
        .background { Capsule().fill(Theme.raised).shadow(color: Theme.ink.opacity(0.08), radius: 11, y: 6) }
        .overlay(Capsule().strokeBorder(Theme.ink.opacity(0.1)))
        .help(String(localized: "이 회의에 이어서 받아써요"))
    }
}
