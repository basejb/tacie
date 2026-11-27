import Foundation

// Usage numbers for PostHog (docs/16): every event is a case here, and its values are numbers, Bools or one of the
// closed enums below — never text a person wrote or Tacie found (speech, questions, answers, card titles, paths, names).
// The only free text is a version string, which comes from the bundle or Sparkle. No PostHog import: tests use this file.

/// Where events go. `Analytics.start()` (Analytics.swift, PostHog) sets the sink; before that, and in tests, nothing goes
/// anywhere.
@MainActor
enum Analytics {
    /// One app (#56): usage numbers only with a Tacie session. Signed out, Tacie sends nothing but model downloads.
    nonisolated static func allowed(signedIn: Bool, hasToken: Bool) -> Bool { signedIn && hasToken }

    static var sink: ((AnalyticsEvent) -> Void)?
    static func capture(_ e: AnalyticsEvent) { sink?(e) }
}

enum AnalyticsValue: Equatable {
    case int(Int), double(Double), bool(Bool), text(String)

    var json: Any {
        switch self {
        case .int(let v): v
        case .double(let v): v
        case .bool(let v): v
        case .text(let v): v
        }
    }
}

enum AMode: String, CaseIterable { case inPerson = "inperson", remote }
enum AStep: String, CaseIterable { case welcome, account, sources, listen, ask, meeting }
enum ATrigger: String, CaseIterable { case hotkey, menu, island, main, onboarding, prompt }
/// The meeting app whose start the pill asked about (docs/04 "회의 시작 알아채기").
enum AMeetingApp: String, CaseIterable {
    case zoom, teams, slack, webex, facetime, discord, browser
}
enum AOrigin: String, CaseIterable { case voice, typed, slack }
enum APlace: String, CaseIterable { case meeting, ask, notes, sync }
enum ACardPlace: String, CaseIterable { case island, main }
enum AUpdatePlace: String, CaseIterable { case main, menu, settings }
/// What the user picked in Sparkle's update window (설치 · 나중에 · 건너뛰기).
enum AUpdateChoice: String, CaseIterable { case install, later, skip }
enum ACardSource: String, CaseIterable { case repo, github, slack, notion, linear, gdrive, jira, confluence, asana, dropbox, meeting, notes }
enum ACardAction: String, CaseIterable { case open, editor, copyPath = "copy_path", copySlack = "copy_slack" }
enum ASignOut: String, CaseIterable { case user, expired, deleted }
/// Which sign-in opened the Tacie account (docs/14 3절).
enum ASignInWith: String, CaseIterable { case google, github, apple }
enum AEndReason: String, CaseIterable { case user, heardEnd = "heard_end", silence, engineFailed = "engine_failed", limit, callEnded = "call_ended" }
/// What ended the meeting (docs/16): the pill's stop button, its 지금 끝내기 during the countdown, ⌃⌥⌘M, the menu bar menu,
/// the main window's 끝내기, quitting the app, or Tacie itself (heard end, replay over, engine gave up).
enum AStopBy: String, CaseIterable { case island, countdown, hotkey, menu, main, quit, auto }
enum AUnanswered: String, CaseIterable { case noCards = "no_cards", saidNoAnswer = "said_no_answer", noSource = "no_source" }
enum ANotesFrom: String, CaseIterable { case afterMeeting = "after_meeting", list, collected }
/// The recording kind the notes were written for (docs/23).
enum ANotesKind: String, CaseIterable {
    case meeting, interview, talk
    init(_ kind: NotesKind) { self = ANotesKind(rawValue: kind.rawValue) ?? .meeting }
}
enum AExport: String, CaseIterable { case copy, share, file }
enum AExportPart: String, CaseIterable { case page, ledger, missing, questions, transcript }
enum ASideView: String, CaseIterable { case todos, decisions, questions }
/// How the feedback card ended without a send: its close button, or 건너뛰기.
enum AFeedbackClose: String, CaseIterable { case close, skip }
enum ANamePick: String, CaseIterable { case typed, merge, person }
enum AKeptBy: String, CaseIterable { case button, pause, ask, question, talk }
enum ASettingsPage: String, CaseIterable { case general, meeting, account, ai, connections, privacy, developer }
enum ASetting: String, CaseIterable {
    case islandAlways = "island_always", meetingNotes = "meeting_notes", openAfterMeeting = "open_after_meeting"
    case autoSync = "auto_sync", autoUpdate = "auto_update", loginItem = "login_item", listenTo = "listen_to"
    case meetingPrompt = "meeting_prompt"
    case showInCapture = "show_in_capture"
    case meetingRetention = "meeting_retention", rememberPeople = "remember_people", localStt = "local_stt"
    case recordMeetings = "record_meetings"
    case recordOnly = "record_only"
    case calendar
    case recordNotice = "record_notice", pauseMusic = "pause_music"
    case mcpAccess = "mcp_access"
    case notesLanguage = "notes_language"
}
/// The AI apps Tacie can be added to (docs/38 5절), the tools they call, who called, and why a call did not answer.
enum AMCPApp: String, CaseIterable { case claudeDesktop = "claude_desktop", cursor, claudeCode = "claude_code", codex, other }
enum AMCPTool: String, CaseIterable { case search, listMeetings = "list_meetings", listLedger = "list_ledger", readTranscript = "read_transcript" }
enum AMCPClient: String, CaseIterable { case claudeDesktop = "claude_desktop", cursor, claudeCode = "claude_code", codex, other }
enum AMCPFailure: String, CaseIterable {
    case none, off, notReady = "not_ready", switching, limit, busy, network, key, down, model, badRequest = "bad_request"
}
enum ASettingValue: String, CaseIterable {
    case on, off, mix, system, days30 = "30d", days90 = "90d", year = "1y", forever
    /// notes_language: 회의 언어 or 앱 언어 (`NotesLanguage`)
    case meeting, app
    init(_ on: Bool) { self = on ? .on : .off }
    init(listenTo: String) { self = listenTo == "system" ? .system : .mix }
    init(retention r: Retention) {
        switch r {
        case .days30: self = .days30
        case .days90: self = .days90
        case .year: self = .year
        case .forever: self = .forever
        }
    }
}
enum ACleared: String, CaseIterable { case meetings, people, searchData = "search_data" }
enum AAccount: String, CaseIterable { case none, waiting, active, blocked }
/// Which model pack finished downloading (docs/16): the search pack, the speaker-voice pack, or the on-device speech pack (docs/22).
enum AModelPack: String, CaseIterable { case search, voice, stt }
enum ABuild: String, CaseIterable { case dev, product }
/// Which app sent the event (docs/27 0절: the iPhone shares these events and tells itself apart by this).
enum APlatform: String, CaseIterable { case mac, ios }
/// What the feedback card's person wished for (Mac: `FeedbackSurvey.Wish`). The raw values are what PostHog and the
/// server see (server feedback.js `WISHES`).
enum AWish: String, CaseIterable { case faster, accurate, `catch`, sources, notes, todos }
/// How the connect card ended (`connect_prompt_answered`, Mac: `ConnectPrompt.Choice`).
enum AConnectChoice: String, CaseIterable { case connect, recordOnly = "record_only", close }
/// 처음 써 보기 (docs/11 14절): which step, and which tool chip.
enum AFirstStep: String, CaseIterable { case source, meeting, card, ask }
enum ATool: String, CaseIterable { case github, notion, linear, gdrive, atlassian, asana, dropbox, notes }

