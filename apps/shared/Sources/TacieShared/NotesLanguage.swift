import Foundation

/// Settings → Meetings → "Notes language" (2026-11-19): the language the notes are written in. Meeting (the default)
/// writes them in the language the meeting was held in, as before; App writes them in the app's language (Korean or
/// English), so a Korean user's English meeting gets Korean notes. The transcript stays as it was said either way. The
/// engine gets the app language as `language` on the notes calls only when App is picked.
enum NotesLanguage {
    static let key = "notesLanguage"
    static let meeting = "meeting"
    static let app = "app"

    /// The value for the engine's `language`: `ko` or `en` when 앱 언어 is picked, nil for 회의 언어.
    static func target(_ defaults: UserDefaults = .standard) -> String? {
        guard defaults.string(forKey: key) == app else { return nil }
        return AppLocale.isKorean ? "ko" : "en"
    }
}
