import Foundation
import Observation

/// Starts the Rust search engine bundled in the app (`Contents/MacOS/tacie-engine`) and waits for its {"port": N} line.
/// TACIE_ENGINE_BIN overrides the binary for development; `EngineLaunch` picks it and the flags.
/// The engine's working directory (its `index` and `data`) is `EngineLaunch.root`: Application Support in a product
/// build, the Tacie checkout in a development build.
@Observable
@MainActor
final class EngineProcess {
    enum State: Equatable { case starting, ready, failed(String) }

    private(set) var state: State = .starting { didSet { if state != oldValue { onChange?(state) } } }
    private(set) var client: EngineClient?
    /// The meeting listens here: a restarted engine knows none of its sessions.
    var onChange: ((State) -> Void)?
    /// Runs on every start, after the port line and before `state` turns `.ready`: hands the new engine what it
    /// holds in memory only (the AI keys). Everything that waits for `.ready` (the meeting renewing its session, the
    /// first question) therefore never reaches an engine without keys. It must not need `client` (still nil then).
    /// False: the engine never took a setting it must have before a meeting reaches it (on-device speech recognition
    /// switched on, docs/22). That engine never reports ready; it is stopped and started again like a crash.
    var prepare: (EngineClient) async -> Bool = { _ in true }
    private var process: Process?
    private var restarts: [Date] = []
    /// The engine's stdin: closes when this app exits for any reason, and the engine then quits. One per launch:
    /// Process closes the child's end in this app, so reusing it made the restart throw and froze the app.
    private var lifeline: Pipe?
    private var stdout: Pipe?
    /// The token of the current start: a port line from an older process is ignored.
    private var currentToken: String?
    /// The key push for the engine that just printed its port (`prepare`), until it reports ready.
    private var preparing: Task<Void, Never>?

