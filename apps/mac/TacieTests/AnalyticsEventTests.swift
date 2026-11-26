import XCTest
@testable import Tacie

final class AnalyticsEventTests: XCTestCase {
    /// One of every event (docs/16 2-3).
    static let samples: [AnalyticsEvent] = [
        .appOpened(firstLaunch: true, loginItem: false),
        .appUpdated(from: "0.1.11", to: "0.1.12"),
        .onboardingStepCompleted(step: .sources, skipped: true),
        .onboardingFinished(skippedCount: 1, practiced: true),
        .onboardingRestarted,
        .signedIn(status: .waiting, with: .github),
        .signedOut(reason: .expired),
        .githubConnected,
        .notionConnected,
        .linearConnected,
        .gdriveConnected,
        .atlassianConnected,
        .asanaConnected,
        .dropboxConnected(team: true, paperLegacy: false),
        .syncStarted(first: true, privateRepo: false),
        .indexReady(first: true, chunks: .of(4200), seconds: 95),
        .modelDownloaded(resumed: false, seconds: 300, pack: .search),
        .meetingStarted(mode: .remote, trigger: .hotkey, practice: false, sample: false, hasSource: true),
        .meetingEnded(mode: .inPerson, practice: false, minutes: 12.3, utterances: 40, questions: 3, answered: 2, unanswered: 1, cards: 6, speakers: 3, namedSpeakers: 1, sttLocal: 7, sttStream: 30, reason: .user),
        .meetingEndHeard,
        .meetingEndKept(by: .question),
        .askSubmitted(inMeeting: false, sample: false),
        .answerShown(origin: .voice, cards: 3, repo: 1, github: 1, slack: 0, notion: 1, linear: 0, gdrive: 0, jira: 0, confluence: 0, lowConfidence: false, hasAnswerLine: true, practice: false),
        .questionUnanswered(origin: .typed, reason: .noCards, practice: false),
        .cardOpened(source: .linear, action: .open, place: .island),
        .errorShown(kind: .credit, place: .meeting),
        .meetingNotesOpened(from: .afterMeeting, hasLedger: true),
        .updateReady(to: "0.1.13"),
        .updateRestartClicked(place: .menu),
        .updateIdleRestart,
        .updateFailed(code: 1004),
        .updatePromptShown(userInitiated: false),
        .sourceDisconnected(tool: .linear),
        .updatePromptChoice(choice: .skip, userInitiated: true),
        .settingsOpened(page: .privacy),
        .settingChanged(setting: .autoSync, value: .off),
        .dataCleared(what: .searchData),
        .notesExported(how: .share, part: .page),
        .notesRewritten,
        .notesWritten(kind: .interview),
        .notesKindChanged(from: .interview, to: .meeting),
        .collectedOpened(view: .decisions, count: 4),
        .todoChecked(done: true),
        .meetingPinned(on: true),
        .meetingRenamed,
        .meetingDeleted,
        .speakerNamed(pick: .merge, lineOnly: false, live: true),
        .textSizeChanged(percent: 115),
        .firstStepDone(item: .card),
        .firstStepToolClicked(tool: .notion, connected: false),
        .connectHintClicked,
        .feedbackShown(shown: 2),
        .feedbackSent(wishes: [.faster, .todos], wrote: true),
        .feedbackClosed(how: .skip),
        .connectPromptShown(meetings: 3, unanswered: 11),
        .connectPromptAnswered(choice: .recordOnly),
        .panelOpened(via: .island, scope: .all),
        .panelAsked(kind: .followUp, scope: .meeting, turn: 2, place: .iphoneAsk),
        .panelAnswered(ms: 2400, sources: 3, notFound: false, errorKind: .credit, place: .iphoneMeeting),
        .panelProposal(kind: .renameTitle, action: .shown, places: 1, lines: 0, rewrite: false, remember: false, place: .mac),
        .askFallback(reason: .engineMissing),
        .askRetried(succeeded: true),
        .panelSourceOpened(kind: .source),
        .meetingLinked(cut: true),
        .mcpInstallClicked(app: .claudeCode),
        .mcpToolCalled(tool: .listMeetings, ms: 850, results: 12, failure: .none, client: .claudeDesktop),
    ]

