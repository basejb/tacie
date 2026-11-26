import CoreAudio
import Foundation

/// Watches which meeting apps use the microphone (Core Audio's process objects, macOS 14.4 and later, no permission)
/// and turns it into "ask" and "ended" through `MeetingStartWatch`. Everything stays on this Mac. On macOS before 14.4
/// it does nothing: the older APIs say only that some app uses the input, not which.
@MainActor
final class MeetingStartWatcher {
    var onAsk: (MeetingApp) -> Void = { _ in }
    var onEnded: (MeetingApp) -> Void = { _ in }
    var onOver: (MeetingApp) -> Void = { _ in }
    /// Tacie's meeting going on: (listening, remote).
    var meeting: () -> (listening: Bool, remote: Bool) = { (false, false) }

    private var rules = MeetingStartWatch()
    private var users: AnyObject?
    private var wake: Task<Void, Never>?
    /// `--debug-control meeting-app zoom|off`: pretends the app holds the microphone (captures, no real call).
    var pretend: Set<MeetingApp>? {
        didSet { check() }
    }

    static var available: Bool {
        if #available(macOS 14.4, *) { return true }
        return false
    }

    func start() {
        guard users == nil else { return }
        if #available(macOS 14.4, *) {
            let u = MicUsers { [weak self] in Task { @MainActor in self?.check() } }
            users = u
        }
        check()
    }

    /// Called on every change of the microphone's users and of Tacie's meeting, and when a wait runs out.
    func check() {
        var holding: Set<MeetingApp> = []
        if let pretend {
            holding = pretend
        } else if #available(macOS 14.4, *), let u = users as? MicUsers {
            holding = u.holding()
        }
        let now = Date()
        let m = meeting()
        let allowed = MeetingApp.promptOn ? Set(MeetingApp.allCases).subtracting(MeetingApp.off) : []
        for e in rules.update(holding: holding, now: now, listening: m.listening, remote: m.remote, allowed: allowed) {
            switch e {
            case .ask(let app):
                Log.info("meeting app started: \(app.rawValue)")
                onAsk(app)
            case .ended(let app):
                Log.info("meeting app let the microphone go: \(app.rawValue)")
                onEnded(app)
            case .over(let app):
                Log.info("meeting app's meeting is over: \(app.rawValue)")
                onOver(app)
            }
        }
        wake?.cancel()
        if let next = rules.nextCheck(now: now) {
            wake = Task { [weak self] in
                try? await Task.sleep(for: .seconds(max(0.05, next.timeIntervalSinceNow) + 0.05))
                guard !Task.isCancelled else { return }
                self?.check()
            }
        }
    }
}

/// The processes Core Audio knows, and which of them run the input. Listens for the process list and each process's
/// input switching on or off; `changed` comes on Core Audio's queue.
@available(macOS 14.4, *)
private final class MicUsers {
    private let changed: () -> Void
    private let queue = DispatchQueue(label: "ai.tacie.mic-users")
    private var watched: [AudioObjectID: AudioObjectPropertyListenerBlock] = [:]
    private var listBlock: AudioObjectPropertyListenerBlock?
    private let me = getpid()

    init(changed: @escaping () -> Void) {
        self.changed = changed
        var a = Self.address(kAudioHardwarePropertyProcessObjectList)
        let block: AudioObjectPropertyListenerBlock = { [weak self] _, _ in
            self?.rewatch()
            self?.changed()
        }
        listBlock = block
        AudioObjectAddPropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject), &a, queue, block)
        queue.sync { rewatch() }
    }

    deinit {
        var a = Self.address(kAudioHardwarePropertyProcessObjectList)
        if let listBlock { AudioObjectRemovePropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject), &a, queue, listBlock) }
        var i = Self.address(kAudioProcessPropertyIsRunningInput)
        for (id, b) in watched { AudioObjectRemovePropertyListenerBlock(id, &i, queue, b) }
    }

    /// The meeting apps whose processes run the input now (Tacie's own is left out).
    func holding() -> Set<MeetingApp> {
        var out: Set<MeetingApp> = []
        for id in Self.processes() {
            guard Self.uint(id, kAudioProcessPropertyIsRunningInput) != 0, Self.pid(id) != me,
                  let bundle = Self.bundleID(id), let app = MeetingApp.of(bundleID: bundle) else { continue }
            out.insert(app)
        }
        return out
    }

    /// One listener per process for its input; processes that went away drop theirs. On `queue`.
    private func rewatch() {
        let now = Set(Self.processes())
        var i = Self.address(kAudioProcessPropertyIsRunningInput)
        for (id, b) in watched where !now.contains(id) {
            AudioObjectRemovePropertyListenerBlock(id, &i, queue, b)
            watched[id] = nil
        }
        for id in now where watched[id] == nil {
            let b: AudioObjectPropertyListenerBlock = { [weak self] _, _ in self?.changed() }
            if AudioObjectAddPropertyListenerBlock(id, &i, queue, b) == noErr { watched[id] = b }
        }
    }

    private static func address(_ s: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(mSelector: s, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    }

    private static func processes() -> [AudioObjectID] {
        var a = address(kAudioHardwarePropertyProcessObjectList)
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size) == noErr, size > 0
        else { return [] }
        var ids = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
        guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size, &ids) == noErr else { return [] }
        return ids
    }

    private static func uint(_ id: AudioObjectID, _ s: AudioObjectPropertySelector) -> UInt32 {
        var a = address(s)
        var v: UInt32 = 0
        var size = UInt32(MemoryLayout<UInt32>.size)
        return AudioObjectGetPropertyData(id, &a, 0, nil, &size, &v) == noErr ? v : 0
    }

    private static func pid(_ id: AudioObjectID) -> pid_t {
        var a = address(kAudioProcessPropertyPID)
        var v: pid_t = 0
        var size = UInt32(MemoryLayout<pid_t>.size)
        return AudioObjectGetPropertyData(id, &a, 0, nil, &size, &v) == noErr ? v : -1
    }

    private static func bundleID(_ id: AudioObjectID) -> String? {
        var a = address(kAudioProcessPropertyBundleID)
        var v: Unmanaged<CFString>?
        var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        guard AudioObjectGetPropertyData(id, &a, 0, nil, &size, &v) == noErr, let s = v?.takeRetainedValue() else { return nil }
        let id = s as String
        return id.isEmpty ? nil : id
    }
}
