import Darwin
import Foundation

/// The open edition's on-device notes server (issue #54): llama.cpp's server, bundled as Contents/MacOS/tacie-llm
/// (scripts/build-llm.sh), runs the notes model 받기 downloaded (`ModelManifest.notesFile`). It is the first "local
/// server" `AIChecks.findLocalServer` finds, ahead of Ollama and LM Studio, so the engine reaches it the same way:
/// an OpenAI-compatible address on 127.0.0.1.
///
/// It starts when the app looks for a local server in "Mac 안에서만" and the model is there. The server loads the model,
/// and after `idleSeconds` without a request it unloads it (llama.cpp's sleep on idle); the next request loads it again.
/// Tacie stops it on quit. A server left by a crash is stopped on the next start (its process id is kept in a file).
@MainActor
final class BundledLLM {
    static let shared = BundledLLM()

    /// How the card and `LocalServer` name it, and the model name the server answers to.
    static let name = "Tacie"
    static let model = "gemma-4-e4b"
    /// Each request gets this much context (the evaluation's 24k, docs/24 7-8절).
    static let slotContext = 24_576
    static let idleSeconds = 120
    static let preferredPort = 11_535

    /// Requests at once: the engine writes a meeting's topics in parallel (docs/24 measured 4 at 9.7 GB). A Mac under
    /// 24 GB gets 2, which keeps the server's memory near 6 GB.
    nonisolated static func slots(memory: UInt64) -> Int { memory >= 24 << 30 ? 4 : 2 }

    nonisolated static func arguments(model: URL, port: Int, memory: UInt64) -> [String] {
        let n = slots(memory: memory)
        return ["-m", model.path, "--host", "127.0.0.1", "--port", String(port), "--alias", Self.model,
                "-np", String(n), "-c", String(n * slotContext), "--reasoning", "off",
                "--sleep-idle-seconds", String(idleSeconds), "--no-mmproj"]
    }

    /// The model file, when 받기 has finished.
    nonisolated static func modelFile(env: [String: String] = ProcessInfo.processInfo.environment,
                                      home: URL = URL(fileURLWithPath: NSHomeDirectory())) -> URL? {
        let pack = ModelManifest.notesPack(env: env, home: home)
        guard ModelManifest.installed(pack.files, in: pack.dir) else { return nil }
        return pack.dir.appendingPathComponent(ModelManifest.notesFile.name)
    }

    static var binary: URL? { Bundle.main.url(forAuxiliaryExecutable: "tacie-llm") }

    private var process: Process?
    private var port: Int?
    private var starting: Task<LocalServer?, Never>?

    private static var pidFile: URL {
        AccountSpace.base.appendingPathComponent("tacie-llm.pid")
    }

    /// The running server, started if needed. nil without the bundled binary or the model, or when it does not answer
    /// within a minute.
    func server() async -> LocalServer? {
        if let process, process.isRunning, let port { return Self.local(port) }
        if let starting { return await starting.value }
        let task = Task { await start() }
        starting = task
        let found = await task.value
        starting = nil
        return found
    }

    /// This app's server process is running (no request, no start).
    var isRunning: Bool { process?.isRunning == true }

    func stop() {
        if let process, process.isRunning { process.terminate() }
        process = nil
        port = nil
        try? FileManager.default.removeItem(at: Self.pidFile)
    }

    private static func local(_ port: Int) -> LocalServer {
        LocalServer(name: name, url: "http://127.0.0.1:\(port)/v1", model: model)
    }

    private func start() async -> LocalServer? {
        guard let binary = Self.binary, let model = Self.modelFile() else { return nil }
        Self.stopLeftover()
        let port = Self.freePort(preferred: Self.preferredPort)
        let p = Process()
        p.executableURL = binary
        p.arguments = Self.arguments(model: model, port: port, memory: ProcessInfo.processInfo.physicalMemory)
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        p.standardInput = FileHandle.nullDevice
        do { try p.run() } catch {
            Log.info("tacie-llm did not start: \(error)")
            return nil
        }
        process = p
        self.port = port
        try? FileManager.default.createDirectory(at: Self.pidFile.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? String(p.processIdentifier).write(to: Self.pidFile, atomically: true, encoding: .utf8)
        Log.info("tacie-llm started on \(port)")
        // /health answers 503 while the model loads and 200 once it can take requests
        let health = URL(string: "http://127.0.0.1:\(port)/health")!
        for _ in 0..<120 {
            if !p.isRunning { break }
            if let (_, http) = try? await AIChecks.live(URLRequest(url: health, timeoutInterval: 1)), http.statusCode == 200 {
                return Self.local(port)
            }
            try? await Task.sleep(for: .milliseconds(500))
        }
        Log.info("tacie-llm did not answer (running: \(p.isRunning))")
        stop()
        return nil
    }

    /// A server an earlier Tacie left running (it crashed or was killed): stop it, if that process is still ours.
    private static func stopLeftover() {
        guard let text = try? String(contentsOf: pidFile, encoding: .utf8), let pid = Int32(text.trimmingCharacters(in: .whitespacesAndNewlines)),
              pid > 0 else { return }
        var buf = [CChar](repeating: 0, count: Int(MAXPATHLEN))
        if proc_pidpath(pid, &buf, UInt32(buf.count)) > 0, String(cString: buf).hasSuffix("/tacie-llm") {
            kill(pid, SIGTERM)
        }
        try? FileManager.default.removeItem(at: pidFile)
    }

    /// The preferred port when nothing listens there, else one the system picks (also for 0).
    nonisolated static func freePort(preferred: Int) -> Int {
        if preferred > 0, bindable(preferred) { return preferred }
        return bound(0) ?? preferred
    }

    private nonisolated static func bindable(_ port: Int) -> Bool { bound(port) != nil }

    /// Binds 127.0.0.1:port and closes it again; the port it got, or nil.
    private nonisolated static func bound(_ port: Int) -> Int? {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { return nil }
        defer { close(fd) }
        var addr = sockaddr_in()
        addr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = in_port_t(UInt16(port).bigEndian)
        addr.sin_addr.s_addr = inet_addr("127.0.0.1")
        let ok = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) }
        }
        guard ok == 0 else { return nil }
        var out = sockaddr_in()
        var len = socklen_t(MemoryLayout<sockaddr_in>.size)
        _ = withUnsafeMutablePointer(to: &out) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(fd, $0, &len) }
        }
        return Int(UInt16(bigEndian: out.sin_port))
    }
}
