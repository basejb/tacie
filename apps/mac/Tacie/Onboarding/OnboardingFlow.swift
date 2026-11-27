import Foundation

/// The first-run steps (docs/11 4절). The two practice steps on the real island (ask, meeting) need something to
/// answer from; without it the flow passes over them.
enum OnboardingStep: Int, CaseIterable, Codable, Comparable {
    case welcome = 0, sources = 1, listen = 2, ask = 3, meeting = 4, done = 5
    /// Signing in to Tacie (docs/14), right after the welcome. Its raw value comes last so saved progress keeps its
    /// meaning; the order is `order`.
    case account = 6
    static let order: [OnboardingStep] = [.welcome, .account, .sources, .listen, .ask, .meeting, .done]
    static var allCases: [OnboardingStep] { order }
    static func < (a: Self, b: Self) -> Bool { order.firstIndex(of: a)! < order.firstIndex(of: b)! }
}

extension OnboardingFlow {
    /// Step and finish events go out on the first walk through only (docs/16 2-3).
    static func countsSteps(_ p: OnboardingProgress) -> Bool { p.counted != true }
}

extension OnboardingStep {
    /// The step's name in the usage numbers (docs/16); done is the finish, not a step.
    var analytics: AStep? {
        switch self {
        case .welcome: .welcome
        case .account: .account
        case .sources: .sources
        case .listen: .listen
        case .ask: .ask
        case .meeting: .meeting
        case .done: nil
        }
    }
}

/// Whether the call's sound reaches Tacie. There is no API to read this permission, so it is known only after real
/// sound came through (`granted`) or a probe heard nothing (`silent`).
enum CallAudioState: String, Codable, Equatable {
    case unknown, granted, silent
}

/// What is already true on this Mac.
struct OnboardingFacts: Equatable {
    var hasSource = false       // a connected tool with something found: a picked repository, or Notion's pages
    var accountStep = false     // Tacie AI needs a sign-in on this build (docs/14): the account step shows
    var signedIn = false
    var notionConnected = false
    var linearConnected = false
    var gdriveConnected = false
    var atlassianConnected = false
    var asanaConnected = false
    var dropboxConnected = false
    var notesOn = false  // a note folder is chosen (docs/34)
    var micGranted = false
    var micConnected = true     // an input device exists (a Mac mini has none until a headset is plugged in)
    var callAudio = CallAudioState.unknown
    var practiceReady = false   // there is something to answer from: a connected repository (later also the sample)
    var open = false            // the open edition: the account step asks how to run AI instead (2-D)
    var aiChosen = false        // the open edition: an AI mode is chosen (`AIMode`)
    var answerCards = true      // answer cards can run: not the open edition's Mac 안에서만, nor cards turned off
}

/// Saved in UserDefaults, so a relaunch (after a permission, a crash or a quit) comes back to the same step.
struct OnboardingProgress: Codable, Equatable {
    var step = OnboardingStep.welcome
    var finished = false
    var skipped: Set<OnboardingStep> = []
    /// Tools that are not ready yet and the user wants to hear about ("slack", "notion", …).
    var notify: Set<String> = []
    /// The user chose not to set up the meeting app's sound now (in-person meetings only).
    var callAudioLater = false
    /// 남은 일 rows the user hid (OnboardingLeftover.id). Optional: progress saved before it existed still loads.
    var dismissed: Set<String>? = nil
    /// The flow reached its end once and the usage numbers counted it (docs/16); walks back from 남은 일 do not count again.
    var counted: Bool? = nil
}

/// Something the user still has to do once the flow is over (5단계 "남은 일", later the main window checklist).
enum OnboardingLeftover: Equatable {
    case mic
    case connectMic
    case connectSource
    case connectNotion  // asked to hear about Notion, which is open now (docs/13 5절)
    case connectLinear  // asked to hear about Linear, which is open now (docs/18 10절)
    case connectDrive  // asked to hear about Google Drive, which is open now (docs/19 11절)
    case connectAtlassian  // asked to hear about Jira · Confluence, which is open now (docs/31)
    case signIn  // Tacie AI waits for a sign-in (docs/14)
    case chooseAI  // the open edition waits for the AI step (2-D)
    case callAudio
    case notify([String])

    /// What 숨기기 remembers. Tools to hear about are one row, whichever tools they are.
    var id: String {
        switch self {
        case .mic: "mic"
        case .connectMic: "connectMic"
        case .connectSource: "connectSource"
        case .connectNotion: "connectNotion"
        case .connectLinear: "connectLinear"
        case .connectDrive: "connectDrive"
        case .connectAtlassian: "connectAtlassian"
        case .signIn: "signIn"
        case .chooseAI: "chooseAI"
        case .callAudio: "callAudio"
        case .notify: "notify"
        }
    }
}

enum OnboardingFlow {
    /// Tacie AI goes through a sign-in in a product build; a development build shows the step with `--account-step`.
    /// The open edition has no Tacie sign-in (docs/41 0절 7번).
    /// the AI step ("회의를 정리할 AI를 골라요") is answered (#56): Tacie AI counts only with a session.
    static func aiChosen(mode: AIMode?, signedIn: Bool) -> Bool {
        guard let mode else { return false }
        return mode != .tacie || signedIn
    }