/// The main window's panel (docs/36 4절). How it was opened: a meeting page's 물어보기, a collected page's 물어보기, ⌘J, or
/// 메인 창에서 이어서 물어보기 on the island's card. `prep`, `why` and `search` are the held-back entrances (`PanelFeatures`),
/// sent only while their code is switched on.
enum PanelVia: String, CaseIterable { case page, collected, shortcut, island, prep, why, search }
enum PanelScopeKind: String, CaseIterable {
    case meeting, all
}
/// What was asked: an example question of the empty screen (`suggestion_*`), one typed, or one after the first in a thread.
enum PanelAskKind: String, CaseIterable {
    case suggestionPrep = "suggestion_prep", suggestionWhy = "suggestion_why", suggestionAsk = "suggestion_ask", typed, followUp = "follow_up"
}
enum PanelSourceKind: String, CaseIterable { case meeting, source }
/// Where a panel event came from: the Mac's main window, or the iPhone's ask screen or meeting page (docs/40).
enum APanelPlace: String, CaseIterable { case mac, iphoneAsk = "iphone_ask", iphoneMeeting = "iphone_meeting" }
/// Why the iPhone ask screen fell back to the older `/meetings/ask` route.
enum AFallbackReason: String, CaseIterable { case engineMissing = "engine_missing", engineTimeout = "engine_timeout", engineFailed = "engine_failed" }
/// A proposal card of the panel (docs/38 12절): which change, and what the person did with it.
enum AProposalKind: String, CaseIterable {
    case fixWords = "fix_words", editLedger = "edit_ledger", nameSpeaker = "name_speaker", mergeSpeakers = "merge_speakers"
    case moveLines = "move_lines", rewriteNotes = "rewrite_notes", changeKind = "change_kind", renameTitle = "rename_title"
    case assignLines = "assign_lines"
    init(_ kind: String) { self = Self(rawValue: kind) ?? .fixWords }
}
enum AProposalAction: String, CaseIterable { case shown, applied, undone, dismissed }
/// The engine's `error_kind` set a panel answer can fail with ("engine": no engine to ask); anything else reads as `down`.
enum PanelError: String, CaseIterable {
    case key, credit, network, busy, down, model, engine
    init(kind: String) { self = Self(rawValue: kind) ?? .down }
}

