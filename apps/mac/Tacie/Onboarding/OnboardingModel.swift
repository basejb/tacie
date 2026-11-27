import AVFoundation
import AppKit

/// The onboarding's live state: the saved progress plus what is true on this Mac right now (docs/11 4절).
@Observable
@MainActor
final class OnboardingModel {
    private let store = OnboardingStore()
    let keeper: IndexKeeper
    var progress: OnboardingProgress { didSet { store.save(progress) } }
    private(set) var mic = AVCaptureDevice.authorizationStatus(for: .audio)
    /// A microphone is plugged in or built in. The permission alone says nothing about it (a Mac mini has none).
    private(set) var micConnected = OnboardingModel.hasMic()
    /// The meter heard a voice at least once since the listen step opened: only then it says 들려요.
    private(set) var heardVoice = false
    private var deviceWatch: [NSObjectProtocol] = []
    private(set) var callAudio: CallAudioState
    private(set) var probing = false
    /// When the current probe started: past 30 s the prompt may be hidden, and the step says so.
    private(set) var probeStarted: Date?
    private var probeID = 0
    /// The microphone's level while the listen step shows (0…1), for the meter and the face.
    private(set) var level: Float = 0
    /// Where the sources step is: the tool grid, the GitHub code, or the repository list.
    var sourcesPage = SourcesPage.tools

    enum SourcesPage { case tools, github, repos }

    // MARK: practice on the real island (3·4단계)

    /// The search model's download, shown under the steps (docs/11 4-2절).
    var download: ModelDownload?

    /// The island's model: the practice steps watch its questions.
    weak var meeting: MeetingModel?
    /// When the shown practice step began: only questions after it count.
    private var practiceSince: Date?
    /// Practice steps that got an answer; kept so going back and forth does not undo it.
    private(set) var practiced: [OnboardingStep: Practice] = [:]

    /// `near`: no certain answer, but the island showed the closest places; the practice still counts (the shortcut
    /// and the cards were seen).
    enum Practice: Equatable {
        case waiting, finding, answered, near, missed
        var done: Bool { self == .answered || self == .near }
    }

    /// Questions most repositories answer somewhere (README, docs, config). Before the practice they are asked once
    /// in the background and only those with a certain answer in this user's material become buttons.
    /// Each has a recorded voice for the meeting practice (`PracticeVoice.recorded`): a new one needs its recording too.
    static var candidates: [String] { SampleLanguage.current.practiceQuestions }
    /// Nothing connected but the app carries the sample data: the practice runs on it (docs/11 12절).
    private var sampleReady: Bool { keeper.info?.sample?.ready == true }
    var usesSample: Bool { !connectedSource && sampleReady }
    /// A service is connected with something found: GitHub with a repository picked, or Notion, Linear, Google Drive or
    /// Jira · Confluence, Asana, Dropbox or a note folder with something in the search data. `--no-source` (development) acts as if nothing were.
    private var connectedSource: Bool {
        guard !LaunchArgs.has("--no-source"), keeper.cleared == false else { return false }
        let github = keeper.connected && keeper.info?.repo != nil
        let notion = keeper.notion.connected && keeper.info?.notion?.pages != nil
        let linear = keeper.linear.connected && keeper.linearInfo?.issues != nil
        let gdrive = keeper.gdrive.connected && keeper.gdriveInfo?.files != nil
        let atlassian = OnboardingFlow.atlassianFound(connected: keeper.atlassian.connected,
                                                      issues: keeper.atlassianInfo?.issues, pages: keeper.atlassianInfo?.pages)
        let asana = OnboardingFlow.asanaFound(connected: keeper.asana.connected, tasks: keeper.asanaInfo?.tasks)
        let dropbox = OnboardingFlow.dropboxFound(connected: keeper.dropbox.connected, files: keeper.dropboxInfo?.files)
        let notes = OnboardingFlow.notesFound(notes: keeper.notesInfo?.notes)
        return OnboardingFlow.hasSource(github: github, notion: notion, linear: linear, gdrive: gdrive, atlassian: atlassian,
                                        asana: asana, dropbox: dropbox, notes: notes)
    }
    /// Whether `examples` came from the sample: connecting meanwhile asks for the user's own questions again.
    private var examplesFromSample: Bool?

