import AppKit
import Network
import Observation

/// Where the conversation happens. Decides what we listen to and how long an answer stays open.
enum MeetingMode: String, CaseIterable {
    case inPerson = "inperson"  // one Mac in the room: the microphone hears everyone
    case remote                 // Zoom, Meet, huddles: the call's sound plus your microphone

    var title: String { self == .inPerson ? String(localized: "대면 회의") : String(localized: "원격 회의") }
    var short: String { self == .inPerson ? String(localized: "대면") : String(localized: "원격") }
    /// People in a room glance at the screen less often, so answers stay longer there.
    var dwell: Duration { self == .inPerson ? .seconds(30) : .seconds(12) }
    var analytics: AMode { self == .inPerson ? .inPerson : .remote }
}

/// Island state: sound → utterances → engine (hear → find → answer). One answer is open at a time;
/// the rest wait in this meeting's history.
@Observable
@MainActor
final class MeetingModel {
    enum State: Equatable { case idle, starting, listening, paused, failed(String) }
    enum Shown: Equatable { case nothing, chooser, history, answer(String) }

    struct Block: Identifiable {
        enum Origin: Equatable {
            case voice(String?)  // the speaker key the engine heard it in ("화자 2"); nil without the voice model
            case typed
            case slack(channel: String, author: String)

            var analytics: AOrigin {
                switch self {
                case .voice: .voice
                case .typed: .typed
                case .slack: .slack
                }
            }
        }
        enum Phase: Equatable { case finding, found, failed(String) }
        let id: String
        let question: String
        let origin: Origin
        let at = Date()
        var phase: Phase = .finding
        var cards: [Card] = []
        var shown = false
        var answer: String?
        var cited: [Int] = []  // chunk ids the answer line cites
        var answerDone = false
        var fallback = false  // Jev failed: embedding order, no answer line
        var trouble: String?  // what failed outside, when it did (MeetingModel.say)
        var noSource = false  // nothing was connected: kept, not searched
        var firstNoSource = false  // the first such question ever: the card also says where to connect
        var waitingSync: String?  // kept because the first sync is still running: how far it is
        var sample = false  // the onboarding practice: answered from the sample data (docs/11 12절)
        var practice = false  // asked inside the onboarding's practice meeting (usage numbers, docs/16)
        var onboarding = false  // the onboarding's step 3 on the user's own data: does not count for 처음 써 보기
        var firstCardHint = false  // the first cards of a real meeting: the card says to click one (docs/11 14절)
        var connectHint = false  // nothing found with only past meetings to search: the card offers 설정 → 연결 (once a meeting)
        var lineKey: String?  // the transcript line the question was heard in, for 대화 기록 on the island

        /// The answer model read the cards and found no answer in them (its fixed sentence, `NO_ANSWER` in engine/crates/tacie-engine/src/pipeline.rs).
        var saidNoAnswer: Bool { answer?.hasPrefix("찾은 자료에는 답이 없어요") == true }
        /// Nothing certain was found: a question the material does not answer yet (a doc-gap candidate).
        var unanswered: Bool { phase == .found && (!shown || cards.isEmpty || saidNoAnswer) }
        /// The answer line stands on this card. Without citations (old engine, no line) every card counts.
        func isCited(_ card: Card) -> Bool { cited.isEmpty || card.chunk.map(cited.contains) == true }
        /// Cards the answer line does not cite read quieter, so the eye goes to the one it stands on.
        func isQuiet(_ card: Card) -> Bool { answer != nil && shown && !fallback && !saidNoAnswer && !isCited(card) }
    }

    private(set) var state: State = .idle {
        didSet { if case .failed = state {} else { limitReached = false } }
    }
    /// The failure on the card is this month's Tacie AI running out: the card offers the account page, not a new meeting.
    private(set) var limitReached = false
    /// Opens 설정 → 계정 (set by AppDelegate).
    var openAccount: () -> Void = {}
    /// Opens settings → AI (the pill's 고르기, 2-D).
    var openAISettings: () -> Void = {}
    /// Hands the engine the keys again (a local server found at the meeting's end, 2-D).
    var pushAIKeys: () async -> Void = {}
    /// Opens 설정 → 연결 (set by AppDelegate).
    var openConnections: () -> Void = {}
    /// A team tool is connected (a picked repository, Notion's pages, …), not only past meetings to search.
    var hasTeamSource: () -> Bool = { true }
    /// This meeting already offered 설정 → 연결 on a card (`connectHint`): once is enough.
    private var connectHinted = false
    private(set) var mode: MeetingMode?
    private(set) var startedAt: Date?
    /// Utterances this meeting whose words the engine recognized on this Mac (`meeting_ended.stt_local`, docs/16).
    private var sttLocalCount = 0
    private(set) var level: Float = 0
    private(set) var blocks: [Block] = []
    var shown: Shown = .nothing
    /// 대화 기록 under the pill (docs/04 3-1, 2026-11-13): while open, answers go under their question in it instead of
    /// a card under the pill, and the pill's history chip steps aside.
    private(set) var transcriptOpen = false
    var asking = false
    var presenting = false
    var query = ""
    /// A kept meeting to show: one with questions as soon as it ends, one without when its notes have something.
    var onEnded: (SavedMeeting) -> Void = { _ in }
    /// The signed-in account's space is not the one open yet (AppDelegate): nothing is asked until it is.
    var accountChanging: () -> Bool = { false }
    /// A service is connected with a repository picked. Without one a question is kept but not searched.
    var hasSource: () -> Bool = { true }
    /// The sources fully in the search data now (ResearchRules.keys); a saved question remembers them (docs/11 14절).
    var sourceKeys: () -> [String] = { [] }
    /// The first sync's progress while it runs ("검색 준비 중 42%"): questions wait for it instead of a connection.
    var firstSync: () -> String? = { nil }

    let engine: EngineProcess
    let library: MeetingLibrary
    /// The main window's panel: which meeting a starting one was prepared from, and the judging of what was prepared.
    var panel: PanelStore?
    /// The remembered people: sent with every session so the engine knows them by voice, and taught at a meeting's end.
    let vault: VoiceVault
    /// The meeting being kept: what was said so far (questions are added from `blocks` when it is saved).
    private var record: SavedMeeting?
    /// Bumped when a restarted engine gives a new session: its utterance ids start at s1u1 again.
    private var epoch = 0
    /// Where the utterancer's 0 sits in the meeting's recording: the earlier sound's length when it was taken up again.
    private var audioOffsetMs = 0
    /// The meeting taken up again (이어서 듣기) this time, if any: its notes are written from the whole transcript.
    private(set) var resumed: String?
    /// The meeting as it was before it was taken up again: put back as it was if the resume never gets to listening.
    private var resumeOriginal: SavedMeeting?
    private var checkpoints: Task<Void, Never>?
    /// Which meeting each question belongs to: a result that lands after its meeting ended still goes to that meeting.
    private var meetingOf: [String: String] = [:]
    /// Unanswered questions of a meeting that ended, kept when the next one starts so their answers still find a home.
    private var closing: [String: Block] = [:]
    private var session: String?
    /// This meeting records only: no question is told apart, so no answer card comes up. Starts from the setting; the
    /// pill switches it for this meeting alone.
    private(set) var recordOnly = false
    private var source: SoundSource?
    private let utterancer = Utterancer()
    /// The sound of the meeting going on, when it is recorded (decided once, as it starts), with its meeting's id.
    private var recorder: (id: String, file: MeetingRecorder)?
    /// The last 3 lines, kept by line key like the engine's ids: backchannels stay out and a joined piece replaces its first half.
    private var context: [(id: String, line: String)] = []
    /// The meeting's voices as the engine last gave them, by speaker key: given back to a restarted engine, and kept at
    /// the end when the engine is gone. Only in memory while the meeting goes on.
    private var prints: [String: MeetingPrint] = [:]
    /// The remembered person each voice sounds closest to without being sure, by speaker key: the namer puts them first.
    private var nearPeople: [String: String] = [:]
    /// Speaker keys heard in this meeting: a new one asks the engine for the voices again.
    private var heardVoices: Set<String> = []
    /// Counts voices fetches: an older one that answers late (with an older exclude list) is dropped.
    private var voicesAsked = 0
    /// Lines heard since the voices were last asked for: every `SmallSpeakers` checks again as voices settle.
    private var linesSinceVoices = 0
    /// The last names table sent: the next waits for it, so the engine ends with the newest.
    private var namesSent: Task<Void, Never>?
    /// Keys ("e<epoch>:<uid>") of first halves the engine joined into a later line: one that comes back after its joined
    /// line stays out. Keyed with the epoch: a restarted engine reuses s1u1.
    private var joined: Set<String> = []
    private var counter = 0
    /// Utterances sent to the engine and not answered yet: the ledger waits for a meeting's last words.
    private var pendingHears = 0
    /// Undo for naming speakers on meeting pages: ⌘Z (the main window hands it out) and the toast's 되돌리기.
    @ObservationIgnored let namingUndo = UndoManager()
    /// The last suggestions made (화자 추천) and what they were made from: a page asks on every draw.
    @ObservationIgnored private var suggestCache: (stamp: SuggestStamp, value: [String: String])?
    /// Meetings whose suggestions were counted once (speaker_suggestions_shown) since the app opened.
    @ObservationIgnored private var suggestionsCounted: Set<String> = []
    /// What the last naming did, shown at the bottom of the page for a moment; nil once it fades or is undone.
    private(set) var namingToast: NamingToast?
    /// Every engine reply to an utterance (`LocalSTT` reads where it was recognized).
    var onHeard: (Heard) -> Void = { _ in }
    /// On-device speech recognition is switched on but its model is not ready: what to say and how many downloads
    /// have started (docs/22 4-4). Nil: nothing to say.
    var sttState: () -> STTMeetingState? = { nil }
    /// The engine's on-device model keeps failing while its file is fine: recognition is paused, not sent to the server.
    var sttPaused: () -> Bool = { false }
    /// The one line under the pill that says this meeting is recognized on the server; it closes by itself.
    private(set) var sttNotice: String?
    private var sttNoticeOnce = STTMeetingNotice()
    private var sttNoticeTask: Task<Void, Never>?
    /// The pill's warning for a paused recognition (the pill shows it like a failing service).
    var sttNote: String? { sttPaused() || sttLocalFailing ? LocalSTTCopy.paused : nil }
    /// The last utterance failed on this Mac's speech recognition model (`model` with `stt == "local"`): the pill says
    /// recognition paused, not "검색 모델을 받고 있어요". The next reply that comes back fine clears it.
    private(set) var sttLocalFailing = false
    private var hovering = false
    private var collapse: Task<Void, Never>?
    /// The pointer rests on the pill row: the open button beside the pill and the pill's pause and stop show. It comes
    /// on after a short pause (a pointer passing over on its way elsewhere shows nothing) and stays a moment after it
    /// leaves, long enough to cross the 10 pt gap to the button and no longer (a longer wait read as the controls hanging on).
    private(set) var pillHovered = false
    private var pillHover: Task<Void, Never>?
    /// Set when someone said the meeting is over ("오늘은 여기까지 하죠"): the meeting ends on its own at this moment unless
    /// someone talks on, a question comes or someone clicks 계속 듣기 (MeetingEnd).
    private(set) var closingAt: Date?
    /// When the closing words' utterance was cut: lines said after it keep the meeting going.
    private var closingFrom: Date?
    private var closer: Task<Void, Never>?
    /// The VAD is inside an utterance right now (someone is talking).
    private var speaking = false
    /// When each utterance still waiting for STT was cut, so the end countdown can wait for words said after the closing.
    private var hearsInFlight: [Int: Date] = [:]
    private var hearSerial = 0
    /// A meeting app began using the microphone: the pill asks to listen along (같이 듣기) until it folds back by itself
    /// (docs/04 "회의 시작 알아채기"). Recording never starts on its own.
    private(set) var prompt: MeetingApp?
    /// The question folded to the app's icon beside the eyes.
    private(set) var promptFolded = false
    private var promptFold: Task<Void, Never>?
    /// The prompt came or went: the macOS notification follows it.
    var onPromptChanged: (MeetingApp?) -> Void = { _ in }
    /// 같이 듣기 was clicked: start a remote meeting.
    var onAcceptPrompt: (MeetingApp) -> Void = { _ in }
    /// Tacie ended a kept meeting by itself (the call app let go, an ending phrase, a long quiet).
    var onAutoEnded: (SavedMeeting, AEndReason) -> Void = { _, _ in }
    /// Something outside keeps failing while we listen (no connection, no key…). The pill says it until a call works.
    private(set) var trouble: String?
    /// The meeting goes on without a microphone (it went away, or a remote meeting began without one).
    /// Unlike `trouble` it stays until the input is back: calls that work say nothing about the microphone.
    private(set) var micNote: String?
    /// A remote meeting through the tap whose sound was never confirmed (a refused permission is silent, not an error).
    /// Cleared the moment real call sound comes through.
    private(set) var callNote: String?
    /// An in-person meeting where nothing was transcribed for a while (`QuietMeeting`): sound playing in earphones
    /// never reaches the microphone. Cleared by the next line heard.
    private(set) var quietNote: String?
    /// 녹음 알림 (docs/35 6-1): under the pill for a few seconds once a remote meeting starts recording.
    private(set) var recordNotice: RecordNotice?
    private var recordNoticeTask: Task<Void, Never>?
    static let recordNoticeKey = "recordNotice"
    static var recordNoticeOn: Bool { UserDefaults.standard.object(forKey: recordNoticeKey) as? Bool ?? true }
    /// Puts the participant notice for a recorded meeting on the clipboard (TacieApp.copyNotice(recording:)).
    var copyRecordNotice: () -> Void = {}
    private var quiet = QuietMeeting()
    private var quietWatch: Task<Void, Never>?
    private var callHeard: NSObjectProtocol?
    /// The meeting going on (or that just ended) is the onboarding's practice: it is not kept.
    private var practicing = false
    /// The meeting going on searches the sample data (a practice with nothing connected).
    private var sampleMeeting = false
    /// The onboarding's meeting practice, answered ahead: its spoken question is known, so nothing is transcribed and
    /// the answer comes up the moment the voice stops, not after STT, the gate, the search and the answer line.
    private var rehearsal: (question: String, sample: Bool, result: Task<Rehearsed?, Never>)?
    private var failures = 0
    /// The connection as the meeting sees it (`NetHealth`): the pill's signal bars. `.ok` outside a meeting.
    private(set) var net: NetHealth.Level = .ok
    private var health = NetHealth()
    private var netWatch: Task<Void, Never>?
    private var pathMonitor: NWPathMonitor?
    /// The line under the pill came once this meeting for a lost connection (it does not come again).
    private var netNoticed = false
    /// The last utterance was recognized on this Mac: a lost connection stops answers, not the transcript.
    private var lastSTTLocal = false
    /// The recorder and the dev-build Soniox shadow share the Utterancer's samples (docs/25 그림자 모드).
    private let fanout = SampleFanout()
    private var shadow: ShadowStt?
    private var aiChanged: NSObjectProtocol?
    /// How this meeting is transcribed (docs/25), decided when it starts.
    private var sttPath: SttPath = .server
    private var streamOpenedAt: Date?
    private var streamOpening = false
    private var streamRetryAt: Date?
    /// Connections this meeting that failed or dropped (not the 9-minute swaps): retries back off, then stop.
    private var streamFailures = 0
    /// Utterances this meeting transcribed by Soniox (`meeting_ended.stt_stream`, docs/16).
    private var sttStreamCount = 0
    /// The sentence being said, for the meeting page (Soniox main path only).
    private var liveNow: LiveLine?
    /// Sentences Soniox finished, shown until the engine's line for each comes back (by utterance).
    private var liveHeld: [(id: Int, text: String)] = []
    private var liveHeldNext = 0
    var liveLine: LiveLine? {
        let line = LiveLine(held: liveHeld.map(\.text), said: liveNow?.said ?? "", partial: liveNow?.partial ?? "")
        return line.isEmpty ? nil : line
    }
    /// Debug control (`net-state`): shows a level without touching the network (captures).
    private var netForced: NetHealth.Level?
    // usage numbers (docs/16): how this meeting began and ended, errors already counted, questions already counted
    private var trigger: ATrigger = .menu
    private var endReason: AEndReason = .user
    private var errorsSent = ErrorOnce()
    /// Whether the engine can call Jev (a TypeSafe or OpenRouter key, or the relay): IndexKeeper keeps it from `/config`.
    var jevAvailable: () -> Bool = { true }
    /// Whether this Mac's speech recognition model is ready (TacieApp sets it from LocalSTT): a Soniox user streams while
    /// it downloads (2-B, ShadowWiring.path).
    var localSttReady: () -> Bool = { true }
    private var outcomeSent: Set<String> = []
    /// `--hold-ms N` waits before searching so the in-between states can be seen (screenshots)
    private let holdMs: Int = {
        LaunchArgs.value("--hold-ms").flatMap(Int.init) ?? 0
    }()

