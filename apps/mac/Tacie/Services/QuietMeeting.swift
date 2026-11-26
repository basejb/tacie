import Foundation

/// An in-person meeting that has transcribed nothing for `after` seconds says so on the pill. In person only the
/// microphone is heard, so a video or call playing in earphones leaves the meeting empty; a user once ended 17 minutes
/// with no line and a blank page, and nothing had told them.
struct QuietMeeting {
    /// Two minutes: a real meeting rarely goes that long without a word, and the user still has most of it ahead.
    static let after: TimeInterval = 120
    static let line = String(localized: "말소리가 안 들려요")
    static let help = String(localized: "대면 회의는 마이크 소리만 들어요. 컴퓨터에서 나는 소리(회의 앱, 영상)는 원격 회의로 시작해야 들려요. 말이 들리면 이 줄이 사라져요.")

    /// Nobody said a word this long, in person or remote: the meeting is over, and the end countdown starts
    /// (`MeetingModel.proposeEnd`). In the AMI meeting corpus (171 meetings, 97 h) the longest silence inside a meeting
    /// was 138 s and none reached 180 s; the replayed remote meeting here, 127 s (docs/04 "말이 끊긴 회의 끝").
    static let endAfter: TimeInterval = 180

    /// When the last line was heard, or the meeting started, or it was resumed.
    private(set) var lastHeard: Date
    /// When the last line with words was heard: sound marks ("[효과음]") are not talk. A meeting left running overnight
    /// filled with them for 11 hours.
    private(set) var lastSpoke: Date

    init(start: Date = Date()) {
        lastHeard = start
        lastSpoke = start
    }

    /// A line heard. `text` nil: not a line but a fresh start (resumed, the countdown kept going).
    mutating func heard(at date: Date, text: String? = nil) {
        lastHeard = date
        if text.map(Self.isSpeech) ?? true { lastSpoke = date }
    }

    func silent(now: Date) -> Bool { now.timeIntervalSince(lastHeard) >= Self.after }

    func over(now: Date) -> Bool { now.timeIntervalSince(lastSpoke) >= Self.endAfter }

    /// Words left once the sound marks in brackets are taken out.
    static func isSpeech(_ text: String) -> Bool {
        let bare = text.replacingOccurrences(of: #"\[[^\]]*\]"#, with: "", options: .regularExpression)
        return bare.contains { $0.isLetter || $0.isNumber }
    }

    /// Under "요약" on the page of a meeting that ended with no line: why, and that the recording is still there.
    static func emptyPage(inPerson: Bool, recorded: Bool) -> String {
        let why = inPerson
            ? String(localized: "대면 회의는 마이크 소리만 들어요. 이어폰으로 듣던 소리나 컴퓨터에서 나는 소리는 원격 회의로 시작해야 들려요.")
            : String(localized: "회의 앱 소리와 마이크 어느 쪽에서도 말소리를 듣지 못했어요.")
        return recorded ? why + String(localized: " 녹음은 남아 있어서 아래에서 들어 볼 수 있어요.") : why
    }
}
