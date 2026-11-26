import AVFoundation
import ScreenCaptureKit

/// Everything the Mac plays (Zoom, Meet, Slack huddles…) through ScreenCaptureKit, as 16 kHz mono. Before macOS 14.2
/// only; newer Macs use the process tap (`CallAudio.make`).
/// Needs the "Screen & System Audio Recording" permission; Tacie's own sound is left out.
final class SystemAudioSource: NSObject, SoundSource, SCStreamOutput, SCStreamDelegate {
    var paused = false
    var onSamples: (([Float]) -> Void)?  // set by MixSource; otherwise samples go straight to the utterancer
    var onError: (String) -> Void = { _ in }
    private var stream: SCStream?
    private weak var utterancer: Utterancer?
    private let queue = DispatchQueue(label: "tacie.system-audio")
    private var loggedFormat = false
    private let dump: FileHandle? = {  // `--dump-audio path`: raw float32 mono as received, for debugging the VAD
        guard let path = LaunchArgs.value("--dump-audio") else { return nil }
        FileManager.default.createFile(atPath: path, contents: nil)
        return FileHandle(forWritingAtPath: path)
    }()

    func start(into utterancer: Utterancer) async throws {
        self.utterancer = utterancer
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        guard let display = content.displays.first else {
            throw NSError(domain: "tacie", code: 2, userInfo: [NSLocalizedDescriptionKey: String(localized: "화면 정보를 읽지 못했어요.")])
        }
        let cfg = SCStreamConfiguration()
        cfg.capturesAudio = true
        cfg.excludesCurrentProcessAudio = true
        cfg.sampleRate = Int(Utterancer.sampleRate)
        cfg.channelCount = 1
        cfg.width = 2  // video is required by the API; keep it tiny and rare
        cfg.height = 2
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        let s = SCStream(filter: SCContentFilter(display: display, excludingWindows: []), configuration: cfg, delegate: self)
        try s.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue)
        try await s.startCapture()
        stream = s
    }

    func stop() {
        let s = stream
        stream = nil
        Task { try? await s?.stopCapture() }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer buffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, !paused else { return }
        if !loggedFormat, let d = CMSampleBufferGetFormatDescription(buffer), let a = CMAudioFormatDescriptionGetStreamBasicDescription(d)?.pointee {
            loggedFormat = true
            Log.info("system audio: \(a.mSampleRate) Hz, \(a.mChannelsPerFrame) ch, flags \(a.mFormatFlags), \(CMSampleBufferGetNumSamples(buffer)) frames/buffer")
        }
        guard let samples = Self.floats(buffer), !samples.isEmpty else { return }
        if let dump { samples.withUnsafeBufferPointer { dump.write(Data(buffer: $0)) } }
        if let onSamples {
            onSamples(samples)
        } else {
            // only the call: everything heard is the other side
            let silence = [Float](repeating: 0, count: samples.count)
            DispatchQueue.main.async { [weak self] in self?.utterancer?.feedMix(mic: silence, call: samples) }
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        onError(String(localized: "시스템 소리 받기가 멈췄어요: \(error.localizedDescription)"))
    }

    /// Mono float samples from an audio sample buffer (averages channels if there are several).
    private static func floats(_ buffer: CMSampleBuffer) -> [Float]? {
        guard let desc = CMSampleBufferGetFormatDescription(buffer),
              let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(desc)?.pointee else { return nil }
        var list = AudioBufferList()
        var block: CMBlockBuffer?
        let status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            buffer, bufferListSizeNeededOut: nil, bufferListOut: &list, bufferListSize: MemoryLayout<AudioBufferList>.size,
            blockBufferAllocator: nil, blockBufferMemoryAllocator: nil, flags: 0, blockBufferOut: &block)
        guard status == noErr, asbd.mFormatFlags & kAudioFormatFlagIsFloat != 0 else { return nil }
        let buffers = UnsafeMutableAudioBufferListPointer(&list)
        let frames = CMSampleBufferGetNumSamples(buffer)
        var out = [Float](repeating: 0, count: frames)
        for b in buffers {
            guard let data = b.mData else { continue }
            let ptr = data.assumingMemoryBound(to: Float.self)
            let channels = Int(b.mNumberChannels)
            for i in 0..<frames {
                var sum: Float = 0
                for c in 0..<channels { sum += ptr[i * channels + c] }
                out[i] += sum / Float(channels)
            }
        }
        if buffers.count > 1 { out = out.map { $0 / Float(buffers.count) } }
        return out
    }
}

