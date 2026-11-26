import AppKit
import ServiceManagement
import SwiftUI

struct GeneralPage: View {
    let context: SettingsContext
    let showLicenses: () -> Void

    @AppStorage("islandAlways") private var islandAlways = true
    @AppStorage(IslandController.anchorKey) private var islandAnchor: String?
    @State private var loginStatus = SMAppService.mainApp.status
    @State private var loginFailed = false
    @State private var loginTurnOffFailed = false
    @State private var report: ReportState = .idle
    @State private var autoUpdate = AppUpdate.shared.automaticallyDownloads
    @State private var appearance = AppearanceMode.shared
    @State private var language = AppLanguage.current()

    /// What the updater holds: a version waiting for a restart, or nothing to say.
    private var updateLine: String? {
        switch AppUpdate.shared.state {
        case .ready(let v): String(localized: "새 버전 \(v)을 받아 뒀어요. 다시 시작하면 바뀌어요.")
        case .available(let v): String(localized: "새 버전 \(v)이 있어요.")
        case .none: nil
        }
    }

    private enum ReportState { case idle, sent, failed }

    private var loginOn: Binding<Bool> {
        Binding(
            get: { loginStatus == .enabled },
            set: { on in
                do {
                    if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
                    Analytics.capture(.settingChanged(setting: .loginItem, value: ASettingValue(on)))
                    loginFailed = false
                    loginTurnOffFailed = false
                } catch {
                    loginFailed = true
                    loginTurnOffFailed = !on
                }
                loginStatus = SMAppService.mainApp.status
            })
    }

    private var version: String {
        let info = Bundle.main.infoDictionary
        return String(localized: "버전 \(info?["CFBundleShortVersionString"] as? String ?? "0") (\(info?["CFBundleVersion"] as? String ?? "0")) · 베타")
    }

    /// Under the links once 문제 신고 was pressed.
    private var reportNote: String? {
        switch report {
        case .idle: nil
        case .sent where Build.isOpen:
            String(localized: "회의 대화와 답을 뺀 앱 기록을 압축해 Finder에서 보여 드렸어요. 열린 GitHub 이슈에 이 파일을 끌어다 놓아 주세요. 이슈는 누구나 볼 수 있으니 올리기 전에 파일 안을 확인해 주세요.")
        case .sent: String(localized: "회의 대화와 답을 뺀 앱 기록을 압축해 Finder에서 보여 드렸어요. \(LogReport.email)으로 보내 주세요. 주소는 복사해 뒀어요.")
        case .failed: String(localized: "로그를 압축하지 못했어요.")
        }
    }

