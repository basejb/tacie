import Foundation

/// The meeting's end, heard rather than clicked: the engine's gate also asks Jev whether the speaker is closing the
/// meeting (`ending`, engine/crates/tacie-engine/src/jev.rs). The island then counts down and ends the meeting itself
/// unless someone keeps it going.
///
/// Anything said after the closing words keeps it going, not only a question (2026-11-09: people talked on after "let's
/// stop here" (여기까지 하죠), the meeting ended under them and the rest was never heard). Only closing words again and
/// backchannels ("yes", 네) let the countdown run on. Farewells cannot be told from talk by p_end: in the replay test
/// the closer's own announcement after the closing words read 0.44 and farewells 0.30–0.89 (docs/01 "회의 끝 뒤에 이어지는 말"),
/// and a meeting that has to be ended by hand loses nothing.
enum MeetingEnd {
    /// How long the island waits after "오늘은 여기까지 하죠" before it ends the meeting on its own.
    static let grace: TimeInterval = 10
    /// At the deadline someone may still be talking, or their words still be on their way back from STT: the meeting
    /// waits for them, this long at most (an utterance is cut at 15 s, STT takes a few more).
    static let holdMax: TimeInterval = 30

    /// Below this p_end a line said after the closing words keeps the meeting going: the engine's own bar for closing
    /// words (`TACIE_END_THR`). A lower bar ended a meeting on talk (0.44) in the replay test.
    static let talkBelow = 0.8

    /// A line heard while the countdown runs keeps the meeting going when it was said after the closing words
    /// (`cutAt` is when its utterance was cut), is not a backchannel and does not read as closing (`pEnd`). No p_end
    /// (the gate failed) counts as talk: losing the rest of a meeting is worse than ending it by hand.
    static func keepsGoing(cutAt: Date, closingFrom: Date, pEnd: Double?, backchannel: Bool) -> Bool {
        cutAt > closingFrom && !backchannel && (pEnd ?? 0) < talkBelow
    }

    /// At the deadline: wait while someone is talking or an utterance cut after the closing words is still being heard.
    static func holds(speaking: Bool, inFlight: [Date], closingFrom: Date, now: Date, deadline: Date) -> Bool {
        guard now < deadline.addingTimeInterval(holdMax) else { return false }
        return speaking || inFlight.contains { $0 > closingFrom }
    }

    /// Whole seconds until the deadline, rounded up so the line never shows 0 while the meeting still runs.
    static func secondsLeft(until deadline: Date, now: Date) -> Int {
        max(0, Int(deadline.timeIntervalSince(now).rounded(.up)))
    }

    /// The pill's line while the countdown runs.
    static func line(until deadline: Date, now: Date) -> String {
        String(localized: "회의를 끝낼까요? \(secondsLeft(until: deadline, now: now))초 뒤 정리해요")
    }
}