enum AErrorKind: String, CaseIterable {
    case key, credit, network, busy, down, model, engine
    /// The on-device speech recognition model failed an utterance (`model` with `stt == "local"`), not the search model.
    case sttLocal = "stt_local"
    /// The meeting's connection turned slow (`NetHealth`): utterances take long to come back or pile up.
    case slow
    /// The engine's `error_kind`; anything unknown (or none) reads as the service being down.
    init(kind: String?) { self = kind.flatMap(Self.init(rawValue:)) ?? .down }
}

enum ChunksBucket: String, CaseIterable {
    case under1k = "<1k", to10k = "1k-10k", to100k = "10k-100k", over100k = "100k+"
    static func of(_ chunks: Int) -> ChunksBucket {
        chunks < 1_000 ? .under1k : chunks < 10_000 ? .to10k : chunks < 100_000 ? .to100k : .over100k
    }
}

enum AnalyticsEvent: Equatable {
    case appOpened(firstLaunch: Bool, loginItem: Bool)
    case appUpdated(from: String, to: String)
    case onboardingStepCompleted(step: AStep, skipped: Bool)
    case onboardingFinished(skippedCount: Int, practiced: Bool)
    case onboardingRestarted
    case signedIn(status: AAccount, with: ASignInWith)
    case signedOut(reason: ASignOut)
    case githubConnected
    case notionConnected
    case linearConnected
    case gdriveConnected
    case atlassianConnected
    case asanaConnected
    case dropboxConnected(team: Bool, paperLegacy: Bool?)
    case sourceDisconnected(tool: ATool)
    case syncStarted(first: Bool, privateRepo: Bool)
    case indexReady(first: Bool, chunks: ChunksBucket, seconds: Int)
    case modelDownloaded(resumed: Bool, seconds: Int, pack: AModelPack)
    case meetingStarted(mode: AMode, trigger: ATrigger, practice: Bool, sample: Bool, hasSource: Bool)
    case meetingEnded(mode: AMode, practice: Bool, minutes: Double, utterances: Int, questions: Int, answered: Int,
                      unanswered: Int, cards: Int, speakers: Int, namedSpeakers: Int, sttLocal: Int, sttStream: Int, reason: AEndReason, by: AStopBy = .auto,
                      eventLinked: Bool = false, invited: Int = 0)
    case meetingEndHeard
    /// 이어서 듣기 (docs/35 5절): how many minutes after it ended the meeting was taken up again.
    case meetingResumed(mode: AMode, minutesAfter: Int)
    case meetingEndKept(by: AKeptBy)
    case meetingPromptShown(app: AMeetingApp)
    case meetingPromptAccepted(app: AMeetingApp)
    case askSubmitted(inMeeting: Bool, sample: Bool)
    case answerShown(origin: AOrigin, cards: Int, repo: Int, github: Int, slack: Int, notion: Int, linear: Int, gdrive: Int, jira: Int,
                     confluence: Int, asana: Int = 0, dropbox: Int = 0, notes: Int = 0, lowConfidence: Bool,
                     hasAnswerLine: Bool, practice: Bool)
    case questionUnanswered(origin: AOrigin, reason: AUnanswered, practice: Bool)
    case cardOpened(source: ACardSource, action: ACardAction, place: ACardPlace)
    case errorShown(kind: AErrorKind, place: APlace)
    case meetingNotesOpened(from: ANotesFrom, hasLedger: Bool)
    case updateReady(to: String)
    case updateRestartClicked(place: AUpdatePlace)
    /// A downloaded version installed by itself while the Mac sat idle (docs/15 7절).
    case updateIdleRestart
    /// Sparkle's error code (SUError, e.g. 1004 = could not resume an install), never the error text.
    case updateFailed(code: Int)
    case updatePromptShown(userInitiated: Bool)
    case updatePromptChoice(choice: AUpdateChoice, userInitiated: Bool)
    case settingsOpened(page: ASettingsPage)
    case settingChanged(setting: ASetting, value: ASettingValue)
    case dataCleared(what: ACleared)
    case notesExported(how: AExport, part: AExportPart)
    case notesRewritten
    /// The notes came back written (not after a failure); `kind` is what they were written as.
    case notesWritten(kind: ANotesKind)
    /// The person changed the kind on the meeting page.
    case notesKindChanged(from: ANotesKind, to: ANotesKind)
    case collectedOpened(view: ASideView, count: Int)
    case todoChecked(done: Bool)
    case meetingPinned(on: Bool)
    case meetingRenamed
    case meetingDeleted
    case speakerNamed(pick: ANamePick, lineOnly: Bool, live: Bool)
    /// A meeting page showed suggested people for speakers left as "화자 N" (once a meeting per app run), and one was
    /// confirmed (✓) or dismissed (✕).
    case speakerSuggestionsShown(count: Int)
    case speakerSuggestionAnswered(accepted: Bool)
    case textSizeChanged(percent: Int)
    case firstStepDone(item: AFirstStep)
    case firstStepToolClicked(tool: ATool, connected: Bool)
    case connectHintClicked
    /// The feedback card (FeedbackSurvey): came up (`shown`: 1 or 2), was sent (which wishes, whether a line was written;
    /// never the line), or ended without a send.
    case feedbackShown(shown: Int)
    case feedbackSent(wishes: Set<AWish>, wrote: Bool)
    case feedbackClosed(how: AFeedbackClose)
    /// The connect card (ConnectPrompt) came up: finished meetings and their unanswered questions so far.
    case connectPromptShown(meetings: Int, unanswered: Int)
    /// The connect card ended: 연결하러 가기, 기록만 할게요 or its close button.
    case connectPromptAnswered(choice: AConnectChoice)
    /// The main window's panel (docs/36 4절): opened, a question sent, its answer in, a source pressed, and a meeting
    /// linked to the one it was prepared from (`cut`: the person cut the link; false when it was made on its own).
    case panelOpened(via: PanelVia, scope: PanelScopeKind)
    case panelAsked(kind: PanelAskKind, scope: PanelScopeKind, turn: Int, place: APanelPlace)
    case panelAnswered(ms: Int, sources: Int, notFound: Bool, errorKind: PanelError?, place: APanelPlace)
    case panelSourceOpened(kind: PanelSourceKind)
    case panelProposal(kind: AProposalKind, action: AProposalAction, places: Int, lines: Int, rewrite: Bool, remember: Bool, place: APanelPlace)
    /// The iPhone ask screen used `/meetings/ask` instead of the tool loop (`reason`), and whether the retry after a failure worked.
    case askFallback(reason: AFallbackReason)
    case askRetried(succeeded: Bool)
    case meetingLinked(cut: Bool)
    /// MCP (docs/38 7절): a row's 연결, 명령 복사 or 다시 연결 ran; an AI app's tool call was answered (`results` counts
    /// the cards, meetings, entries or lines in the answer, 0 on a failure; `failure` is `none` on success).
    case mcpInstallClicked(app: AMCPApp)
    case mcpToolCalled(tool: AMCPTool, ms: Int, results: Int, failure: AMCPFailure, client: AMCPClient)

