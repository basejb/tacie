import Foundation

enum MeetingStats {
    /// This month on this Mac: meetings that ended, time listened (end - start of meetings that started this month;
    /// one still going counts 0) and questions that found an answer (!unanswered).
    static func month(_ meetings: [SavedMeeting], now: Date, calendar: Calendar) -> (meetings: Int, listened: TimeInterval, answers: Int) {
        var count = 0
        var listened: TimeInterval = 0
        var answers = 0
        for m in meetings where calendar.isDate(m.start, equalTo: now, toGranularity: .month) {
            if let end = m.end {
                count += 1
                listened += max(0, end.timeIntervalSince(m.start))
            }
            answers += m.questions.filter { !$0.unanswered }.count
        }
        return (count, listened, answers)
    }

    // MARK: usage numbers (docs/16)

    /// `meeting_ended` from the meeting as it stood when it stopped (nil: nothing was kept, the counts are zero).
    static func ended(_ m: SavedMeeting?, mode: AMode, practice: Bool, startedAt: Date, now: Date, sttLocal: Int, sttStream: Int = 0, reason: AEndReason, by: AStopBy = .auto) -> AnalyticsEvent {
        let qs = m?.questions ?? []
        let open = qs.filter(\.unanswered).count
        return .meetingEnded(mode: mode, practice: practice, minutes: max(0, now.timeIntervalSince(startedAt)) / 60,
                             utterances: m?.transcript.count ?? 0, questions: qs.count, answered: qs.count - open,
                             unanswered: open, cards: qs.reduce(0) { $0 + $1.cards.count }, speakers: speakers(m).all,
                             namedSpeakers: speakers(m).named, sttLocal: sttLocal, sttStream: sttStream, reason: reason, by: by,
                             eventLinked: m?.event != nil, invited: m?.invited?.count ?? 0)
    }

    /// Speakers heard in the meeting (merged ones count once) and how many of them carry a name, typed or remembered.
    static func speakers(_ m: SavedMeeting?) -> (all: Int, named: Int) {
        guard let m else { return (0, 0) }
        let keys = Set(m.attendees.map(\.key))
        let named = (m.voices ?? []).filter { keys.contains($0.key) && $0.same == nil && $0.name != nil }.count
        return (keys.count, named + keys.filter(SavedMeeting.isLinePerson).count)
    }

    /// `answer_shown`: how many cards, by where they came from (a card without a source is from the repository).
    static func answerShown(origin: AOrigin, cards: [Card], lowConfidence: Bool, hasAnswerLine: Bool, practice: Bool) -> AnalyticsEvent {
        let n = { (s: String) in cards.filter { ($0.source ?? "repo") == s }.count }
        return .answerShown(origin: origin, cards: cards.count, repo: n("repo"), github: n("github"), slack: n("slack"),
                            notion: n("notion"), linear: n("linear"), gdrive: n("gdrive"), jira: n("jira"),
                            confluence: n("confluence"), asana: n("asana"), dropbox: n("dropbox"), notes: n("notes"), lowConfidence: lowConfidence, hasAnswerLine: hasAnswerLine, practice: practice)
    }
}

/// `error_shown` once per kind per meeting: the same failure on every utterance is one problem, not twenty.
struct ErrorOnce {
    private var sent: Set<AErrorKind> = []
    mutating func shouldSend(_ k: AErrorKind) -> Bool { sent.insert(k).inserted }
    mutating func reset() { sent = [] }
}
