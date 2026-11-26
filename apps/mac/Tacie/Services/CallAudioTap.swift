import AVFoundation
import AudioToolbox
import CoreAudio

/// The call's sound as 16 kHz mono, whichever way it is captured.
protocol CallAudioSource: SoundSource {
    var onSamples: (([Float]) -> Void)? { get set }
    var onError: (String) -> Void { get set }
}

extension SystemAudioSource: CallAudioSource {}

enum CallAudio {
    /// `defaults write ai.tacie.app callAudio.screenCapture -bool YES` goes back to ScreenCaptureKit.
    static let screenCaptureKey = "callAudio.screenCapture"
    /// The tap's private aggregate device has input channels, so this process sees it among its audio devices. Its UID
    /// starts with this, and the microphone code must never take it for a microphone (on a Mac without one it did,
    /// and starting an engine on it hung the meeting start).
    static let aggregatePrefix = "ai.tacie.call-sound."

    /// A Core Audio process tap on macOS 14.2+ (asks only for system audio recording, no relaunch, docs/11 8절);
    /// ScreenCaptureKit before that.
    static func make() -> CallAudioSource {
        if #available(macOS 14.2, *), !UserDefaults.standard.bool(forKey: screenCaptureKey) { return ProcessTapSource() }
        return SystemAudioSource()
    }

    /// Posted (main queue) the first time a tap hears real sound: the permission is on after all.
    static let heard = Notification.Name("tacie.callAudioHeard")

    /// Whether remote meetings capture through the tap (the one that hands over silence when the permission is off).
    static var usesTap: Bool {
        if #available(macOS 14.2, *) { return !UserDefaults.standard.bool(forKey: screenCaptureKey) }
        return false
    }

    /// The last thing known about the meeting app's sound: a probe's result, or real sound heard in a meeting.
    static var lastState: CallAudioState {
        (UserDefaults.standard.string(forKey: CallAudioProbe.lastKey)?.split(separator: " ").first)
            .flatMap { CallAudioState(rawValue: String($0)) } ?? .unknown
    }

    static func record(_ state: CallAudioState) {
        UserDefaults.standard.set("\(state.rawValue) \(ISO8601DateFormatter().string(from: Date()))", forKey: CallAudioProbe.lastKey)
    }

    /// Forgets the answer to macOS's permission prompt, so the next probe asks again (one refusal is otherwise final:
    /// macOS never shows the prompt twice).
    static func resetPermission() {
        // never another app's answer (the official app's, when this is the open edition): no id, no reset
        guard let id = Bundle.main.bundleIdentifier else { return }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/tccutil")
        p.arguments = ["reset", "AudioCapture", id]
        try? p.run()
        p.waitUntilExit()
        UserDefaults.standard.removeObject(forKey: CallAudioProbe.lastKey)
        Log.info("call audio permission reset (\(p.terminationStatus))")
    }

    static func mono(_ data: UnsafeMutableAudioBufferListPointer, channels: Int) -> [Float] {
        var out: [Float] = []
        for b in data {
            guard let p = b.mData?.assumingMemoryBound(to: Float.self) else { continue }
            let ch = max(1, Int(b.mNumberChannels))
            let frames = Int(b.mDataByteSize) / MemoryLayout<Float>.size / ch
            if out.isEmpty { out = [Float](repeating: 0, count: frames) }
            for i in 0..<min(frames, out.count) {
                var sum: Float = 0
                for c in 0..<ch { sum += p[i * ch + c] }
                out[i] += sum / Float(ch)
            }
        }
        if data.count > 1 { out = out.map { $0 / Float(data.count) } }
        return out
    }

    static func rms(_ s: [Float]) -> Float {
        guard !s.isEmpty else { return 0 }
        return (s.reduce(0) { $0 + $1 * $1 } / Float(s.count)).squareRoot()
    }
}

