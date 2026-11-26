import XCTest
@testable import Tacie

@MainActor
final class LocalSTTTests: XCTestCase {
    private final class FakeEngine: STTEngine, @unchecked Sendable {
        var calls: [(on: Bool, model: URL?)] = []
        func setSTT(on: Bool, model: URL?) async throws -> STTStatus {
            calls.append((on, model))
            return STTStatus(on: on, ready: model != nil)
        }
    }

    private let gib: UInt64 = 1024 * 1024 * 1024
    private var dir: URL!
    private var defaults: UserDefaults!
    private let file = ModelFile(name: "m.bin", size: 100, sha256: "good")
    private var fetched = 0
    private var restarts = 0   // nothing in LocalSTT may restart the engine; stays 0

    override func setUp() {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defaults = UserDefaults(suiteName: UUID().uuidString)!
        fetched = 0
    }

    override func tearDown() { try? FileManager.default.removeItem(at: dir) }

    private func place() throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try Data(count: 100).write(to: dir.appendingPathComponent("m.bin"))
    }

    private func make(memory: UInt64? = nil, free: Int64 = 100_000_000_000, hash: String = "good",
                      engine: FakeEngine) -> LocalSTT {
        let pack = ModelPack(name: "stt", base: URL(string: "http://x/")!, dir: dir, files: [file])
        return LocalSTT(pack: pack, defaults: defaults, memory: memory ?? 16 * gib, free: { free },
                        fetch: { [unowned self] p, progress in
                            await MainActor.run { self.fetched += 1 }
                            progress(50)
                            try Data(count: 100).write(to: p.dir.appendingPathComponent("m.bin"))
                        },
                        hash: { _ in hash }, engine: { engine })
    }

    private func settle(_ s: LocalSTT) async {
        await s.task?.value
        await s.checking?.value
        try? await Task.sleep(for: .milliseconds(50))
    }

    private func broken() -> Heard {
        Heard(utteranceId: nil, text: "", fired: false, pLookup: nil, skipped: nil, mergedFrom: nil,
              error: nil, errorKind: "model", stt: "local", sttBroken: true)
    }

    func testUnsupportedUnder16GB() {
        let s = make(memory: 8 * gib, engine: FakeEngine())
        XCTAssertEqual(s.phase, .unsupported)
        XCTAssertEqual(s.turnOn(consented: true), .unsupported)
        XCTAssertEqual(s.phase, .unsupported)
        XCTAssertFalse(s.enabled)
        XCTAssertTrue(LocalSTT.supported(memory: 16 * gib))
        XCTAssertFalse(LocalSTT.supported(memory: 16 * gib - 1))
    }

    func testTurnOnWithoutModelAsksConsentThenDownloads() async {
        let s = make(engine: FakeEngine())
        XCTAssertEqual(s.turnOn(consented: false), .askConsent)
        XCTAssertFalse(s.enabled)
        XCTAssertEqual(fetched, 0)
        XCTAssertEqual(s.turnOn(consented: true), .started)
        guard case .downloading = s.phase else { return XCTFail("\(s.phase)") }
        XCTAssertTrue(s.enabled)
        await settle(s)
        XCTAssertEqual(s.phase, .ready)
        XCTAssertEqual(fetched, 1)
    }

    func testUsageNumbersFollowTheSwitchAndTheDownload() async {
        var sent: [AnalyticsEvent] = []
        Analytics.sink = { sent.append($0) }
        defer { Analytics.sink = nil }
        let s = make(engine: FakeEngine())
        s.turnOff()   // already off: nothing to say
        XCTAssertEqual(s.turnOn(consented: true), .started)
        await settle(s)
        s.turnOff()
        s.turnOff()
        XCTAssertEqual(sent.filter { if case .settingChanged = $0 { true } else { false } },
                       [.settingChanged(setting: .localStt, value: .on), .settingChanged(setting: .localStt, value: .off)])
        let downloads = sent.compactMap { e -> AModelPack? in if case .modelDownloaded(_, _, let p) = e { p } else { nil } }
        XCTAssertEqual(downloads, [.stt])
    }

    func testTurnOnWithModelSkipsConsent() async throws {
        try place()
        let e = FakeEngine()
        let s = make(engine: e)
        XCTAssertEqual(s.turnOn(consented: false), .ready)
        XCTAssertEqual(s.phase, .ready)
        XCTAssertEqual(fetched, 0)
        await settle(s)
        XCTAssertEqual(e.calls.count, 1)
        XCTAssertEqual(e.calls[0].on, true)
    }

    func testNeedsSpaceKeepsSwitchOff() {
        let s = make(free: 200_000_000, engine: FakeEngine())
        let r = s.turnOn(consented: true)
        guard case .needsSpace(let more) = r else { return XCTFail("\(r)") }
        XCTAssertEqual(s.phase, .needsSpace(more))
        XCTAssertEqual(more, 100 + ModelManifest.syncRoom - 200_000_000)
        XCTAssertFalse(s.enabled)
        XCTAssertEqual(fetched, 0)
    }

    func testExpensiveNetworkDoesNotWait() {
        // LocalSTT never asks about the network type: a metered path still goes straight to downloading
        let s = make(engine: FakeEngine())
        s.turnOn(consented: true)
        guard case .downloading = s.phase else { return XCTFail("\(s.phase)") }
    }

    func testReadyPushesEngineWithoutRestart() async {
        let e = FakeEngine()
        let s = make(engine: e)
        s.turnOn(consented: true)
        await settle(s)
        XCTAssertEqual(e.calls.count, 1)
        XCTAssertEqual(e.calls[0].on, true)
        XCTAssertEqual(e.calls[0].model, dir.appendingPathComponent("m.bin"))
        XCTAssertEqual(restarts, 0)
    }

    func testBrokenModelWithBadHashRedownloads() async throws {
        try place()
        let e = FakeEngine()
        let s = make(hash: "bad", engine: e)
        s.turnOn(consented: true)
        await settle(s)
        e.calls = []
        s.heard(broken())
        await s.checking?.value
        XCTAssertEqual(e.calls.first?.on, true)
        XCTAssertNil(e.calls.first?.model)
        await settle(s)
        XCTAssertEqual(fetched, 1)
        XCTAssertEqual(s.phase, .ready)   // the fake fetch put the file back
    }

    func testBrokenModelWithGoodHashStaysLocal() async throws {
        try place()
        let e = FakeEngine()
        let s = make(engine: e)
        s.turnOn(consented: true)
        await settle(s)
        e.calls = []
        s.heard(broken())
        await s.checking?.value
        XCTAssertEqual(s.phase, .ready)
        XCTAssertTrue(s.paused)
        XCTAssertEqual(fetched, 0)
        XCTAssertTrue(FileManager.default.fileExists(atPath: dir.appendingPathComponent("m.bin").path))
        // the same file, loaded once more (on again, never off): never the server, never a download
        await settle(s)
        XCTAssertEqual(e.calls.map(\.on), [true])
        XCTAssertEqual(e.calls.last?.model, dir.appendingPathComponent("m.bin"))
        XCTAssertTrue(s.paused)
    }

    func testTurnOffTellsEngineAndKeepsFile() async throws {
        try place()
        let e = FakeEngine()
        let s = make(engine: e)
        s.turnOn(consented: true)
        await settle(s)
        e.calls = []
        s.turnOff()
        await settle(s)
        XCTAssertEqual(s.phase, .off)
        XCTAssertFalse(s.enabled)
        XCTAssertEqual(e.calls.last?.on, false)
        XCTAssertTrue(FileManager.default.fileExists(atPath: dir.appendingPathComponent("m.bin").path))
    }

    func testDeleteModelRemovesFileAndTurnsOff() async throws {
        try place()
        let e = FakeEngine()
        let s = make(engine: e)
        s.turnOn(consented: true)
        await settle(s)
        e.calls = []
        s.deleteModel()
        await settle(s)
        XCTAssertFalse(FileManager.default.fileExists(atPath: dir.appendingPathComponent("m.bin").path))
        XCTAssertEqual(e.calls.last?.on, false)
        XCTAssertNil(e.calls.last?.model)
        XCTAssertFalse(s.enabled)
        XCTAssertEqual(s.phase, .off)
    }

    /// Not offered (hidden for now): a switch turned on earlier, with the model on disk, neither loads it nor fetches.
    func testNotOfferedIgnoresAnEarlierSwitch() async {
        defaults.set(true, forKey: LocalSTT.key)
        try? Data(count: 100).write(to: dir.appendingPathComponent("m.bin"))
        let engine = FakeEngine()
        let pack = ModelPack(name: "stt", base: URL(string: "http://x/")!, dir: dir, files: [file])
        let s = LocalSTT(pack: pack, defaults: defaults, memory: 16 * gib, free: { 100_000_000_000 },
                         fetch: { [unowned self] _, _ in await MainActor.run { self.fetched += 1 } },
                         offered: false, engine: { engine })
        XCTAssertEqual(s.phase, .unsupported)
        XCTAssertEqual(s.turnOn(consented: true), .unsupported)
        _ = await s.pushToEngine()
        XCTAssertTrue(engine.calls.isEmpty)
        XCTAssertEqual(fetched, 0)
    }

    func testOnboardingWaitsForTheSearchModel() async {
        var searchDone = false
        let pack = ModelPack(name: "stt", base: URL(string: "http://x/")!, dir: dir, files: [file])
        let s = LocalSTT(pack: pack, defaults: defaults, memory: 16 * gib, free: { 100_000_000_000 },
                         fetch: { [unowned self] p, _ in
                             await MainActor.run { self.fetched += 1 }
                             try Data(count: 100).write(to: p.dir.appendingPathComponent("m.bin"))
                         }, searchReady: { searchDone }, engine: { nil })
        s.turnOn(consented: true, afterSearchModel: true)
        XCTAssertEqual(fetched, 0)
        searchDone = true
        s.searchModelReady()
        await settle(s)
        XCTAssertEqual(fetched, 1)
        XCTAssertEqual(s.phase, .ready)
    }

    // MARK: review round

    private var onlineNow = true
    private var failNext = false
    private var failError: Error = URLError(.notConnectedToInternet)
    private var gateOpen = true

    private func makeNet(searchReady: Bool = true, engine: FakeEngine, enabledAtLaunch: Bool = false) -> LocalSTT {
        if enabledAtLaunch { defaults.set(true, forKey: LocalSTT.key) }
        let pack = ModelPack(name: "stt", base: URL(string: "http://x/")!, dir: dir, files: [file])
        return LocalSTT(pack: pack, defaults: defaults, memory: 16 * gib, free: { 100_000_000_000 },
                        fetch: { [unowned self] p, _ in
                            await MainActor.run { self.fetched += 1 }
                            while await MainActor.run(body: { !self.gateOpen }) { try? await Task.sleep(for: .milliseconds(5)) }
                            if await MainActor.run(body: { self.failNext }) {
                                await MainActor.run { self.failNext = false }
                                throw await MainActor.run { self.failError }
                            }
                            try Data(count: 100).write(to: p.dir.appendingPathComponent("m.bin"))
                        },
                        hash: { _ in "bad" }, searchReady: { searchReady }, online: { [unowned self] in self.onlineNow },
                        engine: { engine })
    }

    func testOfflineTurnOnWaitsThenStartsWhenOnline() async {
        onlineNow = false
        let s = makeNet(engine: FakeEngine())
        s.turnOn(consented: true)
        await settle(s)
        XCTAssertEqual(s.phase, .waiting)
        XCTAssertEqual(fetched, 0)
        s.pathChanged(satisfied: false)
        XCTAssertEqual(fetched, 0)
        onlineNow = true
        s.pathChanged(satisfied: true)
        await settle(s)
        XCTAssertEqual(fetched, 1)
        XCTAssertEqual(s.phase, .ready)
    }

    func testNetworkDropMidDownloadResumesOnReconnect() async {
        let s = makeNet(engine: FakeEngine())
        gateOpen = false
        failNext = true
        s.turnOn(consented: true)   // online: the fetch starts and holds
        try? await Task.sleep(for: .milliseconds(30))
        onlineNow = false           // the network drops inside the fetch
        gateOpen = true
        await settle(s)
        XCTAssertEqual(s.phase, .waiting)
        XCTAssertFalse({ if case .failed = s.phase { return true } else { return false } }())
        onlineNow = true
        s.pathChanged(satisfied: true)
        await settle(s)
        XCTAssertEqual(s.phase, .ready)
        XCTAssertEqual(fetched, 2)
    }

    func testOfflineErrorWaitsEvenWhenThePathStillSaysOnline() async {
        let s = makeNet(engine: FakeEngine())
        failNext = true   // throws URLError(.notConnectedToInternet) while onlineNow stays true
        s.turnOn(consented: true)
        await settle(s)
        XCTAssertEqual(s.phase, .waiting)
        XCTAssertEqual(fetched, 1)
        s.pathChanged(satisfied: true)
        await settle(s)
        XCTAssertEqual(fetched, 2)
        XCTAssertEqual(s.phase, .ready)
    }

    func testTimeoutWhileOnlineFails() async {
        let s = makeNet(engine: FakeEngine())
        failError = URLError(.timedOut)
        failNext = true
        s.turnOn(consented: true)
        await settle(s)
        guard case .failed = s.phase else { return XCTFail("\(s.phase)") }
    }

    func testHTTPErrorFails() async {
        let s = makeNet(engine: FakeEngine())
        failError = ModelDownload.DownloadError.http(503)
        failNext = true
        s.turnOn(consented: true)
        await settle(s)
        guard case .failed = s.phase else { return XCTFail("\(s.phase)") }
    }

    func testCorruptDownloadFails() async {
        let s = makeNet(engine: FakeEngine())
        failError = ModelDownload.DownloadError.corrupt("m.bin")
        failNext = true
        s.turnOn(consented: true)
        await settle(s)
        guard case .failed = s.phase else { return XCTFail("\(s.phase)") }
    }

    func testPausedChecksOnceAndClears() async throws {
        try place()
        let e = FakeEngine()
        var hashes = 0
        let pack = ModelPack(name: "stt", base: URL(string: "http://x/")!, dir: dir, files: [file])
        let s = LocalSTT(pack: pack, defaults: defaults, memory: 16 * gib, free: { 100_000_000_000 },
                         fetch: { _, _ in }, hash: { _ in hashes += 1; return "good" }, engine: { e })
        s.turnOn(consented: true)
        await settle(s)
        s.heard(broken())
        await s.checking?.value
        XCTAssertTrue(s.paused)
        XCTAssertEqual(hashes, 1)
        s.heard(broken())
        XCTAssertNil(s.checking)
        XCTAssertEqual(hashes, 1)
        // a later healthy local reply clears it
        let ok = Heard(utteranceId: "u", text: "x", fired: false, pLookup: nil, skipped: nil, mergedFrom: nil,
                       error: nil, errorKind: nil, stt: "local", sttBroken: nil)
        s.heard(ok)
        XCTAssertFalse(s.paused)
        // a push whose status is not broken clears it too
        s.heard(broken())
        await s.checking?.value
        XCTAssertTrue(s.paused)
        await s.pushToEngine()
        XCTAssertFalse(s.paused)
    }

    func testSwitchOnWithMissingFileAndSearchNotReadyIsNotOff() {
        let s = makeNet(searchReady: false, engine: FakeEngine(), enabledAtLaunch: true)
        guard case .downloading = s.phase else { return XCTFail("\(s.phase)") }
        XCTAssertEqual(fetched, 0)
    }

    func testTurnOffMidDownloadNeverPostsOn() async {
        let e = FakeEngine()
        let s = makeNet(engine: e)
        gateOpen = false
        s.turnOn(consented: true)
        try? await Task.sleep(for: .milliseconds(30))
        s.turnOff()
        gateOpen = true
        await settle(s)
        XCTAssertFalse(e.calls.contains { $0.on })
        XCTAssertEqual(s.phase, .off)
    }

    func testManifestValues() {
        let p = ModelManifest.sttPack(env: [:], home: URL(fileURLWithPath: "/Users/me"))
        XCTAssertEqual(p.dir.path, "/Users/me/Library/Application Support/Tacie/models/whisper-turbo-q5")
        XCTAssertEqual(p.files[0].size, 574_041_195)
        XCTAssertFalse(ModelManifest.packs(env: [:], home: URL(fileURLWithPath: "/Users/me")).contains { $0.name == "stt" })
    }

    // MARK: final review

    /// An engine that answers slowly, can forget the switch, or refuse.
    private final class SlowEngine: STTEngine, @unchecked Sendable {
        var calls: [(on: Bool, model: URL?)] = []
        /// How long each call takes, in order (missing: none).
        var delays: [Duration] = []
        /// The engine's own state; `ignore` calls answer without changing it.
        var on = false
        var ignore = 0
        var fail = 0
        var inFlight = 0
        var maxInFlight = 0
        func setSTT(on: Bool, model: URL?) async throws -> STTStatus {
            calls.append((on, model))
            inFlight += 1
            maxInFlight = max(maxInFlight, inFlight)
            defer { inFlight -= 1 }
            if !delays.isEmpty { try? await Task.sleep(for: delays.removeFirst()) }
            if fail > 0 { fail -= 1; throw EngineError.http(503, "x") }
            if ignore > 0 { ignore -= 1 } else { self.on = on }
            return STTStatus(on: self.on, ready: self.on && model != nil, loading: self.on && model != nil ? true : nil)
        }
    }

    private func makeWith(_ e: SlowEngine, hash: String = "good") -> LocalSTT {
        let pack = ModelPack(name: "stt", base: URL(string: "http://x/")!, dir: dir, files: [file])
        let s = LocalSTT(pack: pack, defaults: defaults, memory: 16 * gib, free: { 100_000_000_000 },
                         fetch: { _, _ in }, hash: { _ in hash }, engine: { e })
        s.retryDelay = .milliseconds(1)
        return s
    }

    private func drain(_ s: LocalSTT) async {
        for _ in 0..<200 {
            try? await Task.sleep(for: .milliseconds(5))
            if !s.pushBusy { return }
        }
        XCTFail("pushes never settled")
    }

    func testQuickOffOnLandsOnInOrder() async throws {
        try place()
        let e = SlowEngine()
        e.delays = [.milliseconds(80), .milliseconds(1), .milliseconds(1)]
        let s = makeWith(e)
        s.turnOn(consented: false)   // slow on
        s.turnOff()                  // both arrive while the first is on its way
        s.turnOn(consented: false)
        await drain(s)
        XCTAssertEqual(e.maxInFlight, 1)
        XCTAssertEqual(e.calls.last?.on, true)
        XCTAssertTrue(e.on)
    }

    func testMismatchedAnswerIsSentAgain() async throws {
        try place()
        let e = SlowEngine()
        e.ignore = 1   // the first answer still says off
        let s = makeWith(e)
        s.turnOn(consented: false)
        await drain(s)
        XCTAssertEqual(e.calls.count, 2)
        XCTAssertTrue(e.on)
        XCTAssertEqual(s.engineStatus?.on, true)
    }

    func testPrepareFailsWhenTheEngineNeverTakesTheSwitch() async throws {
        try place()
        defaults.set(true, forKey: LocalSTT.key)
        let e = SlowEngine()
        let s = makeWith(e)
        e.fail = LocalSTT.pushAttempts
        let ok = await s.pushToEngine(to: e)
        XCTAssertFalse(ok)
        XCTAssertEqual(e.calls.count, LocalSTT.pushAttempts)
        // one refusal then success: ready
        e.calls = []
        e.fail = 1
        let ok2 = await s.pushToEngine(to: e)
        XCTAssertTrue(ok2)
        XCTAssertEqual(e.calls.count, 2)
        // with the switch off a refusal does not hold the engine back (it starts off)
        s.turnOff()
        await drain(s)
        e.fail = LocalSTT.pushAttempts
        let ok3 = await s.pushToEngine(to: e)
        XCTAssertTrue(ok3)
    }

    func testServerUtteranceWhileOnTellsTheEngineAgain() async throws {
        try place()
        let e = SlowEngine()
        let s = makeWith(e)
        s.turnOn(consented: false)
        await drain(s)
        e.calls = []
        e.on = false   // a lost setting
        s.heard(Heard(utteranceId: "u", text: "x", fired: false, pLookup: nil, skipped: nil, mergedFrom: nil,
                      error: nil, errorKind: nil, stt: "server", sttBroken: nil))
        await drain(s)
        XCTAssertEqual(e.calls.first?.on, true)
        XCTAssertTrue(e.on)
    }

    func testPauseForcesOneReloadAndKeepsTheLine() async throws {
        try place()
        let e = SlowEngine()
        let s = makeWith(e)
        s.turnOn(consented: false)
        await drain(s)
        e.calls = []
        s.heard(broken())
        await s.checking?.value
        await drain(s)
        XCTAssertTrue(s.paused)   // the reload answers "loading": nothing proves the model works yet
        XCTAssertEqual(e.calls.map(\.on), [true])
        XCTAssertNotNil(e.calls[0].model)
        // broken again: no second reload while paused
        e.calls = []
        s.heard(broken())
        await drain(s)
        XCTAssertTrue(e.calls.allSatisfy { $0.on })
        XCTAssertFalse(e.calls.contains { !$0.on })
    }

    func testNoRoomIsSaidBeforeConsent() {
        let s = make(free: 200_000_000, engine: FakeEngine())
        let r = s.turnOn(consented: false)
        guard case .needsSpace = r else { return XCTFail("\(r)") }
        XCTAssertFalse(s.enabled)
        XCTAssertEqual(fetched, 0)
    }
}
