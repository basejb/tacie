import Foundation

/// The two checks the app makes itself in the open edition's 2-D. They skip the engine and go through URLSession only.
/// Neither costs money.
/// - OpenRouter key: `GET /api/v1/key` (key info). The key goes only in the Authorization header.
/// - This Mac's Ollama (11434) · LM Studio (1234): OpenAI-compatible `GET /v1/models`, 1.5 s each.
enum AIChecks {
    typealias Get = @Sendable (URLRequest) async throws -> (Data, HTTPURLResponse)

    enum KeyCheck: Equatable { case ok, rejected, unknown }

    static let live: Get = { req in
        let (data, response) = try await URLSession.shared.data(for: req)
        guard let http = response as? HTTPURLResponse else { throw URLError(.badServerResponse) }
        return (data, http)
    }

    static let keyURL = URL(string: "https://openrouter.ai/api/v1/key")!

    /// 200: the key works. 401·403: OpenRouter refused it. Anything else (offline, 5xx): not known now.
    static func checkOpenRouter(_ key: String, get: Get = live) async -> KeyCheck {
        var req = URLRequest(url: keyURL, timeoutInterval: 10)
        req.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        guard let (_, http) = try? await get(req) else { return .unknown }
        switch http.statusCode {
        case 200: return .ok
        case 401, 403: return .rejected
        default: return .unknown
        }
    }

    static let localServers: [(name: String, url: String)] = [
        ("Ollama", "http://127.0.0.1:11434/v1"),
        ("LM Studio", "http://127.0.0.1:1234/v1"),
    ]

    /// The first server on this Mac that answers with at least one model; its first model is the one notes use.
    static func findLocalServer(get: Get = live) async -> LocalServer? {
        for server in localServers {
            let req = URLRequest(url: URL(string: server.url + "/models")!, timeoutInterval: 1.5)
            guard let (data, http) = try? await get(req), http.statusCode == 200,
                  let model = firstModel(data) else { continue }
            return LocalServer(name: server.name, url: server.url, model: model)
        }
        return nil
    }

    /// The server still lists the saved model: an Ollama model removed or swapped after it was found would otherwise
    /// fail every notes request by name ("model not found"), and 다시 쓰기 with it (오픈소스판 e2e, 2026-11-25).
    static func serves(_ server: LocalServer, get: Get = live) async -> Bool {
        let req = URLRequest(url: URL(string: server.url + "/models")!, timeoutInterval: 1.5)
        guard let (data, http) = try? await get(req), http.statusCode == 200 else { return false }
        let ids = (try? JSONDecoder().decode(Models.self, from: data))?.data.map(\.id) ?? []
        return ids.contains(server.model)
    }

    /// The name of a server that is on but has no model that can write notes (Ollama just installed): it needs a model,
    /// not to be turned on.
    static func idleServer(get: Get = live) async -> String? {
        for server in localServers {
            let req = URLRequest(url: URL(string: server.url + "/models")!, timeoutInterval: 1.5)
            guard let (data, http) = try? await get(req), http.statusCode == 200, firstModel(data) == nil else { continue }
            return server.name
        }
        return nil
    }

    /// The notes model "받기" asks this Mac's Ollama for (docs/24 7-8절 measured Gemma 4 E4B at Q4_K_M; the plain
    /// `gemma4:e4b` tag is 6.6–9.5GB depending on the Mac).
    static let notesModel = "gemma4:e4b-it-q4_K_M"
    static let notesModelSize: Int64 = 6_600_000_000
    static let ollamaPullURL = URL(string: "http://127.0.0.1:11434/api/pull")!

    /// Ollama's `/api/pull` progress: one JSON object per line, a total and completed bytes per layer (digest).
    struct PullProgress: Equatable {
        var layers: [String: (total: Int64, done: Int64)] = [:]
        var finished = false
        var error: String?

        var total: Int64 { layers.values.reduce(0) { $0 + $1.total } }
        var done: Int64 { layers.values.reduce(0) { $0 + $1.done } }

        mutating func read(_ line: String) {
            guard let obj = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any] else { return }
            if let e = obj["error"] as? String { error = e; return }
            if obj["status"] as? String == "success" { finished = true; return }
            if let digest = obj["digest"] as? String, let total = (obj["total"] as? NSNumber)?.int64Value, total > 0 {
                let done = (obj["completed"] as? NSNumber)?.int64Value ?? 0
                layers[digest] = (total, min(done, total))
            }
        }

        static func == (a: Self, b: Self) -> Bool {
            a.total == b.total && a.done == b.done && a.finished == b.finished && a.error == b.error
        }
    }

    /// Asks this Mac's Ollama to download `model`, reporting progress as it comes; true once Ollama says success.
    static func pullFromOllama(_ model: String, progress: @escaping @MainActor (PullProgress) -> Void) async -> Bool {
        var req = URLRequest(url: ollamaPullURL, timeoutInterval: 60)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try? JSONSerialization.data(withJSONObject: ["model": model, "stream": true])
        guard let (bytes, response) = try? await URLSession.shared.bytes(for: req),
              (response as? HTTPURLResponse)?.statusCode == 200 else { return false }
        var p = PullProgress()
        do {
            for try await line in bytes.lines {
                p.read(line)
                let now = p
                await progress(now)
                if p.finished || p.error != nil { break }
            }
        } catch { return false }
        return p.finished
    }

    /// Room left on the disk the home folder (and Ollama's models) are on.
    static func freeSpace() -> Int64 {
        let home = URL(fileURLWithPath: NSHomeDirectory())
        return (try? home.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey]))?
            .volumeAvailableCapacityForImportantUsage ?? .max
    }

    private struct Models: Decodable {
        struct Model: Decodable { let id: String }
        let data: [Model]
    }

    /// The first model that can write notes: embedding and ranking models (Ollama often lists one first) are skipped.
    static func firstModel(_ data: Data) -> String? {
        let ids = (try? JSONDecoder().decode(Models.self, from: data))?.data.map(\.id) ?? []
        return ids.first { id in
            let lower = id.lowercased()
            return !id.trimmingCharacters(in: .whitespaces).isEmpty
                && !["embed", "bge", "rerank"].contains(where: lower.contains)
        }
    }
}
