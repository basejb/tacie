import Foundation

/// Cuts a 16 kHz mono stream into utterances: speech starts after 60 ms of speech frames, ends after 700 ms
/// without, and is cut at 15 s. The detector is Silero VAD (SileroVAD.swift); the old energy rule stays as the
/// fallback. In a noisy room the energy rule sent a third of the speech and still fired on typing (docs/01
/// "발화 자르기(VAD) 실측"). A 30 s cut did not help scribe-v2 there, so the cut stays at 15 s.
final class Utterancer {
    static let sampleRate = 16_000.0
    private let frame = 320  // 20 ms
    private let preRoll = 10  // frames kept before speech starts (200 ms)
    private let startFrames = 3
    private let endFrames = 35  // 700 ms: shorter pauses split one question into fragments the gate cannot read
    private let maxFrames = 750
    private let minFrames = 20  // shorter than 400 ms is a cough, not a sentence
    private let detector: SpeechDetector

    init(detector: SpeechDetector? = nil) {
        if let detector {
            self.detector = detector
        } else if let silero = SileroDetector.bundled() {
            self.detector = silero
        } else {
            Log.info("vad: Silero weights not found, using the energy rule")
            self.detector = EnergyDetector()
        }
    }

    private var pending: [Float] = []
    /// The chunk being fed, not yet handed to `onSamples` (`handOver`).
    private var outbox: [Float] = []
    // per-sample energy of each side when the microphone and the call arrive separately (zeros otherwise)
    private var pendingMic: [Float] = []
    private var pendingCall: [Float] = []
    private var speechMic: Float = 0
    private var speechCall: Float = 0
    private var history: [[Float]] = []
    private var speech: [[Float]] = []
    private var loud = 0
    private var quiet = 0
    private var inSpeech = false
    private var frameIndex = 0
    private var speechStart = 0
    private var lastEnd: Int?

    /// WAV of one utterance, the silence before it in ms (nil for the first one), who spoke if it can tell, and where
    /// its first sample sits in the stream in ms (counted from the last resetClock; only fed samples advance it)
    var onUtterance: (Data, Int?, Speaker?, Int) -> Void = { _, _, _, _ in }
    /// Every sample fed, as received (the mixed sum for feedMix); the recording listens here. A chunk that ends an
    /// utterance comes in two parts: up to the cut before `onUtterance`, the rest after it.
    var onSamples: ([Float]) -> Void = { _ in }
    var onLevel: (Float) -> Void = { _ in }  // 0…1, once per 100 ms
    /// Someone started or stopped talking (the VAD went into or out of an utterance). Called on the audio queue.
    var onSpeaking: (Bool) -> Void = { _ in }

    private var levelAcc: Float = 0
    private var levelFrames = 0

    /// Not locked: call it before capture starts, or on the same queue that calls feed/feedMix.
    /// Starts the stream clock over at 0 and drops everything the VAD holds.
    func resetClock() {
        if inSpeech { onSpeaking(false) }
        frameIndex = 0
        pending = []
        outbox = []
        pendingMic = []
        pendingCall = []
        history = []
        speech = []
        speechStart = 0
        speechMic = 0
        speechCall = 0
        loud = 0
        quiet = 0
        lastEnd = nil
        inSpeech = false
        detector.reset()
    }

    func feed(_ samples: [Float]) {
        outbox = samples
        pending += samples
        pendingMic += [Float](repeating: 0, count: samples.count)
        pendingCall += [Float](repeating: 0, count: samples.count)
        drain()
    }

    /// Microphone and call sound side by side: summed for listening, compared to tell who spoke.
    func feedMix(mic: [Float], call: [Float]) {
        let n = min(mic.count, call.count)
        var out = [Float](repeating: 0, count: n)
        for i in 0..<n { out[i] = max(-1, min(1, mic[i] + call[i])) }
        outbox = out
        pending += out
        pendingMic += mic.prefix(n).map { $0 * $0 }
        pendingCall += call.prefix(n).map { $0 * $0 }
        drain()
    }

