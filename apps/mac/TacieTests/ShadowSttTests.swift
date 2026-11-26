import XCTest
@testable import Tacie

/// A socket that records what was sent and hands out queued replies one by one.
final class FakeShadowSocket: ShadowSocket, @unchecked Sendable {
    var texts: [String] = []
    var datas: [Data] = []
    var closed = false
    private var queue: [String] = []
    private var waiting: CheckedContinuation<String, Error>?
    private var failure: Error?

    func send(text: String) async throws { texts.append(text) }
    func send(data: Data) async throws { datas.append(data) }
    func receive() async throws -> String {
        if let f = failure { throw f }
        if !queue.isEmpty { return queue.removeFirst() }
        return try await withCheckedThrowingContinuation { waiting = $0 }
    }
    func close() { closed = true; fail(URLError(.cancelled)) }

    func reply(_ s: String) {
        if let w = waiting { waiting = nil; w.resume(returning: s) } else { queue.append(s) }
    }
    func fail(_ e: Error) {
        failure = e
        if let w = waiting { waiting = nil; w.resume(throwing: e) }
    }
}

@MainActor
final class ShadowSttTests: XCTestCase {
    private func make(timeout: Duration = .seconds(2), keepalive: Duration = .seconds(10)) -> (ShadowStt, FakeShadowSocket) {
        let socket = FakeShadowSocket()
        let s = ShadowStt(socket: socket, config: #"{"model":"stt-rt-v6"}"#, finTimeout: timeout, keepaliveAfter: keepalive)
        return (s, socket)
    }
    private func token(_ text: String, _ start: Int = 0, _ end: Int = 0, final: Bool = true) -> String {
        #"{"tokens":[{"text":"\#(text)","start_ms":\#(start),"end_ms":\#(end),"is_final":\#(final)}]}"#
    }
    private let fin = #"{"tokens":[{"text":"<fin>","is_final":true}]}"#
    private func settle() async { for _ in 0..<20 { await Task.yield() } }

    func testFirstFrameIsTheConfig() async {
        let (s, socket) = make()
        s.start(streamMs: 0)
        await settle()
        XCTAssertEqual(socket.texts.first, #"{"model":"stt-rt-v6"}"#)
    }

    func testFeedSendsPCM16InOrder() async {
        let (s, socket) = make()
        s.start(streamMs: 0)
        s.feed([0.5])
        s.feed([-0.5])
        await settle()
        XCTAssertEqual(socket.datas, [SonioxTokens.pcm16([0.5]), SonioxTokens.pcm16([-0.5])])
    }

    /// With translation on (docs/30), Soniox sends translated tokens on the same connection: they stay out of the
    /// utterance's text and words and come back as its translation, with the language it was translated from.
    func testTranslatedTokensAreKeptApart() async {
        let (s, socket) = make()
        s.start(streamMs: 0)
        async let r = s.finalize(startMs: 0, endMs: 2000)
        await settle()
        socket.reply(#"{"tokens":[{"text":"Got","start_ms":100,"end_ms":300,"is_final":true,"translation_status":"original","language":"en"},{"text":" it.","start_ms":300,"end_ms":500,"is_final":true,"translation_status":"original","language":"en"}]}"#)
        socket.reply(#"{"tokens":[{"text":"알겠어요.","is_final":true,"translation_status":"translation","language":"ko","source_language":"en"}]}"#)
        socket.reply(fin)
        let result = await r
        XCTAssertEqual(result?.text, "Got it.")
        XCTAssertEqual(result?.words.map(\.text), ["Got", "it."])
        XCTAssertEqual(result?.translation, "알겠어요.")
        XCTAssertEqual(result?.translatedFrom, "en")
    }

    func testNoTranslationWhenNoneCame() async {
        let (s, socket) = make()
        s.start(streamMs: 0)
        async let r = s.finalize(startMs: 0, endMs: 400)
        await settle()
        socket.reply(#"{"tokens":[{"text":"네","start_ms":100,"end_ms":300,"is_final":true,"translation_status":"none"}]}"#)
        socket.reply(fin)
        let result = await r
        XCTAssertEqual(result?.text, "네")
        XCTAssertNil(result?.translation)
        XCTAssertNil(result?.translatedFrom)
    }

    func testLiveTextLeavesTranslationOut() async {
        let (s, socket) = make()
        var seen: [String] = []
        s.onLive = { said, partial in seen.append(said + "|" + partial) }
        s.start(streamMs: 0)
        await settle()
        socket.reply(#"{"tokens":[{"text":"Hi","start_ms":0,"end_ms":200,"is_final":true,"translation_status":"original"},{"text":"안녕","is_final":false,"translation_status":"translation"},{"text":" there","start_ms":200,"end_ms":400,"is_final":false,"translation_status":"original"}]}"#)
        await settle()
        XCTAssertEqual(seen.last, "Hi| there")
    }

    func testFinalizeReturnsTokensUpToFin() async {
        let (s, socket) = make()
        s.start(streamMs: 0)
        async let r = s.finalize(startMs: 50, endMs: 400)
        await settle()
        socket.reply(token(" 네", 100, 300))
        socket.reply(fin)
        let result = await r
        XCTAssertEqual(result?.text, "네")
        XCTAssertEqual(result?.startMs, 50)
        XCTAssertEqual(result?.endMs, 400)
        XCTAssertEqual(result?.words, [SpokenWord(text: "네", start: 100, end: 300)])
        XCTAssertGreaterThanOrEqual(result?.ms ?? -1, 0)
        XCTAssertTrue(socket.texts.contains(#"{"type":"finalize"}"#))
    }

    func testTwoFinalizesCloseInOrder() async {
        let (s, socket) = make()
        s.start(streamMs: 0)
        async let a = s.finalize(startMs: 0, endMs: 100)
        await settle()
        async let b = s.finalize(startMs: 200, endMs: 300)
        await settle()
        socket.reply(token(" 첫째"))
        socket.reply(fin)
        socket.reply(token(" 둘째"))
        socket.reply(fin)
        let (ra, rb) = await (a, b)
        XCTAssertEqual(ra?.text, "첫째")
        XCTAssertEqual(rb?.text, "둘째")
    }

    func testNonFinalTokensAreIgnored() async {
        let (s, socket) = make()
        s.start(streamMs: 0)
        async let r = s.finalize(startMs: 0, endMs: 100)
        await settle()
        socket.reply(token(" 잠정", final: false))
        socket.reply(token(" 확정"))
        socket.reply(fin)
        let result = await r
        XCTAssertEqual(result?.text, "확정")
    }

    func testFinTimeoutReturnsNilAndStops() async {
        let (s, _) = make(timeout: .milliseconds(50))
        s.start(streamMs: 0)
        let r = await s.finalize(startMs: 0, endMs: 100)
        XCTAssertNil(r)
        XCTAssertTrue(s.stopped)
        let again = await s.finalize(startMs: 200, endMs: 300)
        XCTAssertNil(again)
    }

    /// Replaced by a new connection (the 9-minute reconnect), the old one still answers the finalize sent just before,
    /// then closes.
    func testDelayedStopLetsTheLastFinalizeFinish() async throws {
        let (s, socket) = make()
        s.start(streamMs: 0)
        async let r = s.finalize(startMs: 0, endMs: 1000)
        await settle()
        s.stop(after: .milliseconds(200))
        XCTAssertFalse(s.stopped)
        socket.reply(token("맞아요", 100, 400))
        socket.reply(fin)
        let result = await r
        XCTAssertEqual(result?.text, "맞아요")
        try await Task.sleep(for: .milliseconds(400))
        await settle()
        XCTAssertTrue(s.stopped)
        XCTAssertTrue(socket.closed)
    }

    func testErrorMessageStops() async {
        let (s, socket) = make()
        s.start(streamMs: 0)
        async let r = s.finalize(startMs: 0, endMs: 100)
        await settle()
        socket.reply(#"{"error_code":401,"error_message":"x"}"#)
        let result = await r
        XCTAssertNil(result)
        XCTAssertTrue(s.stopped)
        // the socket closes on its own task after the stop: give it up to 2 s (it was read too early now and then)
        for _ in 0..<200 where !socket.closed { try? await Task.sleep(for: .milliseconds(10)) }
        XCTAssertTrue(socket.closed)
    }

    func testClockResetRebasesTokenTimes() async {
        let (s, socket) = make()
        s.start(streamMs: 5000)
        s.feed([Float](repeating: 0, count: 16000))  // 1 s of audio: Soniox time 0–1000
        s.clockReset()                                // stream 0 restarts at Soniox time 1000
        s.feed([Float](repeating: 0, count: 16000))
        async let r = s.finalize(startMs: 0, endMs: 1000)
        await settle()
        socket.reply(token(" 다시", 1200, 1500))
        socket.reply(fin)
        let result = await r
        XCTAssertEqual(result?.words, [SpokenWord(text: "다시", start: 200, end: 500)])
    }

    func testStartMapsSonioxZeroToTheStreamPosition() async {
        let (s, socket) = make()
        s.start(streamMs: 5000)
        async let r = s.finalize(startMs: 5000, endMs: 6000)
        await settle()
        socket.reply(token(" 처음", 100, 400))
        socket.reply(fin)
        let result = await r
        XCTAssertEqual(result?.words, [SpokenWord(text: "처음", start: 5100, end: 5400)])
    }

    /// Paused meeting or a dropped mic: no samples come, and Soniox closes a silent connection without keepalives.
    func testKeepaliveWhileNoAudioComes() async throws {
        let (s, socket) = make(keepalive: .milliseconds(40))
        s.start(streamMs: 0)
        try await Task.sleep(for: .milliseconds(150))
        XCTAssertTrue(socket.texts.contains(#"{"type":"keepalive"}"#))
        XCTAssertFalse(s.stopped)
        s.stop()
    }

    func testNoKeepaliveWhileAudioFlows() async throws {
        let (s, socket) = make(keepalive: .milliseconds(80))
        s.start(streamMs: 0)
        for _ in 0..<6 {
            s.feed([0])
            try await Task.sleep(for: .milliseconds(20))
        }
        XCTAssertFalse(socket.texts.contains(#"{"type":"keepalive"}"#))
        s.stop()
    }

    /// An utterance that began before this connection (a 9-minute swap) only has its tail here: nil, the file path.
    func testUtteranceOlderThanTheConnectionIsNil() async {
        let (s, socket) = make()
        s.start(streamMs: 5000)
        let r = await s.finalize(startMs: 4000, endMs: 6000)
        XCTAssertNil(r)
        XCTAssertFalse(s.stopped)
        _ = socket
    }

    /// The meeting page shows the sentence being said: final tokens so far, then the current partial ones.
    func testLiveTextFollowsTokens() async {
        let (s, socket) = make()
        var seen: [String] = []
        s.onLive = { said, partial in seen.append(said + "|" + partial) }
        s.start(streamMs: 0)
        await settle()
        socket.reply(#"{"tokens":[{"text":" 예전에","is_final":true},{"text":" 왜","is_final":false}]}"#)
        socket.reply(#"{"tokens":[{"text":" 왜 이렇게","is_final":false}]}"#)
        await settle()
        XCTAssertEqual(seen, ["예전에| 왜", "예전에| 왜 이렇게"])
        s.stop()
    }

    /// An utterance that began before the connection goes the file path, but its words must not stay on the
    /// connection and come back in front of the next utterance.
    func testSkippedUtteranceDoesNotLeakIntoTheNext() async {
        let (s, socket) = make()
        s.start(streamMs: 5000)
        let skipped = await s.finalize(startMs: 4000, endMs: 6000)
        XCTAssertNil(skipped)
        await settle()
        XCTAssertTrue(socket.texts.contains(#"{"type":"finalize"}"#))
        socket.reply(token(" 앞의 말", 5000, 5900))
        socket.reply(fin)
        async let next = s.finalize(startMs: 7000, endMs: 8000)
        await settle()
        socket.reply(token(" 다음 말", 7100, 7900))
        socket.reply(fin)
        let r = await next
        XCTAssertEqual(r?.text, "다음 말")
    }

    /// After <fin> the sentence belongs to the finalize result: the live text empties.
    func testLiveTextEmptiesAtFin() async {
        let (s, socket) = make()
        var seen: [String] = []
        s.onLive = { said, partial in seen.append(said + "|" + partial) }
        s.start(streamMs: 0)
        async let r = s.finalize(startMs: 0, endMs: 1000)
        await settle()
        socket.reply(token(" 네"))
        socket.reply(fin)
        _ = await r
        XCTAssertEqual(seen.last, "|")
    }

    /// A fragment shorter than 400 ms is not sent on its own, so no finalize closes its words: they come back in front
    /// of the next utterance and must stay in its transcript (2026-11-17).
    func testWordsOfADroppedShortFragmentReachTheNextLine() async throws {
        let tone: (Double) -> [Float] = { sec in
            (0..<Int(sec * 16_000)).map { 0.14 * sin(2 * .pi * 440 * Float($0) / 16_000) }
        }
        let quiet: (Double) -> [Float] = { [Float](repeating: 0, count: Int($0 * 16_000)) }
        let u = Utterancer(detector: EnergyDetector())
        var starts: [Int] = []
        u.onUtterance = { _, _, _, start in starts.append(start) }
        // "네" (0.1 s: with the pre-roll still under 400 ms), a pause, then the sentence
        u.feed(quiet(1)); u.feed(tone(0.1)); u.feed(quiet(1)); u.feed(tone(1)); u.feed(quiet(1))
        XCTAssertEqual(starts.count, 1, "the short fragment is not an utterance of its own")
        let start = try XCTUnwrap(starts.first)
        XCTAssertGreaterThan(start, 1800)

        let (s, socket) = make()
        s.start(streamMs: 0)
        async let r = s.finalize(startMs: start, endMs: start + 1500)
        await settle()
        socket.reply(token(" 네", 1000, 1100))
        socket.reply(token(" 그거는", start + 200, start + 700))
        socket.reply(fin)
        let result = await r
        let t = ShadowWiring.transcript(try XCTUnwrap(result))
        XCTAssertEqual(t?.text, "네 그거는")
        XCTAssertEqual(t?.words.first?.start, 0)
    }
}
