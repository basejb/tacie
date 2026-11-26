import SwiftUI

struct PrivacyPage: View {
    let context: SettingsContext
    let showDelete: () -> Void
    let showForgetPeople: () -> Void
    let showSent: () -> Void

    /// The privacy policy page; nil until the website has one, and the link stays hidden.
    static let policy: URL? = nil

    @AppStorage(Capture.key) private var showInCapture = false

    private var running: Bool { context.meeting.isRunning }
    private var empty: Bool { context.library.meetings.isEmpty }
    private var noPeople: Bool { !context.vault.hasStoredData }
    private var canForgetPeople: Bool { !running && !noPeople }
    private var forgetPeopleDesc: String {
        if running { return String(localized: "회의가 끝난 뒤에 지울 수 있어요.") }
        if noPeople { return String(localized: "기억한 참석자가 없어요.") }
        let n = context.vault.people.count
        return n > 0 ? String(localized: "\(n)명을 기억하고 있어요. 회의 기록에 붙인 이름은 그대로 남아요.")
            : String(localized: "회의 뒤에 이름을 붙일 수 있게 남겨 둔 특징이 있어요. 회의 기록에 붙인 이름은 그대로 남아요.")
    }
    private var canDelete: Bool { !running && !empty }

    /// 2026-11-16: the meeting switches moved to 회의; what leaves this Mac stays a page behind 보내는 정보 보기.
    var body: some View {
        PageHeader(title: String(localized: "개인정보"),
                   desc: String(localized: "회의 기록, 연결한 서비스에서 받은 내용, 검색 데이터는 이 Mac에 저장해요. 음성 인식과 답 찾기에 필요한 부분만 AI로 보내요. 화면 위 Tacie와 메인 창은 기본으로 화면 공유와 녹화에 보이지 않아요."))
        Button(action: showSent) {
            HStack(spacing: 3) {
                Text("보내는 정보 보기").uiFont(12.5, weight: .medium)
                Image(systemName: "chevron.right").uiFont(9, weight: .bold)
            }
            .foregroundStyle(Theme.ink2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.top, -20)
        .padding(.bottom, 30)
        SettingsSection(title: String(localized: "화면 공유")) {
            SettingsRow(title: String(localized: "화면 공유·녹화 표시"),
                        desc: String(localized: "켜면 화면 위 Tacie와 메인 창이 화면 공유와 화면 녹화에 나와요. 답에 담긴 사내 코드와 문서가 상대에게 보일 수 있어요.")) {
                InkToggle(on: $showInCapture)
                    .onChange(of: showInCapture) {
                        Capture.applyToAll()
                        Analytics.capture(.settingChanged(setting: .showInCapture, value: ASettingValue(showInCapture)))
                    }
            }
        }
        SettingsSection(title: String(localized: "데이터 삭제")) {
            Button(action: showDelete) {
                SettingsRow(title: String(localized: "회의 기록 삭제"),
                            desc: running ? String(localized: "회의가 끝난 뒤에 지울 수 있어요.") : empty ? String(localized: "지울 회의 기록이 없어요.") : String(localized: "이 Mac에 저장한 회의 기록을 모두 지워요."),
                            titleColor: Theme.warn) { Chevron() }
                    .opacity(canDelete ? 1 : 0.4)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(!canDelete)
            Button(action: showForgetPeople) {
                SettingsRow(title: String(localized: "기억한 참석자 삭제"), desc: forgetPeopleDesc, titleColor: Theme.warn) { Chevron() }
                    .opacity(canForgetPeople ? 1 : 0.4)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(!canForgetPeople)
        }
    }
}

struct DeleteMeetingsPage: View {
    let context: SettingsContext
    @Bindable var selection: MainSelection
    let back: () -> Void

    @State private var failed = false
    private var running: Bool { context.meeting.isRunning }

    /// The iPhone meetings go everywhere, so the dialog says how many (docs/27 7-21).
    private var message: String { IPhoneMeetingUI.removeAllMessage(for: context.library.meetings) }

    var body: some View {
        DetailHeader(back: String(localized: "개인정보"), action: back)
        PageHeader(title: String(localized: "회의 기록을 모두 지울까요?"), desc: message)
        HStack(spacing: 10) {
            Button {
                if context.library.removeAll() {
                    Analytics.capture(.dataCleared(what: .meetings))
                    selection.id = nil
                    back()
                } else {
                    failed = true
                }
            } label: {
                Text("모두 지우기").uiFont(12.5, weight: .semibold).foregroundStyle(Theme.onStrong)
                    .padding(.horizontal, 13).frame(height: 28)
                    .background(Theme.warn, in: Capsule())
                    .contentShape(Capsule())
                    .opacity(running ? 0.4 : 1)
            }
            .buttonStyle(PressStyle())
            .disabled(running)
            Button(action: back) { OutlineButton(label: String(localized: "취소")) }.buttonStyle(PressStyle())
        }
        if failed {
            Text("일부 회의 기록을 지우지 못했어요. 다시 해 주세요.").uiFont(12.5).foregroundStyle(Theme.warn).padding(.top, 12)
        }
    }
}

/// Forgetting every remembered attendee (VoiceVault.removeAll); the names already in meeting files stay.
struct ForgetPeoplePage: View {
    let context: SettingsContext
    let back: () -> Void

    @State private var failed = false
    private var running: Bool { context.meeting.isRunning }

    var body: some View {
        DetailHeader(back: String(localized: "개인정보"), action: back)
        PageHeader(title: String(localized: "기억한 참석자를 모두 지울까요?"),
                   desc: String(localized: "기억한 참석자 \(context.vault.people.count)명의 특징이 이 Mac에서 사라져요. 회의 기록에 붙인 이름은 그대로 남아요. 되돌릴 수 없어요."))
        HStack(spacing: 10) {
            Button {
                if context.vault.removeAll() {
                    Analytics.capture(.dataCleared(what: .people))
                    back()
                } else {
                    failed = true
                }
            } label: {
                Text("모두 지우기").uiFont(12.5, weight: .semibold).foregroundStyle(Theme.onStrong)
                    .padding(.horizontal, 13).frame(height: 28)
                    .background(Theme.warn, in: Capsule())
                    .contentShape(Capsule())
                    .opacity(running ? 0.4 : 1)
            }
            .buttonStyle(PressStyle())
            .disabled(running)
            Button(action: back) { OutlineButton(label: String(localized: "취소")) }.buttonStyle(PressStyle())
        }
        if failed {
            Text("기억한 참석자를 지우지 못했어요. 다시 해 주세요.").uiFont(12.5).foregroundStyle(Theme.warn).padding(.top, 12)
        }
    }
}

struct SentInfoPage: View {
    let localSTT: LocalSTT
    let back: () -> Void

    var body: some View {
        DetailHeader(back: String(localized: "개인정보"), action: back)
        PageHeader(title: String(localized: "보내는 정보"), desc: String(localized: "회의 기록과 검색 데이터는 이 Mac에 두고, 아래 정보만 밖으로 보내요."))
        SettingsSection(title: String(localized: "AI로 보내는 정보"),
                        desc: Relay.on ? String(localized: "Tacie 서버를 거쳐 가는 정보는 전달만 하고 저장하지 않아요.")
                            : Build.isOpen ? String(localized: "고른 업체로만 보내고, Tacie 서버는 거치지 않아요.")
                            : String(localized: "개발 빌드는 Tacie 서버를 거치지 않고 바로 보내요.")) {
            SettingsRow(title: String(localized: "회의 소리"), desc: localSTT.phase == .ready ? LocalSTTCopy.sentLocal : LocalSTTCopy.sentServer) { EmptyView() }
            SettingsRow(title: String(localized: "질문과 후보 내용 일부"), desc: String(localized: "질문인지 알아보고 후보 순서를 정할 때, 후보마다 900자까지 보내요.")) { EmptyView() }
            SettingsRow(title: String(localized: "답에 쓸 근거 3개"), desc: String(localized: "답 한 줄을 쓸 때 근거 3개의 원문을 보내요.")) { EmptyView() }
            SettingsRow(title: String(localized: "회의 정리"), desc: String(localized: "켜 두면 회의가 끝날 때 맞장구를 뺀 대화 전체를 한 번 보내요.")) { EmptyView() }
            if CalendarStore.offered {
                SettingsRow(title: String(localized: "초대된 사람 이름"), desc: String(localized: "캘린더를 연결하면 회의 일정에 초대된 사람의 이름을 음성 인식으로 보내요. 일정과 이메일은 이 Mac에만 있어요.")) { EmptyView() }
            }
            if let policy = PrivacyPage.policy {
                Button { NSWorkspace.shared.open(policy) } label: {
                    HStack(spacing: 4) {
                        Text("보내는 곳은 개인정보 처리방침에 있어요").uiFont(12.5)
                        Image(systemName: "arrow.up.right").uiFont(9.5, weight: .bold)
                    }
                    .foregroundStyle(Theme.ink2)
                    .padding(.top, 8)
                }
                .buttonStyle(.plain)
            }
        }
        // usage numbers (docs/16): on from the start, no switch; this says what goes and what never does. The open edition
        // has no analytics token and no feedback card, so it sends none of this.
        if Build.isOpen {
            SettingsSection(title: String(localized: "사용 통계"), desc: String(localized: "오픈소스 버전은 사용 통계를 보내지 않아요.")) { EmptyView() }
        } else {
        SettingsSection(title: String(localized: "사용 통계"), desc: String(localized: "Tacie를 더 낫게 만들려고 쓰는 횟수와 시간만 보내요.")) {
            SettingsRow(title: String(localized: "보내는 것"), desc: String(localized: "회의 수와 길이, 카드가 뜬 수와 연 수, 앱·macOS 버전")) { EmptyView() }
            SettingsRow(title: String(localized: "보내지 않는 것"), desc: String(localized: "소리, 대화 기록, 질문과 답, 카드 내용, 저장소·채널 이름, 이메일")) { EmptyView() }
            SettingsRow(title: String(localized: "피드백 카드"), desc: String(localized: "보내기를 누를 때만 고른 항목과 직접 쓴 글을 Tacie 서버로 보내요.")) { EmptyView() }
        }
        }
    }
}
