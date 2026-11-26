import Accelerate
import Foundation

/// Says, every 20 ms frame, whether someone is talking. Utterancer turns these answers into utterances.
protocol SpeechDetector: AnyObject {
    func isSpeech(_ frame: [Float]) -> Bool
    /// Forgets everything heard so far (a new stream starts).
    func reset()
}

/// The old rule, kept as the fallback when the Silero weights cannot be read: 9 dB over the quiet level of the
/// last 3 s, never below -42 dBFS. In a noisy room it drops most speech and still fires on typing (docs/01
/// "발화 자르기(VAD) 실측").
final class EnergyDetector: SpeechDetector {
    private let minThresholdDB: Float = -42
    private let overFloorDB: Float = 9
    private var recentDB: [Float] = []  // last 3 s of frame levels

    func isSpeech(_ f: [Float]) -> Bool {
        let rms = sqrt(f.reduce(0) { $0 + $1 * $1 } / Float(max(f.count, 1)))
        let db = 20 * log10(max(rms, 1e-7))
        recentDB.append(db)
        if recentDB.count > 150 { recentDB.removeFirst() }
        let floor = recentDB.sorted()[recentDB.count / 10]
        return db > max(minThresholdDB, floor + overFloorDB)
    }

    func reset() {}  // the room did not change: keep its quiet level
}

/// Silero VAD v5 (MIT, github.com/snakers4/silero-vad) run with Accelerate: 512-sample windows (32 ms) with the
/// 64 samples before each window as context, as the official ONNX model wants. A frame is speech once the
/// probability reaches 0.5 and stays speech until it falls under 0.35 (the library's own hysteresis).
/// The weights are `silero_vad.bin`, written from the ONNX file by eval/silero_weights.py.
final class SileroDetector: SpeechDetector {
    static let window = 512
    static let context = 64
    private let model: SileroModel
    private var pending: [Float] = []
    private var ctx = [Float](repeating: 0, count: SileroDetector.context)
    private var speaking = false
    private(set) var lastProbability: Float = 0
    let onThreshold: Float = 0.5
    let offThreshold: Float = 0.35

    init(model: SileroModel) { self.model = model }

    /// The detector of the app bundle, or nil if the weights are missing or broken.
    static func bundled() -> SileroDetector? {
        guard let url = Bundle.main.url(forResource: "silero_vad", withExtension: "bin"),
              let model = try? SileroModel(contentsOf: url) else { return nil }
        return SileroDetector(model: model)
    }

    func isSpeech(_ frame: [Float]) -> Bool {
        pending += frame
        while pending.count >= Self.window {
            let chunk = Array(pending.prefix(Self.window))
            pending.removeFirst(Self.window)
            lastProbability = model.step(ctx + chunk)
            ctx = Array(chunk.suffix(Self.context))
            if lastProbability >= onThreshold { speaking = true } else if lastProbability < offThreshold { speaking = false }
        }
        return speaking
    }

    func reset() {
        pending = []
        ctx = [Float](repeating: 0, count: Self.context)
        speaking = false
        lastProbability = 0
        model.resetState()
    }
}

/// The 16 kHz branch of the Silero VAD v5 network: STFT as a strided convolution, four 1-D convolutions with
/// ReLU, one LSTM cell, a 1x1 convolution and a sigmoid. Keeps the LSTM state between windows.
final class SileroModel {
    struct BadWeights: Error {}

    private let basis: [Float]  // [258, 256]: 129 real rows, then 129 imaginary rows
    private let enc: [(w: [Float], b: [Float], out: Int, inp: Int, stride: Int)]
    private let wih: [Float], whh: [Float], bias: [Float]  // [512, 128] each; bias = bias_ih + bias_hh
    private let decW: [Float], decB: Float
    private var h = [Float](repeating: 0, count: 128)
    private var c = [Float](repeating: 0, count: 128)

    /// "SVAD", tensor count, then per tensor: element count and little-endian float32 values, in this order:
    /// stft basis, encoder 0–3 weight and bias, rnn weight_ih, weight_hh, bias_ih, bias_hh, decoder weight, bias.
    convenience init(contentsOf url: URL) throws {
        try self.init(data: try Data(contentsOf: url))
    }

