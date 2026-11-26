import SwiftUI

struct AccountPage: View {
    let context: SettingsContext
    @State private var askSignOut = false
    @State private var askDelete = false
    @State private var deleteFailed = false
    @State private var newsFailed = false

    private var tacie: TacieAccount { TacieAccount.shared }

    var body: some View {
        let account = Account.current
        let stats = MeetingStats.month(context.library.meetings, now: Date(), calendar: MainView.calendar)
        HStack(spacing: 14) {
            Avatar(size: 52)
            VStack(alignment: .leading, spacing: 3) {
                Text(account.name).uiFont(20, weight: .semibold).foregroundStyle(Theme.ink)
                Text(subtitle(account)).uiFont(12.5).foregroundStyle(Theme.ink3)
            }
        }
        .padding(.bottom, 34)
        .task { await tacie.refresh() }

        if !tacie.signedIn {
            SettingsSection(title: String(localized: "로그인"), desc: String(localized: "로그인하면 Tacie AI로 회의를 듣고 답해요. 이름과 이메일만 받아요.")) {
                VStack(alignment: .leading, spacing: 10) {
                    if case .failed(let why) = tacie.phase {
                        Text(why).uiFont(12).foregroundStyle(Theme.warn).fixedSize(horizontal: false, vertical: true)
                    }
                    SignInButtons()
                }
                .padding(.vertical, 14)
            }
        } else if account.status == "waiting" {
            SettingsSection(title: "Tacie AI") {
                SettingsRow(title: String(localized: "승인을 기다리고 있어요"), desc: String(localized: "account.approvedOnly", defaultValue: "지금은 승인받은 분만 쓸 수 있어요. 승인되면 앱을 다시 열지 않아도 바로 써요.")) {
                    Button { Task { await tacie.refresh() } } label: { OutlineButton(label: String(localized: "다시 확인")) }.buttonStyle(PressStyle())
                }
            }
        }

        SettingsSection(title: String(localized: "이번 달 사용량"),
                        desc: account.usageLeft != nil ? String(localized: "베타 기간에는 Tacie가 비용을 내요.") : String(localized: "이 Mac에 남은 회의 기록으로 계산했어요.")) {
            if let left = account.usageLeft, let resets = account.resetsOn {
                UsageMeter(left: left, resetsIn: resets.timeIntervalSinceNow)
            }
            MonthStats(meetings: stats.meetings, listened: stats.listened, answers: stats.answers)
        }

        AIProblemSection(context: context)

        #if !TACIE_OPEN
        if tacie.signedIn, Build.isDev, IPhoneMeetingUI.showsDevices(context.library.meetings) {
            DevicesSection(meetings: context.library.meetings)
        }
        #endif

        if tacie.signedIn {
            SettingsSection(title: String(localized: "계정")) {
                SettingsRow(title: String(localized: "새 기능 소식 메일"),
                            desc: newsFailed ? String(localized: "바꾸지 못했어요. 인터넷 연결을 확인하고 다시 해 주세요.")
                                : String(localized: "Tacie에 새 기능이 생기면 메일로 알려 드려요. 메일 아래 링크로도 끌 수 있어요.")) {
                    InkToggle(on: Binding(get: { tacie.info?.mailNews == true }, set: { on in
                        Task { newsFailed = !(await tacie.setNews(on)) }
                    }))
                }
                // the meeting's notes and search data are this account's: it signs out once the meeting ends
                let inMeeting = context.meeting.isRunning
                SettingsRow(title: String(localized: "로그아웃"), desc: inMeeting ? String(localized: "회의가 끝난 뒤에 로그아웃할 수 있어요.")
                                : String(localized: "이 Mac에서 로그아웃해요. 회의 기록, 연결, 검색 데이터는 이 계정으로 다시 로그인하면 돌아와요.")) {
                    Button { askSignOut = true } label: { OutlineButton(label: String(localized: "로그아웃")) }.buttonStyle(PressStyle())
                        .disabled(inMeeting).opacity(inMeeting ? 0.4 : 1)
                }
                Button { askDelete = true } label: {
                    SettingsRow(title: String(localized: "계정 삭제"),
                                desc: deleteFailed ? String(localized: "계정을 지우지 못했어요. 인터넷 연결을 확인하고 다시 해 주세요.")
                                    : String(localized: "Tacie 서버의 계정과 사용 기록, 이 Mac에 있는 이 계정의 회의 기록과 연결, 검색 데이터를 지워요."),
                                titleColor: Theme.warn) { Chevron() }
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(inMeeting)
            }
            .alert("로그아웃할까요?", isPresented: $askSignOut) {
                Button("로그아웃", role: .destructive) { Task { await tacie.signOut() } }
                Button("취소", role: .cancel) {}
            } message: {
                Text("다시 로그인할 때까지 Tacie AI(받아쓰기, 판단, 답)를 쓸 수 없어요.")
            }
            .alert("계정을 삭제할까요?", isPresented: $askDelete) {
                Button("삭제", role: .destructive) { Task { deleteFailed = !(await tacie.deleteAccount()) } }
                Button("취소", role: .cancel) {}
            } message: {
                Text("Tacie 서버의 계정과 사용 기록, 이 Mac의 회의 기록과 녹음, 기억한 참석자, 연결, 검색 데이터가 모두 지워지고 되돌릴 수 없어요.")
            }
        }
    }

    private func subtitle(_ a: Account) -> String {
        guard let with = a.signedInWith else { return OnboardingModel.accountStep ? String(localized: "로그인하지 않았어요") : String(localized: "로그인 없이 쓰는 개발 빌드예요") }
        let email = tacie.info?.email.map { " · \($0)" } ?? ""
        return String(localized: "\(with) 계정으로 로그인했어요\(email)")
    }

}

/// What is left of this month's Tacie AI: a large percentage, the bar, and when it fills up again (red once it is used up).
private struct UsageMeter: View {
    let left: Double
    let resetsIn: TimeInterval
    var body: some View {
        let left = min(max(left, 0), 1)
        let out = left <= 0
        let tint = out ? Theme.warn : Theme.ink
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text("\(Int((left * 100).rounded()))%").uiFont(22, weight: .semibold, digits: true).foregroundStyle(tint)
                Text("남음").uiFont(13, weight: .medium).foregroundStyle(Theme.ink3)
                Spacer()
                Text("\(Int(((1 - left) * 100).rounded()))% 사용").uiFont(12.5, digits: true).foregroundStyle(out ? AnyShapeStyle(Theme.warn) : AnyShapeStyle(Theme.ink3))
            }
            Meter(value: 1 - left, tint: tint).padding(.top, 10)
            // one string with the bold time inside, so English can put the time last ("Available again in 5 hours")
            resetLine(Text(SettingsText.untilReset(resetsIn)).foregroundColor(Theme.ink).fontWeight(.semibold), out: out)
                .uiFont(12.5).foregroundStyle(Theme.ink3).monospacedDigit()
                .padding(.top, 10)
        }
        .padding(.top, 14)
        .padding(.bottom, 4)
    }

    private func resetLine(_ time: Text, out: Bool) -> Text {
        out ? Text("\(time) 뒤에 다시 쓸 수 있어요") : Text("\(time) 뒤에 자동으로 초기화돼요")
    }
}

/// This month on this Mac, as three numbers side by side.
private struct MonthStats: View {
    let meetings: Int
    let listened: TimeInterval
    let answers: Int
    var body: some View {
        HStack(spacing: 0) {
            cell(String(localized: "\(meetings)번"), String(localized: "회의"))
            divider
            cell(SettingsText.duration(listened), String(localized: "들은 시간"))
            divider
            cell(String(localized: "\(answers)번"), String(localized: "Tacie가 답함"))
        }
        .padding(.vertical, 14)
    }

    private var divider: some View { Rectangle().fill(Theme.hairline).frame(width: 1, height: 30).padding(.trailing, 16) }

    private func cell(_ value: String, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(value).uiFont(17, weight: .semibold, digits: true).foregroundStyle(Theme.ink)
            Text(label).uiFont(12).foregroundStyle(Theme.ink3)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
