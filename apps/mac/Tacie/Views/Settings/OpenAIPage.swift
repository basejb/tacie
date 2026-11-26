import SwiftUI

/// The open edition's 2-D: Settings → AI. The same two cards as the onboarding; only people who want to pick providers
/// themselves go on to "Choose providers".
extension AIChoiceModel {
    /// The app's model: picks apply through `AIModeApply` with the settings context.
    convenience init(context: SettingsContext) {
        self.init(localSTT: context.localSTT) { mode, server in await AIModeApply.apply(mode, server: server, context: context) }
    }
}

struct OpenAIPage: View {
    let context: SettingsContext
    let custom: () -> Void
    @State private var choice: AIChoiceModel

    init(context: SettingsContext, custom: @escaping () -> Void) {
        self.context = context
        self.custom = custom
        _choice = State(initialValue: AIChoiceModel(context: context))
    }

    var body: some View {
        PageHeader(title: "AI", desc: String(localized: "Tacie가 회의를 받아쓰고 정리하는 방법이에요."))
        // 업체 직접 선택 in use: the scene names the picked providers; neither card is on, and a line says so
        AIPathScene(local: choice.open == .local, keyOn: choice.keyOn,
                    custom: choice.open == .custom ? AIPathScene.Custom(stt: AICustomPicks.stt(), chat: AICustomPicks.chat(),
                                                                         cards: AICustomPicks.cards(typesafeKey: AIKeys.exists(.typesafe))) : nil)
            .padding(.bottom, 14)
        if choice.open == .custom {
            Text("지금은 업체를 직접 골라 쓰고 있어요. 카드를 누르면 그 카드의 방법으로 바뀌어요.")
                .uiFont(12.5).foregroundStyle(Theme.ink2)
                .padding(.bottom, 12)
        }
        AIChoiceCards(model: choice, inSettings: true)
        Button(action: custom) {
            HStack(spacing: 4) {
                Text("업체 직접 선택").underline()
                Image(systemName: "chevron.right").uiFont(9, weight: .bold)
            }
        }
        .buttonStyle(.plain).uiFont(12.5).foregroundStyle(Theme.ink2)
        .padding(.top, 14)
    }
}

/// 오픈소스판 2-D: 직접 고르기. 하는 일마다 업체 하나. 키가 필요한 업체를 고를 때만 그 줄 아래에 키 칸이 생겨요. 무엇을
/// 바꾸든 모드는 "업체 직접 선택"가 돼요. 값은 2-A(`AIAddress`)·2-B(`SttProviderChoice`)·AI 키 그대로 저장해요.
struct AICustomPage: View {
    let context: SettingsContext
    let back: () -> Void

    typealias Stt = AICustomPicks.Stt
    typealias Chat = AICustomPicks.Chat
    typealias Cards = AICustomPicks.Cards

    // what is saved for 직접 고르기, not the mode running now: a change saves its own row only
    @State private var stt: Stt = AICustomPicks.stt()
    @State private var chat: Chat = AICustomPicks.chat()
    @State private var cards: Cards = AICustomPicks.cards(typesafeKey: AIKeys.exists(.typesafe))
    @State private var sttKey = ""
    @State private var region = SttProviderChoice.saved()?.region ?? "us"
    @State private var chatURL = AIAddress.saved()?.url ?? ""
    @State private var chatModel = AIAddress.saved()?.model ?? ""
    @State private var chatKey = ""
    @State private var typesafeKey = ""
    @State private var openRouterKey = ""
    /// Bumped when a key is saved or removed, so the lines read the Keychain again.
    @State private var keyTick = 0
    @State private var note: String?

