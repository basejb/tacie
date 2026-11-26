import SwiftUI

/// The scene on top of Settings → Meetings (2026-11-15): a drawn on-screen Tacie showing what the row under the pointer
/// does during a meeting, with the meeting's four steps in the corner. It reads the same settings the rows write, so a
/// switch flipped shows its result at once. Decorative: nothing in it is pressed.
struct MeetingSettingsScene: View {
    enum Focus: Equatable {
        case mode, sound, prompt, calendar, stt, notice, noticeText, music, translation, notes, openAfter, record, people, keep
        /// Which of the four steps (회의를 시작할 때 · 회의 중 · 회의가 끝나면 · 기록) the row belongs to.
        var step: Int {
            switch self {
            case .mode, .sound, .prompt, .calendar: 1
            case .stt, .notice, .noticeText, .music, .translation: 2
            case .notes, .openAfter: 3
            case .record, .people, .keep: 4
            }
        }
    }

    let focus: Focus?

    @AppStorage("listenTo") private var listenTo = "mix"
    @AppStorage("openAfterMeeting") private var openAfterMeeting = true
    @AppStorage("recordNotice") private var recordNotice = true  // MeetingModel.recordNoticeKey
    @AppStorage(MediaPause.key) private var pauseMusic = true
    @AppStorage("recordOnly") private var recordOnly = false  // MeetingModel.recordOnlyKey
    @AppStorage(MeetingApp.promptKey) private var meetingPrompt = true
    @AppStorage("meetingNotes") private var meetingNotes = true
    @AppStorage("rememberPeople") private var rememberPeople = true
    @AppStorage("recordMeetings") private var recordMeetings = true
    @AppStorage("meetingRetention") private var retentionRaw = Retention.forever.rawValue
    /// The on-device choice as the user set it (LocalSTT.key); the scene draws the path the sound takes.
    @AppStorage(LocalSTT.key) private var onDevice = false
    @Environment(\.reducesMotion) private var reduceMotion

