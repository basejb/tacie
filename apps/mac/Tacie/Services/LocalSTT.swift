import Foundation
import Network
import Observation

/// What `LocalSTT` needs from the engine (`EngineClient` in the app, a fake in tests).
protocol STTEngine {
    func setSTT(on: Bool, model: URL?) async throws -> STTStatus
}

extension EngineClient: STTEngine {}

/// On-device speech recognition (docs/22): the opt-in switch, the model file and what the engine is told.
///
/// The model never restarts the engine (that would cut a running meeting): when the file is in place the app posts
/// `/stt`, and again every time an engine starts (`EngineProcess.prepare`).
@Observable
@MainActor
final class LocalSTT {
    enum Phase: Equatable {
        case off
        case unsupported            // under 16 GB of memory
        case needsSpace(Int64)      // this many more bytes are needed
        case downloading(done: Int64, total: Int64)
        case waiting                // offline: starts by itself when the connection is back
        case failed(String)
        case ready
    }

    enum TurnOnResult: Equatable { case askConsent, needsSpace(Int64), started, ready, unsupported }

    static let key = "localSTT"
    /// Whether the app offers on-device recognition at all. Hidden 2026-11-11, offered again 2026-11-18 as one of two ways
    /// (서버 is the default, docs/22 9절). Off hides the onboarding and settings rows, never tells the engine to load the
    /// model and ignores a switch turned on earlier.
    static let offered = true
    /// The user's switch, as far as it counts: off whenever the feature is not offered.
    static var switchedOn: Bool { offered && AIMode.localSttOn }  // the open edition's AI mode decides (2-D)
    static let minimumMemory: UInt64 = 16 * 1024 * 1024 * 1024

    nonisolated static func supported(memory: UInt64) -> Bool { memory >= minimumMemory }

    /// An utterance failed on this Mac's speech recognition model (engine `error_kind` "model" from `stt` "local"); a
    /// "model" from anywhere else is the search model still downloading. The meeting says recognition paused.
    nonisolated static func isLocalFailure(kind: String, stt: String?) -> Bool { kind == "model" && stt == "local" }

    private(set) var phase: Phase
    /// The engine failed again and the file is fine: recognition is paused rather than sent to the server.
    private(set) var paused = false
    /// What the engine last answered to `/stt`.
    private(set) var engineStatus: STTStatus?
    /// A broken-model check in flight (tests await it).
    private(set) var checking: Task<Void, Never>?
    private(set) var task: Task<Void, Never>?
    /// How many downloads have started: a meeting says "server" once more when one starts again inside it.
    private(set) var downloadStarts = 0

    /// The switch the user set (UserDefaults `localSTT`, default false).
    var enabled: Bool {
        get { defaults.bool(forKey: Self.key) }
        set { defaults.set(newValue, forKey: Self.key) }
    }

    /// Fetches the pack's files into its folder (resume, hash check, rename); reports bytes on disk for the whole pack.
    typealias Fetch = (ModelPack, @escaping @Sendable (Int64) -> Void) async throws -> Void

    private let pack: ModelPack
    private let defaults: UserDefaults
    private let memory: UInt64
    private let free: () -> Int64
    private let fetch: Fetch
    private let hash: @Sendable (URL) async throws -> String
    private let engine: () -> STTEngine?
    /// The search model comes first when the user turned this on in the onboarding.
    /// Asked for during onboarding, waiting for the search model before downloading (the AI cards say so, 2-D).
    private(set) var waitingForSearch = false
    private var searchReady: () -> Bool
    private let online: () -> Bool
    private let monitor: NWPathMonitor?
    private var path: NWPath?

