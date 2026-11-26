import Foundation

/// The open edition's AI address (docs/41 0절, 2-A): the OpenAI-compatible address (OpenRouter, OpenAI, or this Mac's
/// Ollama · LM Studio) and model that meeting notes, the main window's AI and changed decisions are sent to. The key is
/// in the keychain as `ai.tacie.chat`. One per Mac (like the AI keys); it reaches the engine in memory only, in the
/// same request as the AI keys (`chat` in `/ai/credentials`). The official app does not use it (the relay wins).
struct AIAddress: Equatable {
    var url: String
    var model: String

    static let urlKey = "ai.chat.url"
    static let modelKey = "ai.chat.model"
    static let keyStore = KeychainTokenStore(service: "ai.tacie.chat", account: "apiKey", label: "Tacie AI address key")

    /// 저장된 주소예요. 주소나 모델이 비어 있으면 없어요.
    static func saved(_ d: UserDefaults = .standard) -> AIAddress? {
        let url = d.string(forKey: urlKey)?.trimmingCharacters(in: .whitespaces) ?? ""
        let model = d.string(forKey: modelKey)?.trimmingCharacters(in: .whitespaces) ?? ""
        return url.isEmpty || model.isEmpty ? nil : AIAddress(url: url, model: model)
    }

    static var isSet: Bool { saved() != nil }

    /// Whether the saved key may be kept for `newURL`: only when it is the address the key was saved for. A key never
    /// follows a switch to another provider's address.
    static func keepsKey(saved: AIAddress?, newURL: String) -> Bool {
        guard let saved else { return false }
        return saved.url.trimmingCharacters(in: .whitespaces) == newURL.trimmingCharacters(in: .whitespaces)
    }

    /// 주소와 키를 저장하거나(nil이면) 지워요. 빈 키는 키를 지워요(로컬 서버는 키가 없어요).
    static func save(_ address: AIAddress?, key: String?, _ d: UserDefaults = .standard) throws {
        if let address {
            d.set(address.url, forKey: urlKey)
            d.set(address.model, forKey: modelKey)
        } else {
            d.removeObject(forKey: urlKey)
            d.removeObject(forKey: modelKey)
        }
        if let key = key?.trimmingCharacters(in: .whitespacesAndNewlines), !key.isEmpty, address != nil {
            try keyStore.save(key)
        } else {
            try keyStore.delete()
        }
    }

    /// 엔진에 넘기는 `chat` 값이에요: 주소가 없거나 중계를 쓰는 동안(`Relay.on`)은 NSNull이에요.
    static func payload(address: AIAddress?, key: String?, relay: Bool) -> Any {
        guard let address, !relay else { return NSNull() }
        var out = ["url": address.url, "model": address.model]
        if let key, !key.isEmpty { out["key"] = key }
        return out
    }

    /// What the engine is handed now: the open edition's AI mode may override the saved address (2-D, `AIMode`).
    @MainActor static var current: Any { AIMode.current.chat }
}
