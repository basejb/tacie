import XCTest
@testable import Tacie

final class FirstStepsTests: XCTestCase {
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    func testMarkIsOnceAndCounts() {
        var s = FirstStepsState()
        XCTAssertTrue(FirstSteps.mark(&s, .meeting, at: t0))
        XCTAssertFalse(FirstSteps.mark(&s, .meeting, at: t0.addingTimeInterval(60)))
        XCTAssertEqual(FirstSteps.count(s), 1)
        XCTAssertEqual(s.done[.meeting], t0, "the first time is kept")
        XCTAssertTrue(FirstSteps.isDone(s, .meeting))
        XCTAssertFalse(FirstSteps.isDone(s, .card))
    }

    func testVisibleUntilAllDoneOrHidden() {
        var s = FirstStepsState()
        XCTAssertTrue(FirstSteps.visible(s))
        for step in FirstStep.allCases { _ = FirstSteps.mark(&s, step, at: t0) }
        XCTAssertEqual(FirstSteps.count(s), 4)
        XCTAssertFalse(FirstSteps.visible(s))
        var hidden = FirstStepsState()
        hidden.hidden = true
        XCTAssertFalse(FirstSteps.visible(hidden))
    }

    func testSourceStaysDoneWhenDisconnected() {
        // there is no way to undo a step: connecting once counts even after the connection goes
        var s = FirstStepsState()
        _ = FirstSteps.mark(&s, .source, at: t0)
        FirstSteps.backfill(&s, hasMeeting: nil, hasSource: false, now: t0)
        XCTAssertTrue(FirstSteps.isDone(s, .source))
    }

    func testBackfill() {
        var s = FirstStepsState()
        let met = t0.addingTimeInterval(-86_400)
        FirstSteps.backfill(&s, hasMeeting: met, hasSource: true, now: t0)
        XCTAssertEqual(s.done[.meeting], met)
        XCTAssertEqual(s.done[.source], t0)
        XCTAssertFalse(FirstSteps.isDone(s, .card))
        XCTAssertFalse(FirstSteps.isDone(s, .ask))
        XCTAssertTrue(s.backfilled)
        let before = s
        FirstSteps.backfill(&s, hasMeeting: t0, hasSource: true, now: t0.addingTimeInterval(9))
        XCTAssertEqual(s, before, "backfill runs once")
    }

    func testFinishShowsOnceAfterTheLastStep() {
        var s = FirstStepsState()
        XCTAssertFalse(FirstSteps.showsFinish(s))
        for step in FirstStep.allCases { _ = FirstSteps.mark(&s, step, at: t0) }
        XCTAssertTrue(FirstSteps.showsFinish(s))
        s.finishedShown = true
        XCTAssertFalse(FirstSteps.showsFinish(s))
        var hidden = FirstStepsState()
        for step in FirstStep.allCases { _ = FirstSteps.mark(&hidden, step, at: t0) }
        hidden.hidden = true
        XCTAssertFalse(FirstSteps.showsFinish(hidden), "put away with ✕: nothing to celebrate on screen")
    }

    func testCardHintOnlyOnTheFirstRealCard() {
        let s = FirstStepsState()
        XCTAssertTrue(FirstSteps.wantsCardHint(s, cards: 3, inMeeting: true, practice: false, sample: false, hintShown: false))
        XCTAssertFalse(FirstSteps.wantsCardHint(s, cards: 0, inMeeting: true, practice: false, sample: false, hintShown: false))
        XCTAssertFalse(FirstSteps.wantsCardHint(s, cards: 3, inMeeting: false, practice: false, sample: false, hintShown: false))
        XCTAssertFalse(FirstSteps.wantsCardHint(s, cards: 3, inMeeting: true, practice: true, sample: false, hintShown: false))
        XCTAssertFalse(FirstSteps.wantsCardHint(s, cards: 3, inMeeting: true, practice: false, sample: true, hintShown: false))
        XCTAssertFalse(FirstSteps.wantsCardHint(s, cards: 3, inMeeting: true, practice: false, sample: false, hintShown: true))
        var opened = s
        _ = FirstSteps.mark(&opened, .card, at: t0)
        XCTAssertFalse(FirstSteps.wantsCardHint(opened, cards: 3, inMeeting: true, practice: false, sample: false, hintShown: false))
    }

    func testOnlyRealAnswersCountAsACard() {
        XCTAssertTrue(FirstSteps.realAnswer(practice: false, sample: false, onboarding: false))
        XCTAssertFalse(FirstSteps.realAnswer(practice: true, sample: false, onboarding: false))
        XCTAssertFalse(FirstSteps.realAnswer(practice: false, sample: true, onboarding: false))
        XCTAssertFalse(FirstSteps.realAnswer(practice: false, sample: false, onboarding: true), "the onboarding's step 3 on the user's own data")
    }

    func testSkippedStepsLeaveTheList() {
        var s = FirstStepsState()
        FirstSteps.skip(&s, .card)
        XCTAssertEqual(FirstSteps.steps(s), [.source, .meeting, .ask])
        XCTAssertEqual(FirstSteps.total(s), 3)
        for step in [FirstStep.source, .meeting] { _ = FirstSteps.mark(&s, step, at: t0) }
        XCTAssertEqual(FirstSteps.count(s), 2)
        XCTAssertTrue(FirstSteps.visible(s))
        _ = FirstSteps.mark(&s, .ask, at: t0)
        XCTAssertFalse(FirstSteps.visible(s), "the rest is done")
        XCTAssertTrue(FirstSteps.showsFinish(s))
    }

    func testSkippingEverythingEndsTheListQuietly() {
        var s = FirstStepsState()
        for step in FirstStep.allCases { FirstSteps.skip(&s, step) }
        XCTAssertFalse(FirstSteps.visible(s))
        XCTAssertFalse(FirstSteps.showsFinish(s))
    }

    func testASkippedCardGetsNoHint() {
        var s = FirstStepsState()
        FirstSteps.skip(&s, .card)
        XCTAssertFalse(FirstSteps.wantsCardHint(s, cards: 3, inMeeting: true, practice: false, sample: false, hintShown: false))
    }

    func testStateFromBeforeSkippingStillLoads() throws {
        let old = Data(#"{"done":{},"hidden":false,"finishedShown":false,"backfilled":true}"#.utf8)
        let s = try JSONDecoder().decode(FirstStepsState.self, from: old)
        XCTAssertEqual(FirstSteps.total(s), 4)
    }

    func testStepsKeepTheirOrder() {
        XCTAssertEqual(FirstStep.allCases, [.source, .meeting, .card, .ask])
    }

    @MainActor
    func testStoreRoundTrip() throws {
        let defaults = try XCTUnwrap(UserDefaults(suiteName: "first-steps-tests"))
        defaults.removePersistentDomain(forName: "first-steps-tests")
        let store = FirstStepsStore(defaults: defaults)
        XCTAssertEqual(store.state, FirstStepsState())
        var told: [FirstStep] = []
        store.onDone = { told.append($0) }
        store.mark(.card)
        store.mark(.card)
        XCTAssertEqual(told, [.card], "told once, the first time")
        store.hide()
        let again = FirstStepsStore(defaults: defaults)
        XCTAssertTrue(FirstSteps.isDone(again.state, .card))
        XCTAssertTrue(again.state.hidden)
        again.reset()
        XCTAssertEqual(again.state, FirstStepsState())
        XCTAssertEqual(FirstStepsStore(defaults: defaults).state, FirstStepsState())
    }
}
