import SwiftUI

/// What the onboarding left for later (`OnboardingFlow.leftovers`), each with the one button that does it. The same
/// list sits on the onboarding's last step and in the main window (docs/11 principle 7, Linear's checklist).
struct LeftoverList: View {
    let items: [OnboardingLeftover]
    /// 숨기기: the row goes away for good (until the onboarding is started again).
    var dismiss: ((OnboardingLeftover) -> Void)? = nil
    let fix: (OnboardingLeftover) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            ForEach(Array(items.enumerated()), id: \.offset) { _, item in row(item) }
        }
    }

    /// Those with something to do now: a tool that is not ready yet only waits.
    static func actionable(_ items: [OnboardingLeftover]) -> [OnboardingLeftover] {
        items.filter { if case .notify = $0 { return false } else { return true } }
    }

    private func row(_ item: OnboardingLeftover) -> some View {
        let (symbol, title, sub, button) = Self.words(item)
        return HStack(alignment: .center, spacing: 10) {
            Image(systemName: symbol).uiFont(12, weight: .semibold).foregroundStyle(Theme.ink3)
                .frame(width: 16)
            VStack(alignment: .leading, spacing: 1) {
                Text(title).uiFont(13, weight: .medium).foregroundStyle(Theme.ink)
                Text(sub).uiFont(12).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            if let button {
                Button(button) { fix(item) }.buttonStyle(GhostButtonStyle())
            }
            if let dismiss {
                Button { dismiss(item) } label: {
                    Image(systemName: "xmark").uiFont(9.5, weight: .bold).foregroundStyle(Theme.ink3)
                        .frame(width: 22, height: 22).background(Theme.fill, in: Circle())
                }
                .buttonStyle(.plain)
                .help("숨기기")
                .accessibilityLabel(String(localized: "\(Self.words(item).1) 숨기기"))
            }
        }
    }

    private static func words(_ item: OnboardingLeftover) -> (String, String, String, String?) {
        switch item {
        case .mic: ("mic", String(localized: "마이크 켜기"), String(localized: "회의에서 나오는 질문을 들으려면 필요해요"), String(localized: "켜기"))
        case .connectMic: ("mic", String(localized: "마이크 연결"), String(localized: "권한은 켜졌어요. 마이크나 이어폰을 연결하면 들어요"), nil)
        case .connectSource: ("link", String(localized: "서비스 연결"), String(localized: "연결하면 회의 중 질문에 답할 수 있어요"), String(localized: "연결"))
        case .connectNotion: ("doc.text", String(localized: "노션 연결"), String(localized: "연결하면 노션 문서에서도 답을 찾아요"), String(localized: "연결"))
        case .connectLinear: ("checklist", String(localized: "Linear 연결"), String(localized: "연결하면 Linear 이슈에서도 답을 찾아요"), String(localized: "연결"))
        case .connectDrive: ("triangle", String(localized: "Google Drive 연결"), String(localized: "연결하면 Google Drive 문서에서도 답을 찾아요"), String(localized: "연결"))
        case .connectAtlassian: ("square.stack", String(localized: "Jira · Confluence 연결"), String(localized: "연결하면 Jira 이슈와 Confluence 페이지에서도 답을 찾아요"), String(localized: "연결"))
        case .signIn: ("person.crop.circle", String(localized: "Tacie에 로그인"), String(localized: "로그인해야 회의를 듣고 답할 수 있어요"), String(localized: "로그인"))
        case .chooseAI: ("sparkles", String(localized: "AI 고르기"), String(localized: "골라야 회의를 받아쓰고 정리할 수 있어요"), String(localized: "고르기"))
        case .callAudio: ("speaker.wave.2", String(localized: "회의 앱 소리 켜기"), String(localized: "원격 회의에서 상대 목소리도 들으려면 필요해요"), String(localized: "켜기"))
        case .notify(let tools):
            ("bell", tools.map(name).joined(separator: ", "), String(localized: "leftover.notify", defaultValue: "준비되면 알려 드릴게요"), nil)
        }
    }

    private static func name(_ id: String) -> String {
        ["slack": String(localized: "슬랙"), "notion": String(localized: "노션"), "drive": "Google Drive", "linear": "Linear",
         "atlassian": "Jira · Confluence", "asana": "Asana"][id] ?? id
    }
}
