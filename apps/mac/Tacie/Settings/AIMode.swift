import Foundation

/// The open edition's 2-D: the one way the person chose (docs/41 0절). `key` is transcription, notes and answer cards
/// with a single OpenRouter key; `local` is On-device processing (on-device transcription + the Ollama or LM Studio
/// found); `custom` is the values on the Choose providers page as they are (2-A address, 2-B provider, AI keys). The
/// mode does not clear the saved Choose providers values; it only overrides what is passed to the engine and the
/// transcription path (`effective`). The official app does not read the mode. One per Mac (like `AIAddress`).
enum AIMode: String {
    case key, local, custom

    static let defaultsKey = "ai.mode"

    static func saved(_ d: UserDefaults = .standard) -> AIMode? {
        d.string(forKey: defaultsKey).flatMap(AIMode.init(rawValue:))
    }

    static func save(_ mode: AIMode?, _ d: UserDefaults = .standard) {
        if let mode { d.set(mode.rawValue, forKey: defaultsKey) } else { d.removeObject(forKey: defaultsKey) }
    }

    /// What the engine is handed and how a meeting transcribes.
    struct EngineValues {
        var keys: [AIKeys.Name: String?]
        /// `/ai/credentials` `chat`: a `[String: String]` or NSNull.
        var chat: Any
        /// `/ai/credentials` `stt`: a `[String: String]` or NSNull.
        var stt: Any
        /// The provider `ShadowWiring.path` follows, or nil.
        var sttPath: String?
        /// Whether on-device recognition is on.
        var localStt: Bool
    }

    /// `local` hands no key at all, so nothing can leave the Mac even when keys are still saved for `custom`.
    static func effective(mode: AIMode?, open: Bool, relay: Bool, keys: [AIKeys.Name: String?], address: AIAddress?,
                          addressKey: String?, provider: SttProviderChoice?, providerKey: String?, localSwitch: Bool,
                          server: LocalServer?, cards: CardsChoice) -> EngineValues {
        let today = EngineValues(
            keys: keys, chat: AIAddress.payload(address: address, key: addressKey, relay: relay),
            stt: SttProviderChoice.payload(choice: provider, key: providerKey, relay: relay),
            sttPath: SttProviderChoice.pathProvider(choice: provider, key: providerKey, relay: relay),
            localStt: localSwitch)
        guard open, !relay, let mode else { return today }
        switch mode {
        case .key:
            return EngineValues(keys: [.openrouter: keys[.openrouter] ?? nil, .typesafe: nil], chat: NSNull(),
                                stt: NSNull(), sttPath: nil, localStt: false)
        case .local:
            let chat = AIAddress.payload(address: server.map { AIAddress(url: $0.url, model: $0.model) }, key: nil,
                                         relay: false)
            return EngineValues(keys: [.openrouter: nil, .typesafe: nil], chat: chat, stt: NSNull(), sttPath: nil,
                                localStt: true)
        case .custom:
            var out = today
            if cards == .openrouter { out.keys[.typesafe] = .some(nil) }
            return out
        }
    }

    @MainActor static var current: EngineValues {
        let provider = SttProviderChoice.saved()
        return effective(mode: saved(), open: Build.isOpen, relay: Relay.on, keys: AIKeys.saved(),
                         address: AIAddress.saved(), addressKey: AIAddress.keyStore.read(), provider: provider,
                         providerKey: provider.flatMap { SttProviderChoice.keyStore(for: $0.name).read() },
                         localSwitch: UserDefaults.standard.bool(forKey: LocalSTT.key), server: LocalServer.saved(),
                         cards: CardsChoice.saved())
    }

    /// Whether on-device recognition is on, without reading any key (`LocalSTT.switchedOn`).
    @MainActor static var localSttOn: Bool {
        let on = UserDefaults.standard.bool(forKey: LocalSTT.key)
        guard Build.isOpen, !Relay.on, let mode = saved() else { return on }
        switch mode {
        case .key: return false
        case .local: return true
        case .custom: return on
        }
    }

    /// Whether the engine is talking to an address of the person's own (it waits longer, 2-A). UserDefaults only: it is
    /// asked on every engine request, off the main thread.
    static func addressSet(_ d: UserDefaults = .standard, open: Bool = Build.isOpen) -> Bool {
        guard open, let mode = saved(d) else { return AIAddress.saved(d) != nil }
        switch mode {
        case .key: return false
        case .local: return LocalServer.saved(d) != nil
        case .custom: return AIAddress.saved(d) != nil
        }
    }
}