    init(engine: EngineProcess, library: MeetingLibrary, vault: VoiceVault) {
        self.engine = engine
        self.library = library
        self.vault = vault
        aiChanged = NotificationCenter.default.addObserver(forName: AIChoice.changed, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.aiChoiceChanged() }
        }
        utterancer.onLevel = { [weak self] l in
            Task { @MainActor in self?.push(level: l) }
        }
        utterancer.onSpeaking = { [weak self] on in
            Task { @MainActor in self?.speaking = on }
        }
        utterancer.onUtterance = { [weak self] wav, gap, speaker, startMs in
            Task { @MainActor in self?.heard(wav, gap: gap, speaker: speaker, startMs: startMs) }
        }
        engine.onChange = { [weak self] s in self?.engineChanged(s) }
    }

    var isRunning: Bool { state == .listening || state == .paused || state == .starting }
    /// The answer on screen: the one opened, or an earlier one the reader is looking at (`peeking`).
    var current: Block? {
        guard case .answer(let id) = shown else { return nil }
        return blocks.first { $0.id == (peeking ?? id) }
    }
    /// While the pointer is on the island the reader keeps what they look at: a question heard meanwhile
    /// opens behind it, and the trail under the card shows earlier ones on hover. Leaving goes to the newest.
    private(set) var peeking: String?
    /// The last few questions, oldest first, for the trail under the card.
    var trail: [Block] { blocks.count > 1 ? Array(blocks.prefix(4).reversed()) : [] }
    var opened: String? {
        guard case .answer(let id) = shown else { return nil }
        return id
    }

    // MARK: keeping the meeting

    /// The meeting going on, with its questions so far (oldest first). Nil when no meeting runs.
    var live: SavedMeeting? {
        guard isRunning, var r = record else { return nil }
        r.questions = blocks.filter { meetingOf[$0.id] == r.id }.reversed().compactMap { Self.saved($0, keys: sourceKeys()) }
        return r
    }

    /// `live?.id` without building the meeting: the main window asks it several times per render.
    var liveID: String? { isRunning ? record?.id : nil }

    /// The meeting a question was caught in (it may have ended since). Nil for questions outside a meeting.
    func meeting(of question: String) -> String? { meetingOf[question] }

    /// Written every minute and after every answer, so a crash keeps the meeting up to then.
    private func checkpoint() {
        if let recorder, record?.id == recorder.id { record?.recordingSeconds = recorder.file.seconds }
        if let live { library.put(live) }
    }

    /// 개인정보's 회의 정리: off sends nothing at the end, so there is no summary or ledger.
    static var notesOn: Bool { UserDefaults.standard.object(forKey: "meetingNotes") as? Bool ?? true }

    /// 개인정보's 회의 녹음 (`recordMeetings`, on by default).
    static var recordMeetings: Bool { MeetingRecording.setting }

    /// 기록만 하기 (settings and the chooser): meetings start without answer cards, only recording, transcript and notes.
    static let recordOnlyKey = "recordOnly"
    static var recordOnlyDefault: Bool { UserDefaults.standard.bool(forKey: recordOnlyKey) }

    static func recordingWanted(setting: Bool, practice: Bool, sample: Bool) -> Bool {
        MeetingRecording.wanted(setting: setting, practice: practice, sample: sample)
    }

    static func line(for h: Heard, key: String, at: Date, startMs: Int?, speaker: String? = nil) -> TranscriptLine {
        MeetingRecording.line(for: h, key: key, at: at, startMs: startMs, speaker: speaker)
    }

    /// Closes the recording. Joining its parts decodes the whole meeting (seconds for an hour), so it runs off the main
    /// thread; the exact length then goes into the saved meeting. Sound that is not kept (the meeting was not kept, or
    /// it was deleted meanwhile) is deleted once the files are closed, so nothing is left behind.
    private func finishRecording(keep: Bool) {
        guard let r = recorder else { return }
        let id = r.id, file = r.file
        recorder = nil
        fanout.recorder = nil
        let library = library
        DispatchQueue.global(qos: .utility).async {
            let seconds = file.finish()
            Task { @MainActor in
                if keep, library.meetings.first(where: { $0.id == id })?.recording == .kept {
                    library.update(id) { $0.recordingSeconds = seconds }
                    library.audioChanged(id)   // the parts became one file: a page holding them looks again
                    Log.info("meeting recording closed: \(Int(seconds)) s")
                } else {
                    library.deleteAudio(id)
                    Log.info("meeting recording not kept: deleted")
                }
            }
        }
    }

    private static func saved(_ b: Block, keys: [String]) -> SavedQuestion? {
        let asker: String?, voice: String?
        switch b.origin {
        case .voice(let key): asker = nil; voice = key  // who asked is the speaker now, not the channel
        case .typed: asker = "typed"; voice = nil
        case .slack: return nil
        }
        return SavedQuestion(id: b.id, question: b.question, asker: asker, at: b.at, answer: b.answer, cards: b.cards.map(\.forRecord),
                             cited: b.cited, unanswered: b.unanswered, noSource: b.noSource ? true : nil, voice: voice,
                             searchedWith: b.noSource || keys.isEmpty ? nil : keys)
    }

    /// A resume that never got to listening: the recording is joined again with nothing new, and the meeting is put
    /// back as it was (its end, no new gap). Nothing of the earlier meeting is lost.
    private func abandonResume() {
        guard let o = resumeOriginal else { return }
        resumeOriginal = nil
        resumed = nil
        library.put(o)
        if recorder?.id == o.id { finishRecording(keep: o.recording == .kept) }
    }

    /// Saves the meeting that just ended or broke off, and returns it when it is kept.
    /// `problem` says why its notes cannot be written (no engine session); `limited`, that it was the month's limit.
    private func closeRecord(problem: String?, limited: Bool = false) -> SavedMeeting? {
        checkpoints?.cancel()
        defer { record = nil }
        if let recorder, record?.id == recorder.id { record?.recordingSeconds = recorder.file.seconds }
        guard var final = live else {
            // a resume stopped before listening began: the meeting goes back to how it was, sound and all
            if let o = resumeOriginal, record == nil || record?.id == o.id {
                abandonResume()
                return nil
            }
            // stopped before listening began: the record written as the recording started goes with its sound
            if let recorder { library.remove(recorder.id) }
            finishRecording(keep: false)
            return nil
        }
        final.end = Date()
        let kept = final.keep && !practicing
        finishRecording(keep: kept)
        guard kept else {
            library.remove(final.id)  // a checkpoint (or the recording's start) may have written it
            return nil
        }
        if !Self.notesOn {
            final.notes = .off
        } else if let problem {
            final.notes = .failed
            final.notesProblem = problem
            final.notesLimit = limited ? .stopped : nil
        }
        library.put(final)
        return final
    }

    /// A question of a meeting that already ended goes straight into its saved copy.
    private func settle(_ b: Block) {
        guard let mid = meetingOf[b.id], mid != record?.id, let q = Self.saved(b, keys: sourceKeys()) else { return }
        library.update(mid) { $0.setQuestion(q) }
    }

    /// A transcribed line goes to the meeting it was heard in, even when that meeting has ended since.
    private func addLine(_ line: TranscriptLine, replacing first: String?, to meeting: String?) {
        guard let meeting else { return }
        if record?.id == meeting {
            record?.addLine(line, replacing: first)
            quiet.heard(at: Date(), text: line.text)
            quietNote = nil
        } else {
            library.update(meeting) { $0.addLine(line, replacing: first) }
        }
    }

    // MARK: meeting

    /// `practice`: the onboarding's meeting practice. It runs like any meeting but is not kept (no saved meeting, no
    /// notes, the main window does not open on it).
    /// `sample`: the practice searches the onboarding's sample data instead of the user's (nothing connected yet).
    func start(_ mode: MeetingMode, source: SoundSource, practice: Bool = false, sample: Bool = false,
               trigger: ATrigger = .menu, app: MeetingApp? = nil, resuming: SavedMeeting? = nil) {
        guard !isRunning else { return }
        resumed = resuming?.id
        resumeOriginal = resuming
        dismissPrompt()
        rehearsal = nil
        self.trigger = practice ? .onboarding : trigger
        endReason = .user
        errorsSent.reset()
        practicing = practice
        sampleMeeting = practice && sample
        sttNoticeOnce.reset()
        sttNotice = nil
        if Build.isOpen, !practice, AIMode.saved() == .local { Task { await warnNoNotesModel() } }
        // the onboarding practice is there to show an answer card
        recordOnly = CardsGate.startsRecordOnly(practice: practice, setting: Self.recordOnlyDefault, open: Build.isOpen,
                                                jev: jevAvailable(), cardsOff: OpenAIMeeting.cardsOffNow)
        // decided now: turning the setting on or off mid-meeting changes the next meeting, not this one
        let recordThis = Self.recordingWanted(setting: Self.recordMeetings, practice: practice, sample: sampleMeeting)
        // a replayed file (captures, --replay) has no meeting app to hear
        callNote = mode == .remote && !(source is FileSource) && CallAudio.usesTap && CallAudio.lastState != .granted ? String(localized: "회의 앱 소리를 확인하지 못했어요") : nil
        if callNote != nil, callHeard == nil {
            callHeard = NotificationCenter.default.addObserver(forName: CallAudio.heard, object: nil, queue: .main) { [weak self] _ in
                Task { @MainActor in self?.callNote = nil }
            }
        }
        state = .starting
        // a meeting beginning stops any recording being played back, before its sound starts
        NotificationCenter.default.post(name: .meetingStarted, object: nil)
        self.mode = mode
        self.source = source
        for b in blocks where !b.answerDone && meetingOf[b.id] != nil { closing[b.id] = b }
        blocks = []
        context = []
        joined = []
        prints = [:]
        nearPeople = [:]
        heardVoices = []
        shown = .nothing
        (source as? MicWatching)?.onInput = { [weak self] ok in
            Task { @MainActor in self?.micChanged(ok) }
        }
        // A replayed recording ends the meeting when the file ends, the way the stop button does. With a handler set the
        // file does not flush, so the last words are cut by stop()'s sendRest while the session still exists.
        if let file = source as? FileSource {
            file.onEnd = { [weak self, weak file] in
                Task { @MainActor in
                    guard let self, let file, self.source === file, self.isRunning else { return }
                    Log.info("replay ended: stopping the meeting")
                    // cleared after stop(): its last words go out as an utterance, and the practice must not send them
                    let rehearsal = self.rehearsal
                    self.stop(by: .auto)
                    self.rehearsal = nil
                    if let rehearsal { self.reveal(rehearsal) }
                }
            }
        }
        Task {
            guard let client = await waitForEngine() else { return }
            // this month's Tacie AI is used up: nothing would be transcribed, so the meeting does not begin
            if Relay.on, await TacieAccount.shared.usedUp() {
                guard state == .starting, self.source === source else { return }
                Log.info("meeting not started: this month's Tacie AI is used up")
                countError(AErrorKind.credit.rawValue, place: .meeting)
                state = .failed(Account.limitLine(kept: false))
                limitReached = true
                return
            }
            do {
                // nothing to search yet: still tell questions apart, so they are kept and searched once the data is here
                let sample = sampleMeeting, gateEmpty = !sampleMeeting && !hasSource(), lookup = !recordOnly
                // 이어서 듣기: the meeting's voices so far go to the new session, so its speakers keep their numbers
                let earlier = resuming.map { vault.prints(meeting: $0.id) } ?? []
                let after = resuming?.highestSpeaker ?? 0
                session = try await openSession(people: rememberedPeople()) { people in
                    if !earlier.isEmpty {
                        do {
                            return try await client.newSession(sample: sample, gateEmpty: gateEmpty, lookup: lookup, people: people,
                                                               restore: earlier, after: after)
                        } catch {
                            // without the voices the speakers still go on after the highest number used
                            Log.info("resume: voices not restored (\(error)), numbers go on after \(after)")
                        }
                    }
                    return try await client.newSession(sample: sample, gateEmpty: gateEmpty, lookup: lookup, people: people, after: after)
                }
                let now = Date()
                let calendarLookup = practicing || sampleMeeting || resuming != nil || !CalendarStore.offered ? nil
                    : Task { await CalendarStore.shared.eventNow(at: now, app: app) }
                let meeting: SavedMeeting
                if var again = resuming {
                    again.reopen(at: now)
                    again.notes = .waiting  // written again from the whole transcript when it ends
                    prints = Dictionary(earlier.map { ($0.key, $0) }, uniquingKeysWith: { a, _ in a })
                    meeting = startRecording(again, wanted: again.recording == .kept, resuming: true)
                    if meeting.recording != .kept { library.put(meeting) }  // written now, as a recorded one is
                } else {
                    var fresh = SavedMeeting(mode: mode.rawValue, start: now)
                    if let link = panel?.prepLink(startingAt: now) {
                        fresh.linkedFrom = link.from
                        Analytics.capture(.meetingLinked(cut: false))
                        fresh.prepared = link.items
                    }
                    meeting = startRecording(fresh, wanted: recordThis)
                }
                do {
                    try await source.start(into: utterancer)
                } catch {
                    // a resume: the earlier meeting goes back as it was, never removed (stop() may have done it already)
                    if resuming != nil { abandonResume(); throw error }
                    if recorder != nil {
                        finishRecording(keep: false)
                        library.remove(meeting.id)
                    }
                    throw error
                }
                // the calendar event, read while the sound starts (at most a second); awaited before the check below,
                // so a stop meanwhile is caught there. Its invited names reach the speech connection's terms.
                let event = await calendarLookup?.value
                // stopped (or another meeting begun) while the source was starting: nothing of this one stays
                guard state == .starting, self.source === source else {
                    source.stop()   // first: no more samples from the audio thread while the recorder lets go
                    if resuming != nil {
                        abandonResume()  // a no-op when stop() already put it back
                        Log.info("resume stopped while starting: the meeting stays as it was")
                        return
                    }
                    if recorder?.id == meeting.id { finishRecording(keep: false) }
                    if meeting.recording != nil, record?.id != meeting.id { library.remove(meeting.id) }
                    Log.info("meeting stopped while starting")
                    return
                }
                startedAt = now
                sttLocalCount = 0
                connectHinted = false
                sttStreamCount = 0
                streamFailures = 0
                streamRetryAt = nil
                record = meeting
                epoch = resuming?.nextEpoch ?? 0
                checkpoints?.cancel()
                checkpoints = Task { [weak self] in
                    while !Task.isCancelled {
                        try? await Task.sleep(for: .seconds(60))
                        guard !Task.isCancelled else { return }
                        self?.checkpoint()
                    }
                }
                if let event {
                    record?.link(MeetingEvent(id: event.id, title: event.title, start: event.start, end: event.end),
                                 invited: event.invited, meaningful: CalendarLink.meaningful(event.title))
                    checkpoint()
                    Log.info("calendar linked (invited: \(event.invited.count))")
                }
                if !practicing && !sampleMeeting { MediaPause.shared.pauseAll() }
                if mode == .remote, recorder != nil, resuming == nil, !practicing, Self.recordNoticeOn { showRecordNotice() }
                state = .listening
                watchQuiet(mode: mode, from: now)
                watchNet()
                startShadow(client: client, session: session, meeting: meeting.id)
                Log.info("meeting started (\(mode.rawValue))\(recorder != nil ? ", recording" : "")")
                if let r = resuming?.end {
                    Analytics.capture(.meetingResumed(mode: mode.analytics, minutesAfter: max(0, Int(now.timeIntervalSince(r) / 60))))
                    pushNames()  // the names given so far, for the new session's speakers
                } else {
                    Analytics.capture(.meetingStarted(mode: mode.analytics, trigger: self.trigger, practice: practicing,
                                                      sample: sampleMeeting, hasSource: hasSource()))
                }
                if !practicing && !sampleMeeting { FirstStepsStore.shared.mark(.meeting) }
            } catch {
                Log.info("meeting failed to start: \(error)")
                state = .failed(Self.explain(error))
            }
        }
    }

    /// Starts the utterancer's clock over (lines are placed by it) and, when the meeting is recorded, opens its recorder.
    /// The meeting's file is written first, so sound on disk always has its meeting (retention and deletion find it).
    /// A recorder that cannot be made (disk) leaves the meeting going on without a recording.
    private func startRecording(_ meeting: SavedMeeting, wanted: Bool, resuming: Bool = false) -> SavedMeeting {
        utterancer.resetClock()
        audioOffsetMs = 0
        fanout.reset()
        fanout.recorder = nil
        utterancer.onSamples = { [fanout] in fanout.feed($0) }
        recorder = nil
        guard wanted else { return meeting }
        var m = meeting
        m.recording = .kept
        if !resuming { m.recordingSeconds = 0 }
        library.put(m)
        do {
            let file = try MeetingRecorder(url: library.audioFile(m.id), resuming: resuming)
            // the utterancer counts from 0 again: lines heard now sit after the sound the meeting already has
            audioOffsetMs = Int(file.seconds * 1000)
            recorder = (m.id, file)
            fanout.recorder = { file.append($0) }
            return m
        } catch {
            Log.info("meeting recording could not start (\(error.localizedDescription)): going on without it")
            if !resuming { library.remove(m.id) }
            return meeting
        }
    }

    /// `meeting_ended` for the meeting stopping now; nil when it never got to listening (no `meeting_started` either).
    private func endedEvent(reason: AEndReason, by: AStopBy = .auto) -> AnalyticsEvent? {
        guard let startedAt, let mode else { return nil }
        return MeetingStats.ended(live, mode: mode.analytics, practice: practicing, startedAt: startedAt, now: Date(), sttLocal: sttLocalCount, sttStream: sttStreamCount, reason: reason, by: by)
    }

    /// ScreenCaptureKit reports a missing permission as a bare error code; say what to do instead.
    private static func explain(_ error: Error) -> String {
        let e = error as NSError
        if e.domain == "com.apple.ScreenCaptureKit.SCStreamErrorDomain" && e.code == -3801 {
            return String(localized: "회의 앱 소리를 들으려면 권한이 필요해요. 시스템 설정 → 개인정보 보호 및 보안 → 화면 및 시스템 오디오 녹음에서 Tacie를 켜 주세요.")
        }
        return e.localizedDescription
    }

    /// `by`: what ended it, for the log and `meeting_ended` (an unexplained stop mid-talk on 2026-11-11 left no trace of which).
    func stop(by: AStopBy) {
        let ended = endedEvent(reason: endReason, by: by)
        let autoReason = by == .auto && [.callEnded, .heardEnd, .silence].contains(endReason) ? endReason : nil
        if isRunning { Log.info("meeting stopping: \(by.rawValue)") }
        let limited = endReason == .limit
        endReason = .user
        closer?.cancel()
        closingAt = nil
        closingFrom = nil
        source?.stop()
        source = nil
        MediaPause.shared.resumeAll()
        recordNoticeTask?.cancel()
        recordNotice = nil
        // the last words, still inside the VAD's 0.7 s of silence, while the session is still there (not past the limit:
        // they would only be refused too)
        if !limited { sendRest() }
        stopShadow()
        let open = session.flatMap { s in engine.client.map { (s, $0) } }
        // the voices fetched during the meeting, before the notes are asked for; the end checks again with all of them
        if record?.foldSmallSpeakers(Array(prints.values)) == true, let (sid, client) = open, let map = record?.engineNames() {
            let before = namesSent
            namesSent = Task {
                await before?.value
                do { try await client.names(session: sid, map) } catch { Log.info("names failed: \(error)") }
            }
        }
        session = nil
        if let ended { Analytics.capture(ended) }
        let problem = limited ? Self.say("credit") : open == nil ? Self.say("engine") : nil
        if let final = closeRecord(problem: problem, limited: limited) {
            keepVoices(of: final.id, session: open)
            if let autoReason { onAutoEnded(final, autoReason) }
            if !final.questions.isEmpty { onEnded(final) }
            if final.notes == .waiting, final.id == resumed {
                // the session heard only the part after the gap: the notes come from the whole transcript instead
                Task {
                    let deadline = Date().addingTimeInterval(5)
                    while pendingHears > 0 && Date() < deadline { try? await Task.sleep(for: .milliseconds(100)) }
                    guard let m = library.meetings.first(where: { $0.id == final.id }) else { return }
                    writeAgain(m.id, m, kind: NotesFlow.rewriteKind(m))
                }
            } else if final.notes == .waiting, let (id, client) = open {
                fetchNotes(for: final.id, asked: !final.questions.isEmpty, session: id, client: client)
            }
        }
        resumed = nil
        resumeOriginal = nil
        startedAt = nil
        level = 0
        state = .idle
        trouble = nil
        micNote = nil
        callNote = nil
        stopQuietWatch()
        stopNetWatch()
        sttNoticeTask?.cancel()
        sttNotice = nil
        sttLocalFailing = false
        failures = 0
        peeking = nil
        shown = .nothing
        transcriptOpen = false
        presenting = false
        Log.info("meeting stopped")
    }

    /// The app is quitting: the recording's open part is closed now, so the last minute is not lost when the process
    /// exits before `stop()`'s background join could run. Only a file close; the parts stay and play as they are.
    func closeRecordingForQuit() {
        guard let recorder else { return }
        let seconds = recorder.file.close()
        if record?.id == recorder.id { record?.recordingSeconds = seconds }
    }

    /// Every few seconds: in person, nothing transcribed for `QuietMeeting.after` puts the line on the pill (a remote
    /// meeting's own check is `callNote`); in either, no word for `QuietMeeting.endAfter` starts the end countdown.
    /// Not in the practice (it is short and scripted).
    private func watchQuiet(mode: MeetingMode, from start: Date) {
        stopQuietWatch()
        guard !practicing else { return }
        quiet = QuietMeeting(start: start)
        let inPerson = mode == .inPerson
        quietWatch = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                guard !Task.isCancelled, let self else { return }
                let now = Date()
                guard self.state == .listening else { continue }
                if inPerson, self.quietNote == nil, self.quiet.silent(now: now) {
                    self.quietNote = QuietMeeting.line
                    Log.info("nothing transcribed for \(Int(QuietMeeting.after)) s in person: pill says so")
                }
                if self.closingAt == nil, self.quiet.over(now: now) {
                    Log.info("no word for \(Int(QuietMeeting.endAfter)) s: the meeting may be over")
                    // talk from here on keeps it going, as after closing words
                    self.proposeEnd(from: now, reason: .silence)
                }
            }
        }
    }

    private func stopQuietWatch() {
        quietWatch?.cancel()
        quietWatch = nil
        quietNote = nil
    }

    /// Every second while the meeting runs: the network path and the utterances still waiting make the pill's bars
    /// (`NetHealth`). The first time the connection is lost a line under the pill says what goes on.
    private func watchNet() {
        stopNetWatch()
        health = NetHealth()
        netNoticed = false
        lastSTTLocal = false
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { [weak self] p in
            let online = p.status == .satisfied
            Task { @MainActor in self?.health.path(online: online, at: Date()) }
        }
        monitor.start(queue: DispatchQueue(label: "tacie.meeting-path"))
        pathMonitor = monitor
        netWatch = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                guard !Task.isCancelled, let self else { return }
                self.updateNet()
            }
        }
    }

    private func updateNet() {
        let now = Date()
        let level = netForced ?? health.level(now: now, waiting: Array(hearsInFlight.values))
        guard level != net else { return }
        Log.info("connection \(net.rawValue) -> \(level.rawValue)")
        net = level
        guard !practicing else { return }
        switch level {
        case .offline:
            countError(AErrorKind.network.rawValue, place: .meeting)
            if !netNoticed {
                netNoticed = true
                flash(NetHealth.notice(recorded: recorder != nil, localSTT: lastSTTLocal), seconds: 7)
            }
        case .slow:
            countError(AErrorKind.slow.rawValue, place: .meeting)
        case .ok:
            break
        }
    }

    private func stopNetWatch() {
        netWatch?.cancel()
        netWatch = nil
        pathMonitor?.cancel()
        pathMonitor = nil
        netForced = nil
        net = .ok
    }

    /// The bars' help: what the connection does to this meeting.
    var netHelp: String {
        NetHealth.help(net, recorded: recorder != nil, localSTT: lastSTTLocal, downFor: health.downFor(now: Date()))
    }

    /// `--debug-control net-state ok|slow|offline|auto`: the bars without the network (captures).
    func debugNet(_ level: NetHealth.Level?) {
        netForced = level
        updateNet()
    }

    /// 기록만 하기 from the pill, for this meeting only (the setting stays): a short line under the pill says what changed.
    /// Turned back on, only what is said from now on is looked up; earlier lines are not searched again.
    func toggleRecordOnly() {
        guard isRunning else { return }
        if recordOnly, !CardsGate.canTurnCardsOn(open: Build.isOpen, jev: jevAvailable(), cardsOff: OpenAIMeeting.cardsOffNow) {
            flash(CardsGate.needsKey)
            return
        }
        recordOnly.toggle()
        flash(recordOnly ? String(localized: "기록만 해요 · 녹음과 대화 기록은 계속돼요") : String(localized: "이 회의에서 이제부터 나온 질문은 답 카드로 보여 줘요"))
        let on = !recordOnly
        if let session, let client = engine.client {
            Task {
                do { try await client.lookup(session: session, on: on) } catch { Log.info("lookup switch failed: \(error)") }
            }
        }
    }

    /// A line under the pill for a few seconds (the place the server-recognition notice uses).
    /// "Mac 안에서만" with no model to write notes: say so as the meeting starts, not only once it has ended with an
    /// empty 정리 (오픈소스판 e2e, 2026-11-25). A server found now is kept, as notes would find it at the end.
    private func warnNoNotesModel() async {
        if let saved = LocalServer.saved(), await AIChecks.serves(saved) { return }
        if let found = await AIChecks.findLocalServer() {
            LocalServer.save(found)
            await pushAIKeys()
            return
        }
        flash(OpenAIMeeting.noModelAtStart, seconds: 6)
    }

    private func flash(_ line: String, seconds: Double = 3) {
        sttNotice = line
        sttNoticeTask?.cancel()
        sttNoticeTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(seconds))
            if !Task.isCancelled { self?.sttNotice = nil }
        }
    }

    /// 녹음 알림 for five seconds; kept while the pointer is on the island.
    private func showRecordNotice() {
        recordNotice = .ask
        recordNoticeTask?.cancel()
        recordNoticeTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(5))
            while let self, self.hovering, self.recordNotice == .ask, !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
            }
            if !Task.isCancelled, self?.recordNotice == .ask { self?.recordNotice = nil }
        }
    }

    /// 알림 문구 복사 on the notice: the words go to the clipboard, the line says so and goes.
    func copyRecordNoticeNow() {
        copyRecordNotice()
        recordNotice = .copied
        recordNoticeTask?.cancel()
        recordNoticeTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(2.5))
            if !Task.isCancelled { self?.recordNotice = nil }
        }
    }

    func togglePause() {
        guard let source else { return }
        source.paused.toggle()
        state = source.paused ? .paused : .listening
        // paused time is not silence: the wait starts over on resuming, and the line goes while paused
        quiet.heard(at: Date())
        quietNote = nil
        if source.paused {
            level = 0
            keepGoing(by: .pause)  // pausing is a hand on the meeting: it does not end by itself meanwhile
        }
    }

    // MARK: the meeting's end, heard

    /// The engine heard the meeting being closed. The pill counts down `MeetingEnd.grace` and then ends the meeting itself.
    /// Internal for `--debug-control ending`, which shows the countdown without saying the words.
    /// `from`: when the closing words' utterance was cut. Talk after it keeps the meeting going. Nil when no words
    /// closed it (the meeting app let the microphone go): the call is over, and talk in the room afterwards is not it.
    /// `reason`: `.silence` when nobody spoke for `QuietMeeting.endAfter`.
    func proposeEnd(from: Date? = nil, reason: AEndReason = .heardEnd) {
        guard state == .listening, closingAt == nil else { return }
        let deadline = Date().addingTimeInterval(MeetingEnd.grace)
        closingAt = deadline
        closingFrom = from
        Analytics.capture(.meetingEndHeard)
        Log.info("meeting end heard: closing in \(Int(MeetingEnd.grace)) s")
        closer = Task { [weak self] in
            try? await Task.sleep(for: .seconds(MeetingEnd.grace))
            guard !Task.isCancelled, let self, self.closingAt == deadline, self.isRunning else { return }
            // someone is still talking, or what they said after the closing words is on its way: hear it first
            var held = false
            while let from, MeetingEnd.holds(speaking: self.speaking, inFlight: Array(self.hearsInFlight.values), closingFrom: from,
                                   now: Date(), deadline: deadline) {
                if !held { Log.info("meeting end heard: waiting for words still being said") }
                held = true
                try? await Task.sleep(for: .milliseconds(250))
                guard !Task.isCancelled, self.closingAt == deadline, self.isRunning else { return }
            }
            Log.info("meeting end heard: closing now")
            self.endReason = reason
            self.stop(by: .auto)
        }
    }

    /// 계속 듣기, or a sign the meeting goes on (a question, a pause): the countdown stops and nothing ends.
    func keepGoing(by: AKeptBy) {
        guard closingAt != nil else { return }
        Analytics.capture(.meetingEndKept(by: by))
        closer?.cancel()
        closer = nil
        closingAt = nil
        closingFrom = nil
        quiet.heard(at: Date())  // kept going: the silence counts again from now
        Log.info("meeting end heard: kept going (\(by.rawValue))")
    }

    // MARK: a meeting app began

    /// How long the question stays open before it folds to the app's icon beside the eyes (the pointer on it holds it
    /// open). 12 s was missed while the user looked at the meeting app; open for the whole meeting was too much.
    static let promptSeconds: Double = 60

    /// The question stays until the meeting app lets the microphone go (`promptOver`), 듣기 시작 or ×. After
    /// `promptSeconds` it folds to the app's icon (`promptFolded`); the icon opens it again.
    func offer(_ app: MeetingApp) {
        guard !isRunning, !asking, shown == .nothing, prompt == nil else { return }
        prompt = app
        promptFolded = false
        Analytics.capture(.meetingPromptShown(app: AMeetingApp(app)))
        onPromptChanged(app)
        foldPromptLater()
    }

    func unfoldPrompt() {
        guard prompt != nil else { return }
        promptFolded = false
        foldPromptLater()
    }

    private func foldPromptLater() {
        promptFold?.cancel()
        promptFold = Task { [weak self] in
            try? await Task.sleep(for: .seconds(Self.promptSeconds))
            guard !Task.isCancelled, let self, self.prompt != nil else { return }
            if self.pillHovered { return self.foldPromptLater() }
            self.promptFolded = true
        }
    }

    func acceptPrompt() {
        guard let app = prompt else { return }
        Analytics.capture(.meetingPromptAccepted(app: AMeetingApp(app)))
        dismissPrompt()
        onAcceptPrompt(app)
    }

    func dismissPrompt() {
        guard prompt != nil else { return }
        promptFold?.cancel()
        promptFold = nil
        promptFolded = false
        prompt = nil
        onPromptChanged(nil)
    }

    /// That app's meeting is over: its question goes away.
    func promptOver(_ app: MeetingApp) {
        if prompt == app { dismissPrompt() }
    }

    // MARK: what the island shows

    func open(_ id: String) {
        peeking = nil
        shown = .answer(id)
        scheduleCollapse()
    }

    /// Hovering a question in the trail shows its answer; the opened one is shown again on its own tab.
    func look(at id: String) {
        guard let opened else { return }
        peeking = id == opened ? nil : id
    }

    /// A meeting that failed with `msg`, on the island's card (debug captures).
    func debugFailed(_ msg: String, limit: Bool = false) {
        guard !isRunning else { return }
        state = .failed(msg)
        limitReached = limit
        dismissPrompt()
        shown = .chooser
    }

    /// 닫기 on the main window's 멈췄어요 row: back to before a meeting, the island too.
    func dismissFailure() {
        if case .failed = state { state = .idle }
    }

    /// The account changed (AccountSpace; never during a meeting): the answers and cards on the island were the last
    /// account's, so none stay.
    func forgetAccount() {
        guard !isRunning else { return }
        close()
        query = ""
        blocks = []
        namingToast = nil
        forgetNamingUndo()  // ⌘Z would bring the last account's people back
        if case .failed = state { state = .idle }
    }

    func close() {
        collapse?.cancel()
        presenting = false
        asking = false
        peeking = nil
        shown = .nothing
    }

    /// Esc on the island: the typed question goes too (close keeps it, so a card closed by a click can be asked again).
    func escape() {
        query = ""
        close()
    }

    func toggleTranscript() {
        dismissPrompt()
        transcriptOpen.toggle()
        if transcriptOpen {
            collapse?.cancel()
            shown = .nothing
            peeking = nil
            presenting = false
        }
    }

    /// "메인 창에서 보기" under 대화 기록: the meeting page shows the same lines, so the island folds it away.
    func closeTranscript() {
        transcriptOpen = false
    }

    /// The meeting's lines so far, for 대화 기록 on the island (sound-only lines left out, as on the meeting page).
    var liveTranscript: [TranscriptLine] {
        guard isRunning, let r = record else { return [] }
        return r.transcript.filter { !$0.isSoundOnly }
    }

    /// Who a line on the island shows: "나" for the mic in a remote meeting, else its name ("화자 N" or a person), with
    /// the speaker's palette index.
    func speaker(of l: TranscriptLine) -> (name: String, color: Int?, me: Bool) {
        if mode == .remote, l.speaker == Speaker.me.rawValue, l.name == nil { return (String(localized: "나"), nil, true) }
        guard let r = record, let name = r.lineName(l) else { return ("", nil, false) }
        return (name, r.owner(of: l).map { r.speakerColor($0, people: vault.people) }, false)
    }

    func toggle(_ s: Shown) {
        dismissPrompt()
        shown = shown == s ? .nothing : s
        peeking = nil
        presenting = false
    }

    /// The field takes the pill; whatever card was open gives way to the coming answer.
    func beginAsking() {
        // the account's space is changing: the question would reach the last account's search data
        guard !accountChanging() else { return }
        dismissPrompt()
        asking = true
        presenting = false
        peeking = nil
        shown = .nothing
    }

    /// While the pointer is over the island nothing folds away or gets replaced; leaving it shows the newest
    /// answer and gives a few more seconds.
    func hover(_ inside: Bool) {
        hovering = inside
        if inside {
            collapse?.cancel()
        } else {
            peeking = nil
            scheduleCollapse(after: .seconds(5))
        }
    }

    func hoverPill(_ inside: Bool) {
        pillHover?.cancel()
        pillHover = Task {
            try? await Task.sleep(for: inside ? .milliseconds(120) : .milliseconds(220))
            guard !Task.isCancelled else { return }
            pillHovered = inside
        }
    }

    /// At once, without the hover delay: a drag folds the pill's controls as it starts and brings them back on release.
    func setPillHover(_ inside: Bool) {
        pillHover?.cancel()
        pillHovered = inside
    }

    private func scheduleCollapse(after delay: Duration? = nil) {
        collapse?.cancel()
        guard let b = current, b.answerDone, !hovering, !presenting, !asking else { return }
        // answers someone asked for (typed, Slack) stay until closed
        guard case .voice = b.origin, let wait = delay ?? mode?.dwell else { return }
        let id = b.id
        collapse = Task {
            try? await Task.sleep(for: wait)
            guard !Task.isCancelled, shown == .answer(id) else { return }
            shown = .nothing
        }
    }

    // MARK: questions

    /// Typed question from the island: same cards and answer, no gate. The question stays in the field
    /// (the pill shows it instead of "who asked"); typing over it asks the next one.
    /// `onboarding`: the onboarding's step 3 asking on the user's own data (not counted for 처음 써 보기).
    func ask(onboarding: Bool = false) {
        let text = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        keepGoing(by: .ask)
        Analytics.capture(.askSubmitted(inMeeting: isRunning, sample: false))
        if isRunning && !practicing && !onboarding { FirstStepsStore.shared.mark(.ask) }
        lookUp(text, origin: .typed, onboarding: onboarding)
    }

    /// A message from a connected Slack channel. The connection itself is not built yet;
    /// `--debug-control` feeds messages here so the view can be checked.
    func fromSlack(channel: String, author: String, text: String) {
        lookUp(text, origin: .slack(channel: channel, author: author))
    }

    /// The onboarding's 3단계 with nothing connected: a sample question, answered from the sample data.
    func askSample(_ text: String) {
        beginAsking()
        query = text
        keepGoing(by: .ask)
        Analytics.capture(.askSubmitted(inMeeting: isRunning, sample: true))
        lookUp(text, origin: .typed, sample: true)
    }

    private func lookUp(_ text: String, origin: Block.Origin, sample: Bool = false, onboarding: Bool = false) {
        counter += 1
        let id = "m\(counter)"
        meetingOf[id] = record?.id
        var block = Block(id: id, question: text, origin: origin)
        block.sample = sample
        block.practice = isRunning && practicing
        block.onboarding = onboarding
        insert(block)
        if !sample, keepWithoutSource(id) { return }
        search(id, text: text, origin: origin, sample: sample)
    }

    /// The practice's cards and answer line, fetched before the question is played.
    struct Rehearsed {
        let ask: AskResponse
        let answer: Answer?
    }

    func rehearse(_ text: String, sample: Bool) async -> Rehearsed? {
        guard let client = await waitForEngine(), let r = try? await client.ask(text, context: [], sample: sample) else { return nil }
        guard r.shown, !r.lowConfidence, !r.cards.isEmpty else { return Rehearsed(ask: r, answer: nil) }
        let a = try? await client.answer(text, context: [], chunks: r.cards.compactMap(\.chunk), sample: sample)
        return Rehearsed(ask: r, answer: a)
    }

    /// The onboarding's meeting practice: plays `source` (the spoken `question`) and, when it ends, shows `result` as the
    /// question heard. The voice is not sent to be transcribed.
    func playRehearsal(_ source: FileSource, question: String, sample: Bool, result: Task<Rehearsed?, Never>) {
        guard !isRunning else { return }
        start(.inPerson, source: source, practice: true, sample: sample)
        rehearsal = (question, sample, result)
    }

    private func reveal(_ r: (question: String, sample: Bool, result: Task<Rehearsed?, Never>)) {
        counter += 1
        let id = "v\(counter)"
        var block = Block(id: id, question: r.question, origin: .voice(nil))
        block.sample = r.sample
        block.practice = true
        insert(block)
        Task {
            guard let got = await r.result.value else {
                update(id) { $0.phase = .failed(String(localized: "답을 찾지 못했어요")); $0.answerDone = true }
                return
            }
            let f = got.ask
            update(id) {
                $0.cards = f.cards; $0.shown = f.shown; $0.fallback = f.lowConfidence; $0.phase = .found
                $0.trouble = f.errorKind.map(Self.troubleSay)
                $0.answer = got.answer?.answer; $0.cited = got.answer?.cited ?? []; $0.answerDone = true
            }
            if let k = f.errorKind { countError(k, place: .meeting) }
            countOutcome(id)
            scheduleCollapse()
        }
    }

    /// Cards and the answer line for a typed or Slack question already on the island.
    private func search(_ id: String, text: String, origin: Block.Origin, sample: Bool) {
        Task {
            guard let client = await waitForEngine() else { return }
            await hold()
            do {
                let r = try await client.ask(text, context: origin == .typed && !sample ? context.map(\.line) : [], sample: sample)
                update(id) {
                    $0.cards = r.cards; $0.shown = r.shown; $0.fallback = r.lowConfidence; $0.phase = .found
                    $0.trouble = r.errorKind.map(Self.askTroubleSay)
                }
                hintFirstCard(id)
                if let k = r.errorKind { countError(k, place: .ask) }
                Log.info("asked \(id): \(r.cards.count) cards, shown \(r.shown)\(r.errorKind.map { ", failed: \($0)" } ?? "")")
                await answer(id, text: text, client: client)
            } catch {
                update(id) { $0.phase = .failed(error.localizedDescription); $0.answerDone = true }
            }
        }
    }

    /// `startMs`: where the utterance starts in the utterancer's stream, which is the recording's when there is one.
    private func heard(_ wav: Data, gap: Int?, speaker: Speaker?, startMs: Int) {
        guard let session, state == .listening, rehearsal == nil, let client = engine.client else { return }
        // placed in the recording only when this meeting is recorded (after its earlier sound when it was taken up again)
        let at = recorder != nil && recorder?.id == record?.id ? startMs + audioOffsetMs : nil
        // only remote meetings have two channels (the engine keeps their speakers apart)
        let speaker = mode == .remote ? speaker : nil
        let t0 = Date()
        let ms = { Int(Date().timeIntervalSince(t0) * 1000) }
        // where this utterance belongs, fixed now: the meeting may end, or a new one start, before STT answers
        let e = epoch
        let meeting = record?.id
        pendingHears += 1
        hearSerial += 1
        let serial = hearSerial
        hearsInFlight[serial] = t0
        Task {
            defer { hearsInFlight[serial] = nil }
            var transcribed = false
            do {
                // shadow (dev): compared afterwards, so the main path does not wait for it
                let given: StreamTranscript?
                if sttPath == .shadow {
                    Task { _ = await streamFinalize(startMs: startMs, wav: wav, meeting: meeting) }
                    given = nil
                } else {
                    given = await streamFinalize(startMs: startMs, wav: wav, meeting: meeting)
                }
                // what Soniox heard stays on the page until its line comes back
                var held: Int?
                if let given {
                    held = liveHeldNext
                    liveHeldNext += 1
                    liveHeld.append((held!, given.text))
                }
                defer { if let held { liveHeld.removeAll { $0.id == held } } }
                let h = try await hearTracked(client, session: session, wav: wav, gap: gap, speaker: speaker, transcript: given)
                transcribed = true
                let heardMs = ms()
                health.heard(seconds: Double(heardMs) / 1000, network: h.errorKind == "network")
                if h.stt != nil { lastSTTLocal = h.stt == "local" }
                updateNet()
                if let kind = h.errorKind { report(kind, stt: h.stt) } else { recovered() }
                if h.errorKind != nil, h.utteranceId == nil, let at { addUnheard(MeetingRecording.span(wav: wav, at: at), to: meeting) }
                // Soniox's translation of the whole utterance (docs/30) goes on its first line
                var translationLeft = given.flatMap { g in g.translation.map { ($0, g.translatedFrom) } }
                // one utterance may come back split where the voice changed (`then`): each piece is a line of its own,
                // placed at its own offset in the recording
                @MainActor func handle(_ h: Heard, at: Int?) -> (@MainActor () async throws -> Void)? {
                    // a gate failure still heard the line, and the engine keeps it as context: so does the app
                    guard let uid = h.utteranceId, !h.text.isEmpty else { return nil }
                    let key = "e\(e):\(uid)"
                    let firstKey = h.mergedFrom.map { "e\(e):\($0)" }
                    let live = meeting != nil && meeting == record?.id
                    if live, let voice = h.voice { met(voice, person: h.person) }
                    if h.skipped == nil, !joined.contains(key) {
                        // by id, not "the last line": utterances are handled in parallel and may come back out of order
                        if let firstKey { joined.insert(firstKey) }
                        context.removeAll { $0.id == firstKey }
                        let who = h.voice.map { v in live ? record?.displayName(key: v) ?? v : v } ?? "화자"
                        context = Array((context + [(key, "\(who): " + h.text)]).suffix(3))
                    }
                    // the transcript keeps backchannels too (they are left out of the gate and the ledger only), but not
                    // lines that are only sound tags ("[pause]", "[배경 소음]"): nobody said them
                    if !joined.contains(key), h.skipped != "sound" {
                        var line = Self.line(for: h, key: key, at: t0, startMs: at, speaker: speaker?.rawValue)
                        if let (text, from) = translationLeft {
                            line.translation = text
                            line.translatedFrom = from
                            translationLeft = nil
                        }
                        addLine(line, replacing: firstKey, to: meeting)
                        if let firstKey {
                            for i in blocks.indices where blocks[i].lineKey == firstKey { blocks[i].lineKey = key }
                        }
                    }
                    if live, let voice = h.voice {
                        linesSinceVoices += 1
                        // a new speaker, and every 10 lines: a small speaker is checked again as the voices settle
                        if heardVoices.insert(voice).inserted || linesSinceVoices >= 10 { fetchVoices() }
                    }
                    let note = (h.skipped.map { " skipped \($0)" } ?? "") + (h.mergedFrom.map { " joined \($0)" } ?? "")
                        + (h.pEnd.map { " end \(String(format: "%.2f", $0))" } ?? "")
                    Log.info("heard \(uid) (\(speaker?.rawValue ?? "-")\(h.voice.map { " \($0)" } ?? "")): fired \(h.fired) p \(String(format: "%.2f", h.pLookup ?? -1))\(note)")
                    // "오늘은 여기까지 하죠": the meeting ends on its own in a moment, unless this very meeting has ended or changed
                    if h.ending == true, !h.fired, meeting == record?.id { proposeEnd(from: t0) }
                    // talk after the closing words keeps the meeting going, a question or not (MeetingEnd)
                    if meeting == record?.id, let from = closingFrom,
                       MeetingEnd.keepsGoing(cutAt: t0, closingFrom: from, pEnd: h.pEnd,
                                             backchannel: h.skipped != nil) {
                        keepGoing(by: .talk)
                    }
                    guard h.fired else { return nil }
                    return { [self] in
                        keepGoing(by: .question)  // a question means the meeting goes on
                        // our own id: a restarted engine numbers its utterances from s1u1 again
                        counter += 1
                        let id = "v\(counter)"
                        meetingOf[id] = meeting
                        var block = Block(id: id, question: h.text, origin: .voice(h.voice))
                        block.lineKey = key
                        block.sample = sampleMeeting
                        block.practice = practicing
                        insert(block)
                        if !sampleMeeting, keepWithoutSource(id) { return }
                        await hold()
                        let f = try await client.find(session: session, utterance: uid)
                        update(id) {
                            $0.cards = f.cards; $0.shown = f.shown; $0.fallback = f.lowConfidence ?? false; $0.phase = .found
                            $0.trouble = f.errorKind.map(Self.troubleSay)
                        }
                        hintFirstCard(id)
                        if let k = f.errorKind { countError(k, place: .meeting) }
                        let cardsMs = ms()
                        await answer(id, text: h.text, client: client)
                        // times from the moment the utterance was cut (speech end + 0.7 s of silence)
                        Log.info("latency \(uid): heard \(heardMs) ms, cards \(cardsMs) ms, answer \(ms()) ms")
                    }
                }
                // every piece's line first, then the questions among them (finding cards takes seconds)
                let found = MeetingRecording.pieces(of: h, startMs: at).compactMap { handle($0.heard, at: $0.startMs) }
                // its line is on the page now: the held sentence goes before the questions are looked up (seconds)
                if let held { liveHeld.removeAll { $0.id == held } }
                for f in found { try await f() }
            } catch EngineError.http(404, _) {
                if !transcribed, let at { addUnheard(MeetingRecording.span(wav: wav, at: at), to: meeting) }
                renewSession()  // the engine no longer knows this meeting (it restarted)
            } catch {
                if !transcribed, let at { addUnheard(MeetingRecording.span(wav: wav, at: at), to: meeting) }
                Log.info("utterance failed: \(error)")
                report("engine")
            }
        }
    }

    /// A recorded stretch whose words did not come back: the recording keeps it, so the page can say so.
    /// Opens the Soniox connection for this meeting (docs/25): the main path in every build (`stream`), or alongside the
    /// file path in a development build (`shadow`). A temporary key comes from the engine (the relay in a shipped app, a key
    /// file in a dev build). Anything failing leaves the meeting on the file path, with one log line.
    private func startShadow(client: EngineClient, session: String?, meeting: String) {
        sttPath = chosenSttPath()
        guard sttPath != .server, let session else { return }
        openStream(client: client, session: session, meeting: meeting)
    }

    /// A new connection, taking over from the one before (rotation at 9 minutes, or a dropped one). The old one gets its
    /// last finalize's time and closes.
    private func openStream(client: EngineClient, session: String, meeting: String) {
        guard !streamOpening else { return }
        streamOpening = true
        Task {
            defer { streamOpening = false }
            do {
                let key = try await client.streamKey()
                // the words the person fixed in the panel first (docs/38 12-4절), then the engine's team terms
                let terms = ShadowWiring.terms(engine: practicing || sampleMeeting ? [] : MeetingTerms.words(),
                                               names: ((try? await client.sessionTerms(session)) ?? []) + (record?.invitedNames ?? []))
                guard record?.id == meeting, state == .listening || state == .paused else { return }
                let s = ShadowStt(socket: URLSessionShadowSocket(url: key.wsUrl, key: key.apiKey),
                                  config: ShadowWiring.config(terms: terms, translateTo: MeetingTranslation.current))
                if sttPath == .stream {
                    s.onLive = { [weak self] said, partial in
                        guard let self else { return }
                        let next = LiveLine(said: said, partial: partial)
                        if self.liveNow != next { self.liveNow = next.isEmpty ? nil : next }
                    }
                }
                s.start(streamMs: fanout.streamMs)
                let old = shadow
                shadow = s
                streamOpenedAt = Date()
                // the Utterancer hands samples over on the main queue, in order; the connection lives on the main actor
                fanout.shadow = { [weak s] samples in
                    DispatchQueue.main.async { MainActor.assumeIsolated { s?.feed(samples) } }
                }
                if let old { closeLater(old) }
                Log.info("stream: on (\(sttPath == .shadow ? "shadow" : "main"), \(terms.count) terms)")
            } catch {
                if Relay.on, (error as? EngineError)?.kind == "credit", record?.id == meeting { stopForLimit(); return }
                streamFailures += 1
                streamRetryAt = Date()
                Log.info("stream: off (\(error.localizedDescription))")
            }
        }
    }

    /// The utterance just cut, on the Soniox connection too. `shadow`: the result is saved next to the meeting's own
    /// transcript. `stream`: the result is the transcript (nil: the file path). Afterwards the connection is renewed when it
    /// is 9 minutes old or has dropped.
    private func streamFinalize(startMs: Int, wav: Data, meeting: String?) async -> StreamTranscript? {
        // on-device recognition turned on mid-meeting: its sound must not leave the Mac from now on
        if sttPath != .server, LocalSTT.switchedOn {
            Log.info("stream: off (on-device recognition turned on)")
            stopShadow()
            return nil
        }
        guard let s = shadow, !s.stopped, let meeting else { renewStream(meeting: meeting); return nil }
        let endMs = startMs + max(wav.count - 44, 0) / 32
        let r = await s.finalize(startMs: startMs, endMs: endMs)
        defer { renewStream(meeting: meeting) }
        guard let r else { return nil }
        Log.info("stream \(r.ms) ms" + (UserDefaults.standard.bool(forKey: DevSettings.debugText) ? ": \(r.text)" : ""))
        guard sttPath == .shadow else { return ShadowWiring.transcript(r) }
        if record?.id == meeting {
            // read first, then write: `record` is observed, and reading it inside its own write is an access conflict
            let list = (record?.shadow ?? []) + [r]
            record?.shadow = list
        } else {
            library.update(meeting) { $0.shadow = ($0.shadow ?? []) + [r] }
        }
        return nil
    }

    /// Opens a new connection when this one is 9 minutes old (its key allows 10) or has stopped, at most every 10 s.
    private func renewStream(meeting: String?) {
        guard sttPath != .server, let meeting, record?.id == meeting, let session, let client = engine.client else { return }
        let age = streamOpenedAt.map { Date().timeIntervalSince($0) } ?? .infinity
        let dropped = shadow?.stopped ?? true
        guard dropped || ShadowWiring.shouldRotate(ageSeconds: age) else { return }
        if dropped {
            // a dropped connection counts once; a failed open was counted where it failed
            if shadow != nil, streamOpenedAt != nil { streamFailures += 1; streamOpenedAt = nil }
            guard let wait = ShadowWiring.retryDelay(failures: max(streamFailures - 1, 0)) else {
                Log.info("stream: off for this meeting (\(streamFailures) failed connections)")
                stopShadow()
                return
            }
            if let last = streamRetryAt, Date().timeIntervalSince(last) < wait { return }
        } else if let last = streamRetryAt, Date().timeIntervalSince(last) < 10 {
            return  // a swap that failed: not again within 10 s
        }
        streamRetryAt = Date()
        openStream(client: client, session: session, meeting: meeting)
    }

    private func closeLater(_ s: ShadowStt) {
        Task {
            try? await Task.sleep(for: .milliseconds(2500))
            s.stop()
        }
    }

    /// The meeting ended: no more samples go to Soniox, and the connection closes once the last finalize had its time.
    /// Where speech would be transcribed with the AI choice saved now.
    private func chosenSttPath() -> SttPath {
        ShadowWiring.path(isDev: Build.isDev, choice: UserDefaults.standard.string(forKey: DevSettings.sttPath),
                          practice: practicing, sample: sampleMeeting, localStt: LocalSTT.switchedOn,
                          localReady: localSttReady(), provider: SttProviderChoice.pathProvider, open: Build.isOpen)
    }

    /// Settings → AI changed mid-meeting (`AIChoice.changed`): close a Soniox connection the new choice would not open,
    /// before the next utterance ends. Before this, switching to on-device processing kept streaming until then, and
    /// switching to an API key kept streaming until the meeting ended (2026-11-26).
    func aiChoiceChanged() {
        guard isRunning, ShadowWiring.stopsOnChange(current: sttPath, next: chosenSttPath()) else { return }
        Log.info("stream: off (AI choice changed)")
        stopShadow()
    }

    private func stopShadow() {
        fanout.shadow = nil
        liveNow = nil
        liveHeld = []
        sttPath = .server
        streamOpenedAt = nil
        guard let s = shadow else { return }
        shadow = nil
        closeLater(s)
    }

    private func addUnheard(_ span: AudioSpan, to meeting: String?) {
        guard let meeting else { return }
        if record?.id == meeting {
            // read first, then write: `record` is observed (@Observable), and reading it inside its own write is a runtime
            // access conflict that stops the app (it did for the shadow line, 2026-11-10)
            let list = (record?.unheard ?? []) + [span]
            record?.unheard = list
        } else {
            library.update(meeting) { $0.unheard = ($0.unheard ?? []) + [span] }
        }
    }

    /// Whatever the VAD still holds goes out now as one utterance (a paused meeting only lets go of it).
    private func sendRest() {
        guard let u = utterancer.cut(), state == .listening else { return }
        heard(u.wav, gap: u.gap, speaker: u.speaker, startMs: u.startMs)
    }

    /// The input device went away or came back mid-meeting (docs/04 §4). In person nothing else is heard, so the words
    /// so far go out and the meeting waits for a microphone; a remote meeting goes on with the call alone.
    private func micChanged(_ ok: Bool) {
        guard isRunning, let mode else { return }
        if ok {
            micNote = nil
        } else {
            if mode == .inPerson {
                sendRest()
                level = 0
            }
            micNote = mode == .inPerson ? String(localized: "마이크가 끊겼어요") : String(localized: "회의 앱 소리만 들어요")
        }
        Log.info("microphone \(ok ? "back" : "missing") in a \(mode.rawValue) meeting")
    }

    private func hearTracked(_ client: EngineClient, session: String, wav: Data, gap: Int?, speaker: Speaker?,
                             transcript: StreamTranscript? = nil) async throws -> Heard {
        defer { pendingHears -= 1 }
        let h = try await client.hear(session: session, wav: wav, gapMs: gap, speaker: speaker?.rawValue, transcript: transcript)
        if h.stt == "stream" { sttStreamCount += 1 }
        onHeard(h)
        noteSTT(h)
        if h.stt == "local" { sttLocalCount += 1 }
        return h
    }

    /// The first utterance of a meeting that the server recognized, while the on-device model is not ready, earns a
    /// line for a few seconds; a download starting again inside the meeting earns it once more. Never in the practice.
    private func noteSTT(_ h: Heard) {
        guard let line = sttNoticeOnce.note(stt: h.stt, practice: practicing, state: sttState()) else { return }
        sttNotice = line
        sttNoticeTask?.cancel()
        sttNoticeTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(7))
            if !Task.isCancelled { self?.sttNotice = nil }
        }
    }

    // MARK: when something fails

    /// Words for a failure kind (engine/crates/tacie-engine/src/errors.rs), short enough for the pill.
    /// The meeting's warning line: in the open edition with nothing chosen, what to do (2-D); otherwise `say`.
    static func troubleSay(_ kind: String) -> String {
        OpenAIMeeting.troubleLine(kind: kind, open: Build.isOpen, mode: AIMode.saved()) ?? say(kind)
    }

    /// `troubleSay` for a typed question: in the open edition a missing key there is about the answer, not the audio.
    static func askTroubleSay(_ kind: String) -> String {
        OpenAIMeeting.askTroubleLine(kind: kind, open: Build.isOpen, mode: AIMode.saved()) ?? troubleSay(kind)
    }

    /// The warning line asks to choose how to run AI: the pill shows 고르기 (2-D).
    var asksToChooseAI: Bool { trouble == OpenAIMeeting.chooseLine }

    static func say(_ kind: String) -> String {
        // Tacie AI (docs/14): a refusal is about the account, not a key
        if Relay.on {
            let account = TacieAccount.shared
            if kind == "key" {
                if !account.signedIn { return String(localized: "설정 → 계정에서 로그인해 주세요") }
                if account.info?.status == "waiting" { return String(localized: "meetingModel.waitingApproval", defaultValue: "승인을 기다리고 있어요") }
                return String(localized: "Tacie AI를 쓸 수 없는 계정이에요")
            }
            if kind == "credit" { return String(localized: "이번 달 AI 사용량을 다 썼어요") }
        }
        let mode = AIMode.saved()
        if let line = OpenAIMeeting.failureLine(kind: kind, open: Build.isOpen, mode: mode,
                                                keySaved: mode != .key || AIKeys.exists(.openrouter)) { return line }
        return FailureWords.say(kind)
    }

    /// Tacie AI refused this account for the rest of the month (the relay's 402, `credit`): every later utterance would
    /// be refused too and leave one more untranscribed stretch, so the meeting ends here, keeping what was said so far.
    /// The pill opens on why.
    private func stopForLimit() {
        guard isRunning else { return }
        Log.info("meeting stopping: this month's Tacie AI is used up")
        countError(AErrorKind.credit.rawValue, place: .meeting)
        endReason = .limit
        let id = record?.id
        stop(by: .auto)
        state = .failed(Account.limitLine(kept: id.map { k in library.meetings.contains { $0.id == k } } ?? false))
        limitReached = true
        // the account's month as the server has it now, so the next start says no before listening
        Task { await TacieAccount.shared.refresh() }
    }

    /// A missing key, balance or search model is said at once; anything else after two failures in a row, so one hiccup
    /// stays quiet.
    private func report(_ kind: String, stt: String? = nil) {
        // the speech recognition model on this Mac, not the search model: recognition paused (docs/22 4-4)
        if LocalSTT.isLocalFailure(kind: kind, stt: stt) {
            sttLocalFailing = true
            countError(AErrorKind.sttLocal.rawValue, place: .meeting)
            return
        }
        if Relay.on, kind == "credit" { stopForLimit(); return }
        failures += 1
        Log.info("call failed: \(kind) (\(failures) in a row)")
        // a lost connection is the pill's bars (`NetHealth`), not a line in place of the mode
        if kind == "network" { return }
        if kind == "key" || kind == "credit" || kind == "model" || failures >= 2 {
            trouble = Self.troubleSay(kind)
            countError(kind, place: .meeting)
        }
    }

    /// `error_shown`, once per kind until the next meeting starts.
    private func countError(_ kind: String, place: APlace) {
        let k = AErrorKind(kind: kind)
        if errorsSent.shouldSend(k) { Analytics.capture(.errorShown(kind: k, place: place)) }
    }

    private func recovered() {
        failures = 0
        trouble = nil
        sttLocalFailing = false
    }

    /// A restarted engine knows no sessions: open a new one and keep listening. If it gave up, the meeting stops.
    private func engineChanged(_ s: EngineProcess.State) {
        guard state == .listening || state == .paused else { return }
        switch s {
        case .starting:
            session = nil
            trouble = String(localized: "검색 엔진을 다시 띄우는 중이에요")
        case .ready:
            renewSession()
        case .failed(let msg):
            closer?.cancel()
            closingAt = nil
            closingFrom = nil
            source?.stop()
            source = nil
            session = nil
            trouble = nil
            if let e = endedEvent(reason: .engineFailed) { Analytics.capture(e) }
            // keep what was said up to here, with the reason, and the voices fetched so far
            if let final = closeRecord(problem: Self.say("engine")) { keepVoices(of: final.id, session: nil) }
            stopQuietWatch()
            startedAt = nil
            state = .failed(msg)
            Log.info("meeting stopped: engine gave up")
        }
    }

    /// The new session gets the meeting's voices so far, so its speakers keep their numbers (and new ones go on after the
    /// highest number the transcript used, even one whose voice was never fetched), and the names given so far.
    private func renewSession() {
        Task {
            guard let client = engine.client else { return }
            let people = rememberedPeople()
            let after = record?.highestSpeaker ?? 0
            let sample = sampleMeeting, gateEmpty = !sampleMeeting && !hasSource(), lookup = !recordOnly
            let id: String
            do {
                id = try await client.newSession(sample: sample, gateEmpty: gateEmpty, lookup: lookup, people: people,
                                                 restore: Array(prints.values), after: after)
            } catch {
                // the voices are a help, never a reason to lose the meeting
                Log.info("session restore failed (\(error)): going on without the meeting's voices")
                guard let plain = try? await openSession(people: people, {
                    try await client.newSession(sample: sample, gateEmpty: gateEmpty, lookup: lookup, people: $0, after: after)
                }) else { return }
                id = plain
            }
            session = id
            epoch += 1
            recovered()
            pushNames()
            Log.info("meeting continues on a new engine session")
        }
    }

    // MARK: speakers

    /// The remembered people the engine compares new voices with; none while remembering is off.
    private func rememberedPeople() -> [(id: String, vector: [Float])] {
        VoiceVault.enabled ? vault.people.map { ($0.id, $0.vector) } : []
    }

    /// A line of the meeting going on came with a speaker. When the engine knew it as a remembered person, the speaker
    /// takes that name unless it has one already.
    private func met(_ voice: String, person: String?) {
        guard let person, let p = vault.people.first(where: { $0.id == person }) else { return }
        // a remembered person is someone of their own, not part of the speaker the app joined them to
        let released = record?.releaseFolded(voice) == true
        if record?.adopt(key: voice, name: p.name, person: p.id) == true || released { pushNames() }
    }

    /// The meeting's voices from the engine, kept in memory (a new speaker was heard). A failure changes nothing.
    private func fetchVoices() {
        guard let session, let client = engine.client, let r = record else { return }
        let exclude = r.ownLines(epoch: epoch)
        voicesAsked += 1
        linesSinceVoices = 0
        let asked = voicesAsked
        Task {
            do {
                let voices = try await client.voices(session: session, exclude: exclude)
                guard record?.id == r.id, asked == voicesAsked else { return }
                for v in voices {
                    prints[v.voice] = v.print
                    nearPeople[v.voice] = v.near
                }
                foldSmallSpeakers()
            } catch {
                Log.info("voices failed: \(error)")
            }
        }
    }

    /// Joins the small speakers of the meeting going on to the speaker each sounds like (`SmallSpeakers`), with the
    /// voices last fetched.
    private func foldSmallSpeakers() {
        guard record?.foldSmallSpeakers(Array(prints.values)) == true else { return }
        checkpoint()
        pushNames()
    }

    /// The whole names table to the engine, after the one before it. A failure is logged and the meeting goes on.
    private func pushNames() {
        let before = namesSent
        namesSent = Task {
            await before?.value
            guard let session, let client = engine.client, let map = record?.engineNames() else { return }
            do { try await client.names(session: session, map) } catch { Log.info("names failed: \(error)") }
        }
    }

    /// Names, merges and one-line names from the meeting page. The meeting going on changes here (its checkpoints write
    /// what this model holds) and the engine hears the new names at once; a meeting that ended changes in the library.
    func editSpeakers(of id: String, _ change: (inout SavedMeeting) -> Void) {
        guard isRunning, var r = record, r.id == id else {
            library.update(id, change)
            return
        }
        change(&r)
        record = r
        checkpoint()
        pushNames()
        // a line named on its own (이 줄만) leaves its speaker's voice now, so the voices kept in memory (restored into a
        // restarted engine, kept when the engine is gone) do not hold it. A line heard before an engine restart cannot
        // leave any more: its voice went into the new session as a whole (restore), and stays in that speaker's voice.
        fetchVoices()
    }

    /// The remembered person the engine says speaker `key` of the meeting going on sounds closest to (or a speaker merged
    /// into it). None for a meeting that ended.
    func near(_ key: String, in id: String) -> String? {
        guard isRunning, let r = record, r.id == id else { return nil }
        let k = r.resolved(key)
        return nearPeople[k] ?? nearPeople.sorted { $0.key < $1.key }.first { r.resolved($0.key) == k }?.value
    }

    /// Speakers of a meeting that ended still called "화자 N", each with the remembered person whose voice is closest
    /// (화자 추천, docs/04 section 4). None during the meeting, or while 참석자 기억하기 is off. Made again only when the
    /// meeting's names, the people or the kept voices change.
    func suggestions(in id: String) -> [String: Person] {
        guard VoiceVault.enabled, !(isRunning && record?.id == id),
              let m = library.meetings.first(where: { $0.id == id }) else { return [:] }
        let people = vault.people
        let stamp = SuggestStamp(meeting: id, voices: m.voices ?? [], names: m.transcript.compactMap(\.name),
                                 people: people.map { $0.id + $0.name }, revision: vault.revision)
        let made: [String: String]
        if let c = suggestCache, c.stamp == stamp {
            made = c.value
        } else {
            made = SpeakerSuggest.make(meeting: m, prints: vault.prints(meeting: id), people: people,
                                       dismissed: vault.dismissed(meeting: id))
            suggestCache = (stamp, made)
            if !made.isEmpty, suggestionsCounted.insert(id).inserted {
                Analytics.capture(.speakerSuggestionsShown(count: made.count))
            }
        }
        return made.compactMapValues { pid in people.first { $0.id == pid } }
    }

    /// ✓ on a suggestion (beside the name, or its row in the namer): the speaker becomes that person.
    func acceptSuggestion(key: String, person: String, in id: String) {
        Analytics.capture(.speakerSuggestionAnswered(accepted: true))
        applyName(.person(person), key: key, lines: [], in: id)
    }

    /// ✕ on a suggestion: this speaker of the meeting gets no suggestion again.
    func dismissSuggestion(key: String, person: String, in id: String) {
        Analytics.capture(.speakerSuggestionAnswered(accepted: false))
        vault.dismiss(SpeakerSuggest.pair(key, person), meeting: id)
    }

    /// What the namer gave: a speaker (`lines` empty) or the lines named on their own (이 줄만) take a typed name, another
    /// speaker of the meeting, or a remembered person. A speaker given another speaker is merged into it. A meeting that ended and kept its voices
    /// remembers the named speaker at once; the meeting going on does at its end (keepVoices).
    func applyName(_ pick: NamerPick, key: String, lines: [String], in id: String) {
        let people = vault.people
        guard let m = record?.id == id ? record : library.meetings.first(where: { $0.id == id }) else { return }
        let undo = (naming: m.naming, vault: vault.snapshot(meeting: id))
        let named: ANamePick = switch pick { case .typed: .typed; case .speaker: .merge; case .person: .person }
        Analytics.capture(.speakerNamed(pick: named, lineOnly: !lines.isEmpty, live: isRunning && record?.id == id))
        // someone named on lines only has no voice: naming them names their lines
        let lines = lines.isEmpty && SavedMeeting.isLinePerson(key) ? m.lines(of: key) : lines
        if !lines.isEmpty {
            let name: String? = switch pick {
            case .typed(let n): n
            case .speaker(let k): m.displayName(key: k)
            case .person(let p): people.first { $0.id == p }?.name
            }
            if let name {
                editSpeakers(of: id) { m in for l in lines { m.renameLine(l, to: name) } }
                noteNaming(id, undo: undo, text: NamingText.done(pick, old: name, new: name, lineOnly: true))
                let person: String? = if case .person(let p) = pick { p } else { nil }
                let into: String? = if case .speaker(let k) = pick, !SavedMeeting.isLinePerson(k) { m.resolved(k) } else { nil }
                split(lines, name: name, person: person, into: into, in: id)
            }
            return
        }
        editSpeakers(of: id) { m in
            switch pick {
            case .typed(let n): m.renameTyped(key: key, to: n, people: people)
            case .speaker(let k): m.merge(key, into: k)
            case .person(let p):
                if let person = people.first(where: { $0.id == p }) { m.rename(key: key, to: person.name, person: person.id) }
            }
        }
        if !(isRunning && record?.id == id) { rememberNow(key: key, in: id) }
        let old = m.displayName(key: key)
        let given: String = switch pick {
        case .speaker(let k): m.displayName(key: k)
        case .typed(let n): n
        case .person(let p): people.first { $0.id == p }?.name ?? old
        }
        let text = NamingText.done(pick, old: old, new: given, lineOnly: false)
        noteNaming(id, undo: undo, text: text)
    }

    // MARK: telling the engine

    /// Lines named on their own while the meeting goes on: two people were heard as one speaker. The engine moves them out
    /// of that voice into the speaker who goes by the name (or a new one), so the next things that person says land there
    /// (docs/04 section 4 "회의 중 화자 바로잡기"). A line given back to its own speaker, a line of an earlier engine session,
    /// or a meeting that ended changes only what the page shows.
    private func split(_ lines: [String], name: String, person: String?, into: String?, in id: String) {
        guard isRunning, let r = record, r.id == id, let session, let client = engine.client else { return }
        let prefix = "e\(epoch):"
        let mine = lines.filter { l in l.hasPrefix(prefix) && r.transcript.first { $0.key == l }?.name != nil }
        guard !mine.isEmpty else { return }
        let target = into ?? r.attendees.first { $0.name == name && !SavedMeeting.isLinePerson($0.key) }?.key
        let ids = mine.map { String($0.dropFirst(prefix.count)) }
        Task {
            do {
                let voice = try await client.split(session: session, utterances: ids, into: target)
                guard record?.id == id else { return }
                editSpeakers(of: id) { $0.moved(mine, to: voice, name: name, person: person) }
            } catch {
                Log.info("split failed: \(error)")
            }
        }
    }

    /// An undo during the meeting gives the lines it moves back to their voices in the engine too.
    private func unsplit(_ id: String, from now: SavedMeeting.Naming, to then: SavedMeeting.Naming) {
        guard isRunning, record?.id == id, let session, let client = engine.client else { return }
        let prefix = "e\(epoch):"
        var back: [String: [String]] = [:]
        for (line, voice) in then.lineVoices where line.hasPrefix(prefix) && now.lineVoices[line] != voice {
            back[voice, default: []].append(String(line.dropFirst(prefix.count)))
        }
        guard !back.isEmpty else { return }
        Task {
            for (voice, ids) in back {
                do { _ = try await client.split(session: session, utterances: ids, into: voice) } catch { Log.info("unsplit failed: \(error)") }
            }
        }
    }

    // MARK: undoing a naming

    /// Remembers how meeting `id` and the vault were before a naming, for ⌘Z and the toast. The undo puts them back and
    /// leaves a redo that brings the naming back.
    private func noteNaming(_ id: String, undo: (naming: SavedMeeting.Naming, vault: VoiceVault.Snapshot), text: String) {
        guard let now = meeting(id) else { return }
        let after = (naming: now.naming, vault: vault.snapshot(meeting: id))
        guard after.naming != undo.naming else { return }
        namingUndo.registerUndo(withTarget: self) { model in
            model.restoreNaming(id, to: undo, redo: after, text: text)
        }
        namingUndo.setActionName(text)
        namingToast = NamingToast(text: text)
    }

    private func restoreNaming(_ id: String, to state: (naming: SavedMeeting.Naming, vault: VoiceVault.Snapshot),
                               redo: (naming: SavedMeeting.Naming, vault: VoiceVault.Snapshot), text: String) {
        namingToast = nil
        vault.restore(state.vault)
        if let now = meeting(id)?.naming { unsplit(id, from: now, to: state.naming) }
        editSpeakers(of: id) { $0.restore(state.naming) }
        namingUndo.registerUndo(withTarget: self) { model in
            model.restoreNaming(id, to: redo, redo: state, text: text)
        }
        namingUndo.setActionName(text)
    }

    /// Undo is for the page in front: leaving it (another meeting) or the meeting ending drops what could be undone,
    /// so ⌘Z never changes a meeting that is not on screen.
    func forgetNamingUndo() {
        namingUndo.removeAllActions()
        namingToast = nil
    }

    /// The toast went away on its own.
    func dropToast(_ toast: NamingToast) {
        if namingToast == toast { namingToast = nil }
    }

    private func meeting(_ id: String) -> SavedMeeting? {
        record?.id == id ? record : library.meetings.first { $0.id == id }
    }

    /// A speaker named in a meeting that ended is remembered right away, when the meeting's voices were kept.
    /// A speaker still linked to a person who was forgotten loses the link and is not remembered again from it.
    private func rememberNow(key: String, in id: String) {
        guard VoiceVault.enabled, !vault.prints(meeting: id).isEmpty,
              let m = library.meetings.first(where: { $0.id == id }) else { return }
        let k = m.resolved(key)
        var check = m
        if check.unlink(keeping: vault.people).contains(k) {
            let people = vault.people
            library.update(id) { $0.unlink(keeping: people) }
            return
        }
        guard let v = m.voices?.first(where: { $0.key == k }), let name = v.name,
              let pid = vault.remember(key: k, name: name, person: v.person, meeting: m), pid != v.person else { return }
        library.update(id) { $0.rename(key: k, to: name, person: pid) }
    }

    /// Forgets a remembered person (개인정보 or the meeting page). The meeting going on stops linking its speakers to them,
    /// so its end does not make them again; its names stay.
    func forget(person: String) {
        vault.forget(person: person)
        guard isRunning, var r = record, !r.unlink(keeping: vault.people).isEmpty else { return }
        record = r
        checkpoint()
    }

    /// What the island says for who asked: the speaker's name (or key) and colour in the meeting the question was heard in.
    func asker(_ b: Block) -> (key: String, name: String, color: Int)? {
        guard case .voice(let key?) = b.origin else { return nil }
        let mid = meetingOf[b.id]
        let m = record?.id == mid ? record : library.meetings.first { $0.id == mid }
        let color = m?.speakerColor(key, people: vault.people) ?? SavedMeeting.colorIndex(for: key, person: nil, personColor: nil)
        return (key, m?.displayName(key: key) ?? key, color)
    }

    /// At a meeting's end its voices are kept on this Mac and every named speaker is remembered for the next meeting.
    /// The engine gives the voices over the whole meeting (lines named on their own left out); without it, or when it
    /// fails, the ones fetched during the meeting do. Nothing is kept while remembering is off. The small speakers are
    /// checked once more with these voices (`SmallSpeakers`), remembering on or off.
    private func keepVoices(of id: String, session: (String, EngineClient)?) {
        // the meeting's end remembers its people: an undo from before would put the vault back over them
        forgetNamingUndo()
        let known = prints
        let e = epoch
        Task {
            var all = known
            if let (sid, client) = session {
                // the last words may still be with STT: their lines and voices are part of the meeting
                let deadline = Date().addingTimeInterval(5)
                while pendingHears > 0 && Date() < deadline { try? await Task.sleep(for: .milliseconds(100)) }
                let exclude = library.meetings.first { $0.id == id }?.ownLines(epoch: e) ?? []
                do {
                    for v in try await client.voices(session: sid, exclude: exclude) { all[v.voice] = v.print }
                } catch {
                    Log.info("voices at the end failed: \(error)")
                }
            }
            if var before = library.meetings.first(where: { $0.id == id }), before.foldSmallSpeakers(Array(all.values)) {
                library.update(id) { _ = $0.foldSmallSpeakers(Array(all.values)) }
            }
            guard VoiceVault.enabled, var m = library.meetings.first(where: { $0.id == id }) else { return }
            vault.save(prints: all.values.sorted { $0.key < $1.key }, meeting: id)
            // a speaker linked to a person forgotten meanwhile loses the link and is not remembered again from it
            let people = vault.people
            let dropped = m.unlink(keeping: people)
            if !dropped.isEmpty { library.update(id) { $0.unlink(keeping: people) } }
            for v in m.voices ?? [] where v.same == nil && !dropped.contains(v.key) {
                guard let name = v.name, let pid = vault.remember(key: v.key, name: name, person: v.person, meeting: m),
                      pid != v.person else { continue }
                library.update(id) { $0.rename(key: v.key, to: name, person: pid) }
            }
        }
    }

    private func answer(_ id: String, text: String, client: EngineClient) async {
        await hold()
        guard let block = blocks.first(where: { $0.id == id }) ?? closing[id], block.shown, !block.fallback else {
            update(id) { $0.answerDone = true }
            countOutcome(id)
            scheduleCollapse()
            return
        }
        let chunks = block.cards.compactMap(\.chunk)
        let a = try? await client.answer(text, context: block.sample ? [] : context.map(\.line), chunks: chunks, sample: block.sample)
        update(id) { $0.answer = a?.answer; $0.cited = a?.cited ?? []; $0.answerDone = true }
        countOutcome(id)
        checkpoint()
        // the engine hid a line that did not cite the cards; counting these says how often it happens in real meetings
        if let why = a?.dropped { Log.info("answer \(id): dropped \(why)") }
        scheduleCollapse()
    }

    /// `answer_shown` when cards came up, `question_unanswered` when nothing certain did; once per question.
    private func countOutcome(_ id: String) {
        guard let b = blocks.first(where: { $0.id == id }) ?? closing[id], outcomeSent.insert(id).inserted else { return }
        let origin = b.origin.analytics
        let practice = b.sample || b.practice
        if b.shown, !b.cards.isEmpty {
            Analytics.capture(MeetingStats.answerShown(origin: origin, cards: b.cards, lowConfidence: b.fallback,
                                                       hasAnswerLine: b.answer != nil && !b.saidNoAnswer, practice: practice))
        }
        if b.unanswered {
            hintConnect(id)
            Analytics.capture(.questionUnanswered(origin: origin, reason: b.saidNoAnswer && b.shown && !b.cards.isEmpty ? .saidNoAnswer : .noCards,
                                                  practice: practice))
        }
    }

    static let connectHintKey = "hint.connectMeetings"
    /// Meetings that offer 설정 → 연결 on a card, ever: someone who keeps Tacie for notes only is not asked forever
    /// (회의 정리의 연결 줄 stays).
    static let connectHintMeetings = 3

    /// Once a meeting, in the first three: a heard question found nothing while only past meetings are searched, so the card says team
    /// material would widen it and offers 설정 → 연결.
    private func hintConnect(_ id: String) {
        let shownIn = UserDefaults.standard.integer(forKey: Self.connectHintKey)
        guard !connectHinted, shownIn < Self.connectHintMeetings, let b = blocks.first(where: { $0.id == id }), case .voice = b.origin,
              !b.sample, !b.practice, !b.noSource, b.waitingSync == nil, !hasTeamSource() else { return }
        connectHinted = true
        UserDefaults.standard.set(shownIn + 1, forKey: Self.connectHintKey)
        update(id) { $0.connectHint = true }
    }

    /// The card's 연결 button: the card closes and the main window opens on 설정 → 연결.
    func connectFromHint() {
        Analytics.capture(.connectHintClicked)
        close()
        openConnections()
    }

    static let noSourceHintKey = "hint.noSourceShown"
    static let firstCardHintKey = "hint.firstCardShown"
    /// Saved questions being searched again now (searchAgain): a second call leaves them to the first.
    private var researching: Set<String> = []
    /// Meetings whose 문서로 남길 질문 are being read (`reviewQuestions`).
    private var reviewing: Set<String> = []

    /// Once ever: the first cards of a real meeting say they can be clicked (docs/11 14절).
    private func hintFirstCard(_ id: String) {
        guard let b = blocks.first(where: { $0.id == id }),
              FirstSteps.wantsCardHint(FirstStepsStore.shared.state, cards: b.shown ? b.cards.count : 0, inMeeting: isRunning,
                                       practice: !FirstSteps.realAnswer(practice: b.practice, sample: false, onboarding: b.onboarding),
                                       sample: b.sample,
                                       hintShown: UserDefaults.standard.bool(forKey: Self.firstCardHintKey)) else { return }
        UserDefaults.standard.set(true, forKey: Self.firstCardHintKey)
        update(id) { $0.firstCardHint = true }
    }

    /// Nothing is connected: the question is kept for the meeting's notes and searched once a service is, instead of
    /// searching an empty index (docs/11 4-1절 3번).
    private func keepWithoutSource(_ id: String) -> Bool {
        guard !hasSource() else { return false }
        // where to connect is said once, on the first question ever kept this way (docs/11 4-1절 3번)
        let sync = firstSync()
        // heard questions do not reach the island while nothing is connected: only a typed one can carry the hint
        let typed = blocks.first(where: { $0.id == id })?.origin == .typed
        let first = typed && sync == nil && !UserDefaults.standard.bool(forKey: Self.noSourceHintKey)
        if first { UserDefaults.standard.set(true, forKey: Self.noSourceHintKey) }
        update(id) {
            $0.noSource = true; $0.firstNoSource = first; $0.waitingSync = sync; $0.phase = .found; $0.answerDone = true
        }
        if let b = blocks.first(where: { $0.id == id }), outcomeSent.insert(id).inserted {
            Analytics.capture(.questionUnanswered(origin: b.origin.analytics, reason: .noSource, practice: b.practice))
        }
        checkpoint()
        scheduleCollapse()
        return true
    }

    /// The search data changed with `keys` fully in (docs/11 14절): questions kept while nothing was connected, and
    /// unanswered ones never searched with one of these sources, are searched now and their answers go into their
    /// meeting. One rerank and one answer call each; a question is searched once per new source (ResearchRules).
    func searchAgain(keys: Set<String>) {
        guard hasSource(), !keys.isEmpty else { return }
        // still on the island: typed questions search now, in place (a meeting's spoken ones are in its saved copy)
        for b in blocks where b.noSource && b.origin == .typed {
            update(b.id) {
                $0.noSource = false; $0.firstNoSource = false; $0.waitingSync = nil; $0.phase = .finding; $0.answerDone = false
            }
            search(b.id, text: b.question, origin: b.origin, sample: false)
        }
        let stamp = Array(keys).sorted()
        let now = Date()
        var legacy = 0
        var waiting: [(String, SavedQuestion)] = []
        for m in library.meetings where m.id != record?.id {
            // a search still on its way from an earlier call: it stamps when it lands
            for q in m.questions where !researching.contains(q.id) {
                var d = ResearchRules.due(q, meetingStart: m.start, keys: keys, now: now)
                if d == .search, q.searchedWith == nil, q.noSource != true {
                    legacy += 1
                    if legacy > ResearchRules.legacyCap { d = .stamp }
                }
                switch d {
                case .search: waiting.append((m.id, q))
                case .stamp:
                    var seen = q
                    seen.searchedWith = ResearchRules.merged(q.searchedWith, stamp)
                    library.update(m.id) { $0.setQuestion(seen) }
                case .skip: break
                }
            }
        }
        guard !waiting.isEmpty else { return }
        researching.formUnion(waiting.map(\.1.id))
        Log.info("searching \(waiting.count) questions again with new sources")
        Task {
            defer { researching.subtract(waiting.map(\.1.id)) }
            guard let client = await waitForEngine() else { return }
            for (mid, q) in waiting {
                // never answered from its own meeting's notes (docs/30)
                guard let r = try? await client.ask(q.question, context: [], exclude: ["meeting/\(mid)"]) else { continue }
                var line: Answer?
                let tried = r.shown && !r.lowConfidence && !r.cards.isEmpty
                if tried {
                    line = try? await client.answer(q.question, context: [], chunks: r.cards.compactMap(\.chunk))
                }
                // a failed search is not a result: the question keeps its chance with these sources
                guard ResearchRules.keepsResult(errorKind: r.errorKind, answerTried: tried, answerGot: line?.answer != nil) else { continue }
                let seen = ResearchRules.merged(q.searchedWith, stamp)
                let saidNo = line?.answer?.hasPrefix("찾은 자료에는 답이 없어요") == true
                let unanswered = !r.shown || r.cards.isEmpty || saidNo
                // nothing better found: keep what the meeting had, only remember these sources were tried
                let found = unanswered && q.noSource != true
                    ? SavedQuestion(id: q.id, question: q.question, asker: q.asker, at: q.at, answer: q.answer, cards: q.cards,
                                    cited: q.cited, unanswered: true, noSource: nil, voice: q.voice, searchedWith: seen)
                    : SavedQuestion(id: q.id, question: q.question, asker: q.asker, at: q.at, answer: line?.answer,
                                    cards: r.cards.map(\.forRecord), cited: line?.cited ?? [], unanswered: unanswered,
                                    voice: q.voice, searchedWith: seen)
                library.update(mid) { $0.setQuestion(found) }
            }
            // questions kept while nothing was connected and still not found are read like the meeting's own (docs/24 4절)
            for mid in Set(waiting.map(\.0)) { await reviewQuestions(for: mid, client: client) }
        }
    }

    /// Asks the engine once for the notes of a meeting that just ended and keeps them with the meeting.
    private func fetchNotes(for id: String, asked: Bool, session: String, client: EngineClient) {
        Task {
            // "그건 제가 볼게요" right before 회의 끝내기 may still be with STT (bounded: the engine also waits)
            let deadline = Date().addingTimeInterval(5)
            while pendingHears > 0 && Date() < deadline { try? await Task.sleep(for: .milliseconds(100)) }
            if await notesCanGo(id) { await keepNotes(for: id) { try await client.ledger(session: session) } }
            // on its own, like the questions' review: the window opening does not wait for it
            Task { await panel?.followUp(id) }
            // on its own: the window opening below does not wait for it
            Task { await reviewQuestions(for: id, client: client) }
            // no questions: open only when the meeting settled something (a summary alone is chit-chat too), and never
            // over a live meeting or a newer one
            guard !isRunning, let m = library.meetings.first, m.id == id,
                  NotesFlow.opensAfterNotes(asked: asked, kind: m.shownKind, hasLedger: !m.ledger.isEmpty) else { return }
            onEnded(m)
        }
    }

    /// 정리 형식 바꾸기: the notes again as another kind, from the transcript kept on this Mac.
    func changeKind(_ id: String, to kind: NotesKind) {
        guard Self.notesOn, let m = library.meetings.first(where: { $0.id == id }), m.end != nil else { return }
        let from = m.shownKind
        var changed = false
        library.update(id) { changed = NotesFlow.markKindChange(&$0, to: kind) }
        guard changed else { return }
        Analytics.capture(.notesKindChanged(from: ANotesKind(from), to: ANotesKind(kind)))
        writeAgain(id, m, kind: kind)
    }

    /// 다시 쓰기: the notes again from the transcript kept on this Mac, so it works after an engine restart too.
    /// `request`: what the panel was asked the notes to be ("요약을 더 자세히", docs/38 12절).
    func rewriteNotes(_ id: String, request: String? = nil) {
        guard Self.notesOn, let m = library.meetings.first(where: { $0.id == id }), m.end != nil else { return }
        library.update(id) { $0.notes = .waiting; $0.notesProblem = nil }
        Analytics.capture(.notesRewritten)
        writeAgain(id, m, kind: NotesFlow.rewriteKind(m), request: request)
    }

    /// "Mac 안에서만" (2-D): look for Ollama or LM Studio once more before notes go out; with none, no request is sent
    /// and the meeting says to turn one on (it would fail as a key error). 정리 다시 쓰기 goes through here too.
    private func notesCanGo(_ id: String) async -> Bool {
        // a saved model that is gone (removed or swapped in Ollama) is looked for again too, not only a missing one
        if Build.isOpen, AIMode.saved() == .local {
            let saved = LocalServer.saved()
            let still = if let saved { await AIChecks.serves(saved) } else { false }
            if !still, let found = await AIChecks.findLocalServer(), found != saved {
                LocalServer.save(found)
                await pushAIKeys()
            }
        }
        guard OpenAIMeeting.notesGo(open: Build.isOpen, mode: AIMode.saved(), server: LocalServer.saved()) else {
            let problem = await AIChecks.idleServer().map(OpenAIMeeting.noModelNotes) ?? OpenAIMeeting.noServerNotes
            library.update(id) { $0.notes = .failed; $0.notesProblem = problem; $0.notesLimit = nil }
            return false
        }
        return true
    }

    private func writeAgain(_ id: String, _ m: SavedMeeting, kind: NotesKind?, request: String? = nil) {
        Task {
            guard let client = await waitForEngine() else {
                library.update(id) { $0.notes = .failed; $0.notesProblem = Self.say("engine"); $0.notesLimit = nil }
                return
            }
            guard await notesCanGo(id) else { return }
            await keepNotes(for: id) {
                try await client.notes(lines: m.transcript, asked: m.transcript.filter(\.question).map(\.key), speaker: m.notesSpeaker,
                                       kind: kind, request: request)
            }
            // on its own, like the questions' review: the window opening does not wait for it
            Task { await panel?.followUp(id) }
            await reviewQuestions(for: id, client: client)
        }
    }

    /// 문서로 남길 질문 not read yet, read once against the whole meeting (docs/24 4절): set aside when they asked for no
    /// fact, rewritten to read on their own, with the answer said aloud when there was one. A failed read leaves them as
    /// heard; the next 다시 쓰기 tries again.
    private func reviewQuestions(for id: String, client: EngineClient) async {
        guard Self.notesOn, !reviewing.contains(id),
              let m = library.meetings.first(where: { $0.id == id }), !m.transcript.isEmpty else { return }
        let open = m.questions.filter { $0.unanswered && $0.noSource != true && $0.review == nil }
        guard !open.isEmpty else { return }
        // one read at a time per meeting: a 다시 쓰기 while the first is out would pay for the same questions twice
        reviewing.insert(id)
        defer { reviewing.remove(id) }
        let asked = open.map { q in (id: q.id, line: m.questionLine(q)?.key, text: q.question) }
        do {
            let r = try await client.reviewQuestions(lines: m.transcript, questions: asked, speaker: m.notesSpeaker)
            if let e = r.errorKind ?? r.error {
                Log.info("questions review: failed (\(e))")
                return
            }
            library.update(id) { m in
                for item in r.questions {
                    guard let i = m.questions.firstIndex(where: { $0.id == item.id }) else { continue }
                    m.questions[i].review = QuestionReview(verdict: item.verdict, question: item.question,
                                                           answer: item.answer.map(m.keyedRefs), refs: m.lineKeys(item.utteranceIds))
                }
            }
            let set = r.questions.filter { $0.verdict == QuestionReview.notQuestion }.count
            Log.info("questions review: \(r.questions.count) of \(open.count) read, \(set) set aside")
        } catch {
            Log.info("questions review: failed (engine)")
        }
    }

    /// Asks for the notes and keeps what comes back with the meeting, or why nothing did.
    private func keepNotes(for id: String, _ ask: () async throws -> Ledger) async {
        var notes: Ledger?
        var problem: String?
        var limit: NotesLimit?
        do {
            let l = try await ask()
            if let kind = l.errorKind {
                problem = Self.say(kind)
                if Relay.on, kind == "credit" { limit = .notes }
                Analytics.capture(.errorShown(kind: AErrorKind(kind: kind), place: .notes))
            } else if l.error == "unreadable" {
                problem = String(localized: "결과를 읽을 수 없었어요")
            } else if l.error != nil {
                problem = Self.say("down")
            } else {
                notes = l
                Analytics.capture(.notesWritten(kind: ANotesKind(l.kind.flatMap(NotesKind.init(rawValue:)) ?? .meeting)))
            }
        } catch EngineError.http(404, _) {
            problem = String(localized: "검색 엔진이 다시 시작됐어요")  // it restarted and forgot this meeting
        } catch {
            problem = Self.say("engine")
        }
        library.update(id) { m in
            // the person picked again while this was being written: the later pick's reply is the one to keep
            if let notes, NotesFlow.isStale(m, replyKind: notes.kind) { return }
            if let notes {
                // notes written again from the transcript name owners as the lines show them: back to the speaker's key
                let entries = notes.items.map { item in
                    var e = LedgerEntry(item: item)
                    e.owner = e.owner.map(m.ownerKey)
                    e.refs = m.lineKeys(item.utteranceIds)
                    return e
                }
                m.apply(title: notes.title, summary: notes.summary ?? [], topics: notes.topics, entries: entries,
                        kind: notes.kind.flatMap(NotesKind.init(rawValue:)), kindSource: notes.kindSource)
            } else {
                m.notes = .failed
                m.notesProblem = problem
                m.notesLimit = limit
            }
        }
        Log.info(notes.map { "notes: \($0.items.count) items, summary \($0.summary?.count ?? 0), topics \($0.topics?.count ?? 0)" } ?? "notes: failed")
        if notes != nil { await markChangedDecisions(id) }
    }

    /// Notes the decisions that changed one from an earlier meeting (docs/30 8절). Nothing changes when it fails.
    /// Also for an iPhone meeting whose decisions just came from the server (IPhoneMeetings).
    func markChangedDecisions(_ id: String) async {
        guard let client = engine.client, let m = library.meetings.first(where: { $0.id == id }),
              m.ledger.contains(where: { $0.kind == "decision" }) else { return }
        let changes = await DecisionTrail.changes(for: m, in: library.meetings, client: client)
        library.update(id) { DecisionTrail.mark(&$0, changes) }
    }

    private func hold() async {
        if holdMs > 0 { try? await Task.sleep(for: .milliseconds(holdMs)) }
    }

    /// Newest question takes the island; the one it replaces stays in the trail and the history.
    /// A heard or Slack question takes the pill back from the ask field so it can say who asked.
    /// Someone reading (pointer on the island) keeps their answer; the new one waits on the trail.
    /// The questions the island lists: heard ones kept without a connection stay off it (they wait in the meeting's
    /// notes, where connecting answers them); typed ones show why nothing was searched.
    var islandBlocks: [Block] { blocks.filter { !($0.noSource && $0.origin != .typed) } }

    private func insert(_ b: Block) {
        blocks.insert(b, at: 0)
        // nothing to search with yet: a heard question is only kept for the meeting's notes, where connecting answers it
        if b.origin != .typed, !b.sample, !hasSource() { return }
        if b.origin != .typed {
            asking = false
            if hovering, peeking == nil, let reading = opened { peeking = reading }
        } else {
            peeking = nil
        }
        if peeking == nil { presenting = false }
        collapse?.cancel()
        if transcriptOpen {
            // the question is a "나" bubble in 대화 기록 now: the pill goes back to the meeting row
            if b.origin == .typed { asking = false; query = "" }
            return
        }
        shown = .answer(b.id)
    }

    private func update(_ id: String, _ change: (inout Block) -> Void) {
        if let i = blocks.firstIndex(where: { $0.id == id }) {
            change(&blocks[i])
            settle(blocks[i])
        } else if var b = closing[id] {
            change(&b)
            closing[id] = b.answerDone ? nil : b
            settle(b)
        }
    }

    private func push(level l: Float) {
        guard state == .listening else { return }
        level = level * 0.4 + l * 0.6
    }

    private func waitForEngine() async -> EngineClient? {
        engine.revive()
        while engine.client == nil {
            if case .failed(let msg) = engine.state { state = .failed(msg); return nil }
            try? await Task.sleep(for: .milliseconds(200))
        }
        return engine.client
    }
}

