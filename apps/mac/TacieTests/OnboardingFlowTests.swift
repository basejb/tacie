import XCTest
@testable import Tacie

final class OnboardingFlowTests: XCTestCase {
    private let now = OnboardingFacts()

    func testPracticeStepsAreSkippedUntilTheyExist() {
        XCTAssertEqual(OnboardingFlow.visible(now), [.welcome, .sources, .listen, .done])
        XCTAssertEqual(OnboardingFlow.next(after: .listen, now), .done)
        XCTAssertEqual(OnboardingFlow.previous(before: .done, now), .listen)
        var later = now
        later.practiceReady = true
        XCTAssertEqual(OnboardingFlow.next(after: .listen, later), .ask)
        XCTAssertEqual(OnboardingFlow.visible(later).count, 6)
    }

    // Mac 안에서만 쓰기 (or 직접 고르기 with answer cards off) has no answer cards: the practice cannot show one, so its
    // steps are left out instead of asking for a key that mode would not use (2026-11-24).
    func testPracticeIsLeftOutWithoutAnswerCards() {
        var f = now
        f.practiceReady = true
        XCTAssertTrue(OnboardingFlow.visible(f).contains(.meeting))
        f.answerCards = false
        XCTAssertFalse(OnboardingFlow.visible(f).contains(.ask))
        XCTAssertFalse(OnboardingFlow.visible(f).contains(.meeting))
        XCTAssertFalse(OnboardingFlow.answerCards(open: true, mode: .local, cardsOff: false))
        XCTAssertFalse(OnboardingFlow.answerCards(open: true, mode: .custom, cardsOff: true))
        XCTAssertTrue(OnboardingFlow.answerCards(open: true, mode: .key, cardsOff: false))
        XCTAssertTrue(OnboardingFlow.answerCards(open: true, mode: nil, cardsOff: false))
        XCTAssertTrue(OnboardingFlow.answerCards(open: false, mode: .local, cardsOff: false))
    }

    func testFirstAndLastSteps() {
        XCTAssertNil(OnboardingFlow.previous(before: .welcome, now))
        XCTAssertEqual(OnboardingFlow.next(after: .done, now), .done)
    }

    func testResumesOnTheSavedStepAfterARelaunch() {
        var p = OnboardingProgress()
        p.step = .listen
        XCTAssertEqual(OnboardingFlow.resume(p, now), .listen)
        p.step = .ask  // saved by a build that had the practice step, opened by one that does not
        XCTAssertEqual(OnboardingFlow.resume(p, now), .done)
        p.finished = true
        XCTAssertEqual(OnboardingFlow.resume(p, now), .done)
    }

    func testOpensAtLaunchUntilFinishedButNotAsALoginItem() {
        XCTAssertTrue(OnboardingFlow.opensAtLaunch(nil, loginItem: false))
        XCTAssertFalse(OnboardingFlow.opensAtLaunch(nil, loginItem: true))
        var p = OnboardingProgress()
        p.finished = true
        XCTAssertFalse(OnboardingFlow.opensAtLaunch(p, loginItem: false))
    }

    func testAlreadySetUpNeedsARepositoryAndTheMicrophone() {
        var f = now
        f.hasSource = true
        XCTAssertFalse(OnboardingFlow.alreadySetUp(f))
        f.micGranted = true
        XCTAssertTrue(OnboardingFlow.alreadySetUp(f))
    }

    func testListenNeedsOnlyTheMicrophone() {
        var f = now
        XCTAssertFalse(OnboardingFlow.listenDone(f))
        f.micGranted = true
        XCTAssertTrue(OnboardingFlow.listenDone(f))
    }

    func testLeftovers() {
        var p = OnboardingProgress()
        p.notify = ["slack", "drive"]
        var f = now
        XCTAssertEqual(OnboardingFlow.leftovers(p, f), [.mic, .callAudio, .notify(["slack"])])
        f.micGranted = true
        f.hasSource = true
        f.callAudio = .granted
        p.notify = []
        XCTAssertEqual(OnboardingFlow.leftovers(p, f), [])
        f.practiceReady = true
        p.skipped = [.ask, .meeting]
        XCTAssertEqual(OnboardingFlow.leftovers(p, f), [], "a skipped practice is not left to do")
    }

    func testAGrantedMicrophoneWithNoDeviceIsLeftToConnect() {
        var f = now
        f.micGranted = true
        f.hasSource = true
        f.callAudio = .granted
        f.micConnected = false
        XCTAssertEqual(OnboardingFlow.leftovers(OnboardingProgress(), f), [.connectMic])
    }

    func testHiddenLeftoversStayHidden() {
        var p = OnboardingProgress()
        var f = now
        f.micGranted = true
        f.micConnected = false
        f.hasSource = true
        f.callAudio = .granted
        XCTAssertEqual(OnboardingFlow.leftovers(p, f), [.connectMic])
        p.dismissed = ["connectMic"]
        XCTAssertEqual(OnboardingFlow.leftovers(p, f), [])
    }