    /// The practice's questions, best first; nil until the check is done.
    private(set) var examples: [String]?
    private var checking = false
    private let voice = PracticeVoice()
    /// The meeting practice is turning the question into speech.
    private(set) var speaking = false

    /// No transcribing-and-answer key reaches the engine (neither saved in the app nor a key file): nothing in the
    /// practice can be heard or answered, so the step asks for it first. nil until the engine has said.
    private(set) var aiKeyMissing: Bool?

    func checkAIKey() async {
        guard let client = meeting?.engine.client, let c = try? await client.config() else { return }
        aiKeyMissing = c.keys.first { $0.name == "openrouter" }?.state == "missing"
    }

    /// The key pasted in the practice step: the same as 설정 > AI's 받아쓰기·답 키, handed to the engine at once.
    func saveAIKey(_ paste: String) async -> String? {
        do {
            try AIKeys.save(paste, as: .openrouter)
        } catch AIKeys.SaveError.notAKey {
            return String(localized: "키 형식이 아니에요. 복사한 키를 그대로 붙여 넣어 주세요.")
        } catch {
            return String(localized: "키체인에 저장하지 못했어요.")
        }
        await keeper.pushAIKeys()
        await checkAIKey()
        return nil
    }

    func beginPractice() {
        practiceSince = Date()
        Task { await checkAIKey() }
        Task { await findExamples() }
    }

    /// Asks every candidate once (one rerank call each, no answer line) and keeps those the island would show as
    /// certain, best first; if none is certain, the two closest. Waits for the first sync to finish.
    func findExamples() async {
        if examplesFromSample != nil, examplesFromSample != usesSample { examples = nil }
        if usesSample {
            // the sample's questions are written to have certain answers: no check needed
            examples = keeper.info?.sample?.questions.filter { $0.use == "ask" }.prefix(3).map(\.text)
            examplesFromSample = true
            return
        }
        examplesFromSample = false
        guard examples == nil, !checking, let meeting else { return }
        checking = true
        defer { checking = false }
        for _ in 0..<600 where meeting.engine.client == nil || keeper.step != nil {
            try? await Task.sleep(for: .seconds(1))
        }
        guard let client = meeting.engine.client else { return }
        let scored = await withTaskGroup(of: (String, Bool, Double)?.self) { group in
            for q in Self.candidates {
                group.addTask {
                    guard let r = try? await client.ask(q, context: []) else { return nil }
                    return (q, r.shown && !r.cards.isEmpty, r.bestP ?? 0)
                }
            }
            var out: [(String, Bool, Double)] = []
            for await r in group { if let r { out.append(r) } }
            return out
        }
        let sure = scored.filter(\.1).sorted { $0.2 > $1.2 }.map(\.0)
        let picked = sure.isEmpty ? Array(scored.sorted { $0.2 > $1.2 }.map(\.0).prefix(2)) : Array(sure.prefix(3))
        examples = picked.isEmpty ? Array(Self.candidates.prefix(2)) : picked
        Log.info("onboarding examples: \(sure.count) certain of \(scored.count)")
    }

    /// 3단계: the question goes into the real island, as if typed there after ⌃⌥⌘A.
    func askOnIsland(_ question: String) {
        guard let meeting else { return }
        if usesSample { return meeting.askSample(question) }
        meeting.beginAsking()
        meeting.query = question
        meeting.ask(onboarding: true)
    }

    /// The question the meeting practice speaks: the second example when there is one, so it differs from 3단계.
    var meetingQuestion: String? { meetingLine?.spoken }

    /// What is spoken, and what is searched: the sample's line was checked as spoken, the user's example as written.
    private var meetingLine: (spoken: String, search: String)? {
        if usesSample {
            return keeper.info?.sample?.questions.first { $0.use == "spoken" }.map { ($0.text, $0.text) }
        }
        guard let e = examples, !e.isEmpty else { return nil }
        let q = e.count > 1 ? e[1] : e[0]
        return (SampleLanguage.current.spokenLine(q), q)
    }