/// Turns the tap's mono sound at the device's rate into 16 kHz, and follows the rate when it changes.
///
/// The rate must be the aggregate device's, not the tap's: the device hands over frames at its own clock. AirPods
/// switch the output from 48 kHz to 24 kHz the moment their microphone opens (the call profile), after the tap was
/// set up. Converting those frames as 48 kHz made half the samples, so the call sounded twice as fast and high in
/// recordings and the transcript (a meeting recorded with AirPods).
final class CallResampler {
    private let outFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Utterancer.sampleRate, channels: 1,
                                          interleaved: false)!
    private var converter: AVAudioConverter?
    private var inFormat: AVAudioFormat?

    var rate: Double { inFormat?.sampleRate ?? 0 }

    /// Rebuilds the converter when `rate` differs from the current one. Returns whether it changed.
    @discardableResult
    func set(rate: Double) -> Bool {
        guard rate > 0, rate != self.rate,
              let mono = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: rate, channels: 1, interleaved: false),
              let c = AVAudioConverter(from: mono, to: outFormat) else { return false }
        inFormat = mono
        converter = c
        return true
    }

    func convert(_ mono: [Float]) -> [Float]? {
        guard let converter, let inFormat, !mono.isEmpty,
              let input = AVAudioPCMBuffer(pcmFormat: inFormat, frameCapacity: AVAudioFrameCount(mono.count)) else { return nil }
        input.frameLength = AVAudioFrameCount(mono.count)
        mono.withUnsafeBufferPointer { input.floatChannelData![0].update(from: $0.baseAddress!, count: mono.count) }
        let capacity = AVAudioFrameCount(Double(mono.count) * outFormat.sampleRate / inFormat.sampleRate) + 16
        guard let output = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: capacity) else { return nil }
        var fed = false
        var err: NSError?
        converter.convert(to: output, error: &err) { _, status in
            if fed { status.pointee = .noDataNow; return nil }
            fed = true
            status.pointee = .haveData
            return input
        }
        if err != nil { return nil }
        return Array(UnsafeBufferPointer(start: output.floatChannelData![0], count: Int(output.frameLength)))
    }
}

/// The rate a device really runs at, from its timestamps: device frames over host seconds, every half second,
/// snapped to a standard rate. A window that matches no standard rate (the device clock restarting) gives nothing.
struct DeviceClock {
    static let rates: [Double] = [8_000, 11_025, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 88_200, 96_000]
    static let hostTicksPerSecond: Double = {
        var info = mach_timebase_info_data_t()
        mach_timebase_info(&info)
        return 1e9 * Double(info.denom) / Double(info.numer)
    }()

    private var start: (sample: Double, host: UInt64)?

    mutating func observe(sampleTime: Double, hostTime: UInt64, ticksPerSecond: Double = DeviceClock.hostTicksPerSecond) -> Double? {
        guard let s = start, hostTime > s.host, sampleTime > s.sample else { start = (sampleTime, hostTime); return nil }
        let seconds = Double(hostTime - s.host) / ticksPerSecond
        guard seconds >= 0.5 else { return nil }
        start = (sampleTime, hostTime)
        let measured = (sampleTime - s.sample) / seconds
        guard let nearest = Self.rates.min(by: { abs($0 - measured) < abs($1 - measured) }),
              abs(measured / nearest - 1) < 0.03 else { return nil }
        return nearest
    }
}

/// Everything the Mac plays except Tacie itself, through a private process tap and a private aggregate device.
/// The first start waits inside Core Audio while macOS asks for the permission, so it runs off the main thread.
/// A denied permission is not an error: the tap simply hands over silence (see `CallAudioProbe`).
@available(macOS 14.2, *)
final class ProcessTapSource: CallAudioSource, @unchecked Sendable {
    var paused = false
    var onSamples: (([Float]) -> Void)?
    var onError: (String) -> Void = { _ in }
    /// Every buffer's level before resampling (the probe listens to this).
    var onLevel: ((Float) -> Void)?
    /// The probe keeps Tacie's own sound in, to hear its test tone.
    var includesSelf = false

    private var tap = AudioObjectID(kAudioObjectUnknown)
    private var aggregate = AudioObjectID(kAudioObjectUnknown)
    private var proc: AudioDeviceIOProcID?
    private let resampler = CallResampler()   // queue only (after open)
    private var clock = DeviceClock()         // queue only
    private weak var utterancer: Utterancer?
    private let queue = DispatchQueue(label: "tacie.call-tap")
    private var confirmed = false

    func start(into utterancer: Utterancer) async throws {
        self.utterancer = utterancer
        try await Task.detached(priority: .userInitiated) { try self.open() }.value
    }

