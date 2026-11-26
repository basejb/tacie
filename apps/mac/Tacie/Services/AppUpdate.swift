import AppKit
import Foundation
import Observation
import Sparkle

/// Tacie's updates (docs/15): Sparkle checks at launch and once a day, downloads a new version quietly and installs it
/// when Tacie quits; the main window and the menu bar menu offer Restart to install it now. Nothing is checked or
/// installed during a meeting. With automatic download off, a scheduled check that finds a version opens Sparkle's
/// window once, and again every 7 days while that version waits (never in a meeting or just after one; Skip in
/// Sparkle's window stops it for that version). A development build does not check by itself. A downloaded version also
/// installs by itself while the Mac sits idle (IdleUpdate), and Tacie comes back as it was: the main window only if it
/// was open.
@Observable
@MainActor
final class AppUpdate: NSObject {
    enum State: Equatable {
        case none
        case available(version: String)  // automatic download is off: 받기 shows Sparkle's window
        case ready(version: String)  // downloaded and checked: a restart installs it
    }

    static let shared = AppUpdate()
    var state: State = .none
    /// The release notes of the waiting version, when the appcast has them.
    var notesURL: URL?
    /// A meeting is on: no checks, no installs (AppDelegate sets it).
    var busy: () -> Bool = { false }
    /// When the last meeting ended: its notes open then, so Sparkle's window waits (AppDelegate sets it).
    var meetingEndedAt: Date?
    /// A window that must not vanish under the person (the onboarding): no idle install while it is up (AppDelegate
    /// sets it).
    var holding: () -> Bool = { false }
    /// The main window is open now, so the idle install brings it back (AppDelegate sets it).
    var mainOpen: () -> Bool = { false }

    private static let promptedVersionKey = "updatePromptedVersion"
    private static let promptedAtKey = "updatePromptedAt"
    private static let quietRelaunchAtKey = "updateQuietRelaunchAt"
    private static let quietRelaunchMainKey = "updateQuietRelaunchMain"

    /// Whether this build updates itself at all: only with an update feed (the official app; docs/41 0절 15번).
    static var offered: Bool { BuildValues.updateFeed != nil }
    private var controller: SPUStandardUpdaterController?
    private var install: (() -> Void)?
    private var idleWatch: Task<Void, Never>?

    var updater: SPUUpdater? { controller?.updater }

    /// Starts the updater (a product build checks on its own; a development one only from 설정 → 일반).
    func start() {
        // a build without an update feed (the open edition from source, a fork) never checks (docs/41 0절 15번)
        guard controller == nil, Self.offered else { return }
        controller = SPUStandardUpdaterController(startingUpdater: true, updaterDelegate: self, userDriverDelegate: self)
        if Build.isDev { controller?.updater.automaticallyChecksForUpdates = false }
    }

    /// 다시 시작: installs the downloaded version and opens it.
    func restart(place: AUpdatePlace) {
        Analytics.capture(.updateRestartClicked(place: place))
        Log.info("update restart asked (\(install == nil ? "nothing downloaded" : "installing"))")
        guard !busy() else { return }
        if let install {
            install()
        } else if case .available = state {
            controller?.checkForUpdates(nil)
        }
    }

    /// While a downloaded version waits: every minute, install it if the Mac has sat idle long enough (IdleUpdate).
    private func watchIdle() {
        guard idleWatch == nil, !Build.isDev else { return }
        idleWatch = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(IdleUpdate.checkEvery))
                self?.installIfIdle()
            }
        }
    }

    private func installIfIdle() {
        guard let install, case .ready = state else { return }
        // seconds since the last key, click or mouse move in this login session (no permission needed)
        let idle = CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType: CGEventType(rawValue: ~0)!)
        guard IdleUpdate.installs(now: Date(), idle: idle, busy: busy(), holding: holding(), meetingEndedAt: meetingEndedAt) else { return }
        let defaults = UserDefaults.standard
        defaults.set(Date(), forKey: Self.quietRelaunchAtKey)
        defaults.set(mainOpen(), forKey: Self.quietRelaunchMainKey)
        Log.info("update installing while idle (\(Int(idle))s)")
        Analytics.capture(.updateIdleRestart)
        idleWatch?.cancel()
        install()
    }

    /// Read once at launch: nil for an ordinary start, else whether the idle install found the main window open.
    static func takeQuietRelaunch(now: Date = Date()) -> Bool? {
        let defaults = UserDefaults.standard
        let askedAt = defaults.object(forKey: quietRelaunchAtKey) as? Date
        let main = defaults.bool(forKey: quietRelaunchMainKey)
        defaults.removeObject(forKey: quietRelaunchAtKey)
        defaults.removeObject(forKey: quietRelaunchMainKey)
        return IdleUpdate.reopensMain(askedAt: askedAt, mainWasOpen: main, now: now)
    }

    /// 설정 → 일반 → 업데이트 확인: Sparkle's window says what it found.
    func checkNow() {
        controller?.checkForUpdates(nil)
    }

    var automaticallyDownloads: Bool {
        get { updater?.automaticallyDownloadsUpdates ?? true }
        set { updater?.automaticallyDownloadsUpdates = newValue }
    }

    /// `update 0.1.12` / `update` (debug captures): shows or clears the waiting version without Sparkle.
    func show(version: String?) {
        state = version.map { .ready(version: $0) } ?? .none
    }
}

extension AppUpdate: SPUUpdaterDelegate {
    /// No background check during a meeting; the next one (a day later, or the next launch) picks it up.
    nonisolated func updater(_ updater: SPUUpdater, mayPerform updateCheck: SPUUpdateCheck) throws {
        let busy = MainActor.assumeIsolated { self.busy() }
        if busy, updateCheck == .updatesInBackground {
            throw NSError(domain: "ai.tacie.update", code: 1, userInfo: [NSLocalizedDescriptionKey: "a meeting is on"])
        }
    }

