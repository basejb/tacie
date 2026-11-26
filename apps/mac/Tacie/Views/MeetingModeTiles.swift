import SwiftUI

/// The two kinds of meeting as tiles: the island's chooser and the main window's Start meeting both show these.
struct MeetingModeTiles: View {
    let onStart: (MeetingMode) -> Void
    // 기록만 하기 takes the answer cards out of the tiles' words: no card comes up, and no code to keep off a shared screen
    @AppStorage(MeetingModel.recordOnlyKey) private var recordOnly = false

    var body: some View {
        HStack(spacing: 10) {
            ModeTile(symbol: "person.2.fill", title: MeetingMode.inPerson.title,
                     detail: String(localized: "회의실 대화를 마이크로 들어요.")
                         + (recordOnly ? "" : String(localized: " 답 카드는 30초 동안 보이고, 크게 볼 수도 있어요."))) { onStart(.inPerson) }
            ModeTile(symbol: "video.fill", title: MeetingMode.remote.title,
                     detail: String(localized: "회의 앱 소리와 내 마이크를 같이 들어요.")
                         + (Capture.allowed ? "" : recordOnly ? String(localized: " 화면 공유에는 안 보여요.")
                            : String(localized: "modeTile.remoteHidden", defaultValue: " 사내 코드가 새지 않게 화면 공유에는 안 보여요."))) { onStart(.remote) }
        }
    }
}

/// 회의 모드 under the tiles (답 카드 · 기록만): the same value as 설정 → 일반 → 회의, so the next meeting starts the same
/// way. Two labelled choices, not a switch: "on" read either way for a name like 기록만 하기.
struct RecordOnlyRow: View {
    @AppStorage(MeetingModel.recordOnlyKey) private var recordOnly = false

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "waveform").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.ink2).frame(width: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text("회의 모드").font(.system(size: 13.5, weight: .medium)).foregroundStyle(Theme.ink)
                Text(recordOnly ? String(localized: "답 카드를 띄우지 않고 녹음, 대화 기록, 회의 정리만 해요.") : String(localized: "질문이 나오면 연결한 곳에서 답을 찾아 카드로 보여 줘요."))
                    .font(.system(size: 12.5)).foregroundStyle(Theme.ink3)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
            ChoiceStrip(choices: [(false, String(localized: "답 카드")), (true, String(localized: "기록만"))], value: $recordOnly)
                .onChange(of: recordOnly) {
                    Analytics.capture(.settingChanged(setting: .recordOnly, value: ASettingValue(recordOnly)))
                }
        }
        .padding(.horizontal, 14).padding(.vertical, 11)
        .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).stroke(Theme.hairline, lineWidth: 1))
    }
}

struct ModeTile: View {
    let symbol: String
    let title: String
    let detail: String
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 10) {
                Image(systemName: symbol).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.ink)
                    .frame(width: 32, height: 32).background(Theme.paper, in: Circle())
                Text(title).font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.ink)
                Text(detail).font(.system(size: 12.5)).foregroundStyle(Theme.ink3).lineSpacing(2)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(14)
            .frame(maxWidth: .infinity, minHeight: 150, alignment: .topLeading)
            .background(hover ? Theme.fillHover : Theme.fill, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
        }
        .buttonStyle(PressStyle())
        .onHover { hover = $0 }
    }
}

/// 회의 시작 in the main window: asks which kind of meeting, with the island's tiles (⌃⌥⌘M still starts the last kind at once).
struct StartMeetingButton: View {
    let start: (MeetingMode) -> Void
    var disabled = false
    @State private var open = false

    var body: some View {
        // plain, not PressStyle: the popover anchors to the button and a squeezed button moves it
        Button { open.toggle() } label: { StrongButton(label: String(localized: "회의 시작"), icon: "plus", chevron: true) }
            .buttonStyle(.plain)
            .opacity(disabled ? 0.25 : 1)
            .disabled(disabled)
            .help("대면·원격 중 골라서 회의를 시작해요. ⌃⌥⌘M은 지난번 방식으로 바로 시작해요")
            .popover(isPresented: $open, arrowEdge: .bottom) {
                VStack(alignment: .leading, spacing: 14) {
                    Text("어떤 회의인가요?").uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink2)
                    MeetingModeTiles { mode in
                        open = false
                        start(mode)
                    }
                    RecordOnlyRow()
                }
                .padding(18)
                .frame(width: 440, alignment: .leading)
                .appearanceScheme()
            }
    }
}