    /// Hands `onSamples` this chunk's samples up to the frame being processed: when that frame ends an utterance, the
    /// shadow's finalize then marks the boundary at the cut, and the rest of the chunk (85–100 ms of a 4096-sample tap)
    /// goes after it. Before 2026-11-17 the whole chunk went first, so the next speaker's first sound could land at the
    /// end of the line before (or a 15 s cut's last words at its end, not the next line's start).
    private func handOver(through end: Int) {
        let n = min(end, outbox.count)
        guard n > 0 else { return }
        onSamples(Array(outbox.prefix(n)))
        outbox.removeFirst(n)
    }

    private func drain() {
        while pending.count >= frame {
            let f = Array(pending.prefix(frame))
            let mic = pendingMic.prefix(frame).reduce(0, +)
            let call = pendingCall.prefix(frame).reduce(0, +)
            pending.removeFirst(frame)
            pendingMic.removeFirst(frame)
            pendingCall.removeFirst(frame)
            process(f)
            if inSpeech { speechMic += mic; speechCall += call }
        }
        handOver(through: outbox.count)
    }

    private func process(_ f: [Float]) {
        frameIndex += 1
        let rms = sqrt(f.reduce(0) { $0 + $1 * $1 } / Float(f.count))
        let db = 20 * log10(max(rms, 1e-7))
        levelAcc = max(levelAcc, min(max((db + 60) / 50, 0), 1))
        levelFrames += 1
        if levelFrames == 5 {
            onLevel(levelAcc)
            levelAcc = 0
            levelFrames = 0
        }
        let isLoud = detector.isSpeech(f)
        if inSpeech {
            speech.append(f)
            quiet = isLoud ? 0 : quiet + 1
            if quiet >= endFrames || speech.count >= maxFrames {
                // pending holds only this chunk's samples past this frame
                handOver(through: outbox.count - pending.count)
                flush()
            }
        } else {
            history.append(f)
            if history.count > preRoll { history.removeFirst() }
            loud = isLoud ? loud + 1 : 0
            if loud >= startFrames {
                inSpeech = true
                onSpeaking(true)
                speechStart = frameIndex - history.count
                speech = history
                history = []
                quiet = 0
                speechMic = 0
                speechCall = 0
            }
        }
    }

    func flush() {
        if let u = cut() { onUtterance(u.wav, u.gap, u.speaker, u.startMs) }
    }

    /// Ends the utterance in progress now and hands it back (nil if there is none worth sending). The meeting's end
    /// sends its last words this way itself, while it still has a session; onUtterance would arrive after it closed.
    func cut() -> (wav: Data, gap: Int?, speaker: Speaker?, startMs: Int)? {
        defer {
            if inSpeech { onSpeaking(false) }
            inSpeech = false; speech = []; loud = 0; quiet = 0
        }
        guard inSpeech, speech.count - quiet >= minFrames else { return nil }
        let gap = lastEnd.map { max(0, speechStart - $0) * 20 }
        lastEnd = frameIndex - quiet
        return (Self.wav(speech.flatMap { $0 }), gap, speaker, speechStart * 20)
    }

    /// The side with clearly more energy (3 dB) spoke; with an open speaker the mic also hears the call, a bit quieter.
    private var speaker: Speaker? {
        if speechCall > 2 * speechMic { return .them }
        if speechMic > 2 * speechCall { return .me }
        return nil
    }

    static func wav(_ samples: [Float]) -> Data {
        var pcm = Data(capacity: samples.count * 2)
        for s in samples {
            var v = Int16(max(-1, min(1, s)) * Float(Int16.max)).littleEndian
            withUnsafeBytes(of: &v) { pcm.append(contentsOf: $0) }
        }
        var d = Data()
        func u32(_ v: UInt32) { var x = v.littleEndian; withUnsafeBytes(of: &x) { d.append(contentsOf: $0) } }
        func u16(_ v: UInt16) { var x = v.littleEndian; withUnsafeBytes(of: &x) { d.append(contentsOf: $0) } }
        d.append(contentsOf: Array("RIFF".utf8)); u32(UInt32(36 + pcm.count)); d.append(contentsOf: Array("WAVE".utf8))
        d.append(contentsOf: Array("fmt ".utf8)); u32(16); u16(1); u16(1); u32(16_000); u32(32_000); u16(2); u16(16)
        d.append(contentsOf: Array("data".utf8)); u32(UInt32(pcm.count))
        d.append(pcm)
        return d
    }
}
