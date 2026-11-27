import SwiftUI

/// Settings → Meetings (2026-11-15): every meeting setting on one page, in the order a meeting runs — When a meeting
/// starts, In the meeting, After the meeting, Recording and storage. They used to be split between General and Privacy.
/// The page title and the scene (`MeetingSettingsScene`) sit above the scroll in `SettingsView`; a row under the
/// pointer sets `focus` and the scene shows what that row does.
struct MeetingSettingsPage: View {
    let context: SettingsContext
    @Binding var focus: MeetingSettingsScene.Focus?

    @AppStorage("listenTo") private var listenTo = "mix"
    @AppStorage("openAfterMeeting") private var openAfterMeeting = true
    @AppStorage(MeetingModel.recordNoticeKey) private var recordNotice = true
    @AppStorage(MediaPause.key) private var pauseMusic = true
    @AppStorage(MeetingModel.recordOnlyKey) private var recordOnly = false
    @AppStorage(MeetingTranslation.key) private var meetingTranslation = false
    @AppStorage(MeetingApp.promptKey) private var meetingPrompt = true
    @AppStorage("meetingNotes") private var meetingNotes = true
    @AppStorage(NotesLanguage.key) private var notesLanguage = NotesLanguage.meeting
    @AppStorage("rememberPeople") private var rememberPeople = true
    /// Read by MeetingModel when a meeting starts, so a change applies from the next meeting.
    @AppStorage("recordMeetings") private var recordMeetings = true
    @AppStorage("meetingRetention") private var retentionRaw = Retention.forever.rawValue
    @State private var promptOff = MeetingApp.off
    @State private var pending: Retention?
    @State private var copied = false

    private var retention: Retention { Retention(rawValue: retentionRaw) ?? .forever }
    private var expiring: Int { pending.map { context.library.wouldExpire(keeping: $0) } ?? 0 }

    private func setListenTo(_ v: String) {
        guard v != listenTo else { return }
        listenTo = v
        Analytics.capture(.settingChanged(setting: .listenTo, value: ASettingValue(listenTo: v)))
    }

    private func setRetention(_ r: Retention) {
        retentionRaw = r.rawValue
        Analytics.capture(.settingChanged(setting: .meetingRetention, value: ASettingValue(retention: r)))
    }

    private func choose(_ r: Retention) {
        guard r != retention else { return }
        if context.library.wouldExpire(keeping: r) == 0 {
            setRetention(r)
        } else {
            pending = r
        }
    }

    private func lit(_ step: Int) -> Bool { focus?.step == step }