    var name: String {
        switch self {
        case .appOpened: "app_opened"
        case .appUpdated: "app_updated"
        case .onboardingStepCompleted: "onboarding_step_completed"
        case .onboardingFinished: "onboarding_finished"
        case .onboardingRestarted: "onboarding_restarted"
        case .signedIn: "signed_in"
        case .signedOut: "signed_out"
        case .githubConnected: "github_connected"
        case .notionConnected: "notion_connected"
        case .linearConnected: "linear_connected"
        case .gdriveConnected: "gdrive_connected"
        case .atlassianConnected: "atlassian_connected"
        case .asanaConnected: "asana_connected"
        case .dropboxConnected: "dropbox_connected"
        case .sourceDisconnected: "source_disconnected"
        case .syncStarted: "sync_started"
        case .indexReady: "index_ready"
        case .modelDownloaded: "model_downloaded"
        case .meetingStarted: "meeting_started"
        case .meetingEnded: "meeting_ended"
        case .meetingResumed: "meeting_resumed"
        case .meetingEndHeard: "meeting_end_heard"
        case .meetingEndKept: "meeting_end_kept"
        case .meetingPromptShown: "meeting_prompt_shown"
        case .meetingPromptAccepted: "meeting_prompt_accepted"
        case .askSubmitted: "ask_submitted"
        case .answerShown: "answer_shown"
        case .questionUnanswered: "question_unanswered"
        case .cardOpened: "card_opened"
        case .errorShown: "error_shown"
        case .meetingNotesOpened: "meeting_notes_opened"
        case .updateReady: "update_ready"
        case .updateRestartClicked: "update_restart_clicked"
        case .updateIdleRestart: "update_idle_restart"
        case .updateFailed: "update_failed"
        case .updatePromptShown: "update_prompt_shown"
        case .updatePromptChoice: "update_prompt_choice"
        case .settingsOpened: "settings_opened"
        case .settingChanged: "setting_changed"
        case .dataCleared: "data_cleared"
        case .notesExported: "notes_exported"
        case .notesRewritten: "notes_rewritten"
        case .notesWritten: "notes_written"
        case .notesKindChanged: "notes_kind_changed"
        case .collectedOpened: "collected_opened"
        case .todoChecked: "todo_checked"
        case .meetingPinned: "meeting_pinned"
        case .meetingRenamed: "meeting_renamed"
        case .meetingDeleted: "meeting_deleted"
        case .speakerNamed: "speaker_named"
        case .speakerSuggestionsShown: "speaker_suggestions_shown"
        case .speakerSuggestionAnswered: "speaker_suggestion_answered"
        case .textSizeChanged: "text_size_changed"
        case .firstStepDone: "first_step_done"
        case .firstStepToolClicked: "first_step_tool_clicked"
        case .connectHintClicked: "connect_hint_clicked"
        case .feedbackShown: "feedback_shown"
        case .feedbackSent: "feedback_sent"
        case .feedbackClosed: "feedback_closed"
        case .connectPromptShown: "connect_prompt_shown"
        case .connectPromptAnswered: "connect_prompt_answered"
        case .panelOpened: "panel_opened"
        case .panelAsked: "panel_asked"
        case .panelAnswered: "panel_answered"
        case .panelSourceOpened: "panel_source_opened"
        case .panelProposal: "panel_proposal"
        case .askFallback: "ask_fallback"
        case .askRetried: "ask_retried"
        case .meetingLinked: "meeting_linked"
        case .mcpInstallClicked: "mcp_install_clicked"
        case .mcpToolCalled: "mcp_tool_called"
        }
    }

