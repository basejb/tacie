import XCTest
@testable import Tacie

final class TacieAccountTests: XCTestCase {
    private func server(_ answers: [(Int, String)]) -> (AccountServer, () -> [URLRequest]) {
        final class Box: @unchecked Sendable { var answers: [(Int, String)] = []; var asked: [URLRequest] = [] }
        let box = Box()
        box.answers = answers
        var s = AccountServer()
        s.send = { req in
            box.asked.append(req)
            let (status, body) = box.answers.removeFirst()
            return (Data(body.utf8), status)
        }
        return (s, { box.asked })
    }

    private let me = #"{"email":"a@b.com","name":"준","picture":null,"status":"waiting","month":{"spent_usd":0.5,"limit_usd":5}}"#

    func testExchangeKeepsTheSessionAndReadsTheAccount() async throws {
        let (s, asked) = server([(200, #"{"token":"t1","account":\#(me)}"#)])
        let (token, info) = try await s.exchange(code: "c")
        XCTAssertEqual(token, "t1")
        XCTAssertEqual(info.status, "waiting")
        XCTAssertEqual(info.month, AccountInfo.Month(spentUsd: 0.5, limitUsd: 5))
        XCTAssertEqual(asked()[0].url?.path, "/auth/exchange")
        XCTAssertNil(asked()[0].value(forHTTPHeaderField: "Authorization"))
    }

    func testMeSendsTheSessionAndSignedOutIsItsOwnError() async throws {
        let (s, asked) = server([(200, me), (401, #"{"error":"signed_out"}"#)])
        _ = try await s.me(token: "t1")
        XCTAssertEqual(asked()[0].value(forHTTPHeaderField: "Authorization"), "Bearer t1")
        do {
            _ = try await s.me(token: "t1")
            XCTFail("signed out")
        } catch {
            XCTAssertEqual(error as? AccountError, .signedOut)
        }
    }

    @MainActor
    func testASessionTheServerForgotSignsOutHere() async {
        let account = TacieAccount(load: false)
        var kept: [String?] = []
        account.keep = { kept.append($0) }
        account.keepSpace = { _ in }
        var changed = 0
        account.changed = { changed += 1 }
        account.setForTesting(token: "t1", info: nil)
        account.server = server([(401, "{}")]).0
        await account.refresh()
        XCTAssertFalse(account.signedIn)
        XCTAssertEqual(kept, [nil])
        XCTAssertEqual(changed, 1)
    }

    @MainActor
    func testUsedUpAsksAgainOnlyWhenTheLastAnswerWasUsedUp() async {
        let account = TacieAccount(load: false)
        account.keep = { _ in }
        account.keepSpace = { _ in }
        let active = #"{"email":"a@b.com","name":"준","picture":null,"status":"active","month":{"spent_usd":%@,"limit_usd":1}}"#
        func info(_ spent: String) -> AccountInfo {
            try! AccountServer.decoder.decode(AccountInfo.self, from: Data(active.replacingOccurrences(of: "%@", with: spent).utf8))
        }
        // under the limit: no call (an empty answer list would crash on one)
        account.setForTesting(token: "t1", info: info("0.4"))
        account.server = server([]).0
        let under = await account.usedUp()
        XCTAssertFalse(under)
        // used up and still so on the server
        account.setForTesting(token: "t1", info: info("1.0008"))
        account.server = server([(200, active.replacingOccurrences(of: "%@", with: "1.0008"))]).0
        let still = await account.usedUp()
        XCTAssertTrue(still)
        // used up here, but the admin raised the limit
        account.server = server([(200, #"{"email":"a@b.com","name":"준","picture":null,"status":"active","month":{"spent_usd":1.0008,"limit_usd":5}}"#)]).0
        let raised = await account.usedUp()
        XCTAssertFalse(raised)
    }

    func testTheLimitLineSaysWhenTacieAIComesBack() {
        let now = ISO8601DateFormatter().date(from: "2026-11-12T06:00:00Z")!
        XCTAssertTrue(Account.limitLine(kept: true, now: now).contains("대화 기록은 저장했어요"))
        XCTAssertTrue(Account.limitLine(kept: false, now: now).contains("시작하지 못했어요"))
        XCTAssertTrue(Account.limitLine(kept: false, now: now).contains("19일 뒤"))
    }

    func testUsageResetsOnTheFirstOfNextMonthInUTC() {
        let f = ISO8601DateFormatter()
        XCTAssertEqual(Account.nextMonth(f.date(from: "2026-10-31T23:30:00Z")!), f.date(from: "2026-11-01T00:00:00Z"))
        XCTAssertEqual(Account.nextMonth(f.date(from: "2026-12-05T00:00:00Z")!), f.date(from: "2027-01-01T00:00:00Z"))
    }

    private final class Recorder {
        var calls: [String] = []
        var sink: AccountAnalytics {
            AccountAnalytics(signedIn: { self.calls.append("in \($0.rawValue) \($1 ?? "-") \($2.rawValue)") },
                             refreshed: { self.calls.append("seen \($0.rawValue) \($1 ?? "-")") },
                             signedOut: { self.calls.append("out \($0.rawValue)") })
        }
    }

    private func meWith(_ id: String?) -> String {
        let field = id.map { #","analytics_id":"\#($0)""# } ?? ""
        return #"{"email":"a@b.com","name":"준","picture":null,"status":"waiting"\#(field),"month":{"spent_usd":0,"limit_usd":5}}"#
    }

    @MainActor
    func testSignInIdentifiesAndSignOutResets() async throws {
        let account = TacieAccount(load: false)
        account.keep = { _ in }
        account.keepProvider = { _ in }
        account.keepSpace = { _ in }
        let rec = Recorder()
        account.analytics = rec.sink
        account.server = server([(200, #"{"token":"t1","account":\#(meWith("abc"))}"#), (200, "{}"),
                                 (200, #"{"token":"t2","account":\#(meWith("def"))}"#)]).0
        try await account.exchange(code: "c1")
        await account.signOut()
        XCTAssertNil(account.provider)
        try await account.exchange(code: "c2", with: .github)
        XCTAssertEqual(account.provider, .github)
        XCTAssertEqual(rec.calls, ["in waiting abc google", "out user", "in waiting def github"])
    }

    @MainActor
    func testLaunchRefreshLinksWithoutASignInEventAndAnEndedSessionIsExpired() async {
        let account = TacieAccount(load: false)
        account.keep = { _ in }
        account.keepProvider = { _ in }
        account.keepSpace = { _ in }
        let rec = Recorder()
        account.analytics = rec.sink
        account.setForTesting(token: "t1", info: nil)
        account.server = server([(200, meWith("abc")), (401, "{}")]).0
        await account.refresh()
        await account.refresh()
        XCTAssertEqual(rec.calls, ["seen waiting abc", "out expired"])
    }

    @MainActor
    func testOldServerWithoutAnalyticsIdStillSignsIn() async throws {
        let account = TacieAccount(load: false)
        account.keep = { _ in }
        account.keepProvider = { _ in }
        account.keepSpace = { _ in }
        let rec = Recorder()
        account.analytics = rec.sink
        account.server = server([(200, #"{"token":"t1","account":\#(meWith(nil))}"#)]).0
        try await account.exchange(code: "c1")
        XCTAssertTrue(account.signedIn)
        XCTAssertNil(account.info?.analyticsId)
        XCTAssertEqual(rec.calls, ["in waiting - google"])
    }

    @MainActor
    func testEachAccountGetsItsOwnSpaceAndSigningOutLeavesIt() async throws {
        let account = TacieAccount(load: false)
        account.keep = { _ in }
        account.keepProvider = { _ in }
        var spaces: [String?] = []
        account.keepSpace = { spaces.append($0) }
        account.server = server([(200, #"{"token":"t1","account":\#(meWith("abc"))}"#), (200, "{}"),
                                 (200, #"{"token":"t2","account":\#(meWith("def"))}"#)]).0
        try await account.exchange(code: "c1")
        XCTAssertEqual(account.spaceID, "abc")
        await account.signOut()
        XCTAssertNil(account.spaceID)
        try await account.exchange(code: "c2")
        XCTAssertEqual(account.spaceID, "def")
        XCTAssertEqual(spaces, ["abc", nil, "def"])
    }

    /// A session from before spaces (2026-11-10) learns its space from the first GET /me, once.
    @MainActor
    func testASessionFromBeforeSpacesLearnsItsSpaceOnce() async {
        let account = TacieAccount(load: false)
        account.keep = { _ in }
        account.keepProvider = { _ in }
        var spaces: [String?] = []
        account.keepSpace = { spaces.append($0) }
        var changed = 0
        account.changed = { changed += 1 }
        account.setForTesting(token: "t1", info: nil)
        account.server = server([(200, meWith("abc")), (200, meWith("abc"))]).0
        await account.refresh()
        await account.refresh()
        XCTAssertEqual(account.spaceID, "abc")
        XCTAssertEqual(spaces, ["abc"])
        XCTAssertEqual(changed, 1)
    }

    /// An older server sends no id: the session stays in the signed-out space rather than a made-up one.
    @MainActor
    func testNoAnalyticsIdKeepsTheSignedOutSpace() async {
        let account = TacieAccount(load: false)
        account.keep = { _ in }
        account.keepProvider = { _ in }
        account.keepSpace = { _ in }
        var changed = 0
        account.changed = { changed += 1 }
        account.setForTesting(token: "t1", info: nil)
        account.server = server([(200, meWith(nil))]).0
        await account.refresh()
        XCTAssertNil(account.spaceID)
        XCTAssertEqual(changed, 0)
    }

    @MainActor
    func testDeletingTheAccountAsksToEraseItsSpace() async throws {
        let account = TacieAccount(load: false)
        account.keep = { _ in }
        account.keepProvider = { _ in }
        account.keepSpace = { _ in }
        var erased: [String] = []
        account.eraseSpace = { erased.append($0) }
        account.server = server([(200, #"{"token":"t1","account":\#(meWith("abc"))}"#), (200, "{}")]).0
        try await account.exchange(code: "c1")
        let ok = await account.deleteAccount()
        XCTAssertTrue(ok)
        XCTAssertEqual(erased, ["abc"])
        XCTAssertNil(account.spaceID)
    }

    @MainActor
    func testSignInSendsOnlyATickedNewsBoxAndTheSheetAsksUntilChosen() async throws {
        let account = TacieAccount(load: false)
        account.keep = { _ in }
        account.keepProvider = { _ in }
        account.keepSpace = { _ in }
        let chose = #"{"email":"a@b.com","name":"준","picture":null,"status":"active","month":{"spent_usd":0,"limit_usd":5},"mail_news":true,"mail_news_at":"2026-11-17T00:00:00.000Z"}"#
        let (s, asked) = server([(200, #"{"token":"t1","account":\#(me)}"#), (200, chose)])
        account.server = s
        account.newsAtSignIn = true
        try await account.exchange(code: "c")
        XCTAssertEqual(asked().map { $0.url?.path }, ["/auth/exchange", "/me/mail"])
        let body = try JSONSerialization.jsonObject(with: asked()[1].httpBody!) as? [String: Bool]
        XCTAssertEqual(body, ["news": true])
        XCTAssertEqual(account.info?.mailNews, true)
        XCTAssertFalse(account.newsAtSignIn)
        XCTAssertFalse(account.asksNews)

        let (quiet, quietAsked) = server([(200, #"{"token":"t2","account":\#(me)}"#)])
        account.server = quiet
        try await account.exchange(code: "c2")
        XCTAssertEqual(quietAsked().count, 1, "an unticked box sends nothing")
        XCTAssertTrue(account.asksNews, "never chosen: the sheet asks")
    }
}
