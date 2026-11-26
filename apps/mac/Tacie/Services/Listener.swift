import AVFoundation
import CoreAudio
import Foundation

/// Where the sound comes from: the microphone, or a recording played at real-time speed (demos and tests).
protocol SoundSource: AnyObject {
    func start(into utterancer: Utterancer) async throws
    func stop()
    var paused: Bool { get set }
}

/// A source with a microphone in it: says when the input device goes away or comes back.
protocol MicWatching: AnyObject {
    var onInput: (Bool) -> Void { get set }
}

/// The Mac's input device, followed while the meeting runs: when it goes away (AirPods off), comes back or changes,
/// the tap moves to whatever the input is now. `waitForDevice` starts without one and begins once one appears.
final class MicSource: SoundSource, MicWatching {
    /// No input device at all (a desktop Mac with the headset unplugged). Remote meetings go on with the call only.
    /// The microphone permission was refused.
    static let notAllowedCode = 3
    static let noInput = NSError(domain: "tacie", code: 4, userInfo: [NSLocalizedDescriptionKey: String(localized: "마이크를 찾지 못했어요. 마이크나 이어폰을 연결한 뒤 다시 시작해 주세요.")])
    private static let defaultInput = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDefaultInputDevice,
                                                                 mScope: kAudioObjectPropertyScopeGlobal,
                                                                 mElement: kAudioObjectPropertyElementMain)
    private var engine = AVAudioEngine()
    private weak var utterancer: Utterancer?
    private var tapping = false
    private var watching = false
    private var retap: DispatchWorkItem?
    private var configObserver: NSObjectProtocol?
    private lazy var deviceListener: AudioObjectPropertyListenerBlock = { [weak self] _, _ in self?.inputChanged() }
    var paused = false
    var onSamples: (([Float]) -> Void)?  // set by MixSource; otherwise samples go straight to the utterancer
    /// The input went away (false) or is there again (true), on the main queue. A waiting start says false at once.
    var onInput: (Bool) -> Void = { _ in }

    func start(into utterancer: Utterancer) async throws {
        try await start(into: utterancer, waitForDevice: false)
    }

    func start(into utterancer: Utterancer, waitForDevice: Bool) async throws {
        guard await AVCaptureDevice.requestAccess(for: .audio) else {
            throw NSError(domain: "tacie", code: Self.notAllowedCode, userInfo: [NSLocalizedDescriptionKey: String(localized: "마이크 권한이 없어요. 시스템 설정 → 개인정보 보호 및 보안 → 마이크에서 Tacie를 켜 주세요.")])
        }
        self.utterancer = utterancer
        // the device listener calls back on the main queue: tap and watch state live there
        try await MainActor.run {
            watch()
            do {
                try tap()
            } catch let e as NSError where e == Self.noInput && waitForDevice {
                Log.info("no microphone: listening to the call only until one is connected")
                onInput(false)
            } catch {
                unwatch()
                throw error
            }
        }
    }

    func stop() {
        unwatch()
        untap()
    }

    private func tap() throws {
        // without an input device the format is empty and installTap throws an Objective-C exception (it crashed the app).
        // Tacie's own call-sound device (the process tap's aggregate) has inputs too, but is not a microphone.
        guard let device = AVCaptureDevice.default(for: .audio), !device.uniqueID.hasPrefix(CallAudio.aggregatePrefix),
              Self.hasRealDefaultInput() else { throw Self.noInput }
        engine = AVAudioEngine()  // a fresh engine takes the input as it is now; a used one may hold on to the old device
        let input = engine.inputNode
        let inFormat = input.outputFormat(forBus: 0)
        guard inFormat.channelCount > 0, inFormat.sampleRate > 0 else { throw Self.noInput }
        guard let outFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Utterancer.sampleRate,
                                            channels: 1, interleaved: false),
              let converter = AVAudioConverter(from: inFormat, to: outFormat) else {
            throw NSError(domain: "tacie", code: 1, userInfo: [NSLocalizedDescriptionKey: String(localized: "마이크 형식을 바꾸지 못했어요.")])
        }
        input.installTap(onBus: 0, bufferSize: 4096, format: inFormat) { [weak self] buffer, _ in
            guard let self, !self.paused, let utterancer = self.utterancer else { return }
            let ratio = Utterancer.sampleRate / inFormat.sampleRate
            let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 32
            guard let out = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: capacity) else { return }
            var fed = false
            converter.convert(to: out, error: nil) { _, status in
                if fed { status.pointee = .noDataNow; return nil }
                fed = true
                status.pointee = .haveData
                return buffer
            }
            guard let ch = out.floatChannelData?[0] else { return }
            let samples = Array(UnsafeBufferPointer(start: ch, count: Int(out.frameLength)))
            if let onSamples = self.onSamples {
                onSamples(samples)
            } else {
                DispatchQueue.main.async { utterancer.feed(samples) }
            }
        }
        do {
            try engine.start()
        } catch {
            input.removeTap(onBus: 0)
            throw error
        }
        tapping = true
        // the same device changing format (a headset switching to its call mode) stops the engine too
        configObserver = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine,
                                                                queue: .main) { [weak self] _ in self?.inputChanged() }
    }

    /// The system's default input device exists and is not Tacie's own call-sound device.
    private static func hasRealDefaultInput() -> Bool {
        var addr = defaultInput
        var id = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size, &id) == noErr,
              id != kAudioObjectUnknown else { return false }
        var uidAddr = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyDeviceUID, mScope: kAudioObjectPropertyScopeGlobal,
                                                 mElement: kAudioObjectPropertyElementMain)
        var uid: Unmanaged<CFString>?
        var uidSize = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        guard AudioObjectGetPropertyData(id, &uidAddr, 0, nil, &uidSize, &uid) == noErr,
              let s = uid?.takeRetainedValue() as String? else { return true }
        return !s.hasPrefix(CallAudio.aggregatePrefix)
    }

    private func untap() {
        if let configObserver { NotificationCenter.default.removeObserver(configObserver) }
        configObserver = nil
        if tapping { engine.inputNode.removeTap(onBus: 0) }
        engine.stop()
        tapping = false
    }

    private func watch() {
        var address = Self.defaultInput
        AudioObjectAddPropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject), &address, .main, deviceListener)
        watching = true
    }

    private func unwatch() {
        guard watching else { return }
        watching = false
        retap?.cancel()
        var address = Self.defaultInput
        AudioObjectRemovePropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject), &address, .main, deviceListener)
    }

    /// One change often comes as several notices, and a headset needs a moment before it gives sound: act once, 0.5 s later.
    private func inputChanged() {
        guard watching else { return }
        retap?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.moveTap() }
        retap = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5, execute: work)
    }

    private func moveTap() {
        guard watching else { return }
        let had = tapping
        untap()
        do {
            try tap()
            Log.info(had ? "microphone: input changed, listening on" : "microphone: connected, listening again")
            if !had { onInput(true) }
        } catch {
            Log.info("microphone: no input (\(error.localizedDescription))")
            if had { onInput(false) }
        }
    }
}