    func testAtlassianAloneIsEnoughToContinue() {
        // nothing found yet: the first fetch is still on its way
        XCTAssertFalse(OnboardingFlow.atlassianFound(connected: true, issues: nil, pages: nil))
        XCTAssertFalse(OnboardingFlow.atlassianFound(connected: false, issues: 12, pages: 3))
        let found = OnboardingFlow.atlassianFound(connected: true, issues: 12, pages: nil)
        XCTAssertTrue(found, "issues before any page are enough")
        XCTAssertTrue(OnboardingFlow.hasSource(github: false, notion: false, linear: false, gdrive: false, atlassian: found))
        XCTAssertFalse(OnboardingFlow.hasSource(github: false, notion: false, linear: false, gdrive: false, atlassian: false))
        var f = now
        f.hasSource = found
        f.atlassianConnected = true
        f.practiceReady = true
        XCTAssertEqual(OnboardingFlow.next(after: .sources, f), .listen)
        var p = OnboardingProgress()
        p.notify = ["atlassian", "slack"]
        f.micGranted = true
        f.callAudio = .granted
        XCTAssertEqual(OnboardingFlow.leftovers(p, f), [.notify(["slack"])], "Jira · Confluence is open now, not waited for")
    }

    func testDropboxAloneIsEnoughToContinue() {
        XCTAssertFalse(OnboardingFlow.dropboxFound(connected: true, files: nil))
        XCTAssertFalse(OnboardingFlow.dropboxFound(connected: false, files: 3))
        let found = OnboardingFlow.dropboxFound(connected: true, files: 0)
        XCTAssertTrue(OnboardingFlow.hasSource(github: false, notion: false, linear: false, gdrive: false, atlassian: false,
                                               dropbox: found))
        var f = now
        f.hasSource = found
        f.dropboxConnected = true
        f.practiceReady = true
        XCTAssertEqual(OnboardingFlow.next(after: .sources, f), .listen)
    }

    func testAsanaAloneIsEnoughToContinue() {
        // nothing found yet: the first fetch is still on its way
        XCTAssertFalse(OnboardingFlow.asanaFound(connected: true, tasks: nil))
        XCTAssertFalse(OnboardingFlow.asanaFound(connected: false, tasks: 12))
        let found = OnboardingFlow.asanaFound(connected: true, tasks: 12)
        XCTAssertTrue(found)
        XCTAssertTrue(OnboardingFlow.hasSource(github: false, notion: false, linear: false, gdrive: false, atlassian: false, asana: found))
        XCTAssertFalse(OnboardingFlow.hasSource(github: false, notion: false, linear: false, gdrive: false, atlassian: false, asana: false))
        var f = now
        f.hasSource = found
        f.asanaConnected = true
        f.practiceReady = true
        XCTAssertEqual(OnboardingFlow.next(after: .sources, f), .listen)
        var p = OnboardingProgress()
        p.notify = ["asana", "slack"]
        f.micGranted = true
        f.callAudio = .granted
        XCTAssertEqual(OnboardingFlow.leftovers(p, f), [.notify(["slack"])], "Asana is open now, not waited for")
    }

    func testConnectionsAreNotLeftovers() {
        // connecting lives in the main window's 처음 써 보기 now (docs/11 14절)
        var p = OnboardingProgress()
        p.notify = ["notion", "linear", "drive", "slack"]
        var f = now
        f.micGranted = true
        f.callAudio = .granted
        XCTAssertEqual(OnboardingFlow.leftovers(p, f), [.notify(["slack"])])
        f.hasSource = true
        XCTAssertEqual(OnboardingFlow.leftovers(p, f), [.notify(["slack"])])
        p.notify = []
        f.hasSource = false
        XCTAssertEqual(OnboardingFlow.leftovers(p, f), [])
    }

    func testProgressSavedBeforeHidingStillLoads() throws {
        let old = Data(#"{"step":4,"finished":false,"skipped":[2,1],"notify":[],"callAudioLater":false}"#.utf8)
        let p = try JSONDecoder().decode(OnboardingProgress.self, from: old)
        XCTAssertEqual(p.step, .meeting)
        XCTAssertNil(p.dismissed)
    }

    func testStoreRoundTrip() throws {
        let defaults = try XCTUnwrap(UserDefaults(suiteName: "onboarding-tests"))
        defaults.removePersistentDomain(forName: "onboarding-tests")
        let store = OnboardingStore(defaults: defaults)
        XCTAssertNil(store.load())
        var p = OnboardingProgress()
        p.step = .listen
        p.skipped = [.ask]
        p.notify = ["slack"]
        p.callAudioLater = true
        store.save(p)
        XCTAssertEqual(store.load(), p)
    }

    func testTheAccountStepComesAfterTheWelcomeAndOldProgressKeepsItsMeaning() throws {
        var f = now
        f.accountStep = true
        XCTAssertEqual(OnboardingFlow.visible(f), [.welcome, .account, .sources, .listen, .done])
        XCTAssertEqual(OnboardingFlow.next(after: .welcome, f), .account)
        XCTAssertEqual(OnboardingFlow.previous(before: .sources, f), .account)
        // a build without the step: welcome goes straight to the sources
        XCTAssertEqual(OnboardingFlow.next(after: .welcome, now), .sources)
        // progress saved before the step existed: 4 is still the meeting practice
        let old = Data(#"{"step":4,"finished":false,"skipped":[2,1],"notify":[],"callAudioLater":false}"#.utf8)
        XCTAssertEqual(try JSONDecoder().decode(OnboardingProgress.self, from: old).step, .meeting)
        // signed out on such a build: 남은 일 says so first
        f.micGranted = true
        f.hasSource = true
        f.callAudio = .granted
        XCTAssertEqual(OnboardingFlow.leftovers(OnboardingProgress(), f), [.signIn])
        f.signedIn = true
        XCTAssertEqual(OnboardingFlow.leftovers(OnboardingProgress(), f), [])
    }
}
