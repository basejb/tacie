import SwiftUI

/// What is wrong with AI right now, in the order the brief sets: the meeting's own, the engine, then a key the engine
/// knows is refused. Keys count only while the app uses its own (with Tacie AI the engine holds none, and that is fine).
@MainActor
enum AIStatus {
    static func problem(_ context: SettingsContext, config: EngineConfig?) -> String? {
        if let t = context.meeting.trouble { return t }
        switch context.engine.state {
        case .failed: return String(localized: "검색 엔진에 연결하지 못했어요")
        case .starting: return String(localized: "검색 엔진 준비 중이에요")
        case .ready: break
        }
        guard !Relay.on else { return nil }
        let states = config?.keys.map(\.state) ?? []
        // a key the engine does not have is as bad as a refused one (OpenRouter does the transcribing and answers)
        let openrouterMissing = config?.keys.contains { $0.name.lowercased() == "openrouter" && $0.state == "missing" } ?? false
        if states.contains("refused") || openrouterMissing { return MeetingModel.say("key") }
        if states.contains("credit") { return MeetingModel.say("credit") }
        return nil
    }
}

/// 계정 → AI: only while something is wrong; nothing at all when AI works.
struct AIProblemSection: View {
    let context: SettingsContext
    @State private var config: EngineConfig?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let problem = AIStatus.problem(context, config: config) {
                SettingsSection(title: "AI") {
                    SettingsRow(title: problem, desc: String(localized: "화면 위 Tacie에도 같은 이유를 보여 줘요.")) {
                        Button { Task { await reload() } } label: { OutlineButton(label: String(localized: "다시 확인")) }.buttonStyle(PressStyle())
                    }
                }
            }
            Color.clear.frame(height: 0)  // keeps the task alive while the section is hidden
        }
        .task(id: context.engine.state) { await reload() }
    }

    private func reload() async {
        config = try? await context.engine.client?.config()
    }
}

struct AIPage: View {
    let context: SettingsContext
    @State private var config: EngineConfig?

