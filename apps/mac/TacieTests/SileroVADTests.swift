import XCTest
@testable import Tacie

/// The hand-written Silero VAD against the official ONNX model (fixtures from eval/silero_weights.py).
final class SileroVADTests: XCTestCase {
    private static let here = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
    private static let weights = here.appendingPathComponent("../Tacie/Resources/silero_vad.bin").standardized
    private static let fixtures = here.appendingPathComponent("Fixtures")

    private func model() throws -> SileroModel { try SileroModel(contentsOf: Self.weights) }

    private func speech() throws -> [Float] {
        let data = try Data(contentsOf: Self.fixtures.appendingPathComponent("silero_speech.wav")).dropFirst(44)
        return data.withUnsafeBytes { raw in
            (0..<(raw.count / 2)).map { Float(Int16(littleEndian: raw.loadUnaligned(fromByteOffset: $0 * 2, as: Int16.self))) / 32767 }
        }
    }

    func testMatchesOnnxRuntime() throws {
        let m = try model()
        let x = try speech()
        let want = try JSONDecoder().decode([Float].self, from: Data(contentsOf: Self.fixtures.appendingPathComponent("silero_speech_probs.json")))
        var ctx = [Float](repeating: 0, count: 64)
        var worst: Float = 0
        var i = 0
        for k in stride(from: 0, to: x.count - 511, by: 512) {
            let chunk = Array(x[k..<k + 512])
            worst = max(worst, abs(m.step(ctx + chunk) - want[i]))
            ctx = Array(chunk.suffix(64))
            i += 1
        }
        XCTAssertEqual(i, want.count)
        XCTAssertLessThan(worst, 1e-3)
    }

    func testSpeechIsHeardAndSilenceIsNot() throws {
        let d = SileroDetector(model: try model())
        let quiet = [Float](repeating: 0, count: 320)
        XCTAssertFalse((0..<100).map { _ in d.isSpeech(quiet) }.contains(true))
        let x = try speech()
        let heard = stride(from: 0, to: x.count - 319, by: 320).map { d.isSpeech(Array(x[$0..<$0 + 320])) }
        XCTAssertGreaterThan(heard.filter { $0 }.count, heard.count / 3)
    }

    func testToneAndNoiseAreNotSpeech() throws {  // the energy rule fires on both
        let d = SileroDetector(model: try model())
        var seed: UInt32 = 1
        func noise() -> Float { seed = seed &* 1_664_525 &+ 1_013_904_223; return Float(Int32(bitPattern: seed)) / Float(Int32.max) }
        let tone = (0..<16_000 * 2).map { 0.14 * sin(2 * .pi * 440 * Float($0) / 16_000) }
        let hiss = (0..<16_000 * 2).map { _ in 0.05 * noise() }
        for signal in [tone, hiss] {
            d.reset()
            let heard = stride(from: 0, to: signal.count - 319, by: 320).map { d.isSpeech(Array(signal[$0..<$0 + 320])) }
            XCTAssertFalse(heard.contains(true))
        }
    }

    func testResetForgetsTheStream() throws {
        let m = try model()
        let x = try speech()
        let window = [Float](repeating: 0, count: 64) + Array(x[8_000..<8_512])
        let first = m.step(window)
        _ = m.step(window)
        m.resetState()
        XCTAssertEqual(m.step(window), first, accuracy: 1e-6)
    }

    func testUtterancerCutsTheSpeech() throws {
        let u = Utterancer(detector: SileroDetector(model: try model()))
        var heard: [Data] = []
        u.onUtterance = { wav, _, _, _ in heard.append(wav) }
        let quiet = [Float](repeating: 0, count: 16_000)
        u.feed(quiet); u.feed(try speech()); u.feed(quiet)
        XCTAssertGreaterThanOrEqual(heard.count, 1)
    }

    func testBrokenWeightsAreRefused() {
        XCTAssertThrowsError(try SileroModel(data: Data("SVAD".utf8) + Data(count: 8)))
        XCTAssertThrowsError(try SileroModel(data: Data("nope".utf8)))
    }
}
