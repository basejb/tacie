import SwiftUI

/// Connections → Clear search data: the confirmation inside the modal.
struct ClearSearchDataPage: View {
    let context: SettingsContext
    let back: () -> Void

    @State private var failed = false
    @State private var working = false
    @State private var cleared = false
    @Environment(\.reducesMotion) private var reduceMotion

    private var keeper: IndexKeeper { context.keeper }
    private var blocker: String? { context.clearBlocker }

    private var message: String {
        // " 125MB" right after the noun in Korean, " (125 MB)" in English
        let size = keeper.info?.bytes.map { AppLocale.isKorean ? " \(SettingsText.size($0))" : " (\(SettingsText.size($0)))" } ?? ""
        return String(localized: "이 Mac의 검색 데이터\(size)를 지워요. 받아 둔 코드는 남겨서 다시 동기화할 때 빨라요. 연결은 그대로예요. 다시 동기화하기 전까지는 질문해도 답을 찾지 못해요.")
    }

    var body: some View {
        DetailHeader(back: String(localized: "settings.connections", defaultValue: "연결"), action: back)
        PageHeader(title: String(localized: "검색 데이터를 지울까요?"), desc: message)
        HStack(spacing: 10) {
            Button {
                guard context.clearBlocker == nil else { return }
                working = true
                Task {
                    if await keeper.clear() {
                        Analytics.capture(.dataCleared(what: .searchData))
                        cleared = true
                        try? await Task.sleep(for: .seconds(0.9))  // a beat of 지웠어요 on the button itself
                        back()
                    } else {
                        failed = true
                        working = false
                    }
                }
            } label: {
                // one pill whose width follows its words: 지우기 → 지우는 중 → 지웠어요 (docs/03 Arc 6번)
                HStack(spacing: 6) {
                    if cleared {
                        Image(systemName: "checkmark").uiFont(11, weight: .bold)
                            .transition(.opacity.combined(with: .scale(scale: 0.6)))
                    } else if working {
                        ProgressView().controlSize(.mini).tint(Theme.onStrong)
                            .transition(.opacity)
                    }
                    Text(cleared ? String(localized: "지웠어요") : working ? String(localized: "지우는 중") : String(localized: "지우기"))
                        .uiFont(12.5, weight: .semibold)
                }
                .foregroundStyle(Theme.onStrong)
                .padding(.horizontal, 13).frame(height: 28)
                .background(Theme.warn, in: Capsule())
                .contentShape(Capsule())
                .opacity(blocker != nil && !working ? 0.4 : 1)
                .animation(Theme.anim(working || cleared ? Theme.settle : Theme.exit, reduce: reduceMotion), value: working)
                .animation(Theme.anim(Theme.settle, reduce: reduceMotion), value: cleared)
            }
            .buttonStyle(PressStyle())
            .disabled(working || blocker != nil)
            Button(action: back) { OutlineButton(label: String(localized: "취소")) }.buttonStyle(PressStyle())
        }
        if let blocker {
            Text(blocker).uiFont(12.5).foregroundStyle(Theme.ink3).padding(.top, 12)
        } else if failed {
            Text("지금은 지울 수 없어요. 잠시 뒤에 다시 해 주세요.").uiFont(12.5).foregroundStyle(Theme.warn).padding(.top, 12)
        }
    }
}
