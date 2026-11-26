import AppKit

/// Pause music (docs/35 6-2): while a meeting is heard, Music and Spotify that were playing are paused, and at its end
/// only those Tacie paused are played again. An app that is not running is never sent anything (a script would launch
/// it). Refusal (Automation not allowed) or failure is quiet.
@MainActor
final class MediaPause {
    static let shared = MediaPause()
    static let key = "pauseMusic"
    static var on: Bool { UserDefaults.standard.object(forKey: key) as? Bool ?? true }

    /// The players Tacie can pause, by bundle id.
    static let players = ["com.apple.Music", "com.spotify.client"]

    /// Runs one AppleScript line against an app and gives back its text result (nil on failure). Tests replace it.
    var run: (_ bundleID: String, _ script: String) -> String? = { id, script in
        var error: NSDictionary?
        let result = NSAppleScript(source: "tell application id \"\(id)\" to \(script)")?.executeAndReturnError(&error)
        if let error { Log.info("music pause: \(id) refused or failed (\(error[NSAppleScript.errorNumber] ?? "?"))") }
        return result?.stringValue
    }
    var running: (_ bundleID: String) -> Bool = { !NSRunningApplication.runningApplications(withBundleIdentifier: $0).isEmpty }

    /// What Tacie paused for the meeting going on.
    private(set) var paused: [String] = []

    /// At the start of listening: pauses each running player that is playing.
    func pauseAll() {
        guard Self.on else { return }
        paused = Self.players.filter { id in
            guard running(id) else { return false }
            // "playing" only when it was, so a stopped player is never started at the end
            return run(id, "if player state is playing then\npause\nreturn \"paused\"\nend if") == "paused"
        }
        if !paused.isEmpty { Log.info("music pause: paused \(paused.count)") }
    }

    /// At the meeting's end: plays again what Tacie paused, unless the person played or quit it meanwhile.
    func resumeAll() {
        let ids = paused
        paused = []
        for id in ids where running(id) {
            _ = run(id, "if player state is paused then play")
        }
    }
}