    var body: some View {
        PageHeader(title: "AI", desc: String(localized: "Tacie는 생성형 AI로 회의를 받아쓰고 정리해요. 질문이 나오면 연결한 서비스에서 답이 적힌 곳을 찾아요."))
        if !Relay.on { keysSection }
        SettingsSection(title: String(localized: "상태")) {
            SettingsRow(title: String(localized: "연결 상태"), desc: String(localized: "문제가 생기면 여기와 화면 위 Tacie에 이유를 보여 줘요.")) {
                if let problem = AIStatus.problem(context, config: config) {
                    HStack(spacing: 5) {
                        Image(systemName: "exclamationmark.circle").uiFont(11, weight: .bold)
                        Text(problem).uiFont(12.5, weight: .medium).lineLimit(2).multilineTextAlignment(.trailing)
                    }
                    .foregroundStyle(Theme.ink2)
                    .frame(maxWidth: 260, alignment: .trailing)
                } else {
                    HStack(spacing: 5) {
                        Image(systemName: "checkmark").uiFont(11, weight: .bold)
                        Text("정상").uiFont(12.5, weight: .medium)
                    }
                    .foregroundStyle(Theme.ink)
                }
            }
        }
        .task(id: context.engine.state) {
            await reloadConfig()
        }
        .task { loadSaved() }
        .alert(removing.map { String(localized: "\(Self.title($0))를 지울까요?") } ?? "", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } })) {
            Button(String(localized: "ai.removeKey", defaultValue: "지우기"), role: .destructive) { if let name = removing { remove(name) } }
            Button("취소", role: .cancel) {}
        } message: {
            Text("이 Mac의 키체인에서 지워요. 다시 쓰려면 키를 새로 붙여 넣어야 해요.")
        }
    }

    // MARK: API keys (until the Tacie server relays AI, #7 5단계)

    @State private var saved: Set<AIKeys.Name> = []
    @State private var editing: AIKeys.Name?
    @State private var draft = ""
    @State private var note: (name: AIKeys.Name, text: String)?
    @State private var removing: AIKeys.Name?
    @FocusState private var fieldFocused: Bool

    private static func title(_ name: AIKeys.Name) -> String {
        switch name {
        case .openrouter: String(localized: "받아쓰기·답 키")
        // optional: without it the engine asks the same judge through OpenRouter (jev.rs `pick`)
        case .typesafe: String(localized: "질문 판단 키 · 선택")
        }
    }

    /// The one line under a key's title; the vendor is named only here, since the key must come from them.
    private static func desc(_ name: AIKeys.Name) -> String {
        switch name {
        case .openrouter: String(localized: "회의 받아쓰기와 답, 회의 정리에 써요. 꼭 필요해요. OpenRouter 키를 넣어요.")
        case .typesafe: String(localized: "질문인지 알아보고 찾은 곳의 순서를 정해요. 없으면 받아쓰기·답 키로 대신해요. TypeSafe 키가 있으면 넣어요.")
        }
    }

    private var keysSection: some View {
        SettingsSection(title: String(localized: "API 키"), desc: String(localized: "키는 이 Mac의 키체인에만 저장하고, 저장한 뒤에는 다시 보여 주지 않아요.")) {
            ForEach(AIKeys.Name.allCases, id: \.self) { name in
                VStack(alignment: .leading, spacing: 0) {
                    SettingsRow(title: Self.title(name), desc: Self.desc(name)) { keyTrailing(name) }
                    if let note, note.name == name {
                        Text(note.text).uiFont(12).foregroundStyle(Theme.warn)
                            .frame(maxWidth: .infinity, alignment: .trailing)
                            .padding(.top, -4).padding(.bottom, 8)
                    }
                }
                if name != AIKeys.Name.allCases.last {
                    Rectangle().fill(Theme.hairline).frame(height: 1)
                }
            }
        }
    }

    @ViewBuilder
    private func keyTrailing(_ name: AIKeys.Name) -> some View {
        if editing == name {
            HStack(spacing: 8) {
                // macOS draws the prompt in the field's font, and Korean spreads out in monospace: the UI font while
                // empty, monospaced once a key is typed
                SecureField("", text: $draft, prompt: Text("키 붙여 넣기"))
                    .textFieldStyle(.plain)
                    .uiFont(12.5, design: draft.isEmpty ? .default : .monospaced)
                    .textContentType(.oneTimeCode)  // an API key, not a website password: no Passwords autofill
                    .accessibilityLabel("\(Self.title(name)) 붙여 넣기")
                    .focused($fieldFocused)
                    .padding(.horizontal, 9)
                    .frame(width: 180, height: 28)
                    .background(Theme.paper, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).stroke(Theme.ink.opacity(fieldFocused ? 0.32 : 0.14)))
                    .onSubmit { save(name) }
                Button { save(name) } label: {
                    Text("저장").uiFont(12.5, weight: .medium).foregroundStyle(Theme.onStrong)
                        .padding(.horizontal, 13).frame(height: 28)
                        .background(Theme.ink, in: Capsule())
                }
                .buttonStyle(PressStyle())
                .fixedSize()
                .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .opacity(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? 0.35 : 1)
                Button("취소") { stopEditing() }
                    .buttonStyle(.plain).uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink3)
                    .fixedSize()
            }
            .fixedSize()
        } else {
            HStack(spacing: 12) {
                keyStatus(name)
                if saved.contains(name) {
                    Menu {
                        Button("다른 키로 바꾸기") { startEditing(name) }
                        Button(String(localized: "ai.removeKey", defaultValue: "지우기"), role: .destructive) { removing = name }
                    } label: {
                        Image(systemName: "ellipsis").uiFont(12, weight: .semibold).foregroundStyle(Theme.ink2)
                            .frame(width: 28, height: 28)
                            .background(Theme.fill, in: Circle())
                    }
                    .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
                    .accessibilityLabel("\(Self.title(name)) 더 보기")
                } else {
                    Button { startEditing(name) } label: { OutlineButton(label: String(localized: "키 넣기")) }.buttonStyle(PressStyle())
                }
            }
        }
    }

    /// "저장됨", or what the engine says is wrong with the saved key. Without a saved key, a development build may
    /// still be using a key file: that is said quietly.
    @ViewBuilder
    private func keyStatus(_ name: AIKeys.Name) -> some View {
        let engineKey = config?.keys.first { $0.name == name.rawValue }
        if saved.contains(name) {
            switch engineKey?.state {
            case "refused": statusLabel(String(localized: "거절된 키예요"), icon: "exclamationmark.circle", tint: Theme.warn)
            case "credit": statusLabel(String(localized: "잔액이 없어요"), icon: "exclamationmark.circle", tint: Theme.warn)
            default: statusLabel(String(localized: "저장됨"), icon: "checkmark", tint: Theme.ink2)
            }
        } else if Build.isDev, engineKey?.state == "ok", engineKey?.inApp == false {
            Text("키 파일 사용 중").uiFont(12.5).foregroundStyle(Theme.ink3)
        }
    }

    private func statusLabel(_ text: String, icon: String, tint: Color) -> some View {
        HStack(spacing: 5) {
            Image(systemName: icon).uiFont(11, weight: .bold)
            Text(text).uiFont(12.5, weight: .medium)
        }
        .foregroundStyle(tint)
    }

    private func startEditing(_ name: AIKeys.Name) {
        editing = name
        draft = ""
        note = nil
        DispatchQueue.main.async { fieldFocused = true }
    }

    private func stopEditing() {
        editing = nil
        draft = ""
    }

    private func loadSaved() {
        saved = Set(AIKeys.Name.allCases.filter(AIKeys.exists))
        // captures (dev build only, shown state only, the keychain is not touched): TACIE_DEBUG_AI_KEYS=edit opens the
        // first field, =saved shows both keys as saved
        switch Build.isDev ? ProcessInfo.processInfo.environment["TACIE_DEBUG_AI_KEYS"] : nil {
        case "edit": startEditing(.openrouter)
        case "saved": saved = Set(AIKeys.Name.allCases)
        default: break
        }
    }

    /// Keeps the key in the keychain and hands it to the running engine right away (no restart, even mid-meeting).
    private func save(_ name: AIKeys.Name) {
        do {
            try AIKeys.save(draft, as: name)
        } catch AIKeys.SaveError.notAKey {
            note = (name, String(localized: "ai.badKeyFormat", defaultValue: "키 형식이 아니에요. 복사한 키를 그대로 붙여 넣어 주세요."))
            return
        } catch {
            note = (name, String(localized: "키체인에 저장하지 못했어요."))
            return
        }
        saved.insert(name)
        stopEditing()
        Task { await pushAndReload() }
    }

    private func remove(_ name: AIKeys.Name) {
        removing = nil
        do {
            try AIKeys.remove(name)
        } catch {
            note = (name, String(localized: "키체인에서 지우지 못했어요."))
            return
        }
        saved.remove(name)
        Task { await pushAndReload() }
    }

    private func pushAndReload() async {
        await context.keeper.pushAIKeys()
        await reloadConfig()
    }

    private func reloadConfig() async {
        config = try? await context.engine.client?.config()
    }
}
