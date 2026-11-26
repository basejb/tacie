import CryptoKit
import Foundation
import Network
import Observation

/// One file of the search model, as the engine needs it (`cli::find_model`: the graph, the weights, the tokenizer).
struct ModelFile: Equatable {
    let name: String
    let size: Int64
    let sha256: String
}

/// One download unit: files that come from one base into one folder. The app fetches the voice pack and the search
/// pack one after the other.
struct ModelPack: Equatable {
    let name: String
    let base: URL
    let dir: URL
    let files: [ModelFile]
}

/// The search model this Mac runs: the official BAAI/bge-m3 ONNX export (docs/11 4-2절, 13절).
///
/// From our R2 bucket it comes half-size: the weights as fp16 (`model.onnx_data.f16`, made by eval/pack_model.py),
/// which the engine unpacks on its next start into the official fp32 file, byte for byte (docs/01 "bge-m3 가중치
/// 반으로 받기"). Until the bucket's address is set (`hosted`) it comes full-size from Hugging Face. Sizes and hashes
/// are the files themselves (measured 2026-10-26).
enum ModelManifest {
    /// Our R2 bucket at its own domain (the r2.dev address older apps use stays on), ending in "/". nil until it exists: then the original on Hugging Face.
    static let hosted: String? = BuildValues.modelsBase.absoluteString + "/bge-m3/v1/"
    static let original = "https://huggingface.co/BAAI/bge-m3/resolve/main/onnx/"

    static let graph = ModelFile(name: "model.onnx", size: 724_923,
                                 sha256: "f84251230831afb359ab26d9fd37d5936d4d9bb5d1d5410e66442f630f24435b")
    static let tokenizer = ModelFile(name: "tokenizer.json", size: 17_082_821,
                                     sha256: "6710678b12670bc442b99edc952c4d996ae309a7020c1fa0096dd245c2faf790")
    /// The official weights, and the same weights half-size (what the engine unpacks).
    static let weights = ModelFile(name: "model.onnx_data", size: 2_266_820_608,
                                   sha256: "1eebfb28493f67bba03ce0ef64bfdc7fc5a3bd9d7493f818bb1d78cd798416b4")
    static let packed = ModelFile(name: "model.onnx_data.f16", size: 1_133_410_304,
                                  sha256: "a9cd7dae2eff7de3cc072043469bd0ffc20db0ac9f2bd511fc01423751255cbb")

    /// Where the files come from and which they are: `TACIE_MODEL_URL` (development: a local copy of the bucket),
    /// else the bucket, else the original. Small files first: a stopped download leaves the big one for last.
    static func source(env: [String: String]) -> (base: URL, files: [ModelFile]) {
        if let our = env["TACIE_MODEL_URL"] ?? hosted { return (URL(string: our)!, [graph, tokenizer, packed]) }
        return (URL(string: original)!, [graph, tokenizer, weights])
    }

    /// The speaker-voice model (ERes2Net, one file), always from our bucket. The engine reads it from `TACIE_VOICE_MODEL`
    /// or its default path, which is this pack's folder.
    static let voice = ModelFile(name: "model.onnx", size: 39_593_761,
                                 sha256: "1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b")
    static let voiceBase = BuildValues.modelsBase.absoluteString + "/eres2net-speaker/v1/"

    /// The speaker-grouping model (CAM++ fine-tuned on AMI and Zeroth-Korean, one file, docs/01 "화자 모델 파인튜닝"). With it
    /// the engine groups a meeting's speakers by this model and keeps ERes2Net for knowing remembered people; without it
    /// ERes2Net does both, as before. The engine reads it from `TACIE_DIAR_MODEL` or its default path, this pack's folder.
    static let diar = ModelFile(name: "model.onnx", size: 30_220_086,
                                sha256: "b0402d6d1fbce51febf36fe4671d5530a72f5fec1c2751aeb371dd53a2677482")
    static let diarBase = BuildValues.modelsBase.absoluteString + "/campp-ft-speaker/v1/"