    func testEveryEventUsesOnlyAllowedKeysAndValueKinds() {
        XCTAssertEqual(Self.samples.count, 66)
        let names = Self.samples.map(\.name)
        XCTAssertEqual(Set(names).count, names.count)
        let version = try! NSRegularExpression(pattern: #"^\d+(\.\d+)*$"#)
        for e in Self.samples {
            XCTAssertTrue(AnalyticsFilter.eventNames.contains(e.name), e.name)
            for (k, v) in e.properties {
                XCTAssertTrue(AnalyticsFilter.eventKeys.contains(k), "\(e.name).\(k)")
                if case .text(let s) = v {
                    let isVersion = version.firstMatch(in: s, range: NSRange(s.startIndex..., in: s)) != nil
                    XCTAssertTrue(AnalyticsFilter.closedValues.contains(s) || isVersion, "\(e.name).\(k) = \(s)")
                }
            }
        }
    }

    func testPanelEvents() {
        XCTAssertEqual(AnalyticsEvent.panelOpened(via: .shortcut, scope: .meeting).name, "panel_opened")
        XCTAssertEqual(AnalyticsEvent.panelOpened(via: .shortcut, scope: .meeting).properties, ["via": .text("shortcut"), "scope": .text("meeting")])
        XCTAssertEqual(AnalyticsEvent.panelOpened(via: .collected, scope: .all).properties, ["via": .text("collected"), "scope": .text("all")])
        XCTAssertEqual(AnalyticsEvent.panelAsked(kind: .suggestionPrep, scope: .meeting, turn: 1, place: .mac).properties["kind"], .text("suggestion_prep"))
        XCTAssertEqual(AnalyticsEvent.panelAsked(kind: .suggestionWhy, scope: .meeting, turn: 1, place: .mac).properties["kind"], .text("suggestion_why"))
        XCTAssertEqual(AnalyticsEvent.panelAsked(kind: .suggestionAsk, scope: .all, turn: 1, place: .mac).properties["kind"], .text("suggestion_ask"))
        XCTAssertEqual(AnalyticsEvent.panelAsked(kind: .typed, scope: .all, turn: 1, place: .mac).properties["kind"], .text("typed"))
        XCTAssertEqual(AnalyticsEvent.panelAsked(kind: .followUp, scope: .all, turn: 3, place: .mac).properties,
                       ["kind": .text("follow_up"), "scope": .text("all"), "turn": .int(3), "place": .text("mac")])
        XCTAssertEqual(AnalyticsEvent.panelAnswered(ms: 900, sources: 2, notFound: true, errorKind: nil, place: .mac).properties,
                       ["ms": .int(900), "sources": .int(2), "not_found": .bool(true), "place": .text("mac")])
        XCTAssertEqual(AnalyticsEvent.panelAnswered(ms: 1, sources: 0, notFound: false, errorKind: .model, place: .mac).properties["error_kind"], .text("model"))
        XCTAssertEqual(AnalyticsEvent.panelSourceOpened(kind: .meeting).properties, ["kind": .text("meeting")])
        XCTAssertEqual(AnalyticsEvent.meetingLinked(cut: true).properties, ["cut": .bool(true)])
        XCTAssertEqual(PanelError(kind: "credit"), .credit)
        XCTAssertEqual(PanelError(kind: "whatever"), .down)
        XCTAssertEqual(PanelScopeKind(.meeting("abc")), .meeting)
        // a meeting id never travels
        XCTAssertFalse(AnalyticsEvent.panelOpened(via: .page, scope: PanelScopeKind(.meeting("abc"))).properties.values.contains(.text("abc")))
    }

    func testPanelEventsCarryPlace() {
        XCTAssertEqual(APanelPlace.allCases.map(\.rawValue), ["mac", "iphone_ask", "iphone_meeting"])
        XCTAssertEqual(AnalyticsEvent.panelAsked(kind: .typed, scope: .all, turn: 1, place: .iphoneAsk).properties["place"], .text("iphone_ask"))
        XCTAssertEqual(AnalyticsEvent.panelAnswered(ms: 1, sources: 0, notFound: false, errorKind: nil, place: .iphoneMeeting).properties["place"],
                       .text("iphone_meeting"))
        XCTAssertEqual(AnalyticsEvent.panelProposal(kind: .fixWords, action: .shown, places: 0, lines: 0, rewrite: false, remember: false, place: .mac)
                        .properties["place"], .text("mac"))
        for place in APanelPlace.allCases {
            XCTAssertTrue(AnalyticsFilter.closedValues.contains(place.rawValue))
        }
        XCTAssertTrue(AnalyticsFilter.eventKeys.contains("place"))
    }

    func testAskFallbackAndRetriedEvents() {
        XCTAssertEqual(AnalyticsEvent.askFallback(reason: .engineMissing).name, "ask_fallback")
        XCTAssertEqual(AnalyticsEvent.askFallback(reason: .engineMissing).properties, ["reason": .text("engine_missing")])
        XCTAssertEqual(AnalyticsEvent.askFallback(reason: .engineTimeout).properties, ["reason": .text("engine_timeout")])
        XCTAssertEqual(AnalyticsEvent.askRetried(succeeded: false).name, "ask_retried")
        XCTAssertEqual(AnalyticsEvent.askRetried(succeeded: false).properties, ["succeeded": .bool(false)])
        XCTAssertNotNil(AnalyticsFilter.filter(event: "ask_fallback", properties: ["reason": "engine_missing"]))
        XCTAssertNotNil(AnalyticsFilter.filter(event: "ask_retried", properties: ["succeeded": true]))
        XCTAssertTrue(AnalyticsFilter.closedValues.isSuperset(of: ["engine_missing", "engine_timeout"]))
    }

    func testMCPEvents() {
        XCTAssertEqual(ASetting.mcpAccess.rawValue, "mcp_access")
        XCTAssertEqual(AnalyticsEvent.mcpInstallClicked(app: .claudeDesktop).name, "mcp_install_clicked")
        XCTAssertEqual(AnalyticsEvent.mcpInstallClicked(app: .claudeDesktop).properties, ["app": .text("claude_desktop")])
        let e = AnalyticsEvent.mcpToolCalled(tool: .readTranscript, ms: 40, results: 7, failure: .notReady, client: .claudeCode)
        XCTAssertEqual(e.name, "mcp_tool_called")
        XCTAssertEqual(e.properties, ["tool": .text("read_transcript"), "ms": .int(40), "results": .int(7),
                                      "failure": .text("not_ready"), "client": .text("claude_code")])
        XCTAssertTrue(AnalyticsFilter.closedValues.isSuperset(of: ["claude_desktop", "cursor", "claude_code", "other", "list_meetings",
                                                                   "list_ledger", "read_transcript", "none", "not_ready", "bad_request"]))
        XCTAssertEqual(AMCPFailure.allCases.count, 11)
    }

    func testNotesKindEvents() {
        XCTAssertEqual(AnalyticsEvent.notesWritten(kind: .talk).name, "notes_written")
        XCTAssertEqual(AnalyticsEvent.notesWritten(kind: .talk).properties, ["notes_kind": .text("talk")])
        let changed = AnalyticsEvent.notesKindChanged(from: .meeting, to: .interview)
        XCTAssertEqual(changed.name, "notes_kind_changed")
        XCTAssertEqual(changed.properties, ["from": .text("meeting"), "to": .text("interview")])
        XCTAssertEqual(ANotesKind(.interview), .interview)
    }

    func testRememberPeopleRawValues() {
        XCTAssertEqual(ASetting.rememberPeople.rawValue, "remember_people")
        XCTAssertEqual(ACleared.people.rawValue, "people")
        XCTAssertEqual(AnalyticsEvent.settingChanged(setting: .rememberPeople, value: .off).properties["setting"], .text("remember_people"))
        XCTAssertEqual(AnalyticsEvent.dataCleared(what: .people).properties["what"], .text("people"))
        XCTAssertTrue(AnalyticsFilter.closedValues.isSuperset(of: ["remember_people", "people"]))
    }

    func testMeetingEndedRoundsMinutesToOneDecimal() {
        let e = AnalyticsEvent.meetingEnded(mode: .remote, practice: false, minutes: 32.46, utterances: 0, questions: 0,
                                            answered: 0, unanswered: 0, cards: 0, speakers: 0, namedSpeakers: 0, sttLocal: 0, sttStream: 0, reason: .user)
        XCTAssertEqual(e.properties["minutes"], .double(32.5))
    }

    func testMeetingEndedCarriesCalendarCounts() {
        var m = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 0))
        let plain = MeetingStats.ended(m, mode: .remote, practice: false, startedAt: m.start, now: m.start, sttLocal: 0, reason: .user)
        XCTAssertEqual(plain.properties["event_linked"], .bool(false))
        XCTAssertEqual(plain.properties["invited"], .int(0))
        m.link(MeetingEvent(id: "e", title: "회의", start: m.start, end: m.start + 60),
               invited: [MeetingInvitee(name: "정하늘", email: "a@x.com"), MeetingInvitee(name: nil, email: "b@x.com")], meaningful: false)
        let linked = MeetingStats.ended(m, mode: .remote, practice: false, startedAt: m.start, now: m.start, sttLocal: 0, reason: .user)
        XCTAssertEqual(linked.properties["event_linked"], .bool(true))
        XCTAssertEqual(linked.properties["invited"], .int(2))
        XCTAssertEqual(AnalyticsEvent.settingChanged(setting: .calendar, value: .on).properties["setting"], .text("calendar"))
    }

    func testLocalSTTNumbers() {
        XCTAssertEqual(ASetting.localStt.rawValue, "local_stt")
        XCTAssertEqual(AnalyticsEvent.settingChanged(setting: .localStt, value: .on).properties["setting"], .text("local_stt"))
        XCTAssertEqual(AModelPack.allCases.map(\.rawValue), ["search", "voice", "stt"])
        let d = AnalyticsEvent.modelDownloaded(resumed: true, seconds: 9, pack: .stt)
        XCTAssertEqual(d.properties["pack"], .text("stt"))
        XCTAssertEqual(d.properties.count, 3)
        let e = AnalyticsEvent.meetingEnded(mode: .remote, practice: false, minutes: 1, utterances: 5, questions: 0, answered: 0,
                                            unanswered: 0, cards: 0, speakers: 0, namedSpeakers: 0, sttLocal: 4, sttStream: 9, reason: .user)
        XCTAssertEqual(e.properties["stt_local"], .int(4))
        XCTAssertEqual(e.properties["stt_stream"], .int(9))
        XCTAssertTrue(AnalyticsFilter.eventKeys.contains("stt_stream"))
        XCTAssertTrue(AnalyticsFilter.eventKeys.contains("stt_local"))
        XCTAssertTrue(AnalyticsFilter.eventKeys.contains("pack"))
        XCTAssertTrue(AnalyticsFilter.closedValues.isSuperset(of: ["local_stt", "search", "voice", "stt"]))
        XCTAssertEqual(AnalyticsFilter.filter(event: "meeting_ended", properties: ["stt_local": 2, "text": "x"])?.keys.sorted(), ["stt_local"])
    }

    func testLocalSTTFailureIsItsOwnErrorKind() {
        XCTAssertEqual(AErrorKind(kind: "stt_local"), .sttLocal)
        XCTAssertEqual(AnalyticsEvent.errorShown(kind: .sttLocal, place: .meeting).properties["error_kind"], .text("stt_local"))
        XCTAssertTrue(AnalyticsFilter.closedValues.contains("stt_local"))
        // a failing on-device recognition is not the search model downloading
        XCTAssertTrue(LocalSTT.isLocalFailure(kind: "model", stt: "local"))
        XCTAssertFalse(LocalSTT.isLocalFailure(kind: "model", stt: nil))
        XCTAssertFalse(LocalSTT.isLocalFailure(kind: "model", stt: "server"))
        XCTAssertFalse(LocalSTT.isLocalFailure(kind: "network", stt: "local"))
    }

    func testChunksBucketEdges() {
        XCTAssertEqual(ChunksBucket.of(0), .under1k)
        XCTAssertEqual(ChunksBucket.of(999), .under1k)
        XCTAssertEqual(ChunksBucket.of(1000).rawValue, "1k-10k")
        XCTAssertEqual(ChunksBucket.of(99_999).rawValue, "10k-100k")
        XCTAssertEqual(ChunksBucket.of(100_000).rawValue, "100k+")
    }

    func testErrorKindMapsUnknownToDown() {
        XCTAssertEqual(AErrorKind(kind: "weird"), .down)
        XCTAssertEqual(AErrorKind(kind: nil), .down)
        XCTAssertEqual(AErrorKind(kind: "credit"), .credit)
    }

    func testFirstStepsEvents() {
        XCTAssertEqual(AnalyticsEvent.firstStepDone(item: .ask).name, "first_step_done")
        XCTAssertEqual(AnalyticsEvent.firstStepDone(item: .ask).properties["item"], .text("ask"))
        let tool = AnalyticsEvent.firstStepToolClicked(tool: .gdrive, connected: true)
        XCTAssertEqual(tool.name, "first_step_tool_clicked")
        XCTAssertEqual(tool.properties["tool"], .text("gdrive"))
        XCTAssertEqual(tool.properties["connected"], .bool(true))
    }

    func testJiraAndConfluenceEvents() {
        XCTAssertEqual(AnalyticsEvent.atlassianConnected.name, "atlassian_connected")
        XCTAssertEqual(AnalyticsEvent.atlassianConnected.properties, [:])
        let shown = AnalyticsEvent.answerShown(origin: .typed, cards: 3, repo: 0, github: 0, slack: 0, notion: 0, linear: 0, gdrive: 0,
                                               jira: 2, confluence: 1, lowConfidence: false, hasAnswerLine: true, practice: false)
        XCTAssertEqual(shown.properties["jira"], .int(2))
        XCTAssertEqual(shown.properties["confluence"], .int(1))
        XCTAssertEqual(AnalyticsEvent.cardOpened(source: .jira, action: .open, place: .island).properties["source"], .text("jira"))
        XCTAssertEqual(AnalyticsEvent.cardOpened(source: .confluence, action: .open, place: .main).properties["source"], .text("confluence"))
        XCTAssertEqual(AnalyticsEvent.sourceDisconnected(tool: .atlassian).properties["tool"], .text("atlassian"))
        for value in ["jira", "confluence", "atlassian"] { XCTAssertTrue(AnalyticsFilter.closedValues.contains(value), value) }
        XCTAssertTrue(AnalyticsFilter.eventKeys.isSuperset(of: ["jira", "confluence"]))
        XCTAssertNotNil(AnalyticsFilter.filter(event: "atlassian_connected", properties: [:]))
    }

    func testDropboxEvents() {
        XCTAssertEqual(AnalyticsEvent.dropboxConnected(team: true, paperLegacy: false).name, "dropbox_connected")
        XCTAssertEqual(AnalyticsEvent.dropboxConnected(team: true, paperLegacy: false).properties, ["team": .bool(true), "paper_legacy": .bool(false)])
        XCTAssertEqual(AnalyticsEvent.dropboxConnected(team: false, paperLegacy: nil).properties, ["team": .bool(false)])
        XCTAssertEqual(AnalyticsEvent.cardOpened(source: .dropbox, action: .open, place: .island).properties["source"], .text("dropbox"))
        XCTAssertEqual(AnalyticsEvent.sourceDisconnected(tool: .dropbox).properties["tool"], .text("dropbox"))
        XCTAssertEqual(AnalyticsEvent.firstStepToolClicked(tool: .dropbox, connected: false).properties["tool"], .text("dropbox"))
        XCTAssertEqual(AnalyticsEvent.answerShown(origin: .voice, cards: 2, repo: 0, github: 0, slack: 0, notion: 0, linear: 0, gdrive: 0,
                                                  jira: 0, confluence: 0, dropbox: 2, lowConfidence: false, hasAnswerLine: true,
                                                  practice: false).properties["dropbox"], .int(2))
        XCTAssertEqual(AnalyticsEvent.answerShown(origin: .voice, cards: 0, repo: 0, github: 0, slack: 0, notion: 0, linear: 0,
                                                  gdrive: 0, jira: 0, confluence: 0, lowConfidence: false, hasAnswerLine: false,
                                                  practice: false).properties["dropbox"], .int(0))
        XCTAssertTrue(AnalyticsFilter.closedValues.contains("dropbox"))
        XCTAssertTrue(AnalyticsFilter.eventKeys.isSuperset(of: ["dropbox", "team", "paper_legacy"]))
        XCTAssertNotNil(AnalyticsFilter.filter(event: "dropbox_connected", properties: [:]))
    }

    func testAsanaEvents() {
        XCTAssertEqual(AnalyticsEvent.asanaConnected.name, "asana_connected")
        XCTAssertEqual(AnalyticsEvent.asanaConnected.properties, [:])
        let shown = AnalyticsEvent.answerShown(origin: .typed, cards: 2, repo: 0, github: 0, slack: 0, notion: 0, linear: 0, gdrive: 0,
                                               jira: 0, confluence: 0, asana: 2, lowConfidence: false, hasAnswerLine: true,
                                               practice: false)
        XCTAssertEqual(shown.properties["asana"], .int(2))
        XCTAssertEqual(AnalyticsEvent.answerShown(origin: .voice, cards: 0, repo: 0, github: 0, slack: 0, notion: 0, linear: 0,
                                                  gdrive: 0, jira: 0, confluence: 0, lowConfidence: false, hasAnswerLine: false,
                                                  practice: false).properties["asana"], .int(0))
        XCTAssertEqual(AnalyticsEvent.cardOpened(source: .asana, action: .open, place: .island).properties["source"], .text("asana"))
        XCTAssertEqual(ACardSource(rawValue: "asana"), .asana)
        XCTAssertEqual(AnalyticsEvent.sourceDisconnected(tool: .asana).properties["tool"], .text("asana"))
        XCTAssertEqual(AnalyticsEvent.firstStepToolClicked(tool: .asana, connected: false).properties["tool"], .text("asana"))
        XCTAssertTrue(AnalyticsFilter.closedValues.contains("asana"))
        XCTAssertTrue(AnalyticsFilter.eventKeys.contains("asana"))
        XCTAssertNotNil(AnalyticsFilter.filter(event: "asana_connected", properties: [:]))
    }

    func testFilterDropsUnknownEventsAndKeys() {
        XCTAssertNil(AnalyticsFilter.filter(event: "$screen", properties: ["mode": "remote"]))
        let kept = AnalyticsFilter.filter(event: "meeting_started",
                                          properties: ["mode": "remote", "$os_version": "26.6", "title": "x", "$current_url": "y"])
        XCTAssertEqual(kept.map { Set($0.keys) }, ["mode", "$os_version"])
    }

    func testVersionCheck() {
        XCTAssertNil(VersionCheck.update(last: nil, current: "0.1.12"))
        XCTAssertNil(VersionCheck.update(last: "0.1.12", current: "0.1.12"))
        let u = VersionCheck.update(last: "0.1.11", current: "0.1.12")
        XCTAssertEqual(u?.from, "0.1.11")
        XCTAssertEqual(u?.to, "0.1.12")
    }

    func testOnboardingStepsMapToTheirNamesAndDoneHasNone() {
        XCTAssertEqual(OnboardingStep.order.compactMap(\.analytics), [.welcome, .account, .sources, .listen, .ask, .meeting])
        XCTAssertNil(OnboardingStep.done.analytics)
    }

    @MainActor
    func testNothingIsSentWithoutASink() {
        XCTAssertNil(Analytics.sink)  // a test run never starts PostHog
        Analytics.capture(.updateFailed(code: 2001))
        var got: [String] = []
        Analytics.sink = { got.append($0.name) }
        defer { Analytics.sink = nil }
        Analytics.capture(.updateFailed(code: 2001))
        XCTAssertEqual(got, ["update_failed"])
    }

    func testSettingValuesAreClosed() {
        XCTAssertEqual(ASettingValue(true), .on)
        XCTAssertEqual(ASettingValue(false), .off)
        XCTAssertEqual(Retention.allCases.map(ASettingValue.init(retention:)).map(\.rawValue), ["30d", "90d", "1y", "forever"])
        XCTAssertEqual(ASettingValue(listenTo: "system"), .system)
        XCTAssertEqual(ASettingValue(listenTo: "anything"), .mix)
    }

    func testOnboardingCountsOnlyTheFirstWalkThrough() throws {
        // progress saved before the usage numbers still loads, with no flag: the walk counts
        let saved = try JSONEncoder().encode(OnboardingProgress())
        var json = try JSONSerialization.jsonObject(with: saved) as! [String: Any]
        json["counted"] = nil
        let old = try JSONDecoder().decode(OnboardingProgress.self, from: JSONSerialization.data(withJSONObject: json))
        XCTAssertTrue(OnboardingFlow.countsSteps(old))
        var p = old
        p.counted = true  // finished once; 남은 일 brings the user back through steps
        XCTAssertFalse(OnboardingFlow.countsSteps(p))
    }
}
