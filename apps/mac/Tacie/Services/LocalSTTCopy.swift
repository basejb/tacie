import Foundation

/// Every word of the on-device speech recognition screens (docs/22 section 4), in one place so a test can read them all.
enum LocalSTTCopy {
    static let title = String(localized: "온디바이스 음성 인식")
    /// 설정 → 회의: the row and its two choices (2026-11-18, the switch became a choice between the two ways).
    static let rowTitle = String(localized: "음성 인식")
    static let serverChoice = String(localized: "stt.choice.server", defaultValue: "서버")
    static let deviceChoice = String(localized: "stt.choice.device", defaultValue: "온디바이스")
    static let serverDetail = String(localized: "회의 소리를 음성 인식 서버로 보내요. 말이 끝나면 바로 대화 기록에 적히고, 영어 용어를 더 정확히 적어요.")
    static let offDetail = serverDetail
    static let readyDetail = String(localized: "회의 소리를 Mac 밖으로 보내지 않아요. 말이 끝나고 1초쯤 뒤에 적히고, 영어 용어를 더 자주 틀려요.")
    static let unsupportedDetail = String(localized: "회의 소리를 음성 인식 서버로 보내요. 온디바이스는 메모리가 16GB 이상인 Mac에서만 쓸 수 있어요.")
    static let waitingDetail = String(localized: "인터넷에 연결되면 이어서 받아요. 그 전까지는 서버에서 음성 인식을 해요.")
    static let failedDetail = String(localized: "음성 인식 모델을 받지 못했어요. 다시 받기 전까지는 서버에서 음성 인식을 해요.")
    /// 개인정보 → 보내는 정보, the 회의 소리 line.
    static let sentLocal = String(localized: "이 Mac에서 음성 인식을 해요. 회의 소리는 Mac 밖으로 보내지 않아요.")
    /// The official app's speech vendor does not keep or train on it; in the open edition the person picks the vendor.
    static let sentServer = Build.isOpen ? String(localized: "회의 중에 고른 음성 인식 업체로 보내요. 저장하거나 학습에 쓰는지는 그 업체의 약관을 따라요.")
        : String(localized: "회의 중에 음성 인식 업체로 보내요. 업체는 저장하거나 학습에 쓰지 않아요.")

    static let consentTitle = String(localized: "음성 인식 모델을 받을까요?")
    static let consentBody = String(localized: "온디바이스 음성 인식에는 0.6GB 모델이 필요해요. 다 받기 전에 시작한 회의는 지금처럼 서버에서 음성 인식을 해요.")
    static let consentButton = String(localized: "받기")
    static let cancel = String(localized: "취소")
    static let spaceTitle = String(localized: "저장 공간이 부족해요")
    static let spaceButton = String(localized: "확인")
    static func spaceBody(more: Int64) -> String {
        String(localized: "음성 인식 모델을 받으려면 \(gb(more))가 더 필요해요. 공간을 비운 뒤 다시 켜 주세요.")
    }

    static let deleteRow = String(localized: "음성 인식 모델 삭제")
    static let deleteDesc = String(localized: "받아 둔 음성 인식 모델 파일을 지우고, 음성 인식을 서버에서 하도록 바꿔요.")
    static let deleteTitle = String(localized: "음성 인식 모델을 지울까요?")
    static let deleteBody = String(localized: "모델 파일을 지우고 온디바이스 음성 인식을 꺼요. 다시 켜면 모델을 다시 받아요.")
    static let deleteButton = String(localized: "지우기")

    static let downloadingTitle = String(localized: "음성 인식 모델 받는 중")
    static let waitingTitle = String(localized: "음성 인식 모델 받기 대기 중")
    static let failedTitle = String(localized: "음성 인식 모델을 받지 못했어요")
    static let doneText = String(localized: "온디바이스 음성 인식을 켰어요")
    static let retry = String(localized: "localSTT.retry", defaultValue: "다시 받기")
    /// LocalSTT's failure text when the download error says nothing of its own (LocalSTT.swift, the same key).
    static var unknownFailure: String { String(localized: "받지 못했어요") }

    static let meetingTitle = String(localized: "이번 회의는 서버에서 음성 인식을 해요")
    static let paused = String(localized: "음성 인식이 잠시 멈췄어요")

    /// 온보딩 2단계: two cards, the server one picked by default (2026-11-18). Numbers measured on 186 talk-show clips
    /// (docs/22 9절): text after speech ends 0.19 s vs 0.87 s, English terms 95.5% vs 85.3%, missed speech 0.2% vs 1.0%,
    /// engine memory up to 806 MB.
    static let onboardingSection = String(localized: "음성 인식 방식")
    static let onboardingSectionSub = String(localized: "설정 → 회의에서 언제든 바꿀 수 있어요")
    static let defaultTag = String(localized: "stt.choice.default", defaultValue: "기본")
    static let serverName = String(localized: "서버 음성 인식")
    static let serverLine = String(localized: "글이 더 빨리 나오고, 영어 용어를 더 정확히 적어요")
    static let serverPros = [String(localized: "말이 끝나면 바로 적혀요"), String(localized: "말을 빠뜨리는 일이 적어요")]
    static let serverCons = [String(localized: "회의 소리를 서버로 보내요")]
    static let deviceLine = String(localized: "회의 소리를 Mac 밖으로 보내지 않아요")
    static let deviceCons = [String(localized: "말이 끝나고 1초쯤 뒤에 적혀요"), String(localized: "영어 용어를 서버보다 자주 틀려요"),
                             String(localized: "말을 문장째 빠뜨릴 때가 있어요"), String(localized: "메모리를 0.8GB쯤 써요"),
                             String(localized: "모델(0.6GB)을 받는 동안은 서버에서 해요")]
    static let onboardingFoot = String(localized: "어느 쪽을 골라도 카드를 찾고 회의를 정리할 때는 대화 기록을 AI로 보내요.")
    /// The settings scene's caption.
    static let sceneServer = String(localized: "음성 인식 서버에서 회의 소리를 글로 바꿔요.")
    static let sceneDevice = String(localized: "이 Mac에서 회의 소리를 글로 바꿔요. 회의 소리는 서버로 보내지 않아요.")

