import XCTest
@testable import Tacie

/// Which unanswered questions to search again once a new source is in (docs/11 14절).
final class ResearchRulesTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func info(_ extra: String = "", partial: Bool = false) -> IndexState {
        let json = #"{"chunks": 120, "commit": null, "built_at": "2026-11-03T00:00:00Z", "repo": "a/b", "#
            + #""refresh": {"state": "done", "step": null, "embedded": null, "error": null, "error_kind": null, "warnings": null, "partial": \#(partial)}"#
            + extra + "}"
        return try! EngineClient.engineDecoder.decode(IndexState.self, from: Data(json.utf8))
    }

    private func q(unanswered: Bool = true, noSource: Bool? = nil, searchedWith: [String]? = nil) -> SavedQuestion {
        var s = SavedQuestion(id: "q", question: "왜요?", asker: nil, at: now, answer: nil, cards: [], cited: [], unanswered: unanswered,
                              noSource: noSource)
        s.searchedWith = searchedWith
        return s
    }

    /// Past meetings are searched too (docs/30) but are not a new source: they never search kept questions again.
    func testMeetingsAreNotASourceKey() {
        let withMeetings = info(#", "meeting": {"meetings": 3, "chunks": 3}"#)
        XCTAssertEqual(ResearchRules.keys(withMeetings, building: false), ["github:a/b"])
    }

    func testKeysWaitForCompleteSources() {
        XCTAssertEqual(ResearchRules.keys(info(), building: false), ["github:a/b"])
        XCTAssertNil(ResearchRules.keys(info(), building: true), "a sync is running")
        XCTAssertNil(ResearchRules.keys(info(partial: true), building: false), "only part is in")
        let notionFetching = info(#", "notion": {"connected": true, "workspace": "ws", "pages": null}"#)
        XCTAssertEqual(ResearchRules.keys(notionFetching, building: false), ["github:a/b"])
        let notionIn = info(#", "notion": {"connected": true, "workspace": "ws", "pages": 30}"#)
        XCTAssertEqual(ResearchRules.keys(notionIn, building: false), ["github:a/b", "notion:ws"])
        let driveHalf = info(#", "gdrive": {"connected": true, "account": "me@x", "files": 10, "initial_done": false}"#)
        XCTAssertEqual(ResearchRules.keys(driveHalf, building: false), ["github:a/b"])
        let driveIn = info(#", "gdrive": {"connected": true, "account": "me@x", "files": 10, "initial_done": true}"#)
        XCTAssertEqual(ResearchRules.keys(driveIn, building: false), ["github:a/b", "gdrive:me@x"])
        let linearIn = info(#", "linear": {"connected": true, "workspace": "acme", "issues": 4}"#)
        XCTAssertEqual(ResearchRules.keys(linearIn, building: false), ["github:a/b", "linear:acme"])
    }

    /// Asana counts once the first fetch is in, keyed by the Asana user (docs/32).
    func testAsanaKeyWaitsForTheFirstFetch() {
        func at(_ fields: String) -> IndexState { info(#", "asana": {"connected": true, \#(fields)}"#) }
        let half = at(#""tasks": 100, "initial_done": false"#)
        XCTAssertEqual(ResearchRules.keys(half, building: false, asanaUser: "u-1"), ["github:a/b"])
        let none = at(#""tasks": null, "initial_done": null"#)
        XCTAssertEqual(ResearchRules.keys(none, building: false, asanaUser: "u-1"), ["github:a/b"])
        let done = at(#""tasks": 100, "initial_done": true"#)
        XCTAssertEqual(ResearchRules.keys(done, building: false, asanaUser: "u-1"), ["github:a/b", "asana:u-1"])
        let off = info(#", "asana": {"connected": false, "refused": false}"#)
        XCTAssertEqual(ResearchRules.keys(off, building: false, asanaUser: nil), ["github:a/b"])
    }

    /// Dropbox counts once the first fetch is in, keyed by the saved connection's account id (not the engine's email).
    func testDropboxKeyOnlyWhenTheFirstFetchIsIn() {
        func at(_ fields: String) -> IndexState {
            info(#", "dropbox": {"connected": true, "account": "me@example.com", \#(fields)}"#)
        }
        XCTAssertEqual(ResearchRules.keys(at(#""files": 5, "initial_done": true"#), building: false, dropboxAccount: "dbid:A1"),
                       ["github:a/b", "dropbox:dbid:A1"])
        XCTAssertEqual(ResearchRules.keys(at(#""files": 5, "initial_done": false"#), building: false, dropboxAccount: "dbid:A1"),
                       ["github:a/b"])
    }

    /// Jira · Confluence counts once the first fetch is in, keyed by the site's cloud id (docs/31).
    func testAtlassianKeyWaitsForTheFirstFetch() {
        func at(_ fields: String) -> IndexState { info(#", "atlassian": {"connected": true, "site": "acme", \#(fields)}"#) }
        let half = at(#""issues": 100, "pages": 3, "initial_done": false"#)
        XCTAssertEqual(ResearchRules.keys(half, building: false, atlassianCloud: "c-1"), ["github:a/b"])
        let none = at(#""issues": null, "pages": null, "initial_done": null"#)
        XCTAssertEqual(ResearchRules.keys(none, building: false, atlassianCloud: "c-1"), ["github:a/b"])
        let done = at(#""issues": 100, "pages": 3, "initial_done": true"#)
        XCTAssertEqual(ResearchRules.keys(done, building: false, atlassianCloud: "c-1"), ["github:a/b", "atlassian:c-1"])
        let off = info(#", "atlassian": {"connected": false, "refused": false}"#)
        XCTAssertEqual(ResearchRules.keys(off, building: false, atlassianCloud: nil), ["github:a/b"])
    }

    func testSameSourceNeverTwice() {
        let old = q(searchedWith: ["github:a/b"])
        XCTAssertEqual(ResearchRules.due(old, meetingStart: now, keys: ["github:a/b"], now: now), .skip)
        XCTAssertEqual(ResearchRules.due(old, meetingStart: now, keys: ["github:a/b", "notion:ws"], now: now), .search)
        XCTAssertEqual(ResearchRules.due(q(searchedWith: ["github:a/b", "notion:ws"]), meetingStart: now,
                                         keys: ["github:a/b", "notion:ws"], now: now), .skip)
    }

    func testRepoSwitchIsNew() {
        XCTAssertEqual(ResearchRules.due(q(searchedWith: ["github:a/b"]), meetingStart: now, keys: ["github:a/c"], now: now), .search)
    }

    func testLegacyWindow() {
        let day: TimeInterval = 86_400
        XCTAssertEqual(ResearchRules.due(q(), meetingStart: now - 10 * day, keys: ["github:a/b"], now: now), .search)
        XCTAssertEqual(ResearchRules.due(q(), meetingStart: now - 40 * day, keys: ["github:a/b"], now: now), .stamp)
        XCTAssertEqual(ResearchRules.legacyCap, 20)
    }

    func testAnsweredSkips() {
        XCTAssertEqual(ResearchRules.due(q(unanswered: false), meetingStart: now, keys: ["github:a/b"], now: now), .skip)
    }

    func testNoSourceSearchesWhenAnyKey() {
        let kept = q(noSource: true)
        XCTAssertEqual(ResearchRules.due(kept, meetingStart: now, keys: ["notion:ws"], now: now), .search)
        XCTAssertEqual(ResearchRules.due(kept, meetingStart: now, keys: [], now: now), .skip)
    }

    func testSavedWhileSyncingKeepsTheLastCompleteSources() {
        // a meeting saved while a sync runs is not a legacy record: it was searched with the last complete sources
        XCTAssertEqual(ResearchRules.stampKeys(current: nil, last: ["github:a/b"]), ["github:a/b"])
        XCTAssertEqual(ResearchRules.stampKeys(current: ["github:a/b", "notion:ws"], last: ["github:a/b"]), ["github:a/b", "notion:ws"])
        XCTAssertEqual(ResearchRules.stampKeys(current: nil, last: []), [])
    }

    func testAFailedSearchKeepsTheQuestionsChance() {
        XCTAssertTrue(ResearchRules.keepsResult(errorKind: nil, answerTried: true, answerGot: true))
        XCTAssertTrue(ResearchRules.keepsResult(errorKind: nil, answerTried: false, answerGot: false), "nothing found is a result")
        XCTAssertFalse(ResearchRules.keepsResult(errorKind: "credit", answerTried: false, answerGot: false))
        XCTAssertFalse(ResearchRules.keepsResult(errorKind: nil, answerTried: true, answerGot: false), "the answer call failed")
    }

    func testStampAddsToWhatWasSeen() {
        XCTAssertEqual(ResearchRules.merged(["github:a/b", "notion:ws"], ["github:a/b"]), ["github:a/b", "notion:ws"])
        XCTAssertEqual(ResearchRules.merged(nil, ["linear:x"]), ["linear:x"])
    }

    func testOldRecordsStillDecode() throws {
        let old = Data(#"{"id":"q","question":"x","at":0,"cards":[],"cited":[],"unanswered":true}"#.utf8)
        XCTAssertNil(try JSONDecoder().decode(SavedQuestion.self, from: old).searchedWith)
    }
}