@MainActor
enum CardActions {
    /// `real`: not the onboarding's practice or sample data, so it counts toward 처음 써 보기 (docs/11 14절).
    static func open(_ card: Card, place: ACardPlace = .island, real: Bool = true) {
        if MeetingCards.gone(card) { return }
        count(card, .open, place)
        if real { FirstStepsStore.shared.mark(.card) }
        if MeetingCards.open(card) { return }
        guard let url = card.link else { return }
        // Linear: the desktop app when it is installed (`linear://linear.app/…`), the browser otherwise (docs/18 1-7절)
        if card.source == "linear", url.host == "linear.app",
           let app = URL(string: "linear://" + url.absoluteString.dropFirst("https://".count)),
           url.scheme == "https", NSWorkspace.shared.urlForApplication(toOpen: app) != nil {
            NSWorkspace.shared.open(app)
            return
        }
        NSWorkspace.shared.open(url)
    }

    static func openInEditor(_ card: Card, real: Bool = true) {
        guard let local = card.localPath else { return }
        count(card, .editor, .island)
        if real { FirstStepsStore.shared.mark(.card) }
        // VS Code opens at the line; anything else gets the file's default app
        if let url = URL(string: "vscode://file\(local):\(card.lineStart)"), NSWorkspace.shared.urlForApplication(toOpen: url) != nil {
            NSWorkspace.shared.open(url)
        } else {
            NSWorkspace.shared.open(URL(fileURLWithPath: local))
        }
    }

