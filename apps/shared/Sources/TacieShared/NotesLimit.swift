import Foundation

/// The month's Tacie AI ran out (the relay's `credit`, docs/12) on this meeting. Kept with the meeting so its page can
/// say which happened and when the notes can be written; read only while `notes == .failed`.
enum NotesLimit: String, Codable {
    /// The Mac ended the meeting at the limit (`MeetingModel.stopForLimit`); what was said until then is kept.
    case stopped
    /// The meeting ran to its end; asking for its notes was refused.
    case notes
}

/// The account's month as the relay counts it (UTC), for the usage meter and the limit block on a meeting page.
enum UsageMonth {
    /// The first moment of next month (the server counts months in UTC).
    static func nextMonth(_ now: Date) -> Date {
        var utc = Calendar(identifier: .gregorian)
        utc.timeZone = TimeZone(identifier: "UTC")!
        let start = utc.date(from: utc.dateComponents([.year, .month], from: now))!
        return utc.date(byAdding: .month, value: 1, to: start)!
    }

    /// How long until the monthly usage resets, the bold part of "12일 뒤에 자동으로 초기화돼요".
    static func untilReset(_ t: TimeInterval) -> String {
        let hours = max(1, Int((t / 3600).rounded(.up)))
        let days = Int((Double(hours) / 24).rounded(.up))
        return hours < 24 ? String(localized: "usage.resetHours", defaultValue: "\(hours)시간")
            : String(localized: "usage.resetDays", defaultValue: "\(days)일")
    }

    /// From now: "14일".
    static func untilReset(now: Date = Date()) -> String { untilReset(nextMonth(now).timeIntervalSince(now)) }
}

/// The words of the limit block, the same on the Mac and the iPhone.
enum NotesLimitText {
    static let kicker = String(localized: "limit.kicker", defaultValue: "이번 달 사용량")

    static func title(_ limit: NotesLimit, ready: Bool) -> String {
        if ready { return String(localized: "limit.readyTitle", defaultValue: "이번 달 사용량이 초기화됐어요") }
        return limit == .stopped
            ? String(localized: "limit.stoppedTitle", defaultValue: "이번 달 AI 사용량을 다 써서 회의를 끝냈어요")
            : String(localized: "limit.notesTitle", defaultValue: "이번 달 AI 사용량을 다 써서 요약을 쓰지 못했어요")
    }

    static func detail(_ limit: NotesLimit, ready: Bool, recorded: Bool, until: String) -> String {
        if ready { return String(localized: "limit.readyDetail", defaultValue: "사용량을 다 써서 쓰지 못한 이 회의 요약을 이제 쓸 수 있어요.") }
        switch (limit, recorded) {
        case (.stopped, true):
            return String(localized: "limit.stoppedRecorded", defaultValue: "회의가 끝나기 전까지의 대화 기록과 녹음은 저장했어요. \(until) 뒤에 사용량이 초기화되면 이 회의 요약을 쓸 수 있어요.")
        case (.stopped, false):
            return String(localized: "limit.stopped", defaultValue: "회의가 끝나기 전까지의 대화 기록은 저장했어요. \(until) 뒤에 사용량이 초기화되면 이 회의 요약을 쓸 수 있어요.")
        case (.notes, true):
            return String(localized: "limit.notesRecorded", defaultValue: "대화 기록과 녹음은 저장했어요. \(until) 뒤에 사용량이 초기화되면 이 회의 요약을 쓸 수 있어요.")
        case (.notes, false):
            return String(localized: "limit.notes", defaultValue: "대화 기록은 저장했어요. \(until) 뒤에 사용량이 초기화되면 이 회의 요약을 쓸 수 있어요.")
        }
    }

    /// "0% 남음"
    static func left(_ percent: Int) -> String { String(localized: "limit.left", defaultValue: "\(percent)% 남음") }

    /// Under the bar: "14일 뒤에 다시 쓸 수 있어요" while used up, "31일 뒤에 자동으로 초기화돼요" after.
    static func reset(_ until: String, out: Bool) -> String {
        out ? String(localized: "limit.resetOut", defaultValue: "\(until) 뒤에 다시 쓸 수 있어요")
            : String(localized: "limit.resetIn", defaultValue: "\(until) 뒤에 자동으로 초기화돼요")
    }

    /// The kept parts: "대화 기록 42분".
    static func transcript(minutes: Int) -> String { String(localized: "limit.keptTranscript", defaultValue: "대화 기록 \(minutes)분") }
    static let recording = String(localized: "limit.keptRecording", defaultValue: "녹음")
    static let write = String(localized: "limit.write", defaultValue: "요약 쓰기")
    static let usage = String(localized: "limit.usage", defaultValue: "설정에서 사용량 보기")
}

extension SavedMeeting {
    /// The minutes this meeting ran, for "대화 기록 42분".
    var limitMinutes: Int { max(1, Int(((end ?? start).timeIntervalSince(start) / 60).rounded())) }
}