    var body: some View {
        PageHeader(title: String(localized: "일반"), desc: AppUpdate.offered ? String(localized: "Tacie를 켜는 방법, 화면, 업데이트를 정해요.")
                   : String(localized: "Tacie를 켜는 방법과 화면을 정해요."))
        SettingsSection(title: String(localized: "시작")) {
            SettingsRow(title: String(localized: "로그인 시 자동 실행"),
                        desc: loginTurnOffFailed ? String(localized: "끄지 못했어요. 시스템 설정의 로그인 항목에서 꺼 주세요.")
                            : (loginFailed || loginStatus == .requiresApproval ? String(localized: "시스템 설정의 로그인 항목에서 허용해 주세요.") : nil)) {
                HStack(spacing: 10) {
                    if loginFailed || loginStatus == .requiresApproval {
                        Button { SMAppService.openSystemSettingsLoginItems() } label: { OutlineButton(label: String(localized: "시스템 설정 열기")) }
                            .buttonStyle(PressStyle())
                    }
                    InkToggle(on: loginOn)
                }
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            loginStatus = SMAppService.mainApp.status
        }
        SettingsSection(title: String(localized: "화면")) {
            SettingsRow(title: String(localized: "테마"), desc: String(localized: "자동은 Mac의 라이트·다크 설정을 따라가요.")) {
                ChoiceStrip(choices: [(AppearanceMode.Choice.system, String(localized: "자동")), (.light, String(localized: "라이트")), (.dark, String(localized: "다크"))],
                            value: $appearance.choice)
            }
            // the language names stay in their own language, as macOS lists them
            SettingsRow(title: String(localized: "언어"),
                        desc: context.meeting.isRunning ? String(localized: "회의가 끝난 뒤에 바꿀 수 있어요.")
                            : String(localized: "바꾸면 Tacie가 다시 시작해요. 자동은 Mac의 언어를 따라가요.")) {
                ChoiceStrip(choices: [(AppLanguage.system, String(localized: "자동")), (.korean, "한국어"), (.english, "English")],
                            value: $language)
                    .disabled(context.meeting.isRunning)
                    .onChange(of: language) { _, picked in
                        guard picked != AppLanguage.current() else { return }
                        picked.save()
                        // only a change of what is shown needs the restart (자동 → 한국어 on a Korean Mac does not)
                        if picked.showsKorean() != AppLocale.isKorean { context.actions.relaunch(.general) }
                    }
            }
        }
        SettingsSection(title: String(localized: "화면 위 Tacie")) {
            SettingsRow(title: String(localized: "항상 표시"), desc: String(localized: "회의 중이 아닐 때도 화면 위에 작게 떠 있어요.")) {
                InkToggle(on: $islandAlways)
                    .onChange(of: islandAlways) { Analytics.capture(.settingChanged(setting: .islandAlways, value: ASettingValue(islandAlways))) }
            }
            // only once it has been dragged off the middle: until then there is nothing to put back
            if islandAnchor != nil {
                SettingsRow(title: String(localized: "위치"), desc: String(localized: "끌어서 옮긴 자리를 기억해요.")) {
                    Button(action: context.actions.resetIsland) { OutlineButton(label: String(localized: "가운데로 되돌리기")) }.buttonStyle(PressStyle())
                }
            }
        }
        .onChange(of: islandAlways) { _, on in context.actions.islandAlwaysChanged(on) }
        // a build without an update feed (the open edition from source, a fork) has no updater to set
        if AppUpdate.offered {
        SettingsSection(title: String(localized: "업데이트"), desc: updateLine) {
            SettingsRow(title: String(localized: "자동 업데이트"), desc: String(localized: "뒤에서 받아 두고, 다시 시작할 때 바꿔요. 회의 중에는 받지 않아요.")) {
                InkToggle(on: Binding(get: { autoUpdate }, set: {
                    autoUpdate = $0
                    AppUpdate.shared.automaticallyDownloads = $0
                    Analytics.capture(.settingChanged(setting: .autoUpdate, value: ASettingValue($0)))
                }))
            }
        }
        }
        // what is used once in a long while: one line of links instead of a section of rows
        VStack(alignment: .leading, spacing: 8) {
            FooterLinks {
                Text(version).uiFont(12.5).foregroundStyle(Theme.ink3)
                if AppUpdate.offered { FooterLink(label: String(localized: "업데이트 확인")) { AppUpdate.shared.checkNow() } }
                FooterLink(label: String(localized: "처음 안내 다시 보기"), action: context.actions.restartOnboarding)
                FooterLink(label: String(localized: "문제 신고"), action: sendReport)
                FooterLink(label: String(localized: "오픈소스 라이선스"), action: showLicenses)
            }
            if let reportNote {
                Text(reportNote).uiFont(12.5).foregroundStyle(report == .failed ? AnyShapeStyle(Theme.warn) : AnyShapeStyle(Theme.ink3))
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func sendReport() {
        do {
            let zip = try LogReport.make()
            NSWorkspace.shared.activateFileViewerSelecting([zip])
            if Build.isOpen {
                NSWorkspace.shared.open(LogReport.issueURL)
            } else {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(LogReport.email, forType: .string)
            }
            report = .sent
        } catch {
            report = .failed
        }
    }
}

