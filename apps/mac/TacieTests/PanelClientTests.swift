import XCTest
@testable import Tacie

/// Answers every request of a test session from `handler`, and keeps the last request body.
final class StubProtocol: URLProtocol {
    nonisolated(unsafe) static var handler: ((URLRequest, Data) -> (Int, Data))?
    nonisolated(unsafe) static var lastBody = Data()

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}
    override func startLoading() {
        var body = request.httpBody ?? Data()
        if body.isEmpty, let stream = request.httpBodyStream {
            stream.open()
            var buf = [UInt8](repeating: 0, count: 4096)
            while stream.hasBytesAvailable {
                let n = stream.read(&buf, maxLength: buf.count)
                if n <= 0 { break }
                body.append(buf, count: n)
            }
            stream.close()
        }
        Self.lastBody = body
        let (code, data) = Self.handler?(request, body) ?? (500, Data())
        let resp = HTTPURLResponse(url: request.url!, statusCode: code, httpVersion: nil, headerFields: nil)!
        client?.urlProtocol(self, didReceive: resp, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: data)
        client?.urlProtocolDidFinishLoading(self)
    }
}

final class PanelClientTests: XCTestCase {
    private static let fixtures = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("Fixtures")

    private func client() -> EngineClient {
        let cfg = URLSessionConfiguration.ephemeral
        cfg.protocolClasses = [StubProtocol.self]
        return EngineClient(port: 1, token: "t", session: URLSession(configuration: cfg))
    }

    private final class Steps: @unchecked Sendable {
        private let lock = NSLock()
        private var list: [PanelStep] = []
        func add(_ s: PanelStep) { lock.lock(); list.append(s); lock.unlock() }
        var all: [PanelStep] { lock.lock(); defer { lock.unlock() }; return list }
    }

    func testPrepStreamDeliversStepsBeforeAnswer() async throws {
        let data = try Data(contentsOf: Self.fixtures.appendingPathComponent("panel_prep.ndjson"))
        StubProtocol.handler = { _, _ in (200, data) }
        let steps = Steps()
        let answer = try await client().prep(meetingIDs: ["m1", "m2"]) { steps.add($0) }
        XCTAssertEqual(steps.all, [.meetings(["m1", "m2"]), .related(["m0"]), .sources(3), .writing])
        XCTAssertEqual(answer.advice, "가격 결정부터 이야기하세요.")
        XCTAssertEqual(answer.open.first?.meetingId, "m1")
        XCTAssertEqual(answer.open.first?.also, ["m2"])
        XCTAssertEqual(answer.todos.first?.owner, "화자 1")
        XCTAssertEqual(answer.sources.first?.lineStart, 1)
        XCTAssertEqual(answer.meetingsRead, 3)
        XCTAssertNil(answer.errorKind)
        let sent = try JSONSerialization.jsonObject(with: StubProtocol.lastBody) as? [String: Any]
        XCTAssertEqual(sent?["meeting_ids"] as? [String], ["m1", "m2"])
    }

    func testPrepThrowsWhenStreamEndsWithoutDone() async {
        StubProtocol.handler = { _, _ in (200, Data("{\"step\":\"writing\"}\n".utf8)) }
        do {
            _ = try await client().prep(meetingIDs: ["m1"]) { _ in }
            XCTFail("expected a throw")
        } catch {}
    }

    func testPrepThrowsHttpErrorOn404() async {
        StubProtocol.handler = { _, _ in (404, Data("{\"error\":\"not found\"}".utf8)) }
        do {
            _ = try await client().prep(meetingIDs: ["m1"]) { _ in }
            XCTFail("expected a throw")
        } catch let e as EngineError {
            guard case .http(404, _) = e else { return XCTFail("\(e)") }
        } catch { XCTFail("\(error)") }
    }