/// Microphone + system sound into one stream (remote meetings with headphones: others come through
/// the system, you through the mic). Each side buffers; every 50 ms the mixer emits what both have.
/// Sendable for the timer's block: the buffers are shared with the capture threads under `lock`, and everything
/// else is touched on the main thread only.
final class MixSource: SoundSource, MicWatching, @unchecked Sendable {
    private let mic = MicSource()
    private let system = CallAudio.make()
    private var micBuf: [Float] = []
    private var sysBuf: [Float] = []
    private var timer: Timer?
    private weak var utterancer: Utterancer?
    private let lock = NSLock()
    var paused = false {
        didSet { mic.paused = paused; system.paused = paused }
    }
    var onInput: (Bool) -> Void = { _ in }

    func start(into utterancer: Utterancer) async throws {
        mic.onSamples = { [weak self] s in self?.append(s, toMic: true) }
        mic.onInput = { [weak self] ok in self?.onInput(ok) }
        system.onSamples = { [weak self] s in self?.append(s, toMic: false) }
        try await system.start(into: utterancer)
        // no microphone (a Mac mini with the AirPods off): the call alone until one appears; mix() pads the missing side.
        // A refused microphone is the same for a remote meeting: the other side is still worth hearing.
        do {
            try await mic.start(into: utterancer, waitForDevice: true)
        } catch let e as NSError where e.domain == "tacie" && e.code == MicSource.notAllowedCode {
            Log.info("microphone not allowed: listening to the call only")
            let say = onInput
            await MainActor.run { say(false) }
        }
        self.utterancer = utterancer
        await MainActor.run {
            timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in self?.mix() }
        }
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        mic.stop()
        system.stop()
    }

    private func append(_ s: [Float], toMic: Bool) {
        lock.lock()
        if toMic { micBuf += s } else { sysBuf += s }
        lock.unlock()
    }

    /// Emit the overlap; if one side has gone quiet for over 300 ms (no data at all), pad it with silence.
    private func mix() {
        guard let utterancer else { return }
        lock.lock()
        let dropped = Self.trimDrift(&micBuf, &sysBuf)
        if dropped > 0 { Log.info("mix: one side ran \(dropped) samples ahead, dropped its oldest") }
        let lag = 4_800
        var n = min(micBuf.count, sysBuf.count)
        if n == 0, max(micBuf.count, sysBuf.count) > lag {
            n = max(micBuf.count, sysBuf.count)
            micBuf += [Float](repeating: 0, count: n - micBuf.count)
            sysBuf += [Float](repeating: 0, count: n - sysBuf.count)
        }
        guard n > 0 else { lock.unlock(); return }
        let mic = Array(micBuf.prefix(n))
        let call = Array(sysBuf.prefix(n))
        micBuf.removeFirst(n)
        sysBuf.removeFirst(n)
        lock.unlock()
        utterancer.feedMix(mic: mic, call: call)
    }

    /// Two seconds: more than any real lag between the two clocks.
    static let maxDrift = 32_000

    /// When one side holds more than `maxDrift` samples beyond the other, its oldest go, down to 300 ms ahead.
    /// Both sides are 16 kHz, so this never happens unless one is converted at the wrong rate; then it keeps the
    /// recording roughly in step and the buffer from growing for the whole meeting. Returns the samples dropped.
    @discardableResult
    static func trimDrift(_ a: inout [Float], _ b: inout [Float]) -> Int {
        let keep = 4_800
        if a.count - b.count > maxDrift { let d = a.count - b.count - keep; a.removeFirst(d); return d }
        if b.count - a.count > maxDrift { let d = b.count - a.count - keep; b.removeFirst(d); return d }
        return 0
    }
}
