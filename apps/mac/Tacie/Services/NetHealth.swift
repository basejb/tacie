import Foundation

/// How the connection looks during a meeting, read from what the meeting itself does: the Mac's network path and how
/// long each utterance takes to come back from speech recognition. The pill shows it as small signal bars beside the
/// time, only when something is wrong (slow: one amber bar; offline: a red slash).
///
/// Before this a dropped connection showed only after two utterances in a row failed, and the engine waits 20 s for
/// the server before it calls an utterance failed: a slow or lost connection could go 40 s and more with nothing on
/// the pill.
struct NetHealth {
    enum Level: String, Equatable { case ok, slow, offline }

    /// The path has to stay down this long before the pill says offline: a Wi-Fi hand-over drops it for a moment.
    static let offlineAfter: TimeInterval = 3
    /// An utterance that takes longer than this to come back is slow; `slowRuns` of them in a row make the meeting slow.
    static let slowSeconds: TimeInterval = 6
    static let slowRuns = 2
    /// An utterance still waiting this long is slow already (a hanging request says nothing until it fails).
    static let stuckSeconds: TimeInterval = 8
    /// This many utterances waiting at once: they are piling up.
    static let backlog = 3
    /// `fastRuns` utterances in a row back within `fastSeconds` clear slow.
    static let fastSeconds: TimeInterval = 3
    static let fastRuns = 3

    /// When the network path went down, nil while it is up.
    private var pathDownSince: Date?
    /// Utterances in a row that failed for the network (`network`); two make the meeting offline even while the path
    /// says it is up (one is a hiccup, as before).
    private var failures = 0
    private var slow = false
    private var slowStreak = 0
    private var fastStreak = 0

    mutating func path(online: Bool, at date: Date) {
        if online { pathDownSince = nil } else if pathDownSince == nil { pathDownSince = date }
    }

    /// An utterance came back after `seconds`. `network`: it failed for the network (no connection, or the server
    /// did not answer in time).
    mutating func heard(seconds: TimeInterval, network: Bool) {
        if network {
            failures += 1
            return
        }
        failures = 0
        if seconds > Self.slowSeconds {
            fastStreak = 0
            slowStreak += 1
            if slowStreak >= Self.slowRuns { slow = true }
        } else {
            slowStreak = 0
            if seconds <= Self.fastSeconds { fastStreak += 1 } else { fastStreak = 0 }
            if fastStreak >= Self.fastRuns { slow = false }
        }
    }

    /// `waiting`: when each utterance still waiting for speech recognition was sent.
    func level(now: Date, waiting: [Date]) -> Level {
        if let down = pathDownSince, now.timeIntervalSince(down) >= Self.offlineAfter { return .offline }
        if failures >= 2 { return .offline }
        if slow || waiting.count >= Self.backlog { return .slow }
        if waiting.contains(where: { now.timeIntervalSince($0) >= Self.stuckSeconds }) { return .slow }
        return .ok
    }

    /// How long the connection has been down, for the bars' help.
    func downFor(now: Date) -> TimeInterval? { pathDownSince.map { now.timeIntervalSince($0) } }

    // MARK: words

    /// The line under the pill the first time a meeting goes offline. `recorded`: the meeting keeps a recording.
    /// `localSTT`: this Mac recognizes speech itself, so the transcript goes on.
    static func notice(recorded: Bool, localSTT: Bool) -> String {
        if localSTT { return String(localized: "인터넷이 끊겼어요 · 받아쓰기는 계속돼요") }
        return recorded ? String(localized: "인터넷이 끊겼어요 · 녹음은 계속돼요") : String(localized: "인터넷이 끊겼어요 · 이 동안은 받아쓰지 못해요")
    }

    /// The bars' help.
    static func help(_ level: Level, recorded: Bool, localSTT: Bool, downFor: TimeInterval?) -> String {
        switch level {
        case .ok:
            return ""
        case .slow:
            return String(localized: "연결이 느려요. 계속 듣고 있어요. 받아쓰기와 답이 조금 늦게 올 수 있어요.")
        case .offline:
            let head = downFor.map { String(localized: "인터넷이 끊긴 지 \(Self.clock($0))예요.") } ?? String(localized: "인터넷이 끊겼어요.")
            let body = localSTT
                ? String(localized: " 받아쓰기는 이 Mac에서 계속돼요. 질문에 답하는 건 연결된 뒤에 다시 돼요.")
                : recorded
                ? String(localized: " 녹음은 계속돼요. 끊긴 동안의 말은 회의 기록에 받아쓰지 못한 부분으로 남고, 녹음으로 들을 수 있어요.")
                : String(localized: " 끊긴 동안의 말은 받아쓰지 못해요. 연결되면 이어서 들어요.")
            return head + body
        }
    }

    static func clock(_ t: TimeInterval) -> String {
        let s = max(0, Int(t))
        return "\(s / 60):" + String(format: "%02d", s % 60)
    }
}
