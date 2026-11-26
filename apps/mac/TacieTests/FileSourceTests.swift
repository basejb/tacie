import XCTest
@testable import Tacie

final class FileSourceTests: XCTestCase {
    private func temp(_ data: Data) throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("tacie-\(UUID().uuidString).wav")
        try data.write(to: url)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return url
    }

    /// 16 kHz mono 16-bit WAV of silence: 44-byte header, then the samples.
    private func silence(seconds: Double) -> Data {
        Utterancer.wav([Float](repeating: 0, count: Int(16_000 * seconds)))
    }

    /// 1 s of silence (the room level), 0.5 s of tone, then 0.3 s of silence: speech still open (700 ms needed) at the end.
    private func speechAtTheEnd() -> Data {
        let tone = (0..<8_000).map { i in Float(sin(Double(i) * 0.3)) * 0.5 }
        return Utterancer.wav([Float](repeating: 0, count: 16_000) + tone + [Float](repeating: 0, count: 4_800))
    }

    @MainActor
    private func play(_ data: Data, onEnd: Bool) async throws -> (ended: Bool, spoken: Int, rest: Bool) {
        let utterancer = Utterancer(detector: EnergyDetector())  // the tones here are not speech to Silero
        var spoken = 0
        utterancer.onUtterance = { _, _, _, _ in spoken += 1 }
        let source = FileSource(url: try temp(data))
        try await source.start(into: utterancer)
        if onEnd {
            let end = expectation(description: "end")
            source.onEnd = { end.fulfill() }
            await fulfillment(of: [end], timeout: 10)  // 2 s ran out on GitHub's macOS runner (2026-11-26)
        } else {
            try await Task.sleep(for: .seconds(2.5))
        }
        source.stop()
        return (onEnd, spoken, utterancer.cut() != nil)
    }

    /// The model stops a replayed meeting from this callback, so it has to come once the last chunk has been played.
    @MainActor
    func testOnEndFiresAfterTheLastChunk() async throws {
        let r = try await play(silence(seconds: 0.3), onEnd: true)
        XCTAssertTrue(r.ended)
    }

    /// With a handler, the last words stay in the utterancer for the meeting's own end to cut.
    @MainActor
    func testWithAHandlerTheSourceLeavesTheTailUnflushed() async throws {
        let r = try await play(speechAtTheEnd(), onEnd: true)
        XCTAssertEqual(r.spoken, 0)
        XCTAssertTrue(r.rest)
    }

    /// Without one, the old behaviour: the tail is flushed at the end.
    @MainActor
    func testWithoutAHandlerTheTailIsFlushed() async throws {
        let r = try await play(speechAtTheEnd(), onEnd: false)
        XCTAssertEqual(r.spoken, 1)
        XCTAssertFalse(r.rest)
    }

    /// An empty file has no chunk to play and still ends.
    @MainActor
    func testAnEmptyFileEnds() async throws {
        let r = try await play(Data(), onEnd: true)
        XCTAssertTrue(r.ended)
    }
}