    func testWhySendsOnlyKeyNameTimeText() async throws {
        StubProtocol.handler = { _, _ in (200, Data("{\"reason\":\"비용 때문이에요\",\"keys\":[\"e1:s1u2\"],\"cost\":0.001}".utf8)) }
        let at = Date(timeIntervalSince1970: 1_800_000_000)
        let line = TranscriptLine(key: "e1:s1u2", at: at, speaker: "me", text: "비용이 문제예요", question: false,
                                  voice: "화자 2", name: "지윤", audioAt: 1234, words: [SpokenWord(text: "비용", start: 0, end: 5)])
        let r = try await client().why(decision: "B안으로 간다", window: [line], names: { _ in "민호" })
        XCTAssertEqual(r.reason, "비용 때문이에요")
        XCTAssertEqual(r.keys, ["e1:s1u2"])
        let body = String(decoding: StubProtocol.lastBody, as: UTF8.self)
        for banned in ["voice", "words", "audioAt", "audio_at", "지윤", "화자 2"] { XCTAssertFalse(body.contains(banned), banned) }
        let sent = try JSONSerialization.jsonObject(with: StubProtocol.lastBody) as? [String: Any]
        XCTAssertEqual(sent?["decision"] as? String, "B안으로 간다")
        let w = (sent?["window"] as? [[String: Any]])?.first
        XCTAssertEqual(Set(w?.keys.map { $0 } ?? []), ["key", "speaker", "at", "text"])
        XCTAssertEqual(w?["speaker"] as? String, "민호")
        XCTAssertEqual(w?["at"] as? String, EngineClient.whyTime.string(from: at))
        XCTAssertNotNil((w?["at"] as? String)?.wholeMatch(of: /\d\d:\d\d/))
    }

    func testWhyFailureBodyKeepsTheErrorKind() async throws {
        let c = client()
        // the engine's failure reply; an older engine left out `reason`
        for body in ["{\"error\":\"boom\",\"error_kind\":\"credit\",\"reason\":\"\",\"keys\":[],\"cost\":0}",
                     "{\"error\":\"boom\",\"error_kind\":\"credit\",\"keys\":[]}",
                     "{\"error\":\"boom\",\"error_kind\":\"credit\"}"] {
            StubProtocol.handler = { _, _ in (200, Data(body.utf8)) }
            let r = try await c.why(decision: "B안", window: [], names: { _ in "" })
            XCTAssertEqual(r, WhyAnswer(reason: "", keys: [], errorKind: "credit"), body)
        }
    }

    func testWhyTimeIsTwentyFourHourWhateverTheMacSays() {
        XCTAssertEqual(EngineClient.whyTime.locale.identifier, "en_US_POSIX")
        XCTAssertEqual(EngineClient.whyTime.dateFormat, "HH:mm")
    }

    func testPanelAskAndFollowupDecode() async throws {
        StubProtocol.handler = { req, _ in
            req.url!.path == "/panel/ask"
                ? (200, Data("{\"answer\":null,\"cited\":[],\"cards\":[],\"cost\":0,\"error_kind\":\"model\"}".utf8))
                : (200, Data("{\"results\":[{\"i\":0,\"decided\":true,\"how\":\"B안\"},{\"i\":1,\"decided\":false}],\"cost\":0}".utf8))
        }
        let c = client()
        let a = try await c.panelAsk("왜요?", meetingIDs: nil, history: [(q: "앞", a: "답")])
        XCTAssertNil(a.answer)
        XCTAssertEqual(a.errorKind, "model")
        let sent = try JSONSerialization.jsonObject(with: StubProtocol.lastBody) as? [String: Any]
        XCTAssertTrue(sent?["meeting_ids"] is NSNull)
        XCTAssertEqual((sent?["history"] as? [[String: String]])?.first, ["q": "앞", "a": "답"])
        let item = PanelItem(meetingId: "m1", index: 0, kind: "open_question", text: "가격", owner: nil, due: nil, also: nil)
        let r = try await c.followup(prepared: [item], ledger: [LedgerEntry(kind: "decision", text: "B안")])
        XCTAssertEqual(r, [FollowResult(i: 0, decided: true, how: "B안"), FollowResult(i: 1, decided: false, how: nil)])
    }
}