    /// Where the engine looks for the speaker-grouping model: the folder of `TACIE_DIAR_MODEL`, else Application Support.
    static func diarDir(env: [String: String], home: URL) -> URL {
        if let m = env["TACIE_DIAR_MODEL"], !m.isEmpty { return URL(fileURLWithPath: m).deletingLastPathComponent() }
        return home.appendingPathComponent("Library/Application Support/Tacie/models/campp-ft-speaker")
    }

    /// Where the engine looks for the voice model: the folder of `TACIE_VOICE_MODEL`, else Application Support.
    static func voiceDir(env: [String: String], home: URL) -> URL {
        if let m = env["TACIE_VOICE_MODEL"], !m.isEmpty { return URL(fileURLWithPath: m).deletingLastPathComponent() }
        return home.appendingPathComponent("Library/Application Support/Tacie/models/eres2net-speaker")
    }

    /// Voice first (small), then the speaker-grouping model (small), then search.
    static func packs(env: [String: String], home: URL) -> [ModelPack] {
        let s = source(env: env)
        return [ModelPack(name: "voice", base: URL(string: voiceBase)!, dir: voiceDir(env: env, home: home), files: [voice]),
                ModelPack(name: "diar", base: URL(string: diarBase)!, dir: diarDir(env: env, home: home), files: [diar]),
                ModelPack(name: "search", base: s.base, dir: dir(env: env, home: home), files: s.files)]
    }

    /// The on-device speech model (whisper large-v3-turbo, q5_0), docs/22. Not in `packs`: it never starts by itself,
    /// only `LocalSTT` fetches it, after the user said yes.
    static let sttFile = ModelFile(name: "ggml-large-v3-turbo-q5_0.bin", size: 574_041_195,
                                   sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2")
    static let sttBase = BuildValues.modelsBase.absoluteString + "/whisper-turbo-q5/v1/"

    /// `TACIE_STT_MODEL_DIR` (development), else Application Support next to the other models.
    static func sttDir(env: [String: String], home: URL) -> URL {
        if let d = env["TACIE_STT_MODEL_DIR"], !d.isEmpty { return URL(fileURLWithPath: d) }
        return home.appendingPathComponent("Library/Application Support/Tacie/models/whisper-turbo-q5")
    }

    static func sttPack(env: [String: String], home: URL) -> ModelPack {
        ModelPack(name: "stt", base: URL(string: env["TACIE_STT_MODEL_URL"] ?? sttBase)!,
                  dir: sttDir(env: env, home: home), files: [sttFile])
    }

    static func searchInstalled(env: [String: String], home: URL) -> Bool {
        packs(env: env, home: home).first { $0.name == "search" }.map { installed($0.files, in: $0.dir) } ?? false
    }

    static func total(_ files: [ModelFile]) -> Int64 { files.reduce(0) { $0 + $1.size } }
    static func total(_ packs: [ModelPack]) -> Int64 { packs.reduce(0) { $0 + total($1.files) } }

    /// Room kept for the first sync's search data on top of the model.
    static let syncRoom: Int64 = 500_000_000

    /// Where the engine looks (`cli::model_dir`): `TACIE_MODEL_DIR`, else Application Support.
    static func dir(env: [String: String], home: URL) -> URL {
        if let d = env["TACIE_MODEL_DIR"], !d.isEmpty { return URL(fileURLWithPath: d) }
        return home.appendingPathComponent("Library/Application Support/Tacie/models/bge-m3-onnx")
    }

    /// The file is in place (its hash was checked before it got its name). The half-size weights also count as there
    /// once the engine has unpacked them (it removes the .f16).
    static func have(_ f: ModelFile, in dir: URL) -> Bool {
        if size(of: dir.appendingPathComponent(f.name)) == f.size { return true }
        return f == packed && size(of: dir.appendingPathComponent(weights.name)) == weights.size
    }

    static func installed(_ files: [ModelFile], in dir: URL) -> Bool { files.allSatisfy { have($0, in: dir) } }

    static func installed(_ packs: [ModelPack]) -> Bool { packs.allSatisfy { installed($0.files, in: $0.dir) } }

    /// Bytes still to fetch, counting what the partial files already hold.
    static func remaining(_ files: [ModelFile], in dir: URL) -> Int64 {
        files.reduce(0) { sum, f in
            if have(f, in: dir) { return sum }
            return sum + f.size - min(f.size, max(0, size(of: dir.appendingPathComponent(f.name + ".part"))))
        }
    }

    static func remaining(_ packs: [ModelPack]) -> Int64 { packs.reduce(0) { $0 + remaining($1.files, in: $1.dir) } }

    /// Disk the engine needs on top of the download: unpacking writes the full weights next to the half-size file.
    static func unpackRoom(_ files: [ModelFile], in dir: URL) -> Int64 {
        files.contains(packed) && size(of: dir.appendingPathComponent(weights.name)) != weights.size ? weights.size : 0
    }

    static func unpackRoom(_ packs: [ModelPack]) -> Int64 { packs.reduce(0) { $0 + unpackRoom($1.files, in: $1.dir) } }

    static func size(of url: URL) -> Int64 {
        ((try? FileManager.default.attributesOfItem(atPath: url.path))?[.size] as? NSNumber)?.int64Value ?? -1
    }
}

/// Whether to start without asking (docs/11 4-2절): only an expensive or saving network, or too little disk, asks.
enum ModelGate: Equatable {
    case go
    case expensive          // a phone hotspot or Low Data Mode: ask before the download
    case space(Int64)       // this many more bytes are needed
    case offline