    static func copyPath(_ card: Card) {
        count(card, .copyPath, .island)
        copy("\(card.path):\(card.lineStart)")
    }

    /// The answer as a Slack message: the sentence, then each file as a link.
    static func copyForSlack(_ block: MeetingModel.Block) {
        if let first = block.cards.first { count(first, .copySlack, .island) }
        var lines: [String] = []
        if let a = block.answer { lines.append(Citations.plain(a)) }
        for (i, c) in block.cards.enumerated() {
            let name = c.name
            // a past meeting has no link: its title, not its id
            lines.append(c.link.map { "\(i + 1). <\($0.absoluteString)|\(name)>" } ?? "\(i + 1). \(c.meetingID == nil ? c.path : name)")
        }
        copy(lines.joined(separator: "\n"))
    }

    /// `card_opened` (docs/16): where the card came from and what was done with it, never which card.
    private static func count(_ card: Card, _ action: ACardAction, _ place: ACardPlace) {
        Analytics.capture(.cardOpened(source: ACardSource(rawValue: card.source ?? "repo") ?? .repo, action: action, place: place))
    }

    private static func copy(_ s: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }
}

/// What a meeting's suggestions were made from (MeetingModel.suggestions).
private struct SuggestStamp: Equatable {
    let meeting: String
    let voices: [VoiceEntry]
    let names: [String]
    let people: [String]
    let revision: Int
}