    var body: some View {
        SettingsSection(title: String(localized: "회의를 시작할 때"), step: 1, lit: lit(1)) {
            SettingsRow(title: String(localized: "회의 모드"),
                        desc: String(localized: "기록만은 회의 중에 답 카드를 띄우지 않고 녹음, 대화 기록, 회의 정리만 해요. 직접 묻기는 그대로 쓸 수 있어요. 회의 중에는 화면 위 Tacie에서도 바꿀 수 있어요.")) {
                ChoiceStrip(choices: [(false, String(localized: "답 카드")), (true, String(localized: "기록만"))], value: $recordOnly)
                    .onChange(of: recordOnly) {
                        Analytics.capture(.settingChanged(setting: .recordOnly, value: ASettingValue(recordOnly)))
                    }
            }
            .sceneFocus(.mode, $focus)
            SettingsRow(title: String(localized: "원격 회의 소리"), desc: String(localized: "두 소리 중 더 크게 들리는 쪽으로 누가 말했는지 나눠요.")) {
                Menu {
                    Button("회의 앱 소리 + 내 마이크") { setListenTo("mix") }
                    Button("회의 앱 소리만") { setListenTo("system") }
                } label: {
                    MenuText(label: listenTo == "system" ? String(localized: "회의 앱 소리만") : String(localized: "회의 앱 소리 + 내 마이크"))
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
            }
            .sceneFocus(.sound, $focus)
            SettingsRow(title: String(localized: "회의 시작 알림"),
                        desc: MeetingStartWatcher.available
                            ? String(localized: "회의 앱이 마이크를 쓰기 시작하면 화면 위 Tacie가 같이 들을지 물어요. 누르기 전에는 듣지 않아요.")
                            : String(localized: "macOS 14.4 이상에서 쓸 수 있어요.")) {
                InkToggle(on: $meetingPrompt)
                    .onChange(of: meetingPrompt) {
                        Analytics.capture(.settingChanged(setting: .meetingPrompt, value: ASettingValue(meetingPrompt)))
                    }
                    .disabled(!MeetingStartWatcher.available)
            }
            .sceneFocus(.prompt, $focus)
            if meetingPrompt && MeetingStartWatcher.available {
                SettingsRow(title: String(localized: "알림 받을 앱"), desc: String(localized: "Meet처럼 브라우저로 하는 회의는 브라우저로 알아채요.")) {
                    Menu {
                        ForEach(MeetingApp.allCases, id: \.self) { app in
                            Toggle(app.name, isOn: Binding(get: { !promptOff.contains(app) }, set: { on in
                                if on { promptOff.remove(app) } else { promptOff.insert(app) }
                                MeetingApp.off = promptOff
                            }))
                        }
                    } label: {
                        MenuText(label: promptOff.isEmpty ? String(localized: "모든 회의 앱")
                                 : String(localized: "\(MeetingApp.allCases.count - promptOff.count)개 앱"))
                    }
                    .menuStyle(.button)
                    .buttonStyle(.plain)
                    .menuIndicator(.hidden)
                    .fixedSize()
                }
                .sceneFocus(.prompt, $focus)
            }
            if CalendarStore.offered {
                CalendarSettingsRow().sceneFocus(.calendar, $focus)
            }
        }
        SettingsSection(title: String(localized: "회의 중"), step: 2, lit: lit(2)) {
            // the open edition chooses this in AI ("회의를 정리할 AI를 골라요", 2-D)
            if LocalSTT.offered && !AIMode.ownAI {
                LocalSTTRow(stt: context.localSTT).sceneFocus(.stt, $focus)
            }
            SettingsRow(title: String(localized: "녹음 알림"), desc: String(localized: "원격 회의 녹음을 시작하면 참석자에게 알릴 문구를 화면 위 Tacie에 잠깐 보여 줘요.")) {
                InkToggle(on: $recordNotice)
                    .onChange(of: recordNotice) { Analytics.capture(.settingChanged(setting: .recordNotice, value: ASettingValue(recordNotice))) }
            }
            .sceneFocus(.notice, $focus)
            SettingsRow(title: String(localized: "참석자 안내 문구"), desc: String(localized: "회의를 듣기 전에 참석자에게 알려 주세요.")) {
                Button {
                    context.actions.copyNotice()
                    copied = true
                } label: { OutlineButton(label: copied ? String(localized: "복사했어요") : String(localized: "복사"), icon: copied ? "checkmark" : nil) }
                    .buttonStyle(PressStyle())
                    .copiedFlash($copied)
                    .help(AppDelegate.participantNotice)
            }
            .sceneFocus(.noticeText, $focus)
            SettingsRow(title: String(localized: "음악 멈추기"), desc: String(localized: "회의 중에 재생 중이던 Music과 Spotify를 멈춰요. 회의가 끝나면 Tacie가 멈춘 앱만 다시 틀어요.")) {
                InkToggle(on: $pauseMusic)
                    .onChange(of: pauseMusic) { Analytics.capture(.settingChanged(setting: .pauseMusic, value: ASettingValue(pauseMusic))) }
            }
            .sceneFocus(.music, $focus)
            // development build only for now (docs/30): the language follows 일반 → 언어, there is none to pick.
            // A shipped app shows the row with 준비 중 instead of the switch (2026-11-15).
            Group {
                if MeetingTranslation.offered {
                    SettingsRow(title: String(localized: "번역"),
                                desc: String(localized: "한국어가 아닌 말은 대화 기록에서 원문 아래에 한국어 번역을 붙여요. 번역할 언어는 일반 → 언어를 따라가요.")) {
                        ChoiceStrip(choices: [(false, String(localized: "translation.off", defaultValue: "끄기")),
                                              (true, String(localized: "translation.on", defaultValue: "켜기"))],
                                    value: $meetingTranslation)
                    }
                } else if !AIMode.ownAI {  // the open edition has no relay to translate through: no promise either
                    SettingsRow(title: String(localized: "번역"),
                                desc: String(localized: "다른 언어로 한 회의도 대화 기록에서 원문 바로 아래 번역으로 읽을 수 있게 돼요. 번역할 언어는 앱 언어를 따라가요.")) {
                        SoonTag()
                    }
                }
            }
            .sceneFocus(.translation, $focus)
        }
        SettingsSection(title: String(localized: "회의가 끝나면"), step: 3, lit: lit(3)) {
            SettingsRow(title: String(localized: "회의 정리"), desc: String(localized: "회의가 끝나면 맞장구를 뺀 대화 전체를 한 번 AI로 보내서 요약, 결정 사항, 할 일을 정리해요.")) {
                InkToggle(on: $meetingNotes)
                    .onChange(of: meetingNotes) {
                        Analytics.capture(.settingChanged(setting: .meetingNotes, value: ASettingValue(meetingNotes)))
                    }
            }
            .sceneFocus(.notes, $focus)
            SettingsRow(title: String(localized: "정리 언어"),
                        desc: String(localized: "회의 언어는 회의에서 쓴 언어로, 앱 언어는 일반 → 언어를 따라 회의 정리를 써요. 대화 기록은 들은 말 그대로 남아요.")) {
                ChoiceStrip(choices: [(NotesLanguage.meeting, String(localized: "회의 언어")), (NotesLanguage.app, String(localized: "앱 언어"))],
                            value: $notesLanguage)
                    .onChange(of: notesLanguage) {
                        Analytics.capture(.settingChanged(setting: .notesLanguage,
                                                          value: notesLanguage == NotesLanguage.app ? .app : .meeting))
                    }
            }
            .sceneFocus(.notes, $focus)
            SettingsRow(title: String(localized: "회의 정리 자동 열기"), desc: String(localized: "회의가 끝나면 메인 창에서 그 회의를 열어요.")) {
                InkToggle(on: $openAfterMeeting)
                    .onChange(of: openAfterMeeting) {
                        Analytics.capture(.settingChanged(setting: .openAfterMeeting, value: ASettingValue(openAfterMeeting)))
                    }
            }
            .sceneFocus(.openAfter, $focus)
        }
        SettingsSection(title: String(localized: "기록"), step: 4, lit: lit(4)) {
            SettingsRow(title: String(localized: "회의 녹음"), desc: String(localized: "켜면 회의 소리를 이 Mac에 저장해서 대화 기록의 말을 눌러 다시 들을 수 있어요. 다음 회의부터 적용돼요.")) {
                InkToggle(on: $recordMeetings)
                    .onChange(of: recordMeetings) {
                        Analytics.capture(.settingChanged(setting: .recordMeetings, value: ASettingValue(recordMeetings)))
                    }
            }
            .sceneFocus(.record, $focus)
            SettingsRow(title: String(localized: "참석자 기억"), desc: String(localized: "이름을 붙인 참석자를 다음 회의에서도 알아보고 그 이름으로 보여 줘요. 알아보는 데 필요한 특징만 저장하고, 소리는 저장하지 않아요.")) {
                InkToggle(on: $rememberPeople)
                    .onChange(of: rememberPeople) {
                        Analytics.capture(.settingChanged(setting: .rememberPeople, value: ASettingValue(rememberPeople)))
                    }
            }
            .sceneFocus(.people, $focus)
            SettingsRow(title: String(localized: "회의 기록 보관"), desc: recordMeetings ? String(localized: "대화 기록과 회의 정리, 녹음을 이 Mac에 저장해요.") : String(localized: "대화 기록과 회의 정리를 이 Mac에 저장해요.")) {
                Menu {
                    ForEach(Retention.allCases, id: \.self) { r in
                        Button(SettingsText.retention(r)) { choose(r) }
                    }
                } label: {
                    MenuText(label: SettingsText.retention(retention))
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
            }
            .sceneFocus(.keep, $focus)
        }
        .alert("회의 기록 \(expiring)개가 지워져요", isPresented: Binding(get: { pending != nil }, set: { if !$0 { pending = nil } })) {
            Button("지우기", role: .destructive) {
                if let r = pending {
                    setRetention(r)
                    context.library.prune(keeping: r)
                }
                pending = nil
            }
            Button("취소", role: .cancel) { pending = nil }
        } message: {
            Text("고른 기간보다 오래된 회의를 지금 지워요. 되돌릴 수 없어요.")
        }
    }
}

extension View {
    /// The row under the pointer tells the page's scene what to show; leaving it keeps the scene as it is, so the
    /// pointer can travel to the control without the scene flicking back.
    func sceneFocus<F: Equatable>(_ f: F, _ focus: Binding<F?>) -> some View {
        onHover { inside in if inside && focus.wrappedValue != f { focus.wrappedValue = f } }
    }
}