    static func check(expensive: Bool, constrained: Bool, online: Bool, free: Int64, remaining: Int64) -> ModelGate {
        let need = remaining + ModelManifest.syncRoom
        if free >= 0 && free < need { return .space(need - free) }
        if !online { return .offline }
        if expensive || constrained { return .expensive }
        return .go
    }
}

/// The sidebar's line for the voice model's top-up download (the search model is already there).
struct VoiceRow: Equatable {
    let title: String
    let percent: String?
    let fraction: Double?
    let detail: String
    let failed: Bool
}

/// Downloads the voice model and the search model in the background, resumes across stops and launches, checks each
/// file's hash, and calls `onReady` once every file is in place (the app then starts the engine again so it loads the model).
@Observable
@MainActor
final class ModelDownload {
    enum State: Equatable {
        case idle, ready
        case waiting(ModelGate)   // asked, or waiting for space or a connection
        case later                // 와이파이에서 받기: starts when the network is no longer expensive
        case downloading
        case verifying
        case failed(String)
    }

    private(set) var state = State.idle {
        didSet { onlyVoice = Self.onlyVoice(packs) }
    }
    /// Bytes on disk out of `total`, while downloading.
    private(set) var done: Int64 = 0
    let total: Int64
    var onReady: () -> Void = {}

    private let packs: [ModelPack]

    /// The voice packs alone (ERes2Net and the speaker-grouping model; the sidebar row counts these, not the search
    /// model's gigabytes).
    var voiceTotal: Int64 { ModelManifest.total(packs.filter { Self.voicePacks.contains($0.name) }) }
    nonisolated static let voicePacks: Set<String> = ["voice", "diar"]
    /// `done` is counted over every pack with the voice pack first; the voice part is what lies past the other packs' bytes.
    var voiceDone: Int64 { Self.voicePart(done: done, total: total, voiceTotal: voiceTotal) }

    nonisolated static func voicePart(done: Int64, total: Int64, voiceTotal: Int64) -> Int64 {
        min(voiceTotal, max(0, done - (total - voiceTotal)))
    }
    private let monitor = NWPathMonitor()
    private var path: NWPath?
    private var task: Task<Void, Never>?

    init(env: [String: String] = ProcessInfo.processInfo.environment,
         home: URL = FileManager.default.homeDirectoryForCurrentUser) {
        packs = ModelManifest.packs(env: env, home: home)
        total = ModelManifest.total(packs)
        if ModelManifest.installed(packs) { state = .ready }
        onlyVoice = Self.onlyVoice(packs)
        monitor.pathUpdateHandler = { [weak self] p in
            Task { @MainActor in self?.pathChanged(p) }
        }
        monitor.start(queue: DispatchQueue(label: "tacie.model-path"))
    }

    var isReady: Bool { state == .ready }

    /// The search model is in place and only a voice model is missing (the sidebar row says so, not "검색 모델").
    /// Read from disk when the state changes, not on every read: the sidebar reads it in a view body.
    private(set) var onlyVoice = false