    init(pack: ModelPack = ModelManifest.sttPack(env: ProcessInfo.processInfo.environment,
                                                 home: FileManager.default.homeDirectoryForCurrentUser),
         defaults: UserDefaults = .standard,
         memory: UInt64 = ProcessInfo.processInfo.physicalMemory,
         free: (() -> Int64)? = nil,
         fetch: Fetch? = nil,
         hash: @escaping @Sendable (URL) async throws -> String = { try ModelDownload.sha256(of: $0) },
         offered: Bool = true,
         searchReady: @escaping () -> Bool = { true },
         online: (() -> Bool)? = nil,
         engine: @escaping () -> STTEngine?) {
        self.pack = pack
        self.defaults = defaults
        self.memory = memory
        self.free = free ?? { Self.freeSpace(near: pack.dir) }
        self.fetch = fetch ?? Self.fetchPack
        self.hash = hash
        self.searchReady = searchReady
        // tests pass `online` and call `pathChanged`; the app watches the real path
        self.online = online ?? { true }
        monitor = online == nil ? NWPathMonitor() : nil
        self.engine = engine
        phase = offered && Self.supported(memory: memory) ? .off : .unsupported
        if phase != .unsupported, defaults.bool(forKey: Self.key) {
            phase = installed ? .ready : .off
            // switched on, file missing (an interrupted download): carry on by itself
            if !installed { resume() }
        }
        if let monitor {
            monitor.pathUpdateHandler = { [weak self] p in
                Task { @MainActor in
                    self?.path = p
                    self?.pathChanged(satisfied: p.status == .satisfied)
                }
            }
            monitor.start(queue: DispatchQueue(label: "tacie.stt-path"))
        }
    }

    deinit { monitor?.cancel() }

    private var isOnline: Bool { monitor != nil ? (path.map { $0.status == .satisfied } ?? true) : online() }

    /// The connection came back: a download that waited for it (or died with it) carries on.
    func pathChanged(satisfied: Bool) {
        guard satisfied, phase == .waiting, enabled, !waitingForSearch else { return }
        startDownload()
    }

    /// What a meeting says while the switch is on and the model is not ready (nil: nothing to say).
    var meetingState: STTMeetingState? {
        guard enabled, let line = LocalSTTCopy.meetingLine(phase) else { return nil }
        return STTMeetingState(line: line, starts: downloadStarts)
    }

    var modelURL: URL { pack.dir.appendingPathComponent(pack.files[0].name) }
    var installed: Bool { ModelManifest.installed(pack.files, in: pack.dir) }
    private var total: Int64 { ModelManifest.total(pack.files) }

    // MARK: switch

    /// `consented`: the user said 받기 in the confirmation. A file already on disk needs no consent.
    /// `afterSearchModel`: the onboarding switch; the download starts once the search model is in.
    @discardableResult
    func turnOn(consented: Bool, afterSearchModel: Bool = false) -> TurnOnResult {
        guard phase != .unsupported else { return .unsupported }
        if installed {
            noteSwitch(on: true)
            enabled = true
            phase = .ready
            paused = false
            requestPush()
            return .ready
        }
        // room first: asking to download 0.6GB that cannot fit would end in a second alert that may never show
        let remaining = ModelManifest.remaining(pack.files, in: pack.dir)
        if case .space(let more) = ModelGate.check(expensive: false, constrained: false, online: true,
                                                  free: free(), remaining: remaining) {
            enabled = false
            phase = .needsSpace(more)
            return .needsSpace(more)
        }
        guard consented else { return .askConsent }
        noteSwitch(on: true)
        enabled = true
        if afterSearchModel && !searchReady() {
            waitingForSearch = true
            phase = .downloading(done: total - remaining, total: total)
            return .started
        }
        startDownload()
        return .started
    }

    /// The search model finished: an onboarding switch that waited for it starts now.
    func searchModelReady() {
        guard waitingForSearch else { return }
        waitingForSearch = false
        if enabled { startDownload() }
    }

    func turnOff() {
        noteSwitch(on: false)
        task?.cancel()
        task = nil
        waitingForSearch = false
        enabled = false
        paused = false
        if phase != .unsupported { phase = .off }
        requestPush()
    }

    /// Removes the file and turns off.
    func deleteModel() {
        noteSwitch(on: false)
        task?.cancel()
        task = nil
        waitingForSearch = false
        enabled = false
        paused = false
        for suffix in ["", ".part"] {
            try? FileManager.default.removeItem(at: pack.dir.appendingPathComponent(pack.files[0].name + suffix))
        }
        if phase != .unsupported { phase = .off }
        requestPush()
    }

