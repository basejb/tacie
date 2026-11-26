import XCTest

@MainActor
final class WaveformTests: XCTestCase {
    private var tmp: URL!

    override func setUpWithError() throws {
        tmp = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
    }

    override func tearDown() {
        try? FileManager.default.removeItem(at: tmp)
    }

    private func tone(_ seconds: Double, loud: Float) -> [Float] {
        (0..<Int(seconds * 16_000)).map { loud * sin(2 * .pi * 220 * Float($0) / 16_000) }
    }

    private func line(_ key: String, audioAt: Int?, end: Int? = nil, voice: String? = nil) -> TranscriptLine {
        TranscriptLine(key: key, at: Date(), speaker: nil, text: key, question: false, voice: voice, audioAt: audioAt,
                       words: end.map { [SpokenWord(text: key, start: audioAt ?? 0, end: $0)] })
    }

    func testPeaksFollowLoudness() throws {
        let url = tmp.appendingPathComponent("m.caf")
        let r = try MeetingRecorder(url: url)
        r.append(tone(2, loud: 0.3)); r.append(tone(2, loud: 0)); r.append(tone(2, loud: 0.3))
        _ = r.finish()
        let p = Waveform.peaks(of: [url], bins: 6)
        XCTAssertEqual(p.count, 6)
        XCTAssertGreaterThan(p[0], 0.8)
        XCTAssertLessThan(p[2], 0.1)
        XCTAssertLessThan(p[3], 0.1)
        XCTAssertGreaterThan(p[5], 0.8)
    }

    func testPeaksOfPartsRunEndToEnd() throws {
        let url = tmp.appendingPathComponent("m.caf")
        let r = try MeetingRecorder(url: url, partSeconds: 1)
        r.append(tone(1, loud: 0)); r.append(tone(1, loud: 0.3)); _ = r.close()
        let p = Waveform.peaks(of: MeetingRecorder.playableFiles(for: url), bins: 4)
        XCTAssertLessThan(p[0], 0.1)
        XCTAssertGreaterThan(p[3], 0.8)
    }

    func testNoFilesNoPeaks() {
        XCTAssertEqual(Waveform.peaks(of: [tmp.appendingPathComponent("none.caf")]), [])
    }

    func testTintsFollowLinesAndUnheard() {
        let lines = [line("a", audioAt: 0, end: 2000, voice: "화자 1"),
                     line("b", audioAt: 4000, voice: "화자 2"),
                     line("c", audioAt: 8000, end: 9000, voice: "화자 1")]
        let t = Waveform.tints(lines: lines, unheard: [AudioSpan(start: 6000, end: 8000)], durationMs: 10_000, bins: 10) { l in
            l.voice == "화자 1" ? 0 : 1
        }
        XCTAssertEqual(t, [.speaker(0), .speaker(0), .quiet, .quiet, .speaker(1), .speaker(1), .unheard, .unheard, .speaker(0), .quiet])
    }

    func testTintsWithoutSpeakersAreVoiceless() {
        let t = Waveform.tints(lines: [line("a", audioAt: 0, end: 500)], unheard: [], durationMs: 1000, bins: 2) { _ in nil }
        XCTAssertEqual(t, [.voiceless, .quiet])
    }

    func testBarsTakeTheLoudestStep() {
        let bars = Waveform.bars(peaks: [0.1, 0.9, 0.2, 0.3], tints: [.quiet, .speaker(2), .quiet, .speaker(1)], count: 2)
        XCTAssertEqual(bars.map(\.height), [0.9, 0.3])
        XCTAssertEqual(bars.map(\.tint), [.speaker(2), .speaker(1)])
    }

}
