import AVFoundation

/// The meeting practice's voice: a question spoken by a recorded voice (or the system's voice in the line's language
/// for a line without a recording), written as the 16 kHz mono 16-bit WAV that `FileSource` replays into a meeting, and played
/// aloud at the same time so the user hears what the island hears. No microphone needed (a Mac mini has none).
@MainActor
final class PracticeVoice {
    private let synth = AVSpeechSynthesizer()
    private var player: AVAudioPlayer?

    /// What the meeting is fed (16 kHz WAV) and what is played aloud (the recording itself, when there is one).
    struct Rendered: Sendable {
        let feed: URL
        let play: URL
    }

    /// Lines with a recorded voice (ElevenLabs, a calm voice that does not stand out in a meeting; 2026-11-01). The
    /// keys are the exact spoken lines: the sample's `spoken` question and OnboardingModel's candidates as spoken
    /// (`SampleLanguage.spokenLine`). English lines use `<name>.en.mp3`; until a file is there, the system voice reads it.
    static let recorded: [String: String] = [
        "음, 재시도 다 실패한 이벤트는 어디로 가요? 혹시 아는 분 있어요?": "practice-sample",
        "음, 이 프로젝트는 어떻게 실행해요? 혹시 아는 분 있어요?": "practice-run",
        "음, 테스트는 어떻게 돌려요? 혹시 아는 분 있어요?": "practice-test",
        "음, 설치는 어떻게 해요? 혹시 아는 분 있어요?": "practice-install",
        "음, 배포는 어떻게 해요? 혹시 아는 분 있어요?": "practice-deploy",
        "음, 환경 변수는 어디서 설정해요? 혹시 아는 분 있어요?": "practice-env",
        "음, API 키는 어디에 둬요? 혹시 아는 분 있어요?": "practice-apikey",
        "Um, where do events go after every retry fails? Does anyone know?": "practice-sample.en",
        "Um, how do I run this project? Does anyone know?": "practice-run.en",
        "Um, how do I run the tests? Does anyone know?": "practice-test.en",
        "Um, how do I install it? Does anyone know?": "practice-install.en",
        "Um, how do I deploy? Does anyone know?": "practice-deploy.en",
        "Um, where are environment variables set? Does anyone know?": "practice-env.en",
        "Um, where do API keys go? Does anyone know?": "practice-apikey.en",
    ]

    func render(_ text: String) async -> Rendered? {
        if let name = Self.recorded[text], let mp3 = Bundle.main.url(forResource: name, withExtension: "mp3") {
            let feed = await Task.detached { Self.feed(from: mp3) }.value
            if let feed { return Rendered(feed: feed, play: mp3) }
        }
        return await speak(text).map { Rendered(feed: $0, play: $0) }
    }

    /// The recording decoded to mono and written as the meeting's WAV.
    nonisolated static func feed(from url: URL) -> URL? {
        guard let file = try? AVAudioFile(forReading: url),
              let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length)),
              (try? file.read(into: buffer)) != nil, let ch = buffer.floatChannelData else { return nil }
        let n = Int(buffer.frameLength), channels = Int(buffer.format.channelCount)
        var mono = [Float](repeating: 0, count: n)
        for c in 0..<channels { for i in 0..<n { mono[i] += ch[c][i] / Float(channels) } }
        return write(resample(mono, from: buffer.format.sampleRate, to: 16_000))
    }

    /// A little silence around the speech, then the temporary file.
    nonisolated private static func write(_ speech: [Float]) -> URL? {
        guard !speech.isEmpty else { return nil }
        let pcm = [Float](repeating: 0, count: 3_200) + speech + [Float](repeating: 0, count: 4_800)
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("tacie-practice-\(UUID().uuidString).wav")
        do { try wav(pcm).write(to: url) } catch { return nil }
        return url
    }

    /// The question as a WAV in the temporary folder, with a little silence around it. The practice is not transcribed
    /// (its answer is ready ahead), so the file ends soon after the voice: the answer comes up when it ends.
    private func speak(_ text: String) async -> URL? {
        let u = AVSpeechUtterance(string: text)
        u.voice = AVSpeechSynthesisVoice(language: SampleLanguage.of(text).voice)
        u.rate = AVSpeechUtteranceDefaultSpeechRate
        let box = Collected()
        await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
            synth.write(u) { buffer in
                guard let pcm = buffer as? AVAudioPCMBuffer else { return }
                if pcm.frameLength == 0 {  // the empty buffer that ends the speech
                    if box.finish() { done.resume() }
                    return
                }
                box.add(pcm)
            }
        }
        return Self.write(Self.resample(box.samples, from: box.rate, to: 16_000))
    }

    func play(_ url: URL) {
        player = try? AVAudioPlayer(contentsOf: url)
        player?.play()
    }

    func stop() { player?.stop() }

    /// Linear interpolation is enough for speech going to the recogniser.
    nonisolated static func resample(_ x: [Float], from rate: Double, to target: Double) -> [Float] {
        guard rate > 0, !x.isEmpty else { return [] }
        if rate == target { return x }
        let n = Int(Double(x.count) * target / rate)
        return (0..<n).map { i in
            let t = Double(i) * rate / target
            let a = Int(t)
            let b = min(a + 1, x.count - 1)
            let f = Float(t - Double(a))
            return x[a] * (1 - f) + x[b] * f
        }
    }

    /// A 44-byte header and 16-bit samples, the layout `FileSource` reads.
    nonisolated static func wav(_ samples: [Float]) -> Data {
        var d = Data()
        func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
        func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
        let bytes = UInt32(samples.count * 2)
        d.append(contentsOf: Array("RIFF".utf8)); u32(36 + bytes)
        d.append(contentsOf: Array("WAVEfmt ".utf8)); u32(16); u16(1); u16(1); u32(16_000); u32(32_000); u16(2); u16(16)
        d.append(contentsOf: Array("data".utf8)); u32(bytes)
        for s in samples { u16(UInt16(bitPattern: Int16(max(-1, min(1, s)) * Float(Int16.max)))) }
        return d
    }

    /// The synthesizer's buffers, gathered on its own thread.
    private final class Collected: @unchecked Sendable {
        private let lock = NSLock()
        private var out: [Float] = []
        private var sampleRate = 0.0
        private var ended = false

        func add(_ pcm: AVAudioPCMBuffer) {
            let n = Int(pcm.frameLength)
            var chunk = [Float](repeating: 0, count: n)
            if let f = pcm.floatChannelData?[0] {
                for i in 0..<n { chunk[i] = f[i] }
            } else if let s = pcm.int16ChannelData?[0] {
                for i in 0..<n { chunk[i] = Float(s[i]) / Float(Int16.max) }
            }
            lock.lock(); out += chunk; sampleRate = pcm.format.sampleRate; lock.unlock()
        }

        /// True only the first time: the end may be reported more than once.
        func finish() -> Bool {
            lock.lock(); defer { lock.unlock() }
            if ended { return false }
            ended = true
            return true
        }

        var samples: [Float] { lock.lock(); defer { lock.unlock() }; return out }
        var rate: Double { lock.lock(); defer { lock.unlock() }; return sampleRate }
    }
}