    /// `setting_changed` local_stt, only when the switch really moves (docs/16).
    private func noteSwitch(on: Bool) {
        guard enabled != on else { return }
        Analytics.capture(.settingChanged(setting: .localStt, value: ASettingValue(on)))
    }

    /// The sidebar's 다시 받기 after a failure.
    func retry() {
        guard enabled, phase != .unsupported else { return }
        startDownload()
    }

    // MARK: engine

    /// The push queue: one `/stt` at a time, and each one sends what the switch says when it goes out, so a quick
    /// off → on never lands as on → off. `pushDirty` asks the running loop for one more round.
    private var pushing: Task<Void, Never>?
    private var pushDirty = false
    /// The next round turns the model off first (a forced reload when recognition pauses).
    private var reloadNext = false
    /// How many times one round sends again when the engine's answer does not match the switch.
    static let pushAttempts = 3
    /// The pause between those attempts (tests make it short).
    var retryDelay: Duration = .milliseconds(300)

    /// Asks for a push without waiting for it.
    private func requestPush() {
        pushDirty = true
        if pushing == nil { pushing = Task { await drainPushes() } }
    }

    /// Tells the engine the switch and the file: on every engine start and after each change here. `engine` is the
    /// client of an engine that has not said ready yet (`EngineProcess.prepare`); nil uses the current one.
    /// Returns false when the engine never took the switch's value (only matters while it is on: a new engine starts
    /// with recognition off).
    @discardableResult
    func pushToEngine(to override: STTEngine? = nil) async -> Bool {
        if let override { return await send(to: override) }
        pushDirty = true
        if let running = pushing {
            await running.value
            // the loop that was running may have finished before seeing this request
            if pushDirty { return await pushToEngine() }
            return lastPushOK
        }
        let t = Task { await drainPushes() }
        pushing = t
        await t.value
        return lastPushOK
    }

    private var lastPushOK = true
    /// A push is on its way (tests wait for it).
    var pushBusy: Bool { pushing != nil }

    private func drainPushes() async {
        while pushDirty {
            pushDirty = false
            guard let client = engine() else { break }
            lastPushOK = await send(to: client)
        }
        pushing = nil
    }

    /// One round: sends the current value, again when the answer disagrees (up to `pushAttempts`) and whenever the
    /// switch moved while a send was on its way (the newest value always goes last).
    private func send(to client: STTEngine) async -> Bool {
        guard phase != .unsupported else { return true }
        // a forced reload: sending the same model again while the engine reports it broken makes the engine load it
        // once more (never "off" first: an utterance in that gap would go to the server). The engine answers
        // "loading", which says nothing yet about the model working, so the pause stays until an utterance comes back
        // recognized here
        let reloading = reloadNext
        reloadNext = false
        var attempt = 0
        while attempt < Self.pushAttempts {
            let on = enabled
            let model = wantedModel
            do {
                let status = try await client.setSTT(on: on, model: model)
                engineStatus = status
                if status.broken != true, !reloading { paused = false }
                if enabled != on || wantedModel != model { continue }
                if status.on == on { return true }
                Log.info("stt: engine says on=\(status.on), wanted \(on); sending again")
            } catch {
                Log.info("stt not accepted: \((error as? EngineError)?.kind ?? "other")")
            }
            attempt += 1
            if attempt < Self.pushAttempts { try? await Task.sleep(for: retryDelay) }
        }
        return !enabled
    }

    private var wantedModel: URL? { enabled && installed ? modelURL : nil }

