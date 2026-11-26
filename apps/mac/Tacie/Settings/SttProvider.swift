import Foundation

/// The open edition's transcription provider (docs/41 0절, 2-B): Soniox (with region) or ElevenLabs, with the user's
/// key. Each provider's key is in the keychain (`ai.tacie.soniox`, `ai.tacie.elevenlabs`), one per Mac. It reaches the
/// engine in memory only, in the same request as the AI keys (`stt` in `/ai/credentials`). The official app does not
/// use it (the relay wins). On-device stays on the current switch (`LocalSTT`).
enum SttProviderName: String, CaseIterable { case soniox, elevenlabs }

struct SttProviderChoice: Equatable {
    var name: SttProviderName
    /// Soniox만: `us`, `jp`, `eu`.
    var region: String

    static let providerKey = "stt.provider"
    static let regionKey = "stt.sonioxRegion"

    static func keyStore(for name: SttProviderName) -> KeychainTokenStore {
        KeychainTokenStore(service: "ai.tacie.\(name.rawValue)", account: "apiKey",
                           label: name == .soniox ? "Tacie Soniox API key" : "Tacie ElevenLabs API key")
    }

    static func saved(_ d: UserDefaults = .standard) -> SttProviderChoice? {
        guard let name = d.string(forKey: providerKey).flatMap(SttProviderName.init(rawValue:)) else { return nil }
        return SttProviderChoice(name: name, region: d.string(forKey: regionKey) ?? "us")
    }

    /// 업체와 그 키를 저장하거나(nil이면) 업체를 지워요. 빈 키는 그 업체의 키를 그대로 둬요.
    static func save(_ choice: SttProviderChoice?, key: String?, _ d: UserDefaults = .standard) throws {
        guard let choice else {
            d.removeObject(forKey: providerKey)
            return
        }
        d.set(choice.name.rawValue, forKey: providerKey)
        d.set(choice.region, forKey: regionKey)
        if let key = key?.trimmingCharacters(in: .whitespacesAndNewlines), !key.isEmpty {
            try keyStore(for: choice.name).save(key)
        }
    }

    /// 엔진에 넘기는 `stt` 값이에요: 업체나 키가 없거나 중계를 쓰는 동안(`Relay.on`)은 NSNull이에요.
    static func payload(choice: SttProviderChoice?, key: String?, relay: Bool) -> Any {
        guard let choice, let key, !key.isEmpty, !relay else { return NSNull() }
        var out = ["provider": choice.name.rawValue, "key": key]
        if choice.name == .soniox { out["region"] = choice.region }
        return out
    }

    /// What the engine is handed now: the open edition's AI mode may override the saved provider (2-D, `AIMode`).
    @MainActor static var current: Any { AIMode.current.stt }

    /// 받아쓰기 경로를 고르는 값(`ShadowWiring.path`): 엔진에 넘기는 업체와 같은 조건이에요(키가 있고 중계를 쓰지
    /// 않을 때). 아니면 nil이라 지금 경로예요.
    static func pathProvider(choice: SttProviderChoice?, key: String?, relay: Bool) -> String? {
        guard let choice, let key, !key.isEmpty, !relay else { return nil }
        return choice.name.rawValue
    }

    @MainActor static var pathProvider: String? { AIMode.current.sttPath }
}