    /// The meeting practice made ahead for its line: the voice and the answer are ready before 들려주기 is pressed, so the
    /// answer comes up as the voice stops (2026-11-01 사용자: 예시인데 답이 너무 느려요).
    private var rehearsal: (line: String, sample: Bool, voice: Task<PracticeVoice.Rendered?, Never>, result: Task<MeetingModel.Rehearsed?, Never>)?

    /// Starts making the practice for the current line, once per line.
    func prepareMeeting() {
        guard let meeting, let line = meetingLine, aiKeyMissing != true else { return }
        let sample = usesSample
        if let r = rehearsal, r.line == line.spoken, r.sample == sample { return }
        let voice = self.voice
        rehearsal = (line.spoken, sample,
                     Task { await voice.render(line.spoken) },
                     Task { await meeting.rehearse(line.search, sample: sample) })
    }

    /// 4단계: a short practice meeting that plays the question aloud, then shows its answer and ends by itself.
    func playMeeting() async {
        guard let meeting, !meeting.isRunning, !speaking else { return }
        prepareMeeting()
        guard let r = rehearsal else { return }
        speaking = true
        defer { speaking = false }
        guard let sound = await r.voice.value else { return }
        // a failed fetch is tried again on the next play
        let result = r.result
        Task { if await result.value == nil, rehearsal?.line == r.line { rehearsal = nil } }
        meeting.playRehearsal(FileSource(url: sound.feed), question: r.line, sample: r.sample, result: result)
        voice.play(sound.play)
    }

    /// Where the step's practice is: the latest question of its kind (typed for 3단계, spoken for 4단계).
    func practice(_ step: OnboardingStep) -> Practice {
        let live = latest(step)
        return live.done ? live : practiced[step] ?? live
    }

    private func latest(_ step: OnboardingStep) -> Practice {
        guard let since = practiceSince, let blocks = meeting?.blocks else { return .waiting }
        let mine = blocks.filter { b in
            guard b.at >= since else { return false }
            if case .voice = b.origin { return step == .meeting }
            return step == .ask && b.origin == .typed
        }
        guard let last = mine.last else { return .waiting }
        switch last.phase {
        case .finding: return .finding
        case .failed: return .missed
        case .found:
            if !last.unanswered { return .answered }
            return last.cards.isEmpty ? .missed : .near
        }
    }

    /// 남은 일's 숨기기: the row stays away until the onboarding is started again from the menu.
    func dismiss(_ item: OnboardingLeftover) {
        progress.dismissed = (progress.dismissed ?? []).union([item.id])
    }

    /// 계속 on a practice step: it was seen, card or not (no key, nothing found), so 남은 일 no longer asks for it.
    func leavePractice() {
        progress.skipped.subtract([.ask, .meeting])
        next()
    }

    /// A card came up: the practice is done, whichever of the two steps showed it.
    func markPracticed(_ step: OnboardingStep, _ how: Practice) {
        practiced[step] = how
        progress.skipped.subtract([.ask, .meeting])
    }