/// Plays a 16 kHz mono 16-bit WAV into the utterancer at real-time speed, without sound.
/// `as: .them` plays it as the call's side of a remote meeting, `.me` as the microphone's.
/// Sendable for the timer's block: everything here runs on the main thread (start is main-actor, the timer fires
/// on the main run loop).
final class FileSource: SoundSource, @unchecked Sendable {
    private let url: URL
    private let side: Speaker?
    private var timer: Timer?
    private weak var utterancer: Utterancer?
    var paused = false
    /// Called after the last chunk. With a handler set the source does not flush: the handler's owner ends the meeting,
    /// and its own end cuts the last words while the session still exists. Without one the tail is flushed here.
    var onEnd: (() -> Void)?

    init(url: URL, as side: Speaker? = nil) {
        self.url = url
        self.side = side
    }

    @MainActor
    func start(into utterancer: Utterancer) async throws {
        self.utterancer = utterancer
        let data = try Data(contentsOf: url)
        // an empty or header-only file still plays: the timer's first tick finds nothing left and ends it
        let samples: [Float] = data.count <= 44 ? [] : data.dropFirst(44).withUnsafeBytes { raw in
            raw.bindMemory(to: Int16.self).map { Float(Int16(littleEndian: $0)) / Float(Int16.max) }
        }
        var pos = 0
        let chunk = 1_600  // 100 ms
        timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] t in
            guard let self, let utterancer = self.utterancer else { t.invalidate(); return }
            if self.paused { return }
            if pos >= samples.count {
                t.invalidate()
                if let onEnd = self.onEnd { onEnd() } else { utterancer.flush() }
                return
            }
            let piece = Array(samples[pos..<min(pos + chunk, samples.count)])
            let silence = [Float](repeating: 0, count: piece.count)
            switch self.side {
            case .them: utterancer.feedMix(mic: silence, call: piece)
            case .me: utterancer.feedMix(mic: piece, call: silence)
            case nil: utterancer.feed(piece)
            }
            pos += chunk
        }
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }
}