    var body: some View {
        DetailHeader(back: "AI", action: back)
        PageHeader(title: String(localized: "업체 직접 선택"), desc: String(localized: "하는 일마다 다른 업체를 써요. 키는 이 Mac의 키체인에만 있어요."))
        // three rows, no section title: the page's title already says what they are
        let _ = keyTick  // the key lines below read the Keychain; a save or removal bumps this
        VStack(alignment: .leading, spacing: 0) {
            SettingsRow(title: String(localized: "음성 인식"), desc: String(localized: "회의 소리를 글로 바꿔요.")) {
                menu($stt, onChange: sttChanged, [(.openrouter, "OpenRouter"), (.device, String(localized: "이 Mac")), (.soniox, "Soniox"), (.elevenlabs, "ElevenLabs")])
            }
            if stt == .soniox || stt == .elevenlabs {
                keyLine($sttKey, prompt: stt == .soniox ? String(localized: "Soniox 키") : String(localized: "ElevenLabs 키"),
                        exists: SttProviderChoice.keyStore(for: stt == .soniox ? .soniox : .elevenlabs).exists(),
                        forget: { forget { try SttProviderChoice.keyStore(for: stt == .soniox ? .soniox : .elevenlabs).delete() } }) {
                    if stt == .soniox {
                        menu($region, onChange: {}, [("us", String(localized: "미국")), ("jp", String(localized: "일본")), ("eu", "EU")])
                    }
                } save: { saveStt() }
            }
            if firstOpenRouterRow == 0 { openRouterLine }
            SettingsRow(title: String(localized: "회의 정리"), desc: String(localized: "대화 기록으로 결정과 할 일을 정리해요. 답 카드의 답과 메인 창 AI도 이 업체가 써요.")) {
                menu($chat, onChange: chatChanged, [(.openrouter, "OpenRouter"), (.device, String(localized: "이 Mac")), (.openai, "OpenAI"), (.other, String(localized: "다른 주소"))])
            }
            if chat == .openai || chat == .other {
                if chat == .other {
                    HStack(spacing: 8) {
                        field($chatURL, prompt: "https://…/v1")
                        field($chatModel, prompt: String(localized: "모델")).frame(width: 140)
                    }
                    .padding(.bottom, 6)
                }
                keyLine($chatKey, prompt: chat == .openai ? String(localized: "OpenAI 키") : String(localized: "키(없으면 비워 둬요)"),
                        exists: keptKey(for: chat == .openai ? Self.openAI : chatURL) != nil,
                        forget: { forget { try AIAddress.keyStore.delete() } }) { EmptyView() } save: { saveChat() }
            }
            if firstOpenRouterRow == 1 { openRouterLine }
            SettingsRow(title: String(localized: "답 카드"), desc: String(localized: "찾아볼 질문인지 알아보고 찾은 자료의 순위를 매겨요. 끄면 답 카드가 뜨지 않아요.")) {
                menu($cards, onChange: cardsChanged, [(.openrouter, "OpenRouter"), (.typesafe, "TypeSafe"), (.off, String(localized: "끄기"))])
            }
            if cards == .typesafe {
                keyLine($typesafeKey, prompt: String(localized: "TypeSafe 키"), exists: AIKeys.exists(.typesafe),
                        forget: { forget { try AIKeys.remove(.typesafe) } }) { EmptyView() } save: { saveCards() }
            }
            if firstOpenRouterRow == 2 { openRouterLine }
        }
        if let note { Text(note).uiFont(12).foregroundStyle(Theme.ink3).padding(.top, 8) }
    }

    private func menu<T: Hashable>(_ value: Binding<T>, onChange: @escaping () -> Void, _ items: [(T, String)]) -> some View {
        ChoiceMenu(value: value, items: items, onChange: onChange)
    }

