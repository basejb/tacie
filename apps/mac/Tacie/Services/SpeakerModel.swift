import Foundation

/// Which speaker model the engine groups a meeting's voices with (docs/04 4절 "화자 나누기와 참석자 확인"): the fine-tuned
/// CAM++ (ERes2Net still knows remembered people), or ERes2Net alone. The server's GET /me says which (`speaker_model`), so
/// CAM++ can be turned off for everyone without a new app if it turns out worse. The last answer is kept, so a launch
/// offline or signed out starts the same way; it applies from the next engine start. A dev build can force one with
/// `--speaker-model eres2net` or `--speaker-model campp-ft`.
enum SpeakerModel {
    static let key = "speakerModel"
    static let eres2net = "eres2net"

    /// Keeps the server's answer (nil: an older server, keep what we had).
    static func remember(_ value: String?, defaults: UserDefaults = .standard) {
        guard let value else { return }
        defaults.set(value, forKey: key)
    }

    /// True when the engine should leave the fine-tuned CAM++ off and group with ERes2Net alone.
    static func eres2netOnly(defaults: UserDefaults = .standard,
                             arguments: [String] = ProcessInfo.processInfo.arguments, dev: Bool = Build.isDev) -> Bool {
        if dev, let i = arguments.firstIndex(of: "--speaker-model"), i + 1 < arguments.count {
            return arguments[i + 1] == eres2net
        }
        return defaults.string(forKey: key) == eres2net
    }
}