    var properties: [String: AnalyticsValue] {
        switch self {
        case let .appOpened(first, login): ["first_launch": .bool(first), "login_item": .bool(login)]
        case let .appUpdated(from, to): ["from_version": .text(from), "to_version": .text(to)]
        case let .onboardingStepCompleted(step, skipped): ["step": .text(step.rawValue), "skipped": .bool(skipped)]
        case let .onboardingFinished(n, practiced): ["skipped_count": .int(n), "practiced": .bool(practiced)]
        case .onboardingRestarted, .githubConnected, .notionConnected, .linearConnected, .gdriveConnected, .atlassianConnected, .asanaConnected,
             .meetingEndHeard,
             .notesRewritten, .meetingRenamed, .meetingDeleted, .connectHintClicked, .updateIdleRestart: [:]
        case let .dropboxConnected(team, paperLegacy):
            // `paper_legacy` is left out when Dropbox did not say
            paperLegacy.map { ["team": .bool(team), "paper_legacy": .bool($0)] } ?? ["team": .bool(team)]
        case let .notesWritten(kind): ["notes_kind": .text(kind.rawValue)]
        case let .notesKindChanged(from, to): ["from": .text(from.rawValue), "to": .text(to.rawValue)]
        case let .signedIn(status, with): ["status": .text(status.rawValue), "provider": .text(with.rawValue)]
        case let .signedOut(reason): ["reason": .text(reason.rawValue)]
        case let .syncStarted(first, priv): ["first": .bool(first), "private_repo": .bool(priv)]
        case let .indexReady(first, chunks, s): ["first": .bool(first), "chunks_bucket": .text(chunks.rawValue), "seconds": .int(s)]
        case let .modelDownloaded(resumed, s, pack): ["resumed": .bool(resumed), "seconds": .int(s), "pack": .text(pack.rawValue)]
        case let .meetingStarted(mode, trigger, practice, sample, source):
            ["mode": .text(mode.rawValue), "trigger": .text(trigger.rawValue), "practice": .bool(practice),
             "sample": .bool(sample), "has_source": .bool(source)]
        case let .meetingEnded(mode, practice, minutes, utterances, questions, answered, unanswered, cards, speakers, named, sttLocal, sttStream, reason, by, eventLinked, invited):
            ["mode": .text(mode.rawValue), "practice": .bool(practice), "minutes": .double((minutes * 10).rounded() / 10),
             "utterances": .int(utterances), "questions": .int(questions), "answered": .int(answered),
             "unanswered": .int(unanswered), "cards": .int(cards), "speakers": .int(speakers),
             "named_speakers": .int(named), "stt_local": .int(sttLocal), "stt_stream": .int(sttStream), "reason": .text(reason.rawValue),
             "by": .text(by.rawValue), "event_linked": .bool(eventLinked), "invited": .int(invited)]
        case let .meetingResumed(mode, after): ["mode": .text(mode.rawValue), "minutes_after": .int(after)]
        case let .meetingEndKept(by): ["by": .text(by.rawValue)]
        case let .meetingPromptShown(app), let .meetingPromptAccepted(app): ["app": .text(app.rawValue)]
        case let .askSubmitted(inMeeting, sample): ["in_meeting": .bool(inMeeting), "sample": .bool(sample)]
        case let .answerShown(origin, cards, repo, github, slack, notion, linear, gdrive, jira, confluence, asana, dropbox, notes, low, line, practice):
            ["origin": .text(origin.rawValue), "cards": .int(cards), "repo": .int(repo), "github": .int(github),
             "slack": .int(slack), "notion": .int(notion), "linear": .int(linear), "gdrive": .int(gdrive), "jira": .int(jira),
             "confluence": .int(confluence), "asana": .int(asana), "dropbox": .int(dropbox), "notes": .int(notes), "low_confidence": .bool(low), "has_answer_line": .bool(line),
             "practice": .bool(practice)]
        case let .questionUnanswered(origin, reason, practice):
            ["origin": .text(origin.rawValue), "reason": .text(reason.rawValue), "practice": .bool(practice)]
        case let .cardOpened(source, action, place):
            ["source": .text(source.rawValue), "action": .text(action.rawValue), "place": .text(place.rawValue)]
        case let .errorShown(kind, place): ["error_kind": .text(kind.rawValue), "place": .text(place.rawValue)]
        case let .meetingNotesOpened(from, ledger): ["from": .text(from.rawValue), "has_ledger": .bool(ledger)]
        case let .updateReady(to): ["to_version": .text(to)]
        case let .updateRestartClicked(place): ["place": .text(place.rawValue)]
        case let .updateFailed(code): ["code": .int(code)]
        case let .sourceDisconnected(tool): ["tool": .text(tool.rawValue)]
        case let .updatePromptShown(user): ["user_initiated": .bool(user)]
        case let .updatePromptChoice(choice, user): ["choice": .text(choice.rawValue), "user_initiated": .bool(user)]
        case let .settingsOpened(page): ["page": .text(page.rawValue)]
        case let .settingChanged(setting, value): ["setting": .text(setting.rawValue), "value": .text(value.rawValue)]
        case let .dataCleared(what): ["what": .text(what.rawValue)]
        case let .notesExported(how, part): ["how": .text(how.rawValue), "part": .text(part.rawValue)]
        case let .collectedOpened(view, count): ["view": .text(view.rawValue), "count": .int(count)]
        case let .todoChecked(done): ["done": .bool(done)]
        case let .meetingPinned(on): ["on": .bool(on)]
        case let .speakerNamed(pick, lineOnly, live): ["pick": .text(pick.rawValue), "line_only": .bool(lineOnly), "live": .bool(live)]
        case let .speakerSuggestionsShown(count): ["count": .int(count)]
        case let .speakerSuggestionAnswered(accepted): ["accepted": .bool(accepted)]
        case let .textSizeChanged(percent): ["percent": .int(percent)]
        case let .firstStepDone(item): ["item": .text(item.rawValue)]
        case let .firstStepToolClicked(tool, connected): ["tool": .text(tool.rawValue), "connected": .bool(connected)]
        case let .feedbackShown(shown): ["shown": .int(shown)]
        case let .feedbackSent(wishes, wrote):
            Dictionary(uniqueKeysWithValues: AWish.allCases.map { ("want_\($0.rawValue)", AnalyticsValue.bool(wishes.contains($0))) })
                .merging(["wrote": .bool(wrote)]) { a, _ in a }
        case let .feedbackClosed(how): ["how": .text(how.rawValue)]
        case let .connectPromptShown(meetings, unanswered): ["meetings": .int(meetings), "unanswered": .int(unanswered)]
        case let .connectPromptAnswered(choice): ["choice": .text(choice.rawValue)]
        case let .panelOpened(via, scope): ["via": .text(via.rawValue), "scope": .text(scope.rawValue)]
        case let .panelAsked(kind, scope, turn, place):
            ["kind": .text(kind.rawValue), "scope": .text(scope.rawValue), "turn": .int(turn), "place": .text(place.rawValue)]
        case let .panelAnswered(ms, sources, notFound, errorKind, place):
            // `error_kind` is left out when the answer came
            ["ms": .int(ms), "sources": .int(sources), "not_found": .bool(notFound), "place": .text(place.rawValue)]
                .merging(errorKind.map { ["error_kind": AnalyticsValue.text($0.rawValue)] } ?? [:]) { a, _ in a }
        case let .panelSourceOpened(kind): ["kind": .text(kind.rawValue)]
        case let .panelProposal(kind, action, places, lines, rewrite, remember, place):
            ["kind": .text(kind.rawValue), "action": .text(action.rawValue), "places": .int(places), "lines": .int(lines),
             "rewrite": .bool(rewrite), "remember": .bool(remember), "place": .text(place.rawValue)]
        case let .askFallback(reason): ["reason": .text(reason.rawValue)]
        case let .askRetried(succeeded): ["succeeded": .bool(succeeded)]
        case let .meetingLinked(cut): ["cut": .bool(cut)]
        case let .mcpInstallClicked(app): ["app": .text(app.rawValue)]
        case let .mcpToolCalled(tool, ms, results, failure, client):
            ["tool": .text(tool.rawValue), "ms": .int(ms), "results": .int(results), "failure": .text(failure.rawValue),
             "client": .text(client.rawValue)]
        }
    }

}

