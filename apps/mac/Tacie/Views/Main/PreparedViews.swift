import SwiftUI

/// "11월 7일 주간 결제 싱크에서 이어져요 · 연결 끊기" (continues from the Nov 7 weekly payments sync · Disconnect): at the top of a
/// meeting prepared from another one (docs/36 1-6절).
struct LinkBanner: View {
    let from: SavedMeeting
    let calendar: Calendar
    let cut: () -> Void

    static func text(from: SavedMeeting, calendar: Calendar) -> String {
        String(format: String(localized: "link.banner", defaultValue: "%1$@ %2$@에서 이어져요"),
               SavedMeeting.format(AppLocale.pattern(ko: "M월 d일", template: "MMMd"), from.start, calendar), from.displayTitle(calendar: calendar))
    }

    var body: some View {
        HStack(spacing: 10) {
            Text(Self.text(from: from, calendar: calendar)).uiFont(12).foregroundStyle(Theme.ink2).lineLimit(1)
            Button(action: cut) {
                Text(String(localized: "link.cut", defaultValue: "연결 끊기")).uiFont(12).foregroundStyle(Theme.ink3).underline()
            }
            .buttonStyle(PressStyle())
        }
        .padding(.horizontal, 12).padding(.vertical, 5)
        .background(Theme.ink.opacity(0.045), in: Capsule())
        .padding(.bottom, 14)
    }
}

/// "준비한 것은 어떻게 됐나요": each prepared item, a green check and how it was settled, or an empty circle.
/// Nothing while the items are not judged yet or the request failed.
struct PreparedBlock: View {
    let items: [PreparedItem]

    static var title: String { String(localized: "followup.title", defaultValue: "준비한 것은 어떻게 됐나요") }

    static func decided(_ how: String) -> String {
        String(format: String(localized: "followup.decided", defaultValue: "정했어요 · %@"), how)
    }
    static var notYet: String { String(localized: "followup.notYet", defaultValue: "정하지 않았어요 · 이번에도 이야기가 나오지 않았어요") }

    /// What a row says under the item.
    static func sub(_ item: PreparedItem) -> String { item.decided == true ? decided(item.how ?? "") : notYet }

    var body: some View {
        let judged = items.filter { $0.decided != nil }
        if !judged.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                DocHeading(title: Self.title)
                VStack(alignment: .leading, spacing: 14) {
                    ForEach(Array(judged.enumerated()), id: \.offset) { _, item in
                        HStack(alignment: .top, spacing: 10) {
                            if item.decided == true {
                                Image(systemName: "checkmark.circle.fill").font(.system(size: 16)).foregroundStyle(Theme.live)
                            } else {
                                Image(systemName: "circle").font(.system(size: 16)).foregroundStyle(Theme.ink3)
                            }
                            VStack(alignment: .leading, spacing: 2) {
                                Text(item.text).uiFont(15).foregroundStyle(Theme.ink)
                                Text(Self.sub(item)).uiFont(12.5).foregroundStyle(Theme.ink3)
                            }
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
                .padding(.top, 10)
            }
            .padding(.bottom, 36)
        }
    }
}
