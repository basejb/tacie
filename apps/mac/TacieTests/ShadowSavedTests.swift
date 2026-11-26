import XCTest
@testable import Tacie

final class ShadowSavedTests: XCTestCase {
    func testOldMeetingWithoutShadowDecodes() throws {
        let plain = try JSONEncoder().encode(SavedMeeting(mode: "inperson", start: Date(timeIntervalSince1970: 0)))
        XCTAssertFalse(String(decoding: plain, as: UTF8.self).contains("shadow"))
        XCTAssertNil(try JSONDecoder().decode(SavedMeeting.self, from: plain).shadow)
    }

    func testShadowRoundTrips() throws {
        var m = SavedMeeting(mode: "inperson", start: Date(timeIntervalSince1970: 0))
        m.shadow = [ShadowResult(startMs: 100, endMs: 900, text: "네", words: [SpokenWord(text: "네", start: 200, end: 400)], ms: 210)]
        let back = try JSONDecoder().decode(SavedMeeting.self, from: JSONEncoder().encode(m))
        XCTAssertEqual(back.shadow, m.shadow)
    }

    func testStreamKeyDecodesTheEngineReply() throws {
        let k = try EngineClient.engineDecoder.decode(StreamKey.self, from: Data(#"{"api_key":"tmp","expires_at":"2026-11-10T10:00:00Z","ws_url":"wss://stt-rt.soniox.com/transcribe-websocket"}"#.utf8))
        XCTAssertEqual(k, StreamKey(apiKey: "tmp", expiresAt: "2026-11-10T10:00:00Z", wsUrl: URL(string: "wss://stt-rt.soniox.com/transcribe-websocket")!))
    }
}