/// What may leave the Mac (docs/16 2-1): PostHog's `beforeSend` runs every event through `filter`.
enum AnalyticsFilter {
    /// The keys posthog-ios 3.92.0 adds itself (PostHogContext.swift, PostHogSDK.swift): device facts and its own bookkeeping.
    /// Recheck this list against `PostHogContext.swift` when the package version moves.
    static let sdkKeys: Set<String> = [
        "$app_build", "$app_name", "$app_namespace", "$app_version", "$device_manufacturer", "$device_model",
        "$device_name", "$device_type", "$os_name", "$os_version", "$screen_height", "$screen_width", "$locale",
        "$timezone", "$network_wifi", "$network_cellular", "$is_emulator", "$lib", "$lib_version", "$session_id",
        "$groups", "$process_person_profile", "$anon_distinct_id", "distinct_id", "token", "$is_identified", "$device_id",
    ]
    static let commonKeys: Set<String> = ["build", "account", "platform"]
    static let eventNames: Set<String> = Set(allSamples.map(\.name)).union(["$identify"])
    static let eventKeys: Set<String> = Set(allSamples.flatMap(\.properties.keys))
    /// Every raw value an event may carry as text (versions aside).
    static let closedValues: Set<String> = {
        var s = Set<String>()
        func add<E: CaseIterable & RawRepresentable>(_: E.Type) where E.RawValue == String { E.allCases.forEach { s.insert($0.rawValue) } }
        add(AMode.self); add(AStep.self); add(ATrigger.self); add(AOrigin.self); add(APlace.self); add(ACardPlace.self)
        add(AUpdatePlace.self); add(AUpdateChoice.self); add(ACardSource.self); add(ACardAction.self); add(ASignOut.self); add(ASignInWith.self); add(AEndReason.self); add(AStopBy.self)
        add(AUnanswered.self); add(ANotesFrom.self); add(ASettingsPage.self); add(ASetting.self); add(ACleared.self)
        add(AAccount.self); add(AModelPack.self); add(ABuild.self); add(APlatform.self); add(AErrorKind.self); add(ChunksBucket.self); add(ASettingValue.self)
        add(AExport.self); add(AExportPart.self); add(ASideView.self); add(ANamePick.self); add(AKeptBy.self)
        add(AMeetingApp.self); add(ANotesKind.self)
        add(PanelVia.self); add(PanelScopeKind.self); add(PanelAskKind.self); add(PanelSourceKind.self); add(PanelError.self); add(APanelPlace.self); add(AFallbackReason.self)
        add(AProposalKind.self); add(AProposalAction.self)
        add(AMCPApp.self); add(AMCPTool.self); add(AMCPClient.self); add(AMCPFailure.self)
        add(AFirstStep.self); add(ATool.self); add(AFeedbackClose.self); add(AConnectChoice.self)
        return s
    }()