    init(data: Data) throws {
        var at = 0
        func u32() throws -> Int {
            guard at + 4 <= data.count else { throw BadWeights() }
            let v = data[data.startIndex + at..<data.startIndex + at + 4].withUnsafeBytes { $0.loadUnaligned(as: UInt32.self) }
            at += 4
            return Int(UInt32(littleEndian: v))
        }
        guard data.count > 8, data.prefix(4) == Data("SVAD".utf8) else { throw BadWeights() }
        at = 4
        guard try u32() == 15 else { throw BadWeights() }
        let sizes = [258 * 256, 128 * 129 * 3, 128, 64 * 128 * 3, 64, 64 * 64 * 3, 64, 128 * 64 * 3, 128,
                     512 * 128, 512 * 128, 512, 512, 128, 1]
        var t: [[Float]] = []
        for want in sizes {
            guard try u32() == want, at + want * 4 <= data.count else { throw BadWeights() }
            let slice = data[data.startIndex + at..<data.startIndex + at + want * 4]
            t.append(slice.withUnsafeBytes { raw in (0..<want).map { Float(bitPattern: UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: $0 * 4, as: UInt32.self))) } })
            at += want * 4
        }
        basis = t[0]
        enc = [(t[1], t[2], 128, 129, 1), (t[3], t[4], 64, 128, 2), (t[5], t[6], 64, 64, 2), (t[7], t[8], 128, 64, 1)]
        wih = t[9]; whh = t[10]
        bias = zip(t[11], t[12]).map(+)
        decW = t[13]; decB = t[14][0]
    }

    func resetState() {
        h = [Float](repeating: 0, count: 128)
        c = [Float](repeating: 0, count: 128)
    }

    /// Speech probability of one window: 64 context samples followed by 512 new ones.
    func step(_ x: [Float]) -> Float {
        precondition(x.count == SileroDetector.context + SileroDetector.window)
        // reflect-pad 64 samples on the right (the edge sample is not repeated)
        var padded = x
        for i in 0..<64 { padded.append(x[x.count - 2 - i]) }
        // STFT: 256-sample frames every 128 samples -> 4 frames, magnitude of 129 bins
        let frames = (padded.count - 256) / 128 + 1
        var cols = [Float](repeating: 0, count: 256 * frames)  // [256, frames]
        for t in 0..<frames { for k in 0..<256 { cols[k * frames + t] = padded[t * 128 + k] } }
        let spec = matmul(basis, rows: 258, inner: 256, cols, frames)
        var y = [Float](repeating: 0, count: 129 * frames)
        for i in 0..<(129 * frames) { y[i] = sqrt(spec[i] * spec[i] + spec[129 * frames + i] * spec[129 * frames + i]) }
        var length = frames
        for layer in enc {
            (y, length) = conv3(y, channels: layer.inp, length: length, w: layer.w, b: layer.b, out: layer.out, stride: layer.stride)
            for i in y.indices where y[i] < 0 { y[i] = 0 }
        }
        // one time step left: the 128 channels of it
        let v = (0..<128).map { y[$0 * length] }
        var gates = bias
        let a = matmul(wih, rows: 512, inner: 128, v, 1)
        let r = matmul(whh, rows: 512, inner: 128, h, 1)
        for i in 0..<512 { gates[i] += a[i] + r[i] }
        func sig(_ z: Float) -> Float { 1 / (1 + exp(-z)) }
        for j in 0..<128 {
            let ig = sig(gates[j]), fg = sig(gates[128 + j]), gg = tanh(gates[256 + j]), og = sig(gates[384 + j])
            c[j] = fg * c[j] + ig * gg
            h[j] = og * tanh(c[j])
        }
        var out = decB
        for j in 0..<128 { out += decW[j] * max(h[j], 0) }
        return sig(out)
    }

    /// Kernel 3, padding 1. `x` is [channels, length]; returns [out, length'] and length'.
    private func conv3(_ x: [Float], channels: Int, length: Int, w: [Float], b: [Float], out: Int, stride: Int) -> ([Float], Int) {
        let outLen = (length + 2 - 3) / stride + 1
        var cols = [Float](repeating: 0, count: channels * 3 * outLen)  // [channels * 3, outLen]
        for ch in 0..<channels {
            for k in 0..<3 {
                for t in 0..<outLen {
                    let src = t * stride + k - 1
                    if src >= 0 && src < length { cols[(ch * 3 + k) * outLen + t] = x[ch * length + src] }
                }
            }
        }
        var y = matmul(w, rows: out, inner: channels * 3, cols, outLen)
        for o in 0..<out { for t in 0..<outLen { y[o * outLen + t] += b[o] } }
        return (y, outLen)
    }

    /// Row-major [rows, inner] x [inner, n].
    private func matmul(_ a: [Float], rows: Int, inner: Int, _ b: [Float], _ n: Int) -> [Float] {
        var y = [Float](repeating: 0, count: rows * n)
        a.withUnsafeBufferPointer { pa in
            b.withUnsafeBufferPointer { pb in
                y.withUnsafeMutableBufferPointer { py in
                    vDSP_mmul(pa.baseAddress!, 1, pb.baseAddress!, 1, py.baseAddress!, 1,
                              vDSP_Length(rows), vDSP_Length(n), vDSP_Length(inner))
                }
            }
        }
        return y
    }
}
