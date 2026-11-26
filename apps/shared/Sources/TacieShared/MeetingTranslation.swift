import Foundation

/// Meeting translation (docs/30): Soniox translates speech that is not in the app's language, on the same connection,
/// and the transcript shows it under the original. Hidden in a shipped app for now (`offered`); a development build
/// shows the switch under Settings → General → Meetings. There is no language to pick: it follows Display → Language.
enum MeetingTranslation {
    static let offered = Build.isDev && !Build.isOpen  // asks Soniox through Tacie's relay
    static let key = "meetingTranslation"

    /// The target language for a new Soniox connection, nil when translation is off or not offered.
    static func target(offered: Bool, on: Bool, appKorean: Bool) -> String? {
        guard offered, on else { return nil }
        return appKorean ? "ko" : "en"
    }

    static var current: String? {
        target(offered: offered, on: UserDefaults.standard.bool(forKey: key), appKorean: AppLocale.isKorean)
    }

    /// Two halves of a line the engine joined: both translations, in order.
    static func joined(_ a: String?, _ b: String?) -> String? {
        let parts = [a, b].compactMap { $0 }.filter { !$0.isEmpty }
        return parts.isEmpty ? nil : parts.joined(separator: " ")
    }
}