    /// Downloaded and checked: it installs when Tacie quits, or now from 다시 시작.
    nonisolated func updater(_ updater: SPUUpdater, willInstallUpdateOnQuit item: SUAppcastItem,
                             immediateInstallationBlock immediateInstallHandler: @escaping () -> Void) -> Bool {
        let version = item.displayVersionString
        let notes = UpdateNotes.since(UpdateNotes.localized(item.releaseNotesURL ?? item.fullReleaseNotesURL),
                                      installed: Self.installedVersion)
        MainActor.assumeIsolated {
            self.install = immediateInstallHandler
            self.notesURL = notes
            self.state = .ready(version: version)
            Log.info("update \(version) downloaded")
            Analytics.capture(.updateReady(to: version))
            self.watchIdle()
        }
        return true
    }

    nonisolated static var installedVersion: String? {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
    }

    /// Sparkle's own pick, with ?since=<installed version> on its release notes link, so Sparkle's window (업데이트 확인)
    /// shows every version this Mac skipped, not only the newest (UpdateNotes). Sparkle has already dropped the items
    /// this Mac cannot take (macOS version, hardware). nil lets Sparkle pick as before.
    nonisolated func bestValidUpdate(in appcast: SUAppcast, for updater: SPUUpdater) -> SUAppcastItem? {
        let installed = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "0"
        let compare = SUStandardVersionComparator.default
        guard let best = appcast.items.max(by: { compare.compareVersion($0.versionString, toVersion: $1.versionString) == .orderedAscending }),
              compare.compareVersion(best.versionString, toVersion: installed) == .orderedDescending else { return nil }
        return UpdateNotes.item(best, installed: Self.installedVersion)
    }

    /// 설치 · 나중에 · 건너뛰기 in Sparkle's window (docs/16): only the pick and whether the user asked for the check.
    nonisolated func updater(_ updater: SPUUpdater, userDidMake choice: SPUUserUpdateChoice, forUpdate updateItem: SUAppcastItem,
                             state: SPUUserUpdateState) {
        let picked: AUpdateChoice = switch choice {
        case .install: .install
        case .skip: .skip
        default: .later
        }
        let user = state.userInitiated
        MainActor.assumeIsolated {
            Log.info("update window: \(picked.rawValue)")
            Analytics.capture(.updatePromptChoice(choice: picked, userInitiated: user))
        }
    }

    nonisolated func updater(_ updater: SPUUpdater, didAbortWithError error: Error) {
        let code = (error as NSError).code
        // "no update" is not a failure worth a line
        if code != Int(SUError.noUpdateError.rawValue) {
            // a check we held back for a meeting is not a failure either
            let ours = (error as NSError).domain == "ai.tacie.update"
            MainActor.assumeIsolated {
                Log.info("update check stopped: \(code)")
                if !ours { Analytics.capture(.updateFailed(code: code)) }
            }
        }
    }
}

extension AppUpdate: SPUStandardUserDriverDelegate {
    /// Tacie has no Dock icon: Sparkle lets such an app remind gently instead of opening its window by itself.
    nonisolated var supportsGentleScheduledUpdateReminders: Bool { true }

    /// A scheduled check that found something (automatic download off): our line in the main window says so, and 받기
    /// opens Sparkle's window. Sparkle's window also opens by itself when `UpdatePrompt.shows` says so. A check the user
    /// asked for shows Sparkle's window at once.
    nonisolated func standardUserDriverShouldHandleShowingScheduledUpdate(_ update: SUAppcastItem,
                                                                          andInImmediateFocus immediateFocus: Bool) -> Bool {
        let version = update.displayVersionString
        return MainActor.assumeIsolated {
            if case .ready = self.state { return false }
            self.state = .available(version: version)
            let defaults = UserDefaults.standard
            let now = Date()
            let prompt = UpdatePrompt.shows(version: version, now: now,
                                           promptedVersion: defaults.string(forKey: Self.promptedVersionKey),
                                           promptedAt: defaults.object(forKey: Self.promptedAtKey) as? Date,
                                           busy: self.busy(), meetingEndedAt: self.meetingEndedAt)
            if prompt {
                defaults.set(version, forKey: Self.promptedVersionKey)
                defaults.set(now, forKey: Self.promptedAtKey)
                Log.info("update \(version) found: showing Sparkle's window")
            }
            return prompt
        }
    }

    /// Sparkle's window for a scheduled check opens behind the app in front (Tacie is never active then, and macOS keeps
    /// an inactive app's windows below): it floats over the other windows instead, without taking the keyboard from
    /// what the user is typing in.
    nonisolated func standardUserDriverWillHandleShowingUpdate(_ handleShowingUpdate: Bool, forUpdate update: SUAppcastItem,
                                                               state: SPUUserUpdateState) {
        guard handleShowingUpdate else { return }
        let user = state.userInitiated
        MainActor.assumeIsolated { Analytics.capture(.updatePromptShown(userInitiated: user)) }
        guard !user else { return }
        // Sparkle puts its window up after this call returns
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
            for w in NSApp.windows where w.isVisible {
                guard let c = w.windowController, String(describing: type(of: c)).contains("SUUpdateAlert") else { continue }
                w.level = .floating
                w.orderFrontRegardless()
            }
        }
    }

    nonisolated func standardUserDriverWillFinishUpdateSession() {
        MainActor.assumeIsolated {
            if case .available = self.state { self.state = .none }
        }
    }
}
