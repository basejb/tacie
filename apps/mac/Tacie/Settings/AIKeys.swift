import Foundation

/// The AI keys (#7 step 5): pasted in Settings > AI, kept in the login keychain (`ai.tacie.openrouter`,
/// `ai.tacie.typesafe`) and handed to the engine, which holds them in memory only, every time it is ready. A
/// development build's engine also reads the `~/.config/<name>/config.json` key files; the app never writes them. A key
/// is never logged or shown again.
enum AIKeys {
    /// openrouter: transcribing, answers and meeting notes. typesafe: the question gate and the ranking.
    enum Name: String, CaseIterable {
        case openrouter, typesafe
    }

    static func store(_ name: Name) -> KeychainTokenStore {
        KeychainTokenStore(service: "ai.tacie.\(name.rawValue)", account: "apiKey", label: label(name))
    }

    /// The keychain item's label (what Keychain Access shows).
    static func label(_ name: Name) -> String {
        switch name {
        case .openrouter: "Tacie OpenRouter API key"
        case .typesafe: "Tacie TypeSafe API key"
        }
    }

    /// Whether a key is saved; the key itself is not read (the settings view uses this).
    static func exists(_ name: Name) -> Bool { store(name).exists() }

    enum SaveError: Error { case notAKey, keychain }

    /// Saves a paste as the key, or throws `notAKey` (see `clean`) or `keychain`.
    static func save(_ paste: String, as name: Name, in store: TokenStore? = nil) throws {
        guard let key = clean(paste) else { throw SaveError.notAKey }
        do { try (store ?? Self.store(name)).save(key) } catch { throw SaveError.keychain }
    }

    static func remove(_ name: Name, in store: TokenStore? = nil) throws {
        try (store ?? Self.store(name)).delete()
    }

    /// What a paste becomes: trimmed, or nil when it cannot be a key the engine sends (printable ASCII without spaces,
    /// up to 4,096 characters, as POST /ai/credentials takes it).
    static func clean(_ paste: String) -> String? {
        let key = paste.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty, key.utf8.count <= 4096, key.utf8.allSatisfy({ (0x21...0x7e).contains($0) }) else { return nil }
        return key
    }

    /// Every key for the engine: the saved one, or nil so the engine drops the one it holds.
    static func saved(read: (Name) -> String? = { store($0).read() }) -> [Name: String?] {
        Dictionary(uniqueKeysWithValues: Name.allCases.map { ($0, read($0).flatMap(clean)) })
    }
}
