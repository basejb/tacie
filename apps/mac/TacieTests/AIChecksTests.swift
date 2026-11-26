import XCTest
@testable import Tacie

/// Open edition 2-D: the two checks the app makes itself (the OpenRouter key, and Ollama · LM Studio on this Mac).
/// Tested with fake responses only.
final class AIChecksTests: XCTestCase {
    private final class Seen: @unchecked Sendable { var requests: [URLRequest] = [] }

    private func respond(_ table: [String: Result<(Int, String), Error>], seen: Seen = Seen()) -> AIChecks.Get {
        { req in
            seen.requests.append(req)
            guard let r = table[req.url!.absoluteString] else { throw URLError(.cannotConnectToHost) }
            let (status, body) = try r.get()
            return (Data(body.utf8), HTTPURLResponse(url: req.url!, statusCode: status, httpVersion: nil, headerFields: nil)!)
        }
    }

    private let keyURL = "https://openrouter.ai/api/v1/key"
    private let ollama = "http://127.0.0.1:11434/v1/models"
    private let lmstudio = "http://127.0.0.1:1234/v1/models"

    func testKeyOk() async {
        let r = await AIChecks.checkOpenRouter("sk-or-fake", get: respond([keyURL: .success((200, "{}"))]))
        XCTAssertEqual(r, .ok)
    }

    func testKeyRejected401() async {
        let r = await AIChecks.checkOpenRouter("sk-or-fake", get: respond([keyURL: .success((401, ""))]))
        XCTAssertEqual(r, .rejected)
    }

    func testKeyRejected403() async {
        let r = await AIChecks.checkOpenRouter("sk-or-fake", get: respond([keyURL: .success((403, ""))]))
        XCTAssertEqual(r, .rejected)
    }

    func testKeyUnknownOn500() async {
        let r = await AIChecks.checkOpenRouter("sk-or-fake", get: respond([keyURL: .success((500, ""))]))
        XCTAssertEqual(r, .unknown)
    }

    func testKeyUnknownOnThrow() async {
        let r = await AIChecks.checkOpenRouter("sk-or-fake", get: respond([:]))
        XCTAssertEqual(r, .unknown)
    }

    func testKeyRequestShape() async {
        let seen = Seen()
        _ = await AIChecks.checkOpenRouter("sk-or-fake", get: respond([keyURL: .success((200, "{}"))], seen: seen))
        XCTAssertEqual(seen.requests.count, 1)
        let req = seen.requests[0]
        XCTAssertEqual(req.url?.absoluteString, keyURL)
        XCTAssertEqual(req.httpMethod ?? "GET", "GET")
        XCTAssertEqual(req.value(forHTTPHeaderField: "Authorization"), "Bearer sk-or-fake")
        XCTAssertNil(req.httpBody)
    }

