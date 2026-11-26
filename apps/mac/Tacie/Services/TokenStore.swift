import Foundation
import Security

protocol TokenStore {
    func read() -> String?
    /// Like `read`, but a keychain that could not answer (locked, refused) throws instead of reading as "none".
    func readChecked() throws -> String?
    func save(_ token: String) throws
    func delete() throws
}

/// A secret that lives only in the login keychain: the GitHub token (`ai.tacie.github`) and the AI keys
/// (`ai.tacie.typesafe`, `ai.tacie.openrouter`, see AIKeys). It is never logged. A development build adds `.dev` to
/// the service (Build.keychainSuffix), so it never touches the installed app's items.
extension TokenStore {
    func readChecked() throws -> String? { read() }
}

struct KeychainTokenStore: TokenStore {
    /// The GitHub connection of the account in use (AccountSpace): `token` signed out, `token@<id>` signed in.
    static var github: KeychainTokenStore { KeychainTokenStore(account: AccountSpace.inUse.key("token")) }

    struct Failure: Error { let status: OSStatus }
    var service = "ai.tacie.github"
    var account = "token"
    /// What Keychain Access shows for the item (set when it is first saved).
    var label: String? = nil

    private var query: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service + Build.keychainSuffix,
         kSecAttrAccount as String: account]
    }

    func read() -> String? {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    func readChecked() throws -> String? {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        let status = SecItemCopyMatching(q as CFDictionary, &out)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = out as? Data else { throw Failure(status: status) }
        return String(data: data, encoding: .utf8)
    }

    /// Whether the item is there, from its attributes only: the secret is not read.
    func exists() -> Bool {
        var q = query
        q[kSecReturnAttributes as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        return SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess
    }

    func save(_ token: String) throws {
        let data = Data(token.utf8)
        let updated = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if updated == errSecSuccess { return }
        guard updated == errSecItemNotFound else { throw Failure(status: updated) }
        var add = query
        add[kSecValueData as String] = data
        if let label { add[kSecAttrLabel as String] = label }
        let status = SecItemAdd(add as CFDictionary, nil)
        guard status == errSecSuccess else { throw Failure(status: status) }
    }

    func delete() throws {
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw Failure(status: status) }
    }
}
