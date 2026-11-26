import XCTest
@testable import Tacie

final class FeedbackSurveyTests: XCTestCase {
    private func meeting(ended: Bool = true, notes: SavedMeeting.Notes = .done) -> SavedMeeting {
        var m = SavedMeeting(mode: "remote", start: Date())
        if ended { m.end = Date() }
        m.notes = notes
        return m
    }

    private func show(_ r: FeedbackSurvey.Record = .init(), notes: Int = 3, page: SavedMeeting? = nil, running: Bool = false,
                      covered: Bool = false) -> Bool {
        FeedbackSurvey.shouldShow(record: r, notes: notes, page: page ?? meeting(), running: running, covered: covered)
    }

    func testOnlyMeetingsThatEndedWithNotesCount() {
        let ms = [meeting(), meeting(), meeting(ended: false), meeting(notes: .failed), meeting(notes: .waiting), meeting(notes: .off)]
        XCTAssertEqual(FeedbackSurvey.notesCount(ms), 2)
    }

    func testComesUpOverAFinishedNotesPageAfterThreeNotes() {
        XCTAssertTrue(show())
        XCTAssertFalse(show(notes: 2))
        XCTAssertFalse(show(page: meeting(ended: false)))
        XCTAssertFalse(show(page: meeting(notes: .waiting)))
        XCTAssertFalse(FeedbackSurvey.shouldShow(record: .init(), notes: 5, page: nil, running: false, covered: false))
    }

    func testNeverDuringAMeetingOrUnderSettings() {
        XCTAssertFalse(show(running: true))
        XCTAssertFalse(show(covered: true))
    }

    func testOnceAnsweredNeverAgainAndLeftAloneAtMostTwice() {
        XCTAssertFalse(show(.init(shows: 1, finished: true)))
        XCTAssertTrue(show(.init(shows: 1, finished: false)))
        XCTAssertFalse(show(.init(shows: 2, finished: false)))
    }

    func testTheRecordSurvivesALaunch() {
        let d = UserDefaults(suiteName: "FeedbackSurveyTests")!
        d.removePersistentDomain(forName: "FeedbackSurveyTests")
        XCTAssertEqual(FeedbackSurvey.load(d), .init())
        FeedbackSurvey.save(.init(shows: 1, finished: true), d)
        XCTAssertEqual(FeedbackSurvey.load(d), .init(shows: 1, finished: true))
    }

    func testTheLineIsTrimmedAndCut() {
        XCTAssertEqual(FeedbackSurvey.cleaned("  슬랙도 \n"), "슬랙도")
        XCTAssertEqual(FeedbackSurvey.cleaned(String(repeating: "가", count: 400)).count, 300)
    }

    func testTheWishesMatchTheServerAndSendOnlyFlags() {
        // apps/server/src/feedback.js WISHES
        XCTAssertEqual(FeedbackSurvey.Wish.allCases.map(\.rawValue), ["faster", "accurate", "catch", "sources", "notes", "todos"])
        let p = AnalyticsEvent.feedbackSent(wishes: [.catch], wrote: true).properties
        XCTAssertEqual(p["want_catch"], .bool(true))
        XCTAssertEqual(p["want_faster"], .bool(false))
        XCTAssertEqual(p["wrote"], .bool(true))
        XCTAssertEqual(p.count, 7)
    }

    func testTheCardGoesToTheServerUnderTheSession() async throws {
        final class Box: @unchecked Sendable { var asked: [URLRequest] = [] }
        let box = Box()
        var s = AccountServer()
        s.send = { req in box.asked.append(req); return (Data("{}".utf8), 200) }
        try await s.feedback(token: "tok", tags: ["faster", "todos"], text: "슬랙도")
        let req = try XCTUnwrap(box.asked.first)
        XCTAssertEqual(req.url?.path, "/feedback")
        XCTAssertEqual(req.httpMethod, "POST")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer tok")
        let body = try JSONSerialization.jsonObject(with: req.httpBody ?? Data()) as? [String: String]
        XCTAssertEqual(body, ["tags": "faster,todos", "text": "슬랙도"])
        s.send = { _ in (Data(), 401) }
        do { try await s.feedback(token: "tok", tags: [], text: "x"); XCTFail() } catch AccountError.signedOut {}
    }
}