    func open() throws {
        let exclude = includesSelf ? [] : Self.ownProcess().map { [$0] } ?? []
        let desc = CATapDescription(stereoGlobalTapButExcludeProcesses: exclude)
        desc.uuid = UUID()
        desc.isPrivate = true
        desc.muteBehavior = .unmuted
        try check(AudioHardwareCreateProcessTap(desc, &tap), String(localized: "회의 앱 소리를 받을 준비를 하지 못했어요"))

        var asbd = AudioStreamBasicDescription()
        try check(Self.get(tap, kAudioTapPropertyFormat, &asbd), String(localized: "회의 앱 소리의 형식을 읽지 못했어요"))
        guard asbd.mSampleRate > 0 else { throw Self.error(String(localized: "회의 앱 소리의 형식을 읽지 못했어요")) }

        let output = Self.defaultOutputUID() ?? ""
        let agg: [String: Any] = [
            kAudioAggregateDeviceNameKey: "Tacie call sound",
            kAudioAggregateDeviceUIDKey: CallAudio.aggregatePrefix + UUID().uuidString,
            kAudioAggregateDeviceMainSubDeviceKey: output,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceTapAutoStartKey: true,
            kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: output]],
            kAudioAggregateDeviceTapListKey: [[kAudioSubTapDriftCompensationKey: true, kAudioSubTapUIDKey: desc.uuid.uuidString]],
        ]
        try check(AudioHardwareCreateAggregateDevice(agg as CFDictionary, &aggregate), String(localized: "회의 앱 소리를 받을 장치를 만들지 못했어요"))
        // the device's clock, not the tap's, sets how many frames arrive each second (CallResampler)
        let deviceRate = Self.nominalRate(aggregate) ?? asbd.mSampleRate
        resampler.set(rate: deviceRate)
        Log.info("call tap: \(asbd.mSampleRate) Hz tap, \(deviceRate) Hz device, \(asbd.mChannelsPerFrame) ch, self \(includesSelf ? "in" : "out")")
        try check(AudioDeviceCreateIOProcIDWithBlock(&proc, aggregate, queue) { [weak self] _, input, inputTime, _, _ in
            self?.receive(UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: input)), at: inputTime.pointee)
        }, String(localized: "회의 앱 소리를 받기 시작하지 못했어요"))
        try check(AudioDeviceStart(aggregate, proc), String(localized: "회의 앱 소리를 받기 시작하지 못했어요"))
    }

    func stop() {
        if aggregate != kAudioObjectUnknown {
            AudioDeviceStop(aggregate, proc)
            if let proc { AudioDeviceDestroyIOProcID(aggregate, proc) }
            AudioHardwareDestroyAggregateDevice(aggregate)
        }
        if tap != kAudioObjectUnknown { AudioHardwareDestroyProcessTap(tap) }
        proc = nil
        aggregate = AudioObjectID(kAudioObjectUnknown)
        tap = AudioObjectID(kAudioObjectUnknown)
    }

    private func receive(_ list: UnsafeMutableAudioBufferListPointer, at time: AudioTimeStamp) {
        guard !paused else { return }
        let mono = CallAudio.mono(list, channels: 2)
        guard !mono.isEmpty else { return }
        // the device's rate changes under a running tap (AirPods opening their microphone): measure it from the device's
        // own timestamps, so neither a stale format nor a late property can slip through
        if let r = clock.observe(sampleTime: time.mSampleTime, hostTime: time.mHostTime), resampler.set(rate: r) {
            Log.info("call tap: device rate now \(r) Hz")
        }
        let level = CallAudio.rms(mono)
        onLevel?(level)
        // a refused permission hands over exact zeros; anything else means it is on
        if !confirmed, !includesSelf, level > 1e-6 {
            confirmed = true
            CallAudio.record(.granted)
            DispatchQueue.main.async { NotificationCenter.default.post(name: CallAudio.heard, object: nil) }
        }
        guard let samples = resample(mono), !samples.isEmpty else { return }
        if let onSamples {
            onSamples(samples)
        } else {
            // only the call: everything heard is the other side
            let silence = [Float](repeating: 0, count: samples.count)
            DispatchQueue.main.async { [weak self] in self?.utterancer?.feedMix(mic: silence, call: samples) }
        }
    }

    private func resample(_ mono: [Float]) -> [Float]? { resampler.convert(mono) }

    private func check(_ status: OSStatus, _ message: String) throws {
        if status != noErr {
            Log.info("call tap: \(message) (\(status))")
            stop()
            throw Self.error(message)
        }
    }

    private static func error(_ message: String) -> NSError {
        NSError(domain: "tacie", code: 4, userInfo: [NSLocalizedDescriptionKey: message])
    }

    private static func get<T>(_ obj: AudioObjectID, _ selector: AudioObjectPropertySelector, _ value: inout T) -> OSStatus {
        var addr = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal,
                                              mElement: kAudioObjectPropertyElementMain)
        var size = UInt32(MemoryLayout<T>.size)
        return AudioObjectGetPropertyData(obj, &addr, 0, nil, &size, &value)
    }

    private static func nominalRate(_ device: AudioObjectID) -> Double? {
        var r = Float64(0)
        return get(device, kAudioDevicePropertyNominalSampleRate, &r) == noErr && r > 0 ? r : nil
    }

    /// Tacie's own audio process, so its sounds stay out of the meeting.
    private static func ownProcess() -> AudioObjectID? {
        var pid = ProcessInfo.processInfo.processIdentifier
        var addr = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyTranslatePIDToProcessObject,
                                              mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var obj = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        let st = AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, UInt32(MemoryLayout<pid_t>.size), &pid, &size, &obj)
        return st == noErr && obj != kAudioObjectUnknown ? obj : nil
    }

    private static func defaultOutputUID() -> String? {
        var dev = AudioObjectID(0)
        guard get(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultSystemOutputDevice, &dev) == noErr else { return nil }
        var uid: Unmanaged<CFString>?
        var addr = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyDeviceUID, mScope: kAudioObjectPropertyScopeGlobal,
                                              mElement: kAudioObjectPropertyElementMain)
        var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        guard AudioObjectGetPropertyData(dev, &addr, 0, nil, &size, &uid) == noErr else { return nil }
        return uid?.takeRetainedValue() as String?
    }
}

