import XCTest
@testable import Tacie

final class UtterancerClockTests: XCTestCase {
    private func silence(seconds: Double) -> [Float] { [Float](repeating: 0, count: Int(seconds * 16_000)) }
    private func tone(seconds: Double) -> [Float] {
        let amp: Float = 0.1 * 1.4142135  // -20 dBFS RMS
        return (0..<Int(seconds * 16_000)).map { amp * sin(2 * .pi * 440 * Float($0) / 16_000) }
    }

    private struct Heard { let wav: Data; let start: Int }

    /// Utterancer with every fed sample and every utterance captured.
    private final class Rig {
        let u: Utterancer
        var fed: [Float] = []
        var heard: [Heard] = []
        // tones stand in for speech: the energy rule hears them
        init(detector: SpeechDetector = EnergyDetector()) {
            u = Utterancer(detector: detector)
            u.onSamples = { [unowned self] in fed += $0 }
            u.onUtterance = { [unowned self] wav, _, _, start in heard.append(Heard(wav: wav, start: start)) }
        }
        func resetClock() { u.resetClock(); fed = []; heard = [] }
    }

    /// Any sound is speech, however long it lasts (the energy rule gets used to a long tone).
    private final class LoudDetector: SpeechDetector {
        func isSpeech(_ frame: [Float]) -> Bool { frame.contains { abs($0) > 0.01 } }
        func reset() {}
    }

    private func pcm(_ wav: Data) -> [Int16] {
        let body = Array(wav.dropFirst(44))
        return stride(from: 0, to: body.count - 1, by: 2).map { Int16(bitPattern: UInt16(body[$0]) | UInt16(body[$0 + 1]) << 8) }
    }

    /// The invariant later tasks rely on: startMs is the exact offset of the WAV's first sample in the fed stream.
    private func assertAligned(_ h: Heard, in fed: [Float], file: StaticString = #filePath, line: UInt = #line) {
        let got = pcm(h.wav)
        XCTAssertEqual(h.start % 20, 0, file: file, line: line)
        let from = h.start * 16
        XCTAssertLessThanOrEqual(from + got.count, fed.count, file: file, line: line)
        guard from + got.count <= fed.count else { return }
        let want = fed[from..<(from + got.count)].map { Int16(max(-1, min(1, $0)) * Float(Int16.max)) }
        XCTAssertEqual(got, want, "WAV is not the stream slice starting at \(h.start) ms", file: file, line: line)
    }

    func testStartMsMatchesSamplesFed() {
        let r = Rig()
        r.u.feed(silence(seconds: 2)); r.u.feed(tone(seconds: 1)); r.u.feed(silence(seconds: 1))
        XCTAssertEqual(r.heard.count, 1)
        // speech is noticed on the 3rd loud frame (index 102); the pre-roll keeps 10 frames up to it: 93 * 20 ms
        XCTAssertEqual(r.heard[0].start, 2000 - 7 * 20)
        assertAligned(r.heard[0], in: r.fed)
    }

    func testClockCountsOnlyFedSamples() {  // a paused mic feeds nothing: the clock stops too
        let r = Rig()
        r.u.feed(silence(seconds: 1)); r.u.feed(tone(seconds: 1)); r.u.feed(silence(seconds: 1))
        // (paused: nothing fed for a while)
        r.u.feed(silence(seconds: 1)); r.u.feed(tone(seconds: 1)); r.u.feed(silence(seconds: 1))
        XCTAssertEqual(r.heard.count, 2)
        XCTAssertEqual(r.heard[0].start, 1000 - 7 * 20)
        XCTAssertEqual(r.heard[1].start, 4000 - 7 * 20)
        r.heard.forEach { assertAligned($0, in: r.fed) }
    }

    func testOnSamplesSeesEverySampleAndResetRestarts() {
        let r = Rig()
        r.u.feed(silence(seconds: 3)); r.resetClock()
        r.u.feedMix(mic: silence(seconds: 1), call: tone(seconds: 1)); r.u.feed(silence(seconds: 1))
        XCTAssertEqual(r.fed.count, 16_000 * 2)
        XCTAssertEqual(r.heard.count, 1)
        XCTAssertEqual(r.heard[0].start, 0)
        assertAligned(r.heard[0], in: r.fed)
    }

    func testOddSizedChunksStayAligned() {
        let r = Rig()
        let stream = silence(seconds: 1.37) + tone(seconds: 1) + silence(seconds: 1) + tone(seconds: 0.8) + silence(seconds: 1)
        for i in stride(from: 0, to: stream.count, by: 333) { r.u.feed(Array(stream[i..<min(i + 333, stream.count)])) }
        XCTAssertEqual(r.fed, stream)
        XCTAssertEqual(r.heard.count, 2)
        r.heard.forEach { assertAligned($0, in: r.fed) }
    }

    func testFeedMixWithMismatchedLengthsStaysAligned() {
        let r = Rig()
        // the longer side's extra samples are dropped, so the stream is only what onSamples saw
        r.u.feedMix(mic: silence(seconds: 1.01), call: silence(seconds: 1))
        r.u.feedMix(mic: tone(seconds: 1), call: tone(seconds: 1.013))
        r.u.feedMix(mic: silence(seconds: 1), call: silence(seconds: 1.007))
        r.u.feedMix(mic: silence(seconds: 0.5), call: silence(seconds: 0.5))
        XCTAssertEqual(r.fed.count, 16_000 * 3 + 8_000 + 0)
        XCTAssertEqual(r.heard.count, 1)
        assertAligned(r.heard[0], in: r.fed)
    }

    /// A chunk that ends an utterance reaches `onSamples` only up to the cut before `onUtterance`, the rest after it: the
    /// shadow's finalize then marks the boundary at the cut (2026-11-17).
    func testSamplesAfterTheCutComeAfterTheUtterance() {
        let r = Rig(detector: LoudDetector())
        var fedAtCut: [Int] = []
        r.u.onUtterance = { [unowned r] wav, _, _, start in
            fedAtCut.append(r.fed.count)
            r.heard.append(Heard(wav: wav, start: start))
        }
        // one chunk holds the end of a sentence and the start of the next; one holds a sentence cut at 15 s
        let stream = silence(seconds: 0.5) + tone(seconds: 1) + silence(seconds: 0.8) + tone(seconds: 0.3)
        r.u.feed(stream)
        r.u.feed(tone(seconds: 16) + silence(seconds: 1))
        XCTAssertEqual(r.heard.count, 3)
        for (h, at) in zip(r.heard, fedAtCut) {
            XCTAssertEqual(at, h.start * 16 + pcm(h.wav).count, "samples past the cut went out before it")
        }
        XCTAssertLessThan(fedAtCut[0], stream.count)
        XCTAssertEqual(r.fed.count, stream.count + 17 * 16_000)
        r.heard.forEach { assertAligned($0, in: r.fed) }
    }
}