    private static func onlyVoice(_ packs: [ModelPack]) -> Bool {
        packs.contains { $0.name == "search" && ModelManifest.installed($0.files, in: $0.dir) }
            && packs.contains { voicePacks.contains($0.name) && !ModelManifest.installed($0.files, in: $0.dir) }
    }

    /// What the sidebar shows for a state; nil hides the row (waiting for a connection or space, or nothing to fetch).
    nonisolated static func voiceRow(state: State, done: Int64, total: Int64) -> VoiceRow? {
        func mb(_ b: Int64) -> Int { Int((Double(b) / 1_000_000).rounded()) }
        func progress(_ done: Int64) -> VoiceRow {
            let f = total > 0 ? min(1, Double(done) / Double(total)) : 0
            return VoiceRow(title: String(localized: "보이스 모델 받는 중"), percent: "\(Int(f * 100))%", fraction: f,
                            detail: String(localized: "\(mb(done))MB / \(mb(total))MB · 다 받으면 다음 회의부터 누가 말했는지 나눠 보여 줘요"), failed: false)
        }
        switch state {
        case .downloading: return progress(done)
        case .verifying: return progress(total)
        case .failed(let why): return VoiceRow(title: String(localized: "보이스 모델을 받지 못했어요"), percent: nil, fraction: nil, detail: why, failed: true)
        case .waiting, .later, .idle, .ready: return nil
        }
    }

    /// Starts unless it is done or running. `force`: the user said 지금 받기 over an expensive network.
    func start(force: Bool = false) {
        switch state {
        case .ready, .downloading, .verifying: return
        default: break
        }
        if ModelManifest.installed(packs) {
            state = .ready
            return
        }
        let gate = ModelGate.check(expensive: path?.isExpensive ?? false, constrained: path?.isConstrained ?? false,
                                   online: path.map { $0.status == .satisfied } ?? true, free: freeSpace(),
                                   remaining: ModelManifest.remaining(packs) + ModelManifest.unpackRoom(packs))
        guard gate == .go || (gate == .expensive && force) else {
            state = .waiting(gate)
            Log.info("model download waits: \(gate)")
            return
        }
        run()
    }

    /// `--debug-control`'s `model-state`: shows a state without the network (screenshots).
    func show(_ s: State) { state = s }

    /// 와이파이에서 받기.
    func later() { state = .later }

    private func pathChanged(_ p: NWPath) {
        path = p
        // back online, or off the hotspot: carry on by itself
        switch state {
        case .waiting(.offline) where p.status == .satisfied: start()
        case .later where p.status == .satisfied && !p.isExpensive && !p.isConstrained: start()
        default: break
        }
    }

    private func freeSpace() -> Int64 {
        let dir = (packs.first { !ModelManifest.installed($0.files, in: $0.dir) } ?? packs[0]).dir
        let probe = FileManager.default.fileExists(atPath: dir.path) ? dir : dir.deletingLastPathComponent()
        let v = try? probe.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
        return v?.volumeAvailableCapacityForImportantUsage ?? -1
    }

    private func run() {
        state = .downloading
        done = total - ModelManifest.remaining(packs)
        let resumed = done > 0, began = Date(), pack: AModelPack = onlyVoice ? .voice : .search
        Log.info("model download: \(done) of \(total) bytes already here")
        task = Task { [weak self] in
            guard let self else { return }
            do {
                for pack in packs {
                    try FileManager.default.createDirectory(at: pack.dir, withIntermediateDirectories: true)
                    for f in pack.files where !ModelManifest.have(f, in: pack.dir) {
                        try await fetch(f, in: pack)
                    }
                }
                state = .ready
                Log.info("model download: ready")
                Analytics.capture(.modelDownloaded(resumed: resumed, seconds: Int(Date().timeIntervalSince(began)), pack: pack))
                onReady()
            } catch {
                Log.info("model download failed: \(error)")
                state = (path?.status ?? .satisfied) == .satisfied ? .failed(Self.explain(error)) : .waiting(.offline)
            }
        }
    }

