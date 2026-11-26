import XCTest
@testable import Tacie

final class ModelDownloadTests: XCTestCase {
    func testAsksOnlyForAnExpensiveNetworkOrTooLittleSpace() {
        let need = 2_300_000_000 as Int64
        XCTAssertEqual(ModelGate.check(expensive: false, constrained: false, online: true, free: 10_000_000_000, remaining: need), .go)
        XCTAssertEqual(ModelGate.check(expensive: true, constrained: false, online: true, free: 10_000_000_000, remaining: need), .expensive)
        XCTAssertEqual(ModelGate.check(expensive: false, constrained: true, online: true, free: 10_000_000_000, remaining: need), .expensive)
        XCTAssertEqual(ModelGate.check(expensive: false, constrained: false, online: false, free: 10_000_000_000, remaining: need), .offline)
        // space comes first: no point asking about the network for a download that cannot fit
        XCTAssertEqual(ModelGate.check(expensive: true, constrained: false, online: true, free: 1_000_000_000, remaining: need),
                       .space(need + ModelManifest.syncRoom - 1_000_000_000))
        // an unknown free space does not block
        XCTAssertEqual(ModelGate.check(expensive: false, constrained: false, online: true, free: -1, remaining: need), .go)
    }

    func testTheModelDirFollowsTheEngine() {
        let home = URL(fileURLWithPath: "/Users/me")
        XCTAssertEqual(ModelManifest.dir(env: [:], home: home).path, "/Users/me/Library/Application Support/Tacie/models/bge-m3-onnx")
        XCTAssertEqual(ModelManifest.dir(env: ["TACIE_MODEL_DIR": "/tmp/m"], home: home).path, "/tmp/m")
    }

