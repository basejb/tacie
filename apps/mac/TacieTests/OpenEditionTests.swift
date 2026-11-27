import XCTest
@testable import Tacie

/// The open edition has no Tacie sign-in or servers (docs/41 0절 7번): what it leaves out.
final class OpenEditionTests: XCTestCase {
    func testAccountStepOffInOpenEdition() {
        XCTAssertTrue(OnboardingFlow.accountStep(dev: false, open: false, args: []))
        XCTAssertFalse(OnboardingFlow.accountStep(dev: true, open: false, args: []))
        XCTAssertTrue(OnboardingFlow.accountStep(dev: true, open: false, args: ["--account-step"]))
        XCTAssertFalse(OnboardingFlow.accountStep(dev: false, open: true, args: []))
        XCTAssertFalse(OnboardingFlow.accountStep(dev: true, open: true, args: ["--account-step"]))
    }

    func testRelayOffInOpenEdition() {
        // the relay rule moved to RelayRuleTests (#56)
        XCTAssertFalse(Relay.on(mode: .key, signedIn: true))
    }

    func testSettingsPanes() {
        XCTAssertEqual(SettingsPane.panes(hasAccount: true), [.account, .general, .meeting, .ai, .connections, .privacy])
        XCTAssertEqual(SettingsPane.panes(hasAccount: false), [.general, .meeting, .ai, .connections, .privacy])
    }

    func testServerFeaturesFollowTheEdition() {
        XCTAssertEqual(TacieAccount.offered, Build.hasAccount)
        XCTAssertEqual(WhatsNew.offered, Build.hasAccount)
        XCTAssertEqual(MeetingTranslation.offered, Build.isDev && !Build.isOpen)
    }
}

final class OpenEditionConnectionsTests: XCTestCase {
    func testOnlyServerlessSourcesInOpenEdition() {
        XCTAssertEqual(ConnectionsScene.Source.available(open: false, dev: false), ConnectionsScene.Source.allCases)
        // 3단계 (2026-11-23): the services that take a pasted token, and the note folder
        XCTAssertEqual(ConnectionsScene.Source.available(open: true, dev: false), [.github, .notion, .linear, .atlassian, .asana, .notes])
        XCTAssertEqual(ConnectionsScene.Source.available(open: true, dev: true), [.github, .notion, .linear, .atlassian, .asana, .notes])
    }
}

final class OpenEditionReviewFixTests: XCTestCase {
    /// ⌘, and every "settings" button land on 계정; the open edition has none, so they open 일반.
    func testAccountPaneFallsBackInOpenEdition() {
        XCTAssertEqual(SettingsPane.resolve(.account, open: true), .general)
        XCTAssertEqual(SettingsPane.resolve(.account, open: false), .account)
        XCTAssertEqual(SettingsPane.resolve(.connections, open: true), .connections)
    }

    /// 처음 써 보기 offers only what the edition can connect.
    func testFirstStepsToolsFollowTheEdition() {
        XCTAssertEqual(ConnectionsScene.Source.firstStepsTools(open: false, dev: false),
                       [.notion, .linear, .gdrive, .atlassian, .asana, .dropbox, .notes])
        XCTAssertEqual(ConnectionsScene.Source.firstStepsTools(open: true, dev: false), [.notion, .linear, .atlassian, .asana, .notes])
    }

    /// The open edition's development build keeps its own engine data, apart from the official development build's checkout.
    func testOpenDevBuildEngineRootIsItsOwn() {
        let home = URL(fileURLWithPath: "/Users/me"), support = URL(fileURLWithPath: "/Users/me/Library/Application Support")
        XCTAssertEqual(EngineLaunch.root(env: [:], isDev: true, open: true, saved: nil, home: home, appSupport: support),
                       URL(fileURLWithPath: "/Users/me/Library/Application Support/Tacie Open Dev"))
        XCTAssertEqual(EngineLaunch.root(env: [:], isDev: true, open: false, saved: nil, home: home, appSupport: support),
                       URL(fileURLWithPath: "/Users/me/Repositories/tacie"))
        XCTAssertEqual(EngineLaunch.root(env: [:], isDev: true, open: true, saved: "/x", home: home, appSupport: support),
                       URL(fileURLWithPath: "/x"))
    }

    // 2-D: the open edition's onboarding asks how to run AI in the sign-in's place
    func testOpenShowsAccountStep() {
        let facts = OnboardingFacts(accountStep: false, open: true)
        XCTAssertTrue(OnboardingFlow.visible(facts).contains(.account))
    }

    func testOfficialAccountStepUnchanged() {
        XCTAssertTrue(OnboardingFlow.visible(OnboardingFacts(accountStep: true)).contains(.account))
        XCTAssertFalse(OnboardingFlow.visible(OnboardingFacts(accountStep: false)).contains(.account))
    }

    func testOpenLeftoverChooseAI() {
        let progress = OnboardingProgress()
        let items = OnboardingFlow.leftovers(progress, OnboardingFacts(micGranted: true, callAudio: .granted, open: true))
        XCTAssertEqual(items, [.chooseAI])
        XCTAssertEqual(OnboardingLeftover.chooseAI.id, "chooseAI")
    }

    func testChosenNoLeftover() {
        let progress = OnboardingProgress()
        let facts = OnboardingFacts(micGranted: true, callAudio: .granted, open: true, aiChosen: true)
        XCTAssertEqual(OnboardingFlow.leftovers(progress, facts), [])
        XCTAssertEqual(OnboardingFlow.leftovers(progress, OnboardingFacts(micGranted: true, callAudio: .granted)), [])
    }
}
