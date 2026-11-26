import XCTest
@testable import Tacie

final class MeetingStatsTests: XCTestCase {
    private var cal: Calendar {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "Asia/Seoul")!
        return c
    }

    private func date(_ month: Int, _ d: Int, _ h: Int, _ m: Int = 0) -> Date {
        cal.date(from: DateComponents(year: 2026, month: month, day: d, hour: h, minute: m))!
    }

    private func question(_ id: String, unanswered: Bool) -> SavedQuestion {
        SavedQuestion(id: id, question: "q", asker: nil, at: date(10, 1, 10), answer: nil, cards: [], cited: [], unanswered: unanswered)
    }

    private func meeting(_ start: Date, _ end: Date?, _ qs: [SavedQuestion] = []) -> SavedMeeting {
        var m = SavedMeeting(mode: "inperson", start: start)
        m.end = end
        m.questions = qs
        return m
    }

    func testMonthCountsOnlyThisMonth() {
        let meetings = [
            meeting(date(10, 1, 10), date(10, 1, 11, 30), [question("a", unanswered: false), question("b", unanswered: true)]),
            meeting(date(10, 23, 14), date(10, 23, 15, 40), [question("c", unanswered: false)]),
            meeting(date(9, 30, 23), date(10, 1, 0, 30), [question("d", unanswered: false)]),
            meeting(date(10, 23, 16), nil),
        ]
        let r = MeetingStats.month(meetings, now: date(10, 23, 16, 30), calendar: cal)
        XCTAssertEqual(r.meetings, 2)
        XCTAssertEqual(r.listened, 5_400 + 6_000)
        XCTAssertEqual(r.answers, 2)
    }

    // MARK: usage numbers (docs/16)

    private func card(_ id: String, _ source: String?) -> Card {
        Card(id: id, path: "a.ts", kind: "code", source: source, byline: nil, url: nil, lineStart: 1, lineEnd: 2, title: "a",
             snippet: "", score: nil, githubUrl: nil, localPath: nil, chunk: nil, repeat: nil)
    }

    func testEndedFromNilMeetingIsZeroes() {
        let e = MeetingStats.ended(nil, mode: .inPerson, practice: true, startedAt: date(10, 1, 10), now: date(10, 1, 10, 1),
                                   sttLocal: 0, sttStream: 0, reason: .user)
        XCTAssertEqual(e, .meetingEnded(mode: .inPerson, practice: true, minutes: 1, utterances: 0, questions: 0, answered: 0,
                                        unanswered: 0, cards: 0, speakers: 0, namedSpeakers: 0, sttLocal: 0, sttStream: 0, reason: .user))
    }

    func testEndedCountsQuestionsCardsUnanswered() {
        var m = meeting(date(10, 1, 10), nil)
        m.transcript = [TranscriptLine(key: "e0:s1u1", at: date(10, 1, 10), speaker: nil, text: "x", question: true)]
        m.questions = [
            SavedQuestion(id: "a", question: "q", asker: nil, at: date(10, 1, 10), answer: nil,
                          cards: [card("1", "repo"), card("2", "github"), card("3", nil)], cited: [], unanswered: false),
            SavedQuestion(id: "b", question: "q", asker: nil, at: date(10, 1, 10), answer: nil, cards: [], cited: [], unanswered: true),
        ]
        let e = MeetingStats.ended(m, mode: .remote, practice: false, startedAt: date(10, 1, 10), now: date(10, 1, 10, 30),
                                   sttLocal: 3, sttStream: 0, reason: .heardEnd)
        XCTAssertEqual(e, .meetingEnded(mode: .remote, practice: false, minutes: 30, utterances: 1, questions: 2, answered: 1,
                                        unanswered: 1, cards: 3, speakers: 0, namedSpeakers: 0, sttLocal: 3, sttStream: 0, reason: .heardEnd))
    }

    func testSpeakersCountMergedOnceAndNamedOnes() {
        var m = meeting(date(10, 1, 10), nil)
        m.transcript = ["화자 1", "화자 2", "화자 3", "화자 1"].enumerated().map { i, v in
            TranscriptLine(key: "e0:s1u\(i)", at: date(10, 1, 10), speaker: nil, text: "말", question: false, voice: v)
        }
        m.voices = [VoiceEntry(key: "화자 1", name: "민지", person: nil, same: nil),
                    VoiceEntry(key: "화자 3", name: nil, person: nil, same: "화자 1"),
                    VoiceEntry(key: "화자 2", name: nil, person: nil, same: nil)]
        XCTAssertEqual(MeetingStats.speakers(m).all, 2)
        XCTAssertEqual(MeetingStats.speakers(m).named, 1)
        XCTAssertEqual(MeetingStats.speakers(nil).all, 0)
    }

    func testErrorOnceSendsEachKindOncePerMeeting() {
        var once = ErrorOnce()
        XCTAssertEqual([once.shouldSend(.network), once.shouldSend(.network), once.shouldSend(.key)], [true, false, true])
        once.reset()
        XCTAssertTrue(once.shouldSend(.network))
    }

    func testAnswerShownCountsCardsBySource() {
        let e = MeetingStats.answerShown(origin: .voice, cards: [card("1", "repo"), card("2", "repo"), card("3", "notion"), card("4", nil), card("5", "linear")],
                                         lowConfidence: true, hasAnswerLine: false, practice: false)
        XCTAssertEqual(e, .answerShown(origin: .voice, cards: 5, repo: 3, github: 0, slack: 0, notion: 1, linear: 1, gdrive: 0, jira: 0, confluence: 0, lowConfidence: true,
                                       hasAnswerLine: false, practice: false))
    }

    func testAnswerShownCountsAsanaCards() {
        let e = MeetingStats.answerShown(origin: .voice, cards: [card("1", "asana"), card("2", "asana"), card("3", "jira")],
                                         lowConfidence: false, hasAnswerLine: true, practice: false)
        XCTAssertEqual(e, .answerShown(origin: .voice, cards: 3, repo: 0, github: 0, slack: 0, notion: 0, linear: 0, gdrive: 0, jira: 1,
                                       confluence: 0, asana: 2, lowConfidence: false, hasAnswerLine: true, practice: false))
    }

    func testAnswerShownCountsDropboxCards() {
        let e = MeetingStats.answerShown(origin: .voice, cards: [card("1", "dropbox"), card("2", "dropbox"), card("3", "asana")],
                                         lowConfidence: false, hasAnswerLine: true, practice: false)
        XCTAssertEqual(e, .answerShown(origin: .voice, cards: 3, repo: 0, github: 0, slack: 0, notion: 0, linear: 0, gdrive: 0, jira: 0,
                                       confluence: 0, asana: 1, dropbox: 2, lowConfidence: false, hasAnswerLine: true, practice: false))
    }

    func testAnswerShownCountsJiraAndConfluenceCards() {
        let e = MeetingStats.answerShown(origin: .typed, cards: [card("1", "jira"), card("2", "confluence"), card("3", "jira"), card("4", "gdrive")],
                                         lowConfidence: false, hasAnswerLine: true, practice: false)
        XCTAssertEqual(e, .answerShown(origin: .typed, cards: 4, repo: 0, github: 0, slack: 0, notion: 0, linear: 0, gdrive: 1, jira: 2,
                                       confluence: 1, lowConfidence: false, hasAnswerLine: true, practice: false))
    }
}