    func testRemainingCountsPartsAndFinishedFiles() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let files = ModelManifest.source(env: ["TACIE_MODEL_URL": "http://x/"]).files
        XCTAssertEqual(files.map(\.name), ["model.onnx", "tokenizer.json", "model.onnx_data.f16"])
        XCTAssertEqual(ModelManifest.remaining(files, in: dir), ModelManifest.total(files))
        // unpacking writes the full weights beside the half-size file
        XCTAssertEqual(ModelManifest.unpackRoom(files, in: dir), ModelManifest.weights.size)
        let small = files[0]
        try Data(count: Int(small.size)).write(to: dir.appendingPathComponent(small.name))
        try Data(count: 1000).write(to: dir.appendingPathComponent(files[1].name + ".part"))
        XCTAssertEqual(ModelManifest.remaining(files, in: dir), ModelManifest.total(files) - small.size - 1000)
        XCTAssertFalse(ModelManifest.installed(files, in: dir))
    }

    func testUnpackedWeightsCountForTheHalfSizeFile() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        // a sparse file of the full weights' size: the engine unpacked and removed the .f16
        FileManager.default.createFile(atPath: dir.appendingPathComponent("model.onnx_data").path, contents: nil)
        let h = try FileHandle(forWritingTo: dir.appendingPathComponent("model.onnx_data"))
        try h.truncate(atOffset: UInt64(ModelManifest.weights.size))
        try h.close()
        XCTAssertTrue(ModelManifest.have(ModelManifest.packed, in: dir))
        XCTAssertEqual(ModelManifest.unpackRoom([ModelManifest.packed], in: dir), 0)
    }

    func testHashesAFile() throws {
        let f = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try Data("abc".utf8).write(to: f)
        defer { try? FileManager.default.removeItem(at: f) }
        XCTAssertEqual(try ModelDownload.sha256(of: f), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    }

    // MARK: packs (voice + search)

    private func sparse(_ f: ModelFile, in dir: URL) throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent(f.name)
        FileManager.default.createFile(atPath: url.path, contents: nil)
        let h = try FileHandle(forWritingTo: url)
        try h.truncate(atOffset: UInt64(f.size))
        try h.close()
    }

    private func tempEnv() -> (root: URL, env: [String: String]) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        return (root, ["TACIE_MODEL_DIR": root.appendingPathComponent("search").path,
                       "TACIE_VOICE_MODEL": root.appendingPathComponent("voice/model.onnx").path,
                       "TACIE_DIAR_MODEL": root.appendingPathComponent("diar/model.onnx").path])
    }

    private func pack(_ name: String, _ packs: [ModelPack]) -> ModelPack { packs.first { $0.name == name }! }

    func testPacksAreVoiceThenDiarThenSearch() {
        let home = URL(fileURLWithPath: "/Users/me")
        let packs = ModelManifest.packs(env: [:], home: home)
        XCTAssertEqual(packs.map(\.name), ["voice", "diar", "search"])
        XCTAssertTrue(packs[0].dir.path.hasSuffix("models/eres2net-speaker"))
        XCTAssertEqual(packs[0].files, [ModelManifest.voice])
        XCTAssertEqual(packs[0].base.absoluteString, ModelManifest.voiceBase)
        XCTAssertTrue(packs[1].dir.path.hasSuffix("models/campp-ft-speaker"))
        XCTAssertEqual(packs[1].files, [ModelManifest.diar])
        XCTAssertEqual(packs[1].base.absoluteString, ModelManifest.diarBase)
        XCTAssertEqual(packs[2].dir, ModelManifest.dir(env: [:], home: home))
    }

    func testInstalledNeedsEveryPack() throws {
        let (root, env) = tempEnv()
        defer { try? FileManager.default.removeItem(at: root) }
        let home = URL(fileURLWithPath: "/Users/me")
        let packs = ModelManifest.packs(env: env, home: home)
        let search = pack("search", packs)
        for f in search.files { try sparse(f, in: search.dir) }
        XCTAssertTrue(ModelManifest.searchInstalled(env: env, home: home))
        XCTAssertFalse(ModelManifest.installed(packs))
        try sparse(ModelManifest.voice, in: pack("voice", packs).dir)
        XCTAssertFalse(ModelManifest.installed(packs))  // an existing user: ERes2Net and search, not the new model
        try sparse(ModelManifest.diar, in: pack("diar", packs).dir)
        XCTAssertTrue(ModelManifest.installed(packs))
    }

    @MainActor
    func testOnlyVoiceWhenSearchIsThereAndAVoiceModelIsNot() throws {
        let (root, env) = tempEnv()
        defer { try? FileManager.default.removeItem(at: root) }
        let home = URL(fileURLWithPath: "/Users/me")
        XCTAssertFalse(ModelDownload(env: env, home: home).onlyVoice)  // nothing there: the whole download
        let packs = ModelManifest.packs(env: env, home: home)
        for f in pack("search", packs).files { try sparse(f, in: root.appendingPathComponent("search")) }
        XCTAssertTrue(ModelDownload(env: env, home: home).onlyVoice)
        // an existing user has ERes2Net: only the speaker-grouping model is left, still a voice download
        try sparse(ModelManifest.voice, in: pack("voice", packs).dir)
        let d = ModelDownload(env: env, home: home)
        XCTAssertTrue(d.onlyVoice)
        XCTAssertEqual(d.voiceTotal, 39_593_761 + 30_220_086)
    }

    func testRemainingCountsOnlyMissingVoice() throws {
        let (root, env) = tempEnv()
        defer { try? FileManager.default.removeItem(at: root) }
        let home = URL(fileURLWithPath: "/Users/me")
        let packs = ModelManifest.packs(env: env, home: home)
        XCTAssertEqual(ModelManifest.remaining(packs), ModelManifest.total(packs))
        let search = pack("search", packs)
        for f in search.files { try sparse(f, in: search.dir) }
        XCTAssertEqual(ModelManifest.remaining(packs), 39_593_761 + 30_220_086)
        try sparse(ModelManifest.voice, in: pack("voice", packs).dir)
        XCTAssertEqual(ModelManifest.remaining(packs), 30_220_086)
        XCTAssertEqual(ModelManifest.total(packs), 39_593_761 + 30_220_086 + ModelManifest.total(search.files))
    }

    func testVoiceDirFollowsEnv() {
        let home = URL(fileURLWithPath: "/Users/me")
        let packs = ModelManifest.packs(env: ["TACIE_VOICE_MODEL": "/x/y/model.onnx", "TACIE_DIAR_MODEL": "/x/z/model.onnx"],
                                        home: home)
        XCTAssertEqual(packs[0].dir.path, "/x/y")
        XCTAssertEqual(packs[1].dir.path, "/x/z")
    }

    func testVoiceRowDownloading() {
        let r = ModelDownload.voiceRow(state: .downloading, done: 24_000_000, total: 39_593_761)
        XCTAssertEqual(r?.title, "보이스 모델 받는 중")
        XCTAssertEqual(r?.percent, "60%")
        XCTAssertEqual(r?.fraction ?? 0, 0.606, accuracy: 0.001)
        XCTAssertTrue(r?.detail.hasPrefix("24MB / 40MB · ") ?? false)
        XCTAssertEqual(r?.failed, false)
        XCTAssertEqual(ModelDownload.voiceRow(state: .verifying, done: 1, total: 2)?.percent, "100%")
    }

    func testVoiceRowFailed() {
        let r = ModelDownload.voiceRow(state: .failed("서버가 503로 답했어요"), done: 0, total: 39_593_761)
        XCTAssertEqual(r?.title, "보이스 모델을 받지 못했어요")
        XCTAssertEqual(r?.detail, "서버가 503로 답했어요")
        XCTAssertEqual(r?.failed, true)
    }

    func testVoiceRowHiddenWhileWaiting() {
        for s: ModelDownload.State in [.waiting(.offline), .waiting(.expensive), .later, .idle, .ready] {
            XCTAssertNil(ModelDownload.voiceRow(state: s, done: 0, total: 39_593_761))
        }
    }

    /// With the search model in place `done` starts near the whole total; the row still counts the voice pack alone.
    func testVoiceRowCountsOnlyTheVoicePack() {
        let voice = ModelManifest.voice.size, total = voice + 2_266_000_000 as Int64
        func row(_ done: Int64) -> VoiceRow? {
            ModelDownload.voiceRow(state: .downloading, done: ModelDownload.voicePart(done: done, total: total, voiceTotal: voice), total: voice)
        }
        let start = row(total - voice)
        XCTAssertEqual(start?.percent, "0%")
        XCTAssertTrue(start?.detail.hasPrefix("0MB / 40MB · ") ?? false)
        let mid = row(total - voice + 24_000_000)
        XCTAssertEqual(mid?.percent, "60%")
        XCTAssertTrue(mid?.detail.hasPrefix("24MB / 40MB · ") ?? false)
        XCTAssertEqual(ModelDownload.voicePart(done: total + 5, total: total, voiceTotal: voice), voice)
        XCTAssertEqual(ModelDownload.voicePart(done: 0, total: total, voiceTotal: voice), 0)
    }
}
