import Foundation

/// The language the app shows: Korean (as before) or English, picked by macOS from the system languages
/// (Localizable.xcstrings, docs/28). Dates and numbers follow it, so a Korean system renders exactly as it always did.
enum AppLocale {
    /// The bundle's pick. A bundle without our English strings (the unit tests run inside xctest, which has an en
    /// localization of its own) shows the Korean source strings, so it counts as Korean too.
    static let isKorean: Bool = {
        let bundle = Bundle.main
        guard bundle.path(forResource: "Localizable", ofType: "strings", inDirectory: nil, forLocalization: "en") != nil
        else { return true }
        return bundle.preferredLocalizations.first?.hasPrefix("en") != true
    }()

    /// Formats dates and numbers: ko_KR for Korean, the system's own locale for English (its region's date order).
    static let locale: Locale = {
        if isKorean { return Locale(identifier: "ko_KR") }
        let current = Locale.current
        return current.language.languageCode?.identifier == "en" ? current : Locale(identifier: "en_US")
    }()

    /// A date pattern: the Korean pattern as it always was, or the system's pattern for the same fields in English
    /// (`template` in DateFormatter template letters, e.g. "MMMd", "jmm").
    static func pattern(ko: String, template: String) -> String {
        if isKorean { return ko }
        return DateFormatter.dateFormat(fromTemplate: template, options: 0, locale: locale) ?? ko
    }

    /// A count with the locale's grouping ("1,234").
    static func number(_ n: Int) -> String {
        n.formatted(.number.grouping(.automatic).locale(locale))
    }
}

/// The language picked in Settings → General (2026-11-11). Stored as this app's own `AppleLanguages`, the same value
/// macOS writes from System Settings → Language & Region → Applications, so both places agree. It takes effect when
/// the app starts, so choosing one restarts Tacie.
enum AppLanguage: String, CaseIterable {
    case system, korean, english

    static let key = "AppleLanguages"

    /// What this app is set to now (in `defaults`, the app's own domain only).
    static func current(_ defaults: UserDefaults = .standard) -> AppLanguage {
        let mine = defaults.persistentDomain(forName: Bundle.main.bundleIdentifier ?? "")?[key] as? [String]
        switch mine?.first {
        case let l? where l.hasPrefix("en"): return .english
        case let l? where l.hasPrefix("ko"): return .korean
        default: return .system
        }
    }

    /// Whether the app would show Korean with this choice: Korean, English, or for `.system` the Mac's own language
    /// order (the global list, not this app's) matched against the app's localizations.
    func showsKorean(macLanguages: [String] = UserDefaults(suiteName: ".GlobalPreferences")?.stringArray(forKey: "AppleLanguages") ?? []) -> Bool {
        switch self {
        case .korean: return true
        case .english: return false
        case .system:
            return Bundle.preferredLocalizations(from: ["ko", "en"], forPreferences: macLanguages).first != "en"
        }
    }

    /// Writes the choice; `.system` removes the app's own value so the Mac's languages apply again.
    func save(_ defaults: UserDefaults = .standard) {
        switch self {
        case .system: defaults.removeObject(forKey: Self.key)
        case .korean: defaults.set(["ko"], forKey: Self.key)
        case .english: defaults.set(["en"], forKey: Self.key)
        }
    }
}
