import Foundation

/// The two checks the app makes itself in the open edition's 2-D. They skip the engine and go through URLSession only.
/// Neither costs money.
/// - OpenRouter key: `GET /api/v1/key` (key info). The key goes only in the Authorization header.
/// - Tacie's own notes server (`BundledLLM`, once 받기 has the model), then this Mac's Ollama (11434) · LM Studio (1234):
///   OpenAI-compatible `GET /v1/models`, 1.5 s each.
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

    typealias Bundled = @Sendable () async -> LocalServer?
    static let ownServer: Bundled = { await BundledLLM.shared.server() }

    /// Tacie's own server when its model is downloaded; else the first server on this Mac that answers with at least
    /// one model, whose first model is the one notes use.
    static func findLocalServer(get: Get = live, bundled: Bundled = ownServer) async -> LocalServer? {
        if let own = await bundled() { return own }
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
    static func serves(_ server: LocalServer, get: Get = live, bundled: Bundled = ownServer) async -> Bool {
        // Tacie's own server: (re)started if it stopped; the same address means the engine already has it
        if server.name == BundledLLM.name { return await bundled() == server }
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

    /// The notes model Ollama users may have pulled with an older Tacie (shown by name as Gemma 4 E4B).
    static let notesModel = "gemma4:e4b-it-q4_K_M"

    /// Room left on the disk the home folder (and the models) are on.
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