    func testFindsOllamaFirst() async {
        let get = respond([ollama: .success((200, #"{"data":[{"id":"gemma4:e4b"},{"id":"qwen3.5:9b"}]}"#)),
                           lmstudio: .success((200, #"{"data":[{"id":"other"}]}"#))])
        let found = await AIChecks.findLocalServer(get: get, bundled: none)
        XCTAssertEqual(found, LocalServer(name: "Ollama", url: "http://127.0.0.1:11434/v1", model: "gemma4:e4b"))
    }

    func testFallsBackToLMStudio() async {
        let get = respond([lmstudio: .success((200, #"{"object":"list","data":[{"id":"google/gemma-4"}]}"#))])
        let found = await AIChecks.findLocalServer(get: get, bundled: none)
        XCTAssertEqual(found, LocalServer(name: "LM Studio", url: "http://127.0.0.1:1234/v1", model: "google/gemma-4"))
    }

    func testEmptyListIsNotFound() async {
        let get = respond([ollama: .success((200, #"{"data":[]}"#)), lmstudio: .success((200, "not json"))])
        let found = await AIChecks.findLocalServer(get: get, bundled: none)
        XCTAssertNil(found)
    }

    func testTimeoutIsNotFound() async {
        let get = respond([ollama: .failure(URLError(.timedOut)), lmstudio: .failure(URLError(.timedOut))])
        let found = await AIChecks.findLocalServer(get: get, bundled: none)
        XCTAssertNil(found)
    }

    func testTimeoutIs1_5s() async {
        let seen = Seen()
        _ = await AIChecks.findLocalServer(get: respond([:], seen: seen), bundled: none)
        XCTAssertEqual(seen.requests.map(\.timeoutInterval), [1.5, 1.5])
    }

    // final review: Ollama often lists an embedding model first; notes need a chat model
    func testSkipsEmbeddingModels() {
        let data = Data(#"{"data":[{"id":"nomic-embed-text:latest"},{"id":"bge-m3"},{"id":"gemma4:e4b"}]}"#.utf8)
        XCTAssertEqual(AIChecks.firstModel(data), "gemma4:e4b")
        XCTAssertNil(AIChecks.firstModel(Data(#"{"data":[{"id":"mxbai-embed-large"}]}"#.utf8)))
    }

    // 오픈소스판 e2e (2026-11-25): a model removed or swapped in Ollama after it was found failed every notes request by name
    func testServesTheSavedModelOnlyWhileListed() async {
        let saved = LocalServer(name: "Ollama", url: "http://127.0.0.1:11434/v1", model: "qwen2.5:1.5b")
        let swapped = respond([ollama: .success((200, #"{"data":[{"id":"qwen2.5:3b"}]}"#))])
        let still = respond([ollama: .success((200, #"{"data":[{"id":"qwen2.5:3b"},{"id":"qwen2.5:1.5b"}]}"#))])
        let off = respond([ollama: .failure(URLError(.cannotConnectToHost))])
        let gone = await AIChecks.serves(saved, get: swapped, bundled: none)
        let kept = await AIChecks.serves(saved, get: still, bundled: none)
        let down = await AIChecks.serves(saved, get: off, bundled: none)
        XCTAssertFalse(gone)
        XCTAssertTrue(kept)
        XCTAssertFalse(down)
    }

    // Ollama just installed: on, but no model yet — it needs a model, not to be turned on
    func testIdleServerIsOnWithoutAModel() async {
        let empty = respond([ollama: .success((200, #"{"object":"list","data":[]}"#))])
        let embedOnly = respond([lmstudio: .success((200, #"{"data":[{"id":"nomic-embed-text"}]}"#))])
        let ready = respond([ollama: .success((200, #"{"data":[{"id":"qwen2.5:3b"}]}"#))])
        let a = await AIChecks.idleServer(get: empty)
        let b = await AIChecks.idleServer(get: embedOnly)
        let c = await AIChecks.idleServer(get: ready)
        let d = await AIChecks.idleServer(get: respond([:]))
        XCTAssertEqual(a, "Ollama")
        XCTAssertEqual(b, "LM Studio")
        XCTAssertNil(c)
        XCTAssertNil(d)
    }

    private let none: AIChecks.Bundled = { nil }
    private let own = LocalServer(name: BundledLLM.name, url: "http://127.0.0.1:11535/v1", model: BundledLLM.model)

    // Tacie's own server, once its model is downloaded, comes before Ollama and LM Studio (issue #54)
    func testFindsTaciesOwnServerFirst() async {
        let own = self.own
        let seen = Seen()
        let get = respond([ollama: .success((200, #"{"data":[{"id":"qwen2.5:3b"}]}"#))], seen: seen)
        let found = await AIChecks.findLocalServer(get: get, bundled: { own })
        XCTAssertEqual(found, own)
        XCTAssertTrue(seen.requests.isEmpty)
        let without = await AIChecks.findLocalServer(get: get, bundled: none)
        XCTAssertEqual(without?.name, "Ollama")
    }

    // the saved server is Tacie's own: it serves when the bundled server (re)starts at the same address
    func testServesTaciesOwnServer() async {
        let own = self.own
        let moved = LocalServer(name: BundledLLM.name, url: "http://127.0.0.1:50000/v1", model: BundledLLM.model)
        let up = await AIChecks.serves(own, get: respond([:]), bundled: { own })
        let elsewhere = await AIChecks.serves(own, get: respond([:]), bundled: { moved })
        let gone = await AIChecks.serves(own, get: respond([:]), bundled: none)
        XCTAssertTrue(up)
        XCTAssertFalse(elsewhere)
        XCTAssertFalse(gone)
    }
}