    /// One file: resumes its `.part`, checks the hash, then gives it its name. A wrong hash starts that file over once.
    private func fetch(_ f: ModelFile, in pack: ModelPack, retry: Bool = true) async throws {
        let part = pack.dir.appendingPathComponent(f.name + ".part")
        let final = pack.dir.appendingPathComponent(f.name)
        if ModelManifest.size(of: part) < f.size {
            let have = max(0, ModelManifest.size(of: part))
            let before = done - have  // the other files; the callback counts this file's bytes from 0
            try await FileFetcher.fetch(pack.base.appendingPathComponent(f.name), to: part, from: have) { [weak self] written in
                Task { @MainActor in self?.done = before + written }
            }
        }
        state = .verifying
        let ok = try await Task.detached { try Self.sha256(of: part) == f.sha256 }.value
        guard ok else {
            try? FileManager.default.removeItem(at: part)
            done = total - ModelManifest.remaining(packs)
            guard retry else { throw DownloadError.corrupt(f.name) }
            Log.info("model download: \(f.name) did not match its hash, fetching it again")
            state = .downloading
            return try await fetch(f, in: pack, retry: false)
        }
        try? FileManager.default.removeItem(at: final)
        try FileManager.default.moveItem(at: part, to: final)
        state = .downloading
    }

    nonisolated static func sha256(of url: URL) throws -> String {
        let h = try FileHandle(forReadingFrom: url)
        defer { try? h.close() }
        var hasher = SHA256()
        while let chunk = try h.read(upToCount: 8 << 20), !chunk.isEmpty { hasher.update(data: chunk) }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    enum DownloadError: LocalizedError {
        case http(Int)
        case corrupt(String)
        var errorDescription: String? {
            switch self {
            case .http(let code): return String(localized: "서버가 \(code)로 답했어요")
            case .corrupt: return String(localized: "받은 파일이 깨졌어요")
            }
        }
    }

    private static func explain(_ error: Error) -> String {
        if let e = error as? DownloadError { return e.localizedDescription }
        return String(localized: "받지 못했어요")
    }
}

/// Streams one URL into a file from a byte offset (a Range request), reporting the bytes written in total.
final class FileFetcher: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    private let handle: FileHandle
    private var written: Int64
    private let progress: (Int64) -> Void
    private var continuation: CheckedContinuation<Void, Error>?
    private var failure: Error?
    private var lastReport = Date.distantPast

    private init(handle: FileHandle, written: Int64, progress: @escaping (Int64) -> Void) {
        self.handle = handle
        self.written = written
        self.progress = progress
    }

    static func fetch(_ url: URL, to file: URL, from offset: Int64, progress: @escaping (Int64) -> Void) async throws {
        if !FileManager.default.fileExists(atPath: file.path) {
            FileManager.default.createFile(atPath: file.path, contents: nil)
        }
        let handle = try FileHandle(forWritingTo: file)
        defer { try? handle.close() }
        try handle.truncate(atOffset: UInt64(offset))
        try handle.seekToEnd()
        let fetcher = FileFetcher(handle: handle, written: offset, progress: progress)
        var req = URLRequest(url: url)
        if offset > 0 { req.setValue("bytes=\(offset)-", forHTTPHeaderField: "Range") }
        let session = URLSession(configuration: .default, delegate: fetcher, delegateQueue: nil)
        defer { session.finishTasksAndInvalidate() }
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (c: CheckedContinuation<Void, Error>) in
                fetcher.continuation = c
                session.dataTask(with: req).resume()
            }
        } onCancel: {
            session.invalidateAndCancel()
        }
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse,
                    completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
        let code = (response as? HTTPURLResponse)?.statusCode ?? 0
        if code == 200 && written > 0 {
            // the server ignored the range: start this file over
            try? handle.truncate(atOffset: 0)
            written = 0
        } else if code != 200 && code != 206 {
            failure = ModelDownload.DownloadError.http(code)
            completionHandler(.cancel)
            return
        }
        completionHandler(.allow)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        do {
            try handle.write(contentsOf: data)
            written += Int64(data.count)
            if Date().timeIntervalSince(lastReport) > 0.25 {
                lastReport = Date()
                progress(written)
            }
        } catch {
            failure = error
            dataTask.cancel()
        }
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        progress(written)
        if let e = failure ?? error { continuation?.resume(throwing: e) } else { continuation?.resume() }
        continuation = nil
    }
}