    private func field(_ text: Binding<String>, prompt: String, secure: Bool = false) -> some View {
        Group {
            if secure { SecureField("", text: text, prompt: Text(prompt)) } else { TextField("", text: text, prompt: Text(prompt)) }
        }
        .textFieldStyle(.plain).uiFont(12.5, design: .monospaced)
        .padding(.horizontal, 8).frame(height: 28)
        .background(Theme.ink.opacity(0.045), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
    }

    private func keyLine<Extra: View>(_ text: Binding<String>, prompt: String, exists: Bool, forget: (() -> Void)? = nil,
                                      @ViewBuilder extra: () -> Extra, save: @escaping () -> Void) -> some View {
        HStack(spacing: 8) {
            field(text, prompt: exists ? String(localized: "저장된 키를 쓰고 있어요") : prompt, secure: true).onSubmit(save)
            extra()
            Button(action: save) { OutlineButton(label: String(localized: "저장")) }.buttonStyle(PressStyle()).fixedSize()
            // 2026-11-24: a saved key can be taken out here (there was no way in the app)
            if exists, let forget {
                Button(String(localized: "지우기"), action: forget)
                    .buttonStyle(.plain).uiFont(12.5).underline().foregroundStyle(Theme.ink3).fixedSize()
            }
        }
        .padding(.bottom, 10)
    }

    /// The first row (0 음성 인식, 1 회의 정리, 2 답 카드) that uses OpenRouter while no OpenRouter key is saved: the
    /// key line goes under it, so a row never runs on a key the page has no place for.
    private var firstOpenRouterRow: Int? {
        guard !AIKeys.exists(.openrouter) else { return nil }
        return [stt == .openrouter, chat == .openrouter, cards == .openrouter].firstIndex(of: true)
    }

    private var openRouterLine: some View {
        keyLine($openRouterKey, prompt: String(localized: "OpenRouter 키"), exists: false) { EmptyView() } save: { saveOpenRouter() }
    }

    private func saveOpenRouter() {
        guard !openRouterKey.isEmpty else { return }
        do {
            try AIKeys.save(openRouterKey, as: .openrouter)
        } catch AIKeys.SaveError.notAKey {
            note = String(localized: "키 형식이 아니에요. 복사한 키를 그대로 붙여 넣어 주세요."); return
        } catch { note = String(localized: "키를 저장하지 못했어요"); return }
        openRouterKey = ""
        keyTick += 1
        apply()
    }

    /// Takes a saved key out of the Keychain and hands the engine what is left.
    private func forget(_ remove: () throws -> Void) {
        do { try remove() } catch { note = String(localized: "키를 지우지 못했어요"); return }
        keyTick += 1
        apply()
    }

    // MARK: saving

    // A menu changed: that row only. Items that need no key (or whose key is saved) take effect now; the others wait
    // for their key line's 저장.
    private func sttChanged() {
        switch stt {
        case .openrouter, .device: saveStt()
        default: if SttProviderChoice.keyStore(for: stt == .soniox ? .soniox : .elevenlabs).exists() { saveStt() }
        }
    }

    private func chatChanged() {
        // OpenAI takes effect now only when its own key is saved; another address's key never follows the switch
        if chat == .openrouter || chat == .device || (chat == .openai && AIAddress.keepsKey(saved: AIAddress.saved(), newURL: Self.openAI) && AIAddress.keyStore.exists()) { saveChat() }
    }

    private func cardsChanged() {
        if cards != .typesafe || AIKeys.exists(.typesafe) { saveCards() }
    }

    private func saveStt() {
        do {
            switch stt {
            case .openrouter, .device: try SttProviderChoice.save(nil, key: nil)
            case .soniox: try SttProviderChoice.save(SttProviderChoice(name: .soniox, region: region), key: sttKey)
            case .elevenlabs: try SttProviderChoice.save(SttProviderChoice(name: .elevenlabs, region: region), key: sttKey)
            }
            sttKey = ""
            if stt == .device { context.localSTT.turnOn(consented: true) } else if context.localSTT.enabled { context.localSTT.turnOff() }
            apply()
        } catch { note = String(localized: "키를 저장하지 못했어요") }
    }

    private static let openAI = "https://api.openai.com/v1"

    /// The saved key, only for the address it was saved for (`AIAddress.keepsKey`).
    private func keptKey(for url: String) -> String? {
        AIAddress.keepsKey(saved: AIAddress.saved(), newURL: url) ? AIAddress.keyStore.read() : nil
    }

    private func saveChat() {
        Task {
            do {
                switch chat {
                case .openrouter: try AIAddress.save(nil, key: nil)  // the engine's own OpenRouter notes, with the OpenRouter key
                case .device:
                    guard let s = await AIChecks.findLocalServer() else {
                        note = await AIChecks.idleServer().map { String(localized: "\($0)에서 모델을 하나 받으면 돼요") }
                            ?? String(localized: "Ollama나 LM Studio를 켜면 돼요")
                        return
                    }
                    try AIAddress.save(AIAddress(url: s.url, model: s.model), key: nil)
                    note = String(localized: "\(s.name)의 \(s.model)로 정리해요")
                case .openai: try AIAddress.save(AIAddress(url: Self.openAI, model: "gpt-5-mini"), key: chatKey.isEmpty ? keptKey(for: Self.openAI) : chatKey)
                case .other:
                    guard !chatURL.isEmpty, !chatModel.isEmpty else { note = String(localized: "주소와 모델을 넣어 주세요"); return }
                    try AIAddress.save(AIAddress(url: chatURL, model: chatModel), key: chatKey.isEmpty ? keptKey(for: chatURL) : chatKey)
                }
                chatKey = ""
                apply()
            } catch { note = String(localized: "키를 저장하지 못했어요") }
        }
    }

    private func saveCards() {
        switch cards {
        case .openrouter: CardsChoice.save(.openrouter)
        case .off: CardsChoice.save(.off)
        case .typesafe:
            if !typesafeKey.isEmpty {
                do { try AIKeys.save(typesafeKey, as: .typesafe) } catch { note = String(localized: "키를 저장하지 못했어요"); return }
                typesafeKey = ""
            }
            CardsChoice.save(.auto)
        }
        apply()
    }

    private func apply() {
        Task { await AIModeApply.apply(.custom, context: context) }
    }
}