    /// nil drops the event; otherwise only allowed keys go on.
    static func filter(event: String, properties: [String: Any]) -> [String: Any]? {
        guard eventNames.contains(event) else { return nil }
        let allowed = sdkKeys.union(commonKeys).union(eventKeys)
        return properties.filter { allowed.contains($0.key) }
    }

    /// One of every case, for the key lists (the payloads are placeholders).
    private static let allSamples: [AnalyticsEvent] = [
        .appOpened(firstLaunch: false, loginItem: false), .appUpdated(from: "", to: ""),
        .onboardingStepCompleted(step: .welcome, skipped: false), .onboardingFinished(skippedCount: 0, practiced: false),
        .onboardingRestarted, .signedIn(status: .none, with: .google), .signedOut(reason: .user), .githubConnected, .notionConnected, .linearConnected, .gdriveConnected,
        .atlassianConnected, .asanaConnected, .dropboxConnected(team: false, paperLegacy: false), .sourceDisconnected(tool: .github), .settingChanged(setting: .recordMeetings, value: .on),
        .syncStarted(first: false, privateRepo: false), .indexReady(first: false, chunks: .under1k, seconds: 0),
        .modelDownloaded(resumed: false, seconds: 0, pack: .search),
        .meetingStarted(mode: .remote, trigger: .menu, practice: false, sample: false, hasSource: false),
        .meetingResumed(mode: .remote, minutesAfter: 3),
        .meetingEnded(mode: .remote, practice: false, minutes: 0, utterances: 0, questions: 0, answered: 0, unanswered: 0,
                      cards: 0, speakers: 0, namedSpeakers: 0, sttLocal: 0, sttStream: 0, reason: .user),
        .meetingEndHeard, .meetingEndKept(by: .button),
        .meetingPromptShown(app: .zoom), .meetingPromptAccepted(app: .zoom),
        .askSubmitted(inMeeting: false, sample: false),
        .answerShown(origin: .voice, cards: 0, repo: 0, github: 0, slack: 0, notion: 0, linear: 0, gdrive: 0, jira: 0, confluence: 0,
                     lowConfidence: false, hasAnswerLine: false, practice: false),
        .questionUnanswered(origin: .voice, reason: .noCards, practice: false),
        .cardOpened(source: .repo, action: .open, place: .island), .errorShown(kind: .down, place: .meeting),
        .meetingNotesOpened(from: .list, hasLedger: false), .updateReady(to: ""), .updateRestartClicked(place: .main), .updateIdleRestart,
        .updateFailed(code: 0), .updatePromptShown(userInitiated: false), .updatePromptChoice(choice: .skip, userInitiated: false),
        .settingsOpened(page: .general), .settingChanged(setting: .autoSync, value: .on),
        .dataCleared(what: .meetings), .settingChanged(setting: .rememberPeople, value: .on), .settingChanged(setting: .localStt, value: .on), .dataCleared(what: .people),
        .notesExported(how: .copy, part: .page), .notesRewritten, .notesWritten(kind: .meeting),
        .notesKindChanged(from: .meeting, to: .meeting), .collectedOpened(view: .todos, count: 0),
        .todoChecked(done: false), .meetingPinned(on: false), .meetingRenamed, .meetingDeleted,
        .speakerNamed(pick: .typed, lineOnly: false, live: false), .speakerSuggestionsShown(count: 1),
        .speakerSuggestionAnswered(accepted: true), .textSizeChanged(percent: 100),
        .firstStepDone(item: .source), .firstStepToolClicked(tool: .github, connected: false), .connectHintClicked,
        .feedbackShown(shown: 1), .feedbackSent(wishes: [], wrote: false), .feedbackClosed(how: .close),
        .connectPromptShown(meetings: 3, unanswered: 1), .connectPromptAnswered(choice: .close),
        .panelOpened(via: .collected, scope: .meeting), .panelAsked(kind: .typed, scope: .all, turn: 1, place: .mac),
        .panelAnswered(ms: 0, sources: 0, notFound: false, errorKind: nil, place: .mac), .panelSourceOpened(kind: .meeting),
        .panelProposal(kind: .fixWords, action: .shown, places: 0, lines: 0, rewrite: false, remember: false, place: .mac),
        .askFallback(reason: .engineMissing), .askRetried(succeeded: false),
        .meetingLinked(cut: false),
        .mcpInstallClicked(app: .cursor),
        .mcpToolCalled(tool: .search, ms: 0, results: 0, failure: .none, client: .other),
    ]
}

/// `app_updated`: the version this Mac last opened against the one opening now; nil on a first launch or the same version.
enum VersionCheck {
    static func update(last: String?, current: String) -> (from: String, to: String)? {
        guard let last, last != current else { return nil }
        return (last, current)
    }
}

/// What the account tells the usage numbers (docs/16 3절): a sign-in links this Mac to the server's `analytics_id`,
/// a launch that is already signed in links again without an event, a sign-out starts a new anonymous id.
struct AccountAnalytics {
    var signedIn: (AAccount, String?, ASignInWith) -> Void
    var refreshed: (AAccount, String?) -> Void
    var signedOut: (ASignOut) -> Void

    static let none = AccountAnalytics(signedIn: { _, _, _ in }, refreshed: { _, _ in }, signedOut: { _ in })
}