    /// Reads what an utterance's reply says about the engine's model.
    func heard(_ h: Heard) {
        if h.stt == "local" && h.sttBroken != true { paused = false }
        // the switch is on and the model ready, yet the engine sent this one to the server: it lost the setting
        if h.stt == "server", enabled, phase == .ready, installed {
            Log.info("stt: engine recognized on the server while the switch is on; telling it again")
            requestPush()
        }
        guard h.sttBroken == true, enabled, phase == .ready, !paused, checking == nil else { return }
        let url = modelURL, expected = pack.files[0].sha256, hash = hash
        checking = Task { [weak self] in
            let ok = (try? await hash(url)) == expected
            guard let self else { return }
            checking = nil
            guard enabled, phase == .ready else { return }
            if ok {
                // the file is fine: stay local, say the recognition paused, and have the engine load it once more
                paused = true
                Log.info("stt: model broken in the engine but the file is intact; reloading it once")
                reloadNext = true
                requestPush()
            } else {
                Log.info("stt: model file is corrupt, fetching it again")
                try? FileManager.default.removeItem(at: url)
                await pushToEngine()
                startDownload()
            }
        }
    }

    // MARK: download

    private func resume() {
        if searchReady() {
            startDownload()
        } else {
            waitingForSearch = true
            phase = .downloading(done: total - ModelManifest.remaining(pack.files, in: pack.dir), total: total)
        }
    }

    private func startDownload() {
        task?.cancel()
        downloadStarts += 1
        paused = false
        guard isOnline else {
            phase = .waiting
            return
        }
        let total = total
        let began = Date(), resumed = ModelManifest.remaining(pack.files, in: pack.dir) < total
        phase = .downloading(done: total - ModelManifest.remaining(pack.files, in: pack.dir), total: total)
        let pack = pack, fetch = fetch
        task = Task { [weak self] in
            do {
                try FileManager.default.createDirectory(at: pack.dir, withIntermediateDirectories: true)
                try await fetch(pack) { done in
                    Task { @MainActor [weak self] in
                        guard let self, case .downloading = self.phase else { return }
                        self.phase = .downloading(done: done, total: total)
                    }
                }
                guard let self, !Task.isCancelled, enabled else { return }
                phase = .ready
                Log.info("stt: model ready")
                Analytics.capture(.modelDownloaded(resumed: resumed, seconds: Int(Date().timeIntervalSince(began)), pack: .stt))
                await pushToEngine()   // the engine keeps running: it loads the file where it is
            } catch {
                guard let self, !Task.isCancelled else { return }
                Log.info("stt download failed: \(error)")
                if !isOnline || Self.isOffline(error) { phase = .waiting; return }   // the network dropped: carry on when it is back
                phase = .failed((error as? LocalizedError)?.errorDescription ?? String(localized: "받지 못했어요"))
            }
        }
    }

    // MARK: real work

    /// The download itself says the network is gone (the path monitor may not have told us yet). A timeout or a refused
    /// connection happens online too (server down), so those stay failures the user retries.
    private static func isOffline(_ error: Error) -> Bool {
        guard let e = error as? URLError else { return false }
        return [.notConnectedToInternet, .networkConnectionLost, .dataNotAllowed].contains(e.code)
    }

    private static func freeSpace(near dir: URL) -> Int64 {
        var probe = dir
        while !FileManager.default.fileExists(atPath: probe.path), probe.path != "/" { probe = probe.deletingLastPathComponent() }
        let v = try? probe.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
        return v?.volumeAvailableCapacityForImportantUsage ?? -1
    }

    /// Resumes a `.part`, checks the hash, gives the file its name. A wrong hash starts over once.
    private static func fetchPack(_ pack: ModelPack, progress: @escaping @Sendable (Int64) -> Void) async throws {
        for f in pack.files where !ModelManifest.have(f, in: pack.dir) {
            for attempt in 0..<2 {
                let part = pack.dir.appendingPathComponent(f.name + ".part")
                if ModelManifest.size(of: part) < f.size {
                    let have = max(0, ModelManifest.size(of: part))
                    try await FileFetcher.fetch(pack.base.appendingPathComponent(f.name), to: part, from: have, progress: progress)
                }
                let ok = try await Task.detached { try ModelDownload.sha256(of: part) == f.sha256 }.value
                if ok {
                    let final = pack.dir.appendingPathComponent(f.name)
                    try? FileManager.default.removeItem(at: final)
                    try FileManager.default.moveItem(at: part, to: final)
                    break
                }
                try? FileManager.default.removeItem(at: part)
                if attempt == 1 { throw ModelDownload.DownloadError.corrupt(f.name) }
            }
        }
    }
}
