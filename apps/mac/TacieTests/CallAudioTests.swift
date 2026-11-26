import AudioToolbox
import XCTest
@testable import Tacie

final class CallAudioTests: XCTestCase {
    func testMonoAveragesInterleavedChannels() {
        var samples: [Float] = [1, 0, 0.5, 0.5, -1, 1]  // three stereo frames
        let out = samples.withUnsafeMutableBytes { raw -> [Float] in
            var list = AudioBufferList(mNumberBuffers: 1, mBuffers: AudioBuffer(mNumberChannels: 2, mDataByteSize: UInt32(raw.count), mData: raw.baseAddress))
            return CallAudio.mono(UnsafeMutableAudioBufferListPointer(&list), channels: 2)
        }
        XCTAssertEqual(out, [0.5, 0.5, 0])
    }

    func testRMS() {
        XCTAssertEqual(CallAudio.rms([]), 0)
        XCTAssertEqual(CallAudio.rms([0.5, -0.5]), 0.5, accuracy: 1e-6)
    }

    func testTheTapDeviceIsMarkedAsOurs() {
        XCTAssertTrue("\(CallAudio.aggregatePrefix)1234".hasPrefix("ai.tacie.call-sound."))
    }
}

/// AirPods drop the output to 24 kHz when their microphone opens, under a running tap (one meeting came
/// out twice as fast). The tap must follow the device's real rate.
final class CallRateTests: XCTestCase {
    private func sine(_ rate: Double, seconds: Double) -> [Float] {
        (0..<Int(rate * seconds)).map { Float(sin(2 * .pi * 440 * Double($0) / rate)) * 0.5 }
    }

    /// One second of sound in, in 10 ms device buffers; samples out.
    private func run(_ r: CallResampler, rate: Double) -> Int {
        let s = sine(rate, seconds: 1)
        let chunk = Int(rate / 100)
        return stride(from: 0, to: s.count, by: chunk).reduce(0) { $0 + (r.convert(Array(s[$1..<min($1 + chunk, s.count)]))?.count ?? 0) }
    }

    func testOneSecondIsSixteenThousandSamplesAtEitherRate() {
        let r = CallResampler()
        XCTAssertTrue(r.set(rate: 48_000))
        XCTAssertEqual(Double(run(r, rate: 48_000)), 16_000, accuracy: 200)
        XCTAssertTrue(r.set(rate: 24_000))   // the AirPods switch
        XCTAssertEqual(Double(run(r, rate: 24_000)), 16_000, accuracy: 200)
        XCTAssertFalse(r.set(rate: 24_000))  // same rate: the converter stays
    }

    func testClockMeasuresTheDeviceRate() {
        var c = DeviceClock()
        let tps = 1_000_000.0   // host ticks per second in this test
        var sample = 1000.0, host: UInt64 = 5_000_000
        var got: [Double] = []
        func step(_ rate: Double) {   // 10 ms of device time
            sample += rate / 100; host += 10_000
            if let r = c.observe(sampleTime: sample, hostTime: host, ticksPerSecond: tps) { got.append(r) }
        }
        _ = c.observe(sampleTime: sample, hostTime: host, ticksPerSecond: tps)
        for _ in 0..<100 { step(48_000) }
        for _ in 0..<100 { step(24_000) }
        XCTAssertEqual(got, [48_000, 48_000, 24_000, 24_000])
    }

    func testClockIgnoresARestartedDeviceClock() {
        var c = DeviceClock()
        _ = c.observe(sampleTime: 1_000_000, hostTime: 1_000_000, ticksPerSecond: 1_000_000)
        XCTAssertNil(c.observe(sampleTime: 10, hostTime: 1_600_000, ticksPerSecond: 1_000_000))       // went back: restart
        XCTAssertNil(c.observe(sampleTime: 30_000, hostTime: 2_200_000, ticksPerSecond: 1_000_000))   // 50 kHz: no standard rate
    }

    func testMixDropsTheOldestOfARunawaySide() {
        var mic = [Float](repeating: 1, count: 50_000)
        var call = [Float](repeating: 0, count: 10_000)
        XCTAssertEqual(MixSource.trimDrift(&mic, &call), 50_000 - 10_000 - 4_800)
        XCTAssertEqual(mic.count, 14_800)
        var a = [Float](repeating: 0, count: 20_000), b = [Float](repeating: 0, count: 0)
        XCTAssertEqual(MixSource.trimDrift(&a, &b), 0)   // within two seconds: left alone
    }
}