    static func accountStep(dev: Bool, open: Bool, args: [String]) -> Bool {
        !open && (!dev || args.contains("--account-step"))
    }

    /// Whether answer cards can run, so the practice has something to show: the open edition's Mac 안에서만 쓰기 sends
    /// no key, and 직접 고르기 can turn the cards off (CardsGate).
    static func answerCards(open: Bool, mode: AIMode?, cardsOff: Bool) -> Bool {
        !open || (mode != .local && !cardsOff)
    }

    static func available(_ step: OnboardingStep, _ facts: OnboardingFacts) -> Bool {
        switch step {
        case .ask, .meeting: facts.practiceReady && facts.answerCards
        case .account: facts.accountStep || facts.open
        default: true
        }
    }

    /// The steps that show as dots, in order.
    static func visible(_ facts: OnboardingFacts) -> [OnboardingStep] {
        OnboardingStep.allCases.filter { available($0, facts) }
    }

    static func next(after step: OnboardingStep, _ facts: OnboardingFacts) -> OnboardingStep {
        visible(facts).first { $0 > step } ?? .done
    }

    static func previous(before step: OnboardingStep, _ facts: OnboardingFacts) -> OnboardingStep? {
        visible(facts).last { $0 < step }
    }

    /// Where a reopened app comes back: the saved step, or the next one if that step no longer exists.
    static func resume(_ progress: OnboardingProgress, _ facts: OnboardingFacts) -> OnboardingStep {
        if progress.finished { return .done }
        if available(progress.step, facts) { return progress.step }
        return next(after: progress.step, facts)
    }

    /// The flow opens by itself until it is finished, but not when the Mac starts Tacie at login.
    static func opensAtLaunch(_ progress: OnboardingProgress?, loginItem: Bool) -> Bool {
        !loginItem && !(progress?.finished ?? false)
    }

    /// Someone who used Tacie before the onboarding existed: a repository and a microphone already. Checked once, on
    /// the first launch with no saved progress; later changes never bring the flow back by themselves (the island and
    /// 남은 일 say what is missing).
    static func alreadySetUp(_ facts: OnboardingFacts) -> Bool { facts.hasSource && facts.micGranted }

    /// A service is connected with something found: any one is enough to go on from the sources step (docs/31 8절).
    static func hasSource(github: Bool, notion: Bool, linear: Bool, gdrive: Bool, atlassian: Bool, asana: Bool = false,
                          dropbox: Bool = false, notes: Bool = false) -> Bool {
        github || notion || linear || gdrive || atlassian || asana || dropbox || notes
    }

    /// Note folders count once at least one note is in the search data (docs/34): a chosen but empty folder answers nothing.
    static func notesFound(notes: Int?) -> Bool {
        (notes ?? 0) > 0
    }

    /// Dropbox has found something once its file count is in (docs/33).
    static func dropboxFound(connected: Bool, files: Int?) -> Bool {
        connected && files != nil
    }

    /// Asana has found something once its task count is in (docs/32 8절).
    static func asanaFound(connected: Bool, tasks: Int?) -> Bool {
        connected && tasks != nil
    }

    /// Jira · Confluence has found something once either count is in (the first fetch comes newest first, so a
    /// project's issues can come before any page).
    static func atlassianFound(connected: Bool, issues: Int?, pages: Int?) -> Bool {
        connected && (issues != nil || pages != nil)
    }

    /// The listen step can move on once the microphone works; the meeting app's sound is optional.
    static func listenDone(_ facts: OnboardingFacts) -> Bool { facts.micGranted }

    static func leftovers(_ progress: OnboardingProgress, _ facts: OnboardingFacts) -> [OnboardingLeftover] {
        var out: [OnboardingLeftover] = []
        if facts.accountStep, !facts.signedIn { out.append(.signIn) }
        if facts.open, !facts.aiChosen { out.append(.chooseAI) }
        if !facts.micGranted { out.append(.mic) } else if !facts.micConnected { out.append(.connectMic) }
        // connecting lives in the main window's 처음 써 보기 (docs/11 14절); the cases stay so old hidden ids decode
        if facts.callAudio != .granted { out.append(.callAudio) }
        // seeing an answer is a demo, not setup: skipping it leaves nothing to do (2026-11-01)
        let waiting = progress.notify.subtracting(["notion", "linear", "drive", "atlassian", "asana"])
        if !waiting.isEmpty { out.append(.notify(waiting.sorted())) }
        let hidden = progress.dismissed ?? []
        return out.filter { !hidden.contains($0.id) }
    }
}

/// The saved progress in UserDefaults (JSON under one key).
struct OnboardingStore {
    static let key = "onboarding.progress"
    var defaults = UserDefaults.standard

    func load() -> OnboardingProgress? {
        defaults.data(forKey: Self.key).flatMap { try? JSONDecoder().decode(OnboardingProgress.self, from: $0) }
    }

    func save(_ progress: OnboardingProgress) {
        if let data = try? JSONEncoder().encode(progress) { defaults.set(data, forKey: Self.key) }
    }
}