    static var root: URL {
        let fm = FileManager.default
        return EngineLaunch.root(env: ProcessInfo.processInfo.environment, isDev: Build.isDev,
                                 saved: UserDefaults.standard.string(forKey: "engineRoot"),
                                 home: fm.homeDirectoryForCurrentUser,
                                 appSupport: fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0],
                                 space: AccountSpace.inUse)
    }

    /// A development build's own checkout of the indexed repository, to open cards in (developer page). No default.
    static var checkout: String? { UserDefaults.standard.string(forKey: "checkoutRoot") }

    /// The account's space is changing (AppDelegate.switchSpace): no start of any kind (a crash restart, the model
    /// arriving) until it is open, so an engine never comes up in the folder being moved.
    var held = false
    /// Goes up on every start: a request pinned to one engine (MCPBridge) knows a later one is another.
    private(set) var generation = 0

    func start() {
        guard !held else {
            Log.info("engine start waits: the account's space is changing")
            return
        }
        generation += 1
        let launch = EngineLaunch.executable(env: ProcessInfo.processInfo.environment,
                                             bundled: Bundle.main.url(forAuxiliaryExecutable: "tacie-engine"),
                                             isExecutable: FileManager.default.isExecutableFile(atPath:))
        let executable: URL
        switch launch {
        case .success(let url): executable = url
        case .failure(let error):
            Log.info("engine not started: \(error.message)")
            state = .failed(error.message)
            return
        }
        // a new Mac has no Application Support/Tacie yet (the engine makes index/ and data/ inside it)
        let root = Self.root
        do {
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        } catch {
            Log.info("engine not started: cannot make \(root.path): \(error.localizedDescription)")
            state = .failed(String(localized: "검색 데이터를 둘 폴더를 만들 수 없어요: \(root.path)"))
            return
        }
        let p = Process()
        p.executableURL = executable
        let sample = EngineLaunch.sampleDir(.current) { Bundle.main.url(forResource: $0, withExtension: nil)?.path }
        p.arguments = EngineLaunch.arguments(checkout: Build.isDev ? Self.checkout : nil, sample: sample)
        let token = UUID().uuidString + UUID().uuidString
        p.currentDirectoryURL = root
        var env = ProcessInfo.processInfo.environment
        env["TACIE_ENGINE_TOKEN"] = token
        // an app started from Finder gets a bare PATH; only the dev build's gh login mode needs gh (Homebrew)
        env["PATH"] = (env["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin") + ":/opt/homebrew/bin:/usr/local/bin"
        env.merge(DevSettings.environment(.standard, dev: Build.isDev)) { _, new in new }
        // the Soniox region for this Mac (docs/25 8-2); a developer page pick wins
        if env["TACIE_SONIOX_REGION"] == nil { env["TACIE_SONIOX_REGION"] = SonioxRegion.current }
        // the languages this Mac's meetings may be in (SpeechLanguages): the engine keeps their letters
        env["TACIE_SPEECH_LANGS"] = SpeechLanguages.current.joined(separator: ",")
        // the Mac's first language: a Korean Mac keeps on-device whisper on Korean, any other lets it detect the language
        if let first = SpeechLanguages.first(preferred: Locale.preferredLanguages) { env["TACIE_MAC_LANG"] = first }
        if Build.isDev, ProcessInfo.processInfo.arguments.contains("--debug-text") { env["TACIE_LOG_TEXT"] = "1" }
        // the token itself is handed over by HTTP once the engine is up; without it the engine must not use gh's login
        env["TACIE_GITHUB_MODE"] = Build.usesTacieLogin ? "none" : "gh"
        // AI keys come from the keychain over HTTP once the engine is up; only a dev build lets it read ~/.config key files
        // …and never in the open edition's "Mac 안에서만" (2-D)
        if AIModeApply.keyFiles(dev: Build.isDev, mode: AIMode.saved(), open: Build.isOpen) { env["TACIE_KEY_FILES"] = "1" } else { env["TACIE_KEY_FILES"] = nil }
        // Tacie AI (docs/14): the AI calls go to our relay with the session in place of a key
        launchedWithRelay = Relay.on
        if launchedWithRelay {
            let region = RelayRegion.current
            env["TACIE_OPENROUTER_URL"] = AccountServer.openRouterRelay(region)
            env["TACIE_JEV_URL"] = AccountServer.jevRelay(region)
        }
        if LaunchArgs.has("--jev-fail") { env["TACIE_JEV_FAIL"] = "1" }
        // the server can turn the fine-tuned CAM++ off (SpeakerModel): the engine then groups voices with ERes2Net alone
        if SpeakerModel.eres2netOnly() { env["TACIE_DIAR_MODEL"] = "off" }
        p.environment = env
        let out = Pipe()
        stdout = out
        currentToken = token
        p.standardOutput = out
        let life = Pipe()
        lifeline = life
        p.standardInput = life
        p.standardError = Self.logHandle()
        p.terminationHandler = { [weak self] proc in
            Task { @MainActor in self?.terminated(status: proc.terminationStatus) }
        }
        out.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            if data.isEmpty {  // EOF: the process is gone
                handle.readabilityHandler = nil
                return
            }
            guard let line = String(data: data, encoding: .utf8)?.split(separator: "\n").first,
                  let json = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any],
                  let port = json["port"] as? Int else { return }
            handle.readabilityHandler = nil
            Task { @MainActor in self?.ready(port: port, token: token) }
        }
        do {
            try p.run()
            process = p
            state = .starting
        } catch {
            state = .failed(String(localized: "검색 엔진을 띄우지 못했어요: \(error.localizedDescription)"))
        }
    }

    /// After the engine gave up, the next meeting or question tries again from a clean slate.
    func revive() {
        guard case .failed = state else { return }
        restarts = []
        state = .starting
        start()
    }

    /// The engine was started with its AI addresses at the Tacie AI relay (a sign-in or out changes that: restart).
    private(set) var launchedWithRelay = false

    /// The developer tab's "적용하고 엔진 다시 띄우기": a clean start with today's settings, not counted as a crash.
    func restart() {
        stop()
        client = nil
        restarts = []
        state = .starting
        start()
    }

    /// Stops the engine and waits (at most five seconds, then kills it) until it has exited, so nothing writes to its
    /// folder any more: the account's space is about to change (AppDelegate.switchSpace).
    func stopAndWait() async {
        let p = process
        stop()
        client = nil
        restarts = []
        state = .starting
        guard let p else { return }
        let exited = await Task.detached { () -> Bool in
            for _ in 0..<50 where p.isRunning { try? await Task.sleep(for: .milliseconds(100)) }
            return !p.isRunning
        }.value
        if !exited {
            kill(p.processIdentifier, SIGKILL)
            Log.info("engine did not exit in time; killed")
            for _ in 0..<20 where p.isRunning { try? await Task.sleep(for: .milliseconds(100)) }
        }
    }

    func stop() {
        stdout?.fileHandleForReading.readabilityHandler = nil
        stdout = nil
        currentToken = nil
        preparing?.cancel()
        preparing = nil
        process?.terminationHandler = nil
        process?.terminate()
        process = nil
    }

    private func ready(port: Int, token: String) {
        guard token == currentToken else { return }
        Log.info("engine ready on port \(port)")
        let c = EngineClient(port: port, token: token)
        preparing?.cancel()
        preparing = Task { @MainActor in
            let ok = await prepare(c)
            // the engine died or was replaced while the keys were on their way: it never reports ready (a crash loop
            // stays .failed; a successor gets its own push)
            guard !Task.isCancelled, token == currentToken, process != nil else { return }
            guard ok else {
                // its exit runs `terminated`, which starts another one (or reports a crash loop)
                Log.info("engine did not take its settings; restarting it")
                process?.terminate()
                return
            }
            preparing = nil
            client = c
            state = .ready
        }
    }

    private func terminated(status: Int32) {
        Log.info("engine exited with status \(status)")
        // its token is dead: a prepare still in flight for it must not mark it ready
        currentToken = nil
        preparing?.cancel()
        preparing = nil
        client = nil
        process = nil
        // restart on its own up to 3 times in 10 minutes; a crash loop gets reported instead
        restarts = restarts.filter { $0.timeIntervalSinceNow > -600 }
        if restarts.count < 3 {
            restarts.append(Date())
            state = .starting
            start()
        } else {
            state = .failed(String(localized: "검색 엔진이 멈췄어요 (종료 코드 \(Int(status))). 로그: ~/Library/Logs/\(Build.logFolder)/engine.log"))
        }
    }

    private static func logHandle() -> FileHandle? {
        let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/\(Build.logFolder)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let file = dir.appendingPathComponent("engine.log")
        // keep the log of the engine that just died: the one before this start becomes engine.log.1
        let old = dir.appendingPathComponent("engine.log.1")
        if FileManager.default.fileExists(atPath: file.path) {
            try? FileManager.default.removeItem(at: old)
            try? FileManager.default.moveItem(at: file, to: old)
        }
        FileManager.default.createFile(atPath: file.path, contents: nil)
        return try? FileHandle(forWritingTo: file)
    }
}