    init(keeper: IndexKeeper) {
        self.keeper = keeper
        progress = store.load() ?? OnboardingProgress()
        callAudio = CallAudio.lastState
        // a headset plugged in or pulled out while the window is open
        for name in [AVCaptureDevice.wasConnectedNotification, AVCaptureDevice.wasDisconnectedNotification] {
            deviceWatch.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                Task { @MainActor in self?.deviceChanged() }
            })
        }
    }

    /// The default input, unless it is one of our own call-sound aggregates (MicSource refuses those too).
    static func hasMic() -> Bool {
        guard let d = AVCaptureDevice.default(for: .audio) else { return false }
        return !d.uniqueID.hasPrefix(CallAudio.aggregatePrefix)
    }

    private func deviceChanged() {
        guard !pinned else { return }
        micConnected = Self.hasMic()
        guard meter != nil else { return }
        stopMeter()
        startMeter()
    }

    var facts: OnboardingFacts {
        let source = connectedSource
        // one app (#56): the AI step ("회의를 정리할 AI를 골라요", with Tacie AI's sign-in in its card) replaces the sign-in step
        return OnboardingFacts(hasSource: source, accountStep: false, signedIn: TacieAccount.shared.signedIn,
                               notionConnected: keeper.notion.connected, linearConnected: keeper.linear.connected,
                               gdriveConnected: keeper.gdrive.connected,
                               atlassianConnected: keeper.atlassian.connected, asanaConnected: keeper.asana.connected,
                               dropboxConnected: keeper.dropbox.connected, notesOn: keeper.notesOn,
                               micGranted: mic == .authorized,
                               micConnected: micConnected, callAudio: callAudio, practiceReady: source || sampleReady,
                               open: true, aiChosen: OnboardingFlow.aiChosen(mode: AIMode.saved(), signedIn: TacieAccount.shared.signedIn),
                               answerCards: OnboardingFlow.answerCards(open: AIMode.ownAI, mode: AIMode.saved(),
                                                                       cardsOff: OpenAIMeeting.cardsOffNow))
    }

    var step: OnboardingStep { OnboardingFlow.resume(progress, facts) }

    /// Tacie AI goes through a sign-in in a product build; a development build shows the step with `--account-step`.
    /// The open edition has no Tacie sign-in (docs/41 0절 7번).
    static var accountStep: Bool {
        OnboardingFlow.accountStep(dev: Build.isDev, open: Build.isOpen, args: ProcessInfo.processInfo.arguments)
    }
    var dots: [OnboardingStep] { OnboardingFlow.visible(facts).filter { $0 != .welcome } }

    func go(_ step: OnboardingStep) {
        progress.step = step
        // 뒤로 from the last step reopens the flow; reaching it again finishes it
        progress.finished = step == .done
    }

    func next() { forward(skipped: false) }

    /// Forward (계속, 나중에, a step finishing by itself): the step left is counted; 뒤로 is not.
    private func forward(skipped: Bool) {
        let from = step
        let to = OnboardingFlow.next(after: from, facts)
        let counts = OnboardingFlow.countsSteps(progress)
        if counts, let s = from.analytics { Analytics.capture(.onboardingStepCompleted(step: s, skipped: skipped)) }
        go(to)
        if counts, to == .done {
            Analytics.capture(.onboardingFinished(skippedCount: progress.skipped.count, practiced: !practiced.isEmpty))
            progress.counted = true
        }
    }
    func back() { if let p = OnboardingFlow.previous(before: step, facts) { go(p) } }

    /// "나중에": this step is left for the main window's checklist.
    func later() {
        progress.skipped.insert(step)
        forward(skipped: true)
    }

    /// Opened again from the menu: from the welcome, keeping what is already done.
    func restart() {
        Analytics.capture(.onboardingRestarted)
        progress.step = .welcome
        progress.finished = false
        progress.dismissed = nil
        sourcesPage = .tools
    }

    func toggleNotify(_ tool: String) {
        if progress.notify.contains(tool) { progress.notify.remove(tool) } else { progress.notify.insert(tool) }
    }

    // MARK: microphone

    func refreshPermissions() {
        guard !pinned else { return }
        mic = AVCaptureDevice.authorizationStatus(for: .audio)
        micConnected = Self.hasMic()
    }

    /// Debug captures only (`listen-preview`): pins the listen step's permissions so each state can be drawn.
    private var pinned = false
    func preview(mic: AVAuthorizationStatus, connected: Bool, call: CallAudioState, probing: Bool, level: Float) {
        pinned = true
        self.mic = mic
        micConnected = connected
        callAudio = call
        self.probing = probing
        probeStarted = probing ? .now : nil
        self.level = level
        heardVoice = level > 0.15
    }

    /// The one button: asks macOS the first time, opens System Settings once it was refused.
    func turnOnMic() async {
        refreshPermissions()
        switch mic {
        case .notDetermined:
            _ = await AVCaptureDevice.requestAccess(for: .audio)
            refreshPermissions()
        case .denied, .restricted:
            Self.openPrivacy("Privacy_Microphone")
        default: break
        }
        if mic == .authorized { startMeter() }
    }

    private var meter: AVAudioEngine?

    func startMeter() {
        guard meter == nil, mic == .authorized else { return }
        let engine = AVAudioEngine()
        let input = engine.inputNode
        let format = input.inputFormat(forBus: 0)
        guard format.channelCount > 0, format.sampleRate > 0 else { return }  // no input device: installTap would throw
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buf, _ in
            guard let ch = buf.floatChannelData?[0] else { return }
            let n = Int(buf.frameLength)
            var sum: Float = 0
            for i in 0..<n { sum += ch[i] * ch[i] }
            let rms = n > 0 ? (sum / Float(n)).squareRoot() : 0
            Task { @MainActor in
                guard self?.pinned == false else { return }
                self?.level = min(1, rms * 12)
                if rms * 12 > 0.15 { self?.heardVoice = true }
            }
        }
        do { try engine.start(); meter = engine } catch { input.removeTap(onBus: 0) }
    }

    func stopMeter() {
        meter?.inputNode.removeTap(onBus: 0)
        meter?.stop()
        meter = nil
        level = 0
    }

    // MARK: the meeting app's sound

    func turnOnCallAudio() async {
        guard !probing else { return }
        probeID += 1
        let id = probeID
        probing = true
        probeStarted = Date()
        progress.callAudioLater = false
        let result = await CallAudioProbe.run()
        guard id == probeID else { return }  // the user stopped waiting; this answer is stale
        callAudio = result
        probing = false
        probeStarted = nil
    }

    /// "그만 기다리기": the prompt was missed or hidden. The waiting call inside Core Audio cannot be cancelled; its
    /// answer is dropped.
    func stopWaiting() {
        probeID += 1
        probing = false
        probeStarted = nil
        callAudio = CallAudio.lastState
    }

    /// "권한 다시 받기": forget the refusal, then check again (macOS shows its prompt again).
    func askCallAudioAgain() async {
        CallAudio.resetPermission()
        callAudio = .unknown
        await turnOnCallAudio()
    }

    static let askMicOnLaunchKey = "onboarding.askMicOnLaunch"

    /// The microphone's "다시 묻기": forget the refusal, then open Tacie again and ask there. The running app keeps
    /// reading the old answer after a reset (and System Settings no longer lists Tacie to switch it on), so only a new
    /// process gets macOS's prompt; the saved progress brings it back to this step.
    func askMicAgain() {
        guard let id = Bundle.main.bundleIdentifier else { return }
        let reset = Process()
        reset.executableURL = URL(fileURLWithPath: "/usr/bin/tccutil")
        reset.arguments = ["reset", "Microphone", id]
        try? reset.run()
        reset.waitUntilExit()
        UserDefaults.standard.set(true, forKey: Self.askMicOnLaunchKey)
        progress.step = .listen
        progress.finished = false
        Log.info("microphone permission reset (\(reset.terminationStatus)); reopening to ask")
        let reopen = Process()
        reopen.executableURL = URL(fileURLWithPath: "/bin/sh")
        reopen.arguments = ["-c", "sleep 1; /usr/bin/open -n \"$0\" --args \"$@\"", Bundle.main.bundlePath]
            + CommandLine.arguments.dropFirst()
        try? reopen.run()
        NSApp.terminate(nil)
    }

    func callAudioLater() {
        progress.callAudioLater = true
    }

    static func openPrivacy(_ anchor: String) {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(anchor)") {
            NSWorkspace.shared.open(url)
        }
    }

    /// What the stuck screen shows and copies.
    var diagnostics: String {
        let id = Bundle.main.bundleIdentifier ?? "-"
        let raw = UserDefaults.standard.string(forKey: CallAudioProbe.lastKey) ?? ""
        let word = ["granted": String(localized: "들림"), "silent": String(localized: "안 들림")][String(raw.split(separator: " ").first ?? "")]
            ?? String(localized: "없음")
        let when = raw.split(separator: " ").dropFirst().first.flatMap { ISO8601DateFormatter().date(from: String($0)) }
            .map { $0.formatted(date: .omitted, time: .shortened) }
        let last = when.map { "\(word) · \($0)" } ?? word
        return String(localized: "앱 \(id)\n마지막 확인 \(last)\nmacOS \(ProcessInfo.processInfo.operatingSystemVersionString)")
    }
}