/// Posted whenever the AI choice reaches the engine again (IndexKeeper.pushAIKeys).
enum AIChoice {
    static let changed = Notification.Name("tacie.aiChoiceChanged")
}

/// The Ollama or LM Studio found on this Mac for `local` (UserDefaults `ai.local.*`).
struct LocalServer: Equatable {
    var name: String
    var url: String
    var model: String

    static let nameKey = "ai.local.name"
    static let urlKey = "ai.local.url"
    static let modelKey = "ai.local.model"

    static func saved(_ d: UserDefaults = .standard) -> LocalServer? {
        guard let name = d.string(forKey: nameKey), let url = d.string(forKey: urlKey),
              let model = d.string(forKey: modelKey), !url.isEmpty, !model.isEmpty else { return nil }
        return LocalServer(name: name, url: url, model: model)
    }

    static func save(_ server: LocalServer?, _ d: UserDefaults = .standard) {
        if let server {
            d.set(server.name, forKey: nameKey)
            d.set(server.url, forKey: urlKey)
            d.set(server.model, forKey: modelKey)
        } else {
            for k in [nameKey, urlKey, modelKey] { d.removeObject(forKey: k) }
        }
    }
}

/// 직접 고르기의 답 카드 줄: `auto`는 엔진이 고르게(TypeSafe가 있으면 TypeSafe), `openrouter`는 TypeSafe 키를 넘기지
/// 않고, `off`는 회의를 기록만으로 시작해요(`CardsGate`).
enum CardsChoice: String {
    case auto, openrouter, off

    static let defaultsKey = "ai.cards"

    static func saved(_ d: UserDefaults = .standard) -> CardsChoice {
        d.string(forKey: defaultsKey).flatMap(CardsChoice.init(rawValue:)) ?? .auto
    }

    static func save(_ c: CardsChoice, _ d: UserDefaults = .standard) { d.set(c.rawValue, forKey: defaultsKey) }
}

/// Applying a mode (2-D). The pure parts live here; `apply` is in `AIModeApply+App.swift` (it needs the engine).
enum AIModeApply {
    /// Whether the engine may read the development `~/.config` key files: a dev build, except the open edition's
    /// "Mac 안에서만" (nothing may leave the Mac, not even through a developer's key file).
    static func keyFiles(dev: Bool, mode: AIMode?, open: Bool) -> Bool {
        dev && !(open && mode == .local)
    }

    /// The engine reads key files only at launch: a mode change that flips them needs a new engine.
    static func needsRestart(from: AIMode?, to: AIMode?, dev: Bool, open: Bool) -> Bool {
        keyFiles(dev: dev, mode: from, open: open) != keyFiles(dev: dev, mode: to, open: open)
    }
}

/// 직접 고르기's three rows as saved for `custom`, whatever mode runs now: changing one row saves that row only and
/// never rewrites the others from the mode (2-D final review).
enum AICustomPicks {
    enum Stt: String { case openrouter, device, soniox, elevenlabs }
    enum Chat: String { case openrouter, device, openai, other }
    enum Cards: String { case openrouter, typesafe, off }

    static func stt(_ d: UserDefaults = .standard) -> Stt {
        if d.bool(forKey: LocalSTT.key) { return .device }
        switch SttProviderChoice.saved(d)?.name {
        case .soniox: return .soniox
        case .elevenlabs: return .elevenlabs
        case nil: return .openrouter
        }
    }

    static func chat(_ d: UserDefaults = .standard) -> Chat {
        guard let a = AIAddress.saved(d) else { return .openrouter }
        if a.url.contains("127.0.0.1") || a.url.contains("localhost") { return .device }
        if a.url.hasPrefix("https://api.openai.com") { return .openai }
        if a.url.hasPrefix("https://openrouter.ai") { return .openrouter }
        return .other
    }

    static func cards(_ d: UserDefaults = .standard, typesafeKey: Bool) -> Cards {
        switch CardsChoice.saved(d) {
        case .off: return .off
        case .openrouter: return .openrouter
        case .auto: return typesafeKey ? .typesafe : .openrouter
        }
    }
}