/// Is the call's sound really arriving? Plays a short, quiet two-note chime from Tacie and listens for it through a
/// tap that keeps Tacie's own sound in. Waits for macOS's permission prompt on the first run (minutes at most).
enum CallAudioProbe {
    static let lastKey = "callAudio.lastCheck"  // "granted"/"silent" and when, for the stuck screen

    static func run(threshold: Float = 0.003) async -> CallAudioState {
        guard #available(macOS 14.2, *) else { return .unknown }
        let source = ProcessTapSource()
        source.includesSelf = true
        let peak = Peak()
        source.onLevel = { peak.add($0) }
        source.onSamples = { _ in }
        do { try await Task.detached(priority: .userInitiated) { try source.open() }.value } catch { return .unknown }
        // right after the user answers the prompt (open() waits on it) the tap can hand over silence for a moment, so a
        // silent first chime gets a second one before the answer is "silent"
        for attempt in 0..<2 where peak.value <= threshold {
            if attempt > 0 { try? await Task.sleep(for: .milliseconds(600)) }
            let chime = Chime()
            chime.play()
            try? await Task.sleep(for: .milliseconds(1400))
            chime.stop()
        }
        source.stop()
        let state: CallAudioState = peak.value > threshold ? .granted : .silent
        CallAudio.record(state)
        Log.info("call audio probe: \(state.rawValue), peak \(peak.value)")
        return state
    }

    private final class Peak: @unchecked Sendable {
        private let lock = NSLock()
        private var v: Float = 0
        func add(_ x: Float) { lock.lock(); v = max(v, x); lock.unlock() }
        var value: Float { lock.lock(); defer { lock.unlock() }; return v }
    }

    /// Two soft notes (E5, A5) at about -30 dBFS, faded in and out.
    private final class Chime {
        private let engine = AVAudioEngine()
        func play() {
            let rate = engine.outputNode.outputFormat(forBus: 0).sampleRate
            let total = Int(rate * 1.1)
            var n = 0
            let node = AVAudioSourceNode { _, _, frames, abl in
                let bufs = UnsafeMutableAudioBufferListPointer(abl)
                for f in 0..<Int(frames) {
                    let t = Double(n) / rate
                    let freq = t < 0.5 ? 659.25 : 880.0
                    let local = t < 0.5 ? t : t - 0.5
                    let env = n < total ? min(1, local * 40) * max(0, 1 - local / 0.55) : 0
                    let v = Float(sin(2 * .pi * freq * t) * 0.03 * env)
                    for b in bufs { b.mData!.assumingMemoryBound(to: Float.self)[f] = v }
                    n += 1
                }
                return noErr
            }
            engine.attach(node)
            engine.connect(node, to: engine.mainMixerNode, format: AVAudioFormat(standardFormatWithSampleRate: rate, channels: 1))
            try? engine.start()
        }
        func stop() { engine.stop() }
    }
}
