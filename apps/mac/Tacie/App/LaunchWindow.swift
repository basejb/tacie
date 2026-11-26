import Foundation

/// Whether Tacie opens its main window as it starts. Someone who opens the app (Finder, Spotlight, Dock) wants
/// to see it; a start at login, or a scripted one (screenshots, replays, exports), keeps to the island.
/// Launch flags for scripts, screenshots and debugging (`--debug-control`, `--listen`, `--replay`, `--meetings-dir`, …).
/// Only a development build reads them. In a shipped build any process of the same user could otherwise open Tacie
/// with them and use its microphone, screen and calendar permissions or point it at other folders.
enum LaunchArgs {
    static let all: [String] = Build.isDev ? ProcessInfo.processInfo.arguments : []

    static func has(_ flag: String) -> Bool { all.contains(flag) }

    static func value(_ flag: String) -> String? {
        guard let i = all.firstIndex(of: flag), all.indices.contains(i + 1) else { return nil }
        return all[i + 1]
    }
}

enum LaunchWindow {
    static let scriptedFlags = ["--debug-control", "--ask", "--replay", "--listen", "--export-eyes", "--export-brand"]

    static func showsMain(arguments: [String], loginItem: Bool) -> Bool {
        !loginItem && !arguments.contains(where: scriptedFlags.contains)
    }
}