    var body: some View {
        SettingsSceneFrame(caption: caption) {
            Group {
                if focus == .stt {
                    STTPathScene(onDevice: onDevice && LocalSTT.offered)
                } else {
                    VStack(spacing: 10) {
                        pill
                        below
                    }
                    .padding(.top, 40)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                }
            }
            .id(focus.map { "\($0)" } ?? "none")
            .transition(.opacity.combined(with: .offset(y: reduceMotion ? 0 : 3)))
        } corner: {
            steps
        }
        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: focus)
        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: stateKey)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(caption))
    }

    /// Changes whenever a setting the scene draws changes, so the scene animates to it.
    private var stateKey: String {
        "\(listenTo)\(openAfterMeeting)\(recordNotice)\(pauseMusic)\(recordOnly)\(meetingPrompt)\(meetingNotes)\(rememberPeople)\(recordMeetings)\(retentionRaw)\(onDevice)"
    }

    // MARK: 화면 위 Tacie

    @ViewBuilder private var pill: some View {
        switch focus {
        case .prompt:
            ScenePill(mood: .asleep) {
                Image(systemName: "video.fill").uiFont(10, weight: .semibold).foregroundStyle(Theme.ink2)
                Text("Zoom 회의가 시작됐어요").uiFont(12, weight: .medium).foregroundStyle(Theme.ink)
                Text("같이 듣기").uiFont(11, weight: .semibold).foregroundStyle(Theme.onStrong)
                    .padding(.horizontal, 8).frame(height: 20).background(Capsule().fill(Theme.ink))
            }
        case .calendar:
            ScenePill(mood: .listening) {
                Circle().fill(Theme.live).frame(width: 6, height: 6)
                Text("주간 회의").uiFont(12, weight: .medium).foregroundStyle(Theme.ink)
                Text("0:12").uiFont(12, digits: true).foregroundStyle(Theme.ink3)
            }
        case .notes, .openAfter:
            ScenePill(mood: .happy) {
                Text("회의가 끝났어요").uiFont(12, weight: .medium).foregroundStyle(Theme.ink)
            }
        default:
            ScenePill(mood: .listening) {
                Circle().fill(Theme.live).frame(width: 6, height: 6)
                Text("원격 · 12:04").uiFont(12, weight: .medium, digits: true).foregroundStyle(Theme.ink)
                if focus == .mode && recordOnly {
                    Text("기록만").uiFont(11, weight: .medium).foregroundStyle(Theme.ink2)
                        .padding(.horizontal, 7).frame(height: 18).background(Capsule().fill(Theme.fill))
                }
            }
        }
    }

    // MARK: under the pill

    @ViewBuilder private var below: some View {
        switch focus {
        case .mode where !recordOnly:
            SceneCard {
                Text("상대가 물었어요 · 이 버그 누가 보고 있어요?").uiFont(11).foregroundStyle(Theme.ink3)
                Text("지윤 님이 지난주부터 보고 있어요.").uiFont(13, weight: .medium).foregroundStyle(Theme.ink)
                SceneChip(logo: "ServiceLinear", label: "ENG-212")
            }
        case .sound:
            SceneLine {
                HStack(spacing: 14) {
                    level(String(localized: "회의 앱"), on: true)
                    level(String(localized: "내 마이크"), on: listenTo != "system")
                }
            }
        case .notice, .noticeText:
            if recordNotice || focus == .noticeText {
                SceneLine {
                    Text("녹음 중이에요. 참석자에게 알렸나요?").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                    Text("알림 문구 복사").uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink)
                }
            }
        case .music:
            SceneLine {
                Image(systemName: pauseMusic ? "pause.fill" : "music.note").uiFont(9.5, weight: .bold).foregroundStyle(Theme.ink2)
                Text(pauseMusic ? String(localized: "Spotify를 멈췄어요 · 회의가 끝나면 다시 틀어요")
                                : String(localized: "Music과 Spotify는 그대로 재생돼요"))
                    .uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
            }
        case .translation:
            SceneTranscript(who: String(localized: "화자 2"), text: "We should keep the retry count at three.",
                            under: String(localized: "재시도는 세 번으로 두는 게 좋겠어요."))
        case .notes:
            if meetingNotes {
                SceneCard {
                    Text("회의 정리").uiFont(11).foregroundStyle(Theme.ink3)
                    Text("재시도는 세 번으로 정했어요.").uiFont(13, weight: .medium).foregroundStyle(Theme.ink)
                    HStack(spacing: 5) {
                        SceneChip(label: String(localized: "결정 2"))
                        SceneChip(label: String(localized: "할 일 3"))
                    }
                }
            } else {
                SceneLine { Text("대화 기록만 남아요").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2) }
            }
        case .openAfter:
            if openAfterMeeting { SceneWindow(title: String(localized: "주간 회의"), meta: String(localized: "방금")) }
        case .record:
            SceneTranscript(who: String(localized: "화자 2 · 12:01"), text: String(localized: "재시도는 세 번으로 두는 게 좋겠어요."),
                            play: recordMeetings)
        case .people:
            SceneTranscript(who: String(localized: "화자 2"), text: String(localized: "재시도는 세 번으로 두는 게 좋겠어요."),
                            renamed: rememberPeople ? String(localized: "지윤") : nil)
        case .keep:
            SceneList(rows: [(String(localized: "주간 회의"), String(localized: "오늘")),
                             (String(localized: "디자인 리뷰"), String(localized: "어제")),
                             (String(localized: "1:1"), String(localized: "11월 10일"))])
        default:
            EmptyView()
        }
    }

    private func level(_ name: String, on: Bool) -> some View {
        HStack(spacing: 6) {
            Text(name).uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
            SceneLevel(phase: name.count)
        }
        .opacity(on ? 1 : 0.3)
    }

    // MARK: words

    private var caption: String {
        switch focus {
        case nil: String(localized: "줄에 마우스를 올리면 회의 중 어디에서 보이는지 여기 나와요.")
        case .mode: recordOnly ? String(localized: "답 카드 없이 기록만 해요. 직접 묻기는 그대로 쓸 수 있어요.")
                               : String(localized: "질문이 나오면 답을 찾아 답 카드로 보여 줘요.")
        case .sound: String(localized: "두 소리 중 더 크게 들리는 쪽으로 화자를 나눠요.")
        case .prompt: meetingPrompt ? String(localized: "누르기 전에는 듣지 않아요.")
                                    : String(localized: "알림 없이, 직접 회의를 시작할 때만 들어요.")
        case .calendar: String(localized: "회의를 시작할 때 가까운 일정의 제목과 초대된 사람을 채워요.")
        case .stt: onDevice && LocalSTT.offered ? LocalSTTCopy.sceneDevice : LocalSTTCopy.sceneServer
        case .notice: recordNotice ? String(localized: "원격 회의 녹음을 시작하면 5초 동안 보여요.")
                                   : String(localized: "원격 회의 녹음을 시작해도 안내를 띄우지 않아요.")
        case .noticeText: String(localized: "회의를 듣기 전에 참석자에게 알릴 문구를 복사해요.")
        case .music: pauseMusic ? String(localized: "회의가 끝나면 Tacie가 멈춘 앱만 다시 틀어요.")
                                : String(localized: "회의 중에도 음악을 멈추지 않아요.")
        case .translation: MeetingTranslation.offered ? String(localized: "한국어가 아닌 말은 원문 바로 아래에 번역을 붙여요.")
                                                      : String(localized: "번역은 준비 중이에요. 원문 바로 아래에 번역을 붙일 거예요.")
        case .notes: meetingNotes ? String(localized: "맞장구를 뺀 대화 전체를 한 번 보내 요약, 결정 사항, 할 일을 정리해요.")
                                  : String(localized: "회의가 끝나도 대화를 보내지 않고, 회의 정리를 만들지 않아요.")
        case .openAfter: openAfterMeeting ? String(localized: "메인 창에서 그 회의가 열려요.")
                                          : String(localized: "메인 창은 직접 열 때까지 그대로예요.")
        case .record: recordMeetings ? String(localized: "대화 기록의 말을 누르면 그 부분을 다시 들어요.")
                                     : String(localized: "소리는 남기지 않고 글만 남아요.")
        case .people: rememberPeople ? String(localized: "이름을 붙인 참석자를 다음 회의에서도 그 이름으로 보여 줘요.")
                                     : String(localized: "참석자를 화자 번호로 보여 줘요.")
        case .keep: String(localized: "대화 기록과 회의 정리를 이 Mac에 \(SettingsText.retention(Retention(rawValue: retentionRaw) ?? .forever))해요.")
        }
    }

    // MARK: the four steps

    private var steps: some View {
        HStack(spacing: 4) {
            ForEach(1...4, id: \.self) { n in
                if n > 1 { Rectangle().fill(Theme.ink4).frame(width: 9, height: 1) }
                HStack(spacing: 4) {
                    StepDot(n: n, on: focus?.step == n, size: 15)
                    Text(Self.stepNames[n - 1]).uiFont(11, weight: focus?.step == n ? .medium : .regular)
                        .foregroundStyle(focus?.step == n ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3))
                }
            }
        }
    }

    private static let stepNames = [String(localized: "시작할 때"), String(localized: "회의 중"), String(localized: "끝나면"), String(localized: "기록")]
}