    /// "0.4GB": gigabytes of 1,000,000,000 bytes, one decimal, rounded up. English puts a space before the unit.
    static func gb(_ bytes: Int64) -> String {
        let tenths = (Double(max(0, bytes)) / 100_000_000).rounded(.up)
        return String(format: "%.1f", tenths / 10) + (AppLocale.isKorean ? "GB" : " GB")
    }

    static func mb(_ bytes: Int64) -> Int { Int((Double(bytes) / 1_000_000).rounded()) }

    static func percent(done: Int64, total: Int64) -> Int {
        total > 0 ? Int(min(1, Double(max(0, done)) / Double(total)) * 100) : 0
    }

    /// The switch counts as on in every phase where the user asked for it and it has not been turned off.
    static func isOn(_ phase: LocalSTT.Phase) -> Bool {
        switch phase {
        case .downloading, .waiting, .failed, .ready: true
        case .off, .unsupported, .needsSpace: false
        }
    }

    /// The settings row's second line.
    static func detail(_ phase: LocalSTT.Phase) -> String {
        switch phase {
        case .off, .needsSpace: offDetail
        case .unsupported: unsupportedDetail
        case .ready: readyDetail
        case .waiting: waitingDetail
        case .failed: failedDetail
        case .downloading(let done, let total):
            String(localized: "\(downloadingTitle) · \(mb(done))MB / \(mb(total))MB · 다 받기 전까지는 서버에서 음성 인식을 해요.")
        }
    }

    /// The sidebar row for a phase; nil hides it (nothing to fetch, or finished: the done mark takes over).
    static func sidebar(_ phase: LocalSTT.Phase) -> VoiceRow? {
        switch phase {
        case .downloading(let done, let total):
            let f = total > 0 ? min(1, Double(done) / Double(total)) : 0
            return VoiceRow(title: downloadingTitle, percent: "\(percent(done: done, total: total))%", fraction: f,
                            detail: String(localized: "\(mb(done))MB / \(mb(total))MB · 다 받으면 온디바이스 음성 인식으로 바뀌어요"), failed: false)
        case .waiting:
            return VoiceRow(title: waitingTitle, percent: nil, fraction: nil, detail: String(localized: "인터넷에 연결되면 이어서 받아요"), failed: false)
        case .failed(let why):
            // LocalSTT's own fallback text repeats the title: say what to do instead. Both sides go through the same
            // catalog key, so the check holds in every language.
            return VoiceRow(title: failedTitle, percent: nil, fraction: nil,
                            detail: why == unknownFailure ? String(localized: "연결을 확인하고 다시 받아 주세요") : why, failed: true)
        case .off, .unsupported, .needsSpace, .ready:
            return nil
        }
    }

    /// The line under 화면 위 Tacie when a meeting starts or goes on while the model is not ready.
    static func meetingLine(_ phase: LocalSTT.Phase) -> String? {
        switch phase {
        case .downloading(let done, let total): String(localized: "\(meetingTitle) · 음성 인식 모델 \(percent(done: done, total: total))%")
        case .waiting: String(localized: "\(meetingTitle) · 음성 인식 모델 받기 대기 중")
        case .failed: "\(meetingTitle) · \(failedTitle)"
        case .off, .unsupported, .needsSpace, .ready: nil
        }
    }

    /// Everything above that a test must keep free of the words the product does not use.
    static var allStatic: [String] {
        [title, rowTitle, serverChoice, deviceChoice, serverDetail, readyDetail, unsupportedDetail, waitingDetail, failedDetail,
         sentLocal, sentServer,
         consentTitle, consentBody, consentButton, spaceTitle, spaceButton, spaceBody(more: 400_000_000),
         deleteRow, deleteDesc, deleteTitle, deleteBody, deleteButton, downloadingTitle, waitingTitle, failedTitle,
         doneText, retry, meetingTitle, paused, onboardingSection, onboardingSectionSub, defaultTag, serverName, serverLine,
         deviceLine, onboardingFoot, sceneServer, sceneDevice] + serverPros + serverCons + deviceCons
    }
}

/// What a meeting needs to know to say the model is not ready: the line, and how many downloads have started
/// (a new one during the meeting earns the line once more).
struct STTMeetingState: Equatable {
    let line: String
    let starts: Int
}

/// The once-per-meeting line (docs/22 4-4): shown for the first server-recognized utterance of a meeting, and once
/// more after a download starts again inside the same meeting. Never in the onboarding's practice.
struct STTMeetingNotice {
    private var shownAtStart: Int?

    mutating func reset() { shownAtStart = nil }

    /// The line to show for this utterance, or nil.
    mutating func note(stt: String?, practice: Bool, state: STTMeetingState?) -> String? {
        guard stt == "server", !practice, let state else { return nil }
        if shownAtStart == state.starts { return nil }
        shownAtStart = state.starts
        return state.line
    }
}
