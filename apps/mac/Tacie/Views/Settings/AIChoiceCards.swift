import SwiftUI

/// The open edition's 2-D: the two cards of "How do you want to run Tacie?" (shared by the onboarding and Settings →
/// AI). Only the pressed card opens, and downloading and finding run inside that card. Provider, address and model are
/// not offered here (they are on the Choose providers page).
@Observable
@MainActor
final class AIChoiceModel {
    enum KeyState: Equatable { case empty, checking, ok, saved, rejected, unknown }
    enum Search: Equatable { case idle, searching, found(LocalServer), none, noModel(String) }

    let localSTT: LocalSTT
    /// Applies a chosen mode (`AIModeApply.apply` in the app; a stand-in in the render tests).
    let apply: @MainActor (AIMode, LocalServer?) async -> Void
    /// The card that is open: the chosen mode, or 키 하나로 쓰기 before anything is chosen. `.custom` (직접 고르기 in use)
    /// opens neither card, so picking either one switches back to it.
    var open: AIMode
    var keyDraft = ""
    /// Set only here and by the render tests.
    var keyState: KeyState
    /// Set only here and by the render tests.
    var search: Search
    /// 받기 (the notes model through this Mac's Ollama). Set only here and by the render tests.
    enum Pull: Equatable { case idle, pulling(done: Int64, total: Int64), failed(String) }
    var pull: Pull = .idle
    private var pulling: Task<Void, Never>?
    private var checking: Task<Void, Never>?
    private var searching: Task<Void, Never>?

    init(localSTT: LocalSTT, apply: @escaping @MainActor (AIMode, LocalServer?) async -> Void) {
        self.localSTT = localSTT
        self.apply = apply
        open = .key
        keyState = .empty
        search = .idle
        refresh()
    }

    /// Back to what is saved (the cards show again after the mode changed elsewhere), unless a check is under way.
    func refresh() {
        guard keyState != .checking, search != .searching else { return }
        let mode = AIMode.saved()
        open = mode == .local ? .local : (mode == .custom ? .custom : .key)
        keyState = mode == .key && AIKeys.exists(.openrouter) ? .saved : .empty
        search = mode == .local ? (LocalServer.saved().map { .found($0) } ?? .none) : .idle
    }

    var mode: AIMode? { AIMode.saved() }
    /// The 키 card's key works, is saved, or is being checked: the AI page's scene draws the path to OpenRouter.
    var keyOn: Bool { [.checking, .ok, .saved, .unknown].contains(keyState) }
    var localSupported: Bool { localSTT.phase != .unsupported }

    /// 키 지우기: the OpenRouter key leaves the Keychain and the engine; the 키 card waits for a new one.
    func forgetKey() {
        checking?.cancel()
        try? AIKeys.remove(.openrouter)
        keyDraft = ""
        keyState = .empty
        Task { if open == .key { await apply(.key, nil) } }
    }

    func pick(_ card: AIMode) {
        guard card != open else { return }
        if card == .local && !localSupported { return }
        // the other card's check or search, still under way, must not apply its mode after this pick
        checking?.cancel()
        searching?.cancel()
        if keyState == .checking { keyState = .empty }
        if search == .searching { search = .idle }
        open = card
        if card == .local { chooseLocal() }
        // a key saved earlier: back to it at once, nothing to paste again
        if card == .key, AIKeys.exists(.openrouter) {
            keyState = .saved
            Task { if open == .key { await apply(.key, nil) } }
        }
    }

    /// "Mac 안에서만" with nothing found yet: the page looks again each time it opens, so an Ollama installed (or given a
    /// model) since then is picked up without choosing the card again (오픈소스판 e2e, 2026-11-25). Not in `refresh`: the
    /// render tests build the model without a Mac to search.
    func lookAgain() {
        guard open == .local, AIMode.saved() == .local, LocalServer.saved() == nil, search != .searching else { return }
        chooseLocal()
    }

    /// 받기: this Mac's Ollama downloads the notes model; once it has, the Mac is searched again and the notes use it.
    /// The disk is checked first, as for the search model.
    func pullNotesModel() {
        guard pulling == nil else { return }
        let need = AIChecks.notesModelSize + 1_000_000_000 - AIChecks.freeSpace()
        if need > 0 {
            pull = .failed(String(localized: "디스크 공간이 \(ByteCountFormatter.string(fromByteCount: need, countStyle: .file)) 더 필요해요"))
            return
        }
        pull = .pulling(done: 0, total: AIChecks.notesModelSize)
        pulling = Task {
            let ok = await AIChecks.pullFromOllama(AIChecks.notesModel) { [weak self] p in
                guard let self, p.total > 0 else { return }
                self.pull = .pulling(done: p.done, total: p.total)
            }
            pulling = nil
            guard !Task.isCancelled else { return }
            if ok {
                pull = .idle
                if open == .local { chooseLocal() }
            } else {
                pull = .failed(String(localized: "받지 못했어요. Ollama가 켜져 있는지 확인한 뒤 다시 받아 주세요."))
            }
        }
    }

    /// Choosing the card is the consent: the speech model starts (after the search model) and the Mac is searched.
    func chooseLocal() {
        search = .searching
        searching = Task {
            let found = await AIChecks.findLocalServer()
            let idle = found == nil ? await AIChecks.idleServer() : nil
            guard !Task.isCancelled, open == .local else { return }
            await apply(.local, found)
            search = found.map { .found($0) } ?? idle.map { .noModel($0) } ?? .none
        }
    }

    /// A paste or typing: checked 0.7 s after the last change, saved and applied only when OpenRouter does not refuse it.
    func keyChanged() {
        checking?.cancel()
        guard let key = AIKeys.clean(keyDraft) else {
            keyState = mode == .key && AIKeys.exists(.openrouter) ? .saved : .empty
            return
        }
        keyState = .checking
        checking = Task {
            try? await Task.sleep(for: .milliseconds(700))
            guard !Task.isCancelled else { return }
            let result = await AIChecks.checkOpenRouter(key)
            guard !Task.isCancelled, open == .key else { return }
            switch result {
            case .rejected:
                keyState = .rejected
            case .ok, .unknown:
                do { try AIKeys.save(key, as: .openrouter) } catch { keyState = .unknown; return }
                await apply(.key, nil)
                keyState = result == .ok ? .ok : .unknown
            }
        }
    }
}

struct AIChoiceCards: View {
    @Bindable var model: AIChoiceModel
    /// 설정: the active card says "쓰고 있어요"; 온보딩: the 키 card carries the 추천 tag.
    var inSettings = false

    var body: some View {
        cards.onAppear { model.refresh(); model.lookAgain() }
    }

    private var cards: some View {
        VStack(spacing: 10) {
            card(.key, symbol: "key", title: String(localized: "aiCard.key", defaultValue: "API 키"),
                 desc: inSettings ? String(localized: "OpenRouter 키 하나로 음성 인식, 회의 정리, 답 카드가 모두 돼요.")
                    : String(localized: "OpenRouter 키를 붙여 넣으면 음성 인식, 회의 정리, 답 카드가 모두 돼요."),
                 tag: inSettings ? nil : String(localized: "추천")) { keyBody }
            card(.local, symbol: "lock", title: String(localized: "온디바이스 처리"),
                 desc: model.localSupported ? String(localized: "키가 필요 없어요. 회의 소리와 대화 기록을 Mac 밖으로 보내지 않아요. 답 카드는 키가 필요해서 이 방법에서는 쓸 수 없어요.")
                    : String(localized: "이 Mac은 메모리가 16GB보다 작아서 쓸 수 없어요."),
                 tag: nil) { localBody }
                .opacity(model.localSupported ? 1 : 0.5)
        }
    }

    private func card<Body: View>(_ kind: AIMode, symbol: String, title: String, desc: String, tag: String?,
                                  @ViewBuilder content: () -> Body) -> some View {
        let on = model.open == kind
        return VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Image(systemName: symbol).uiFont(13, weight: .semibold)
                Text(title).uiFont(15, weight: .bold)
                if let tag {
                    Text(tag).uiFont(11, weight: .semibold).foregroundStyle(Theme.ink2)
                        .padding(.horizontal, 8).padding(.vertical, 2)
                        .background(Theme.ink.opacity(0.08), in: Capsule())
                }
            }
            .foregroundStyle(Theme.ink)
            Text(desc).uiFont(13).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            if on { content().padding(.top, 8) }
        }
        .padding(.horizontal, 18).padding(.vertical, 16)
        .frame(maxWidth: .infinity, alignment: .leading)
        // the chosen card lifts as paper; the other stays a flat cell (docs/29 5절 모양) — no thick outline
        .background {
            let shape = RoundedRectangle(cornerRadius: 16, style: .continuous)
            if on { shape.fill(Theme.paper).softShadow() } else { shape.fill(Theme.fill) }
        }
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Theme.hairline))
        .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .onTapGesture { model.pick(kind) }
        .accessibilityElement(children: .contain)
        .accessibilityAddTraits(on ? [.isSelected] : [])
    }

    // MARK: 키 하나로 쓰기

    private var keyBody: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 10) {
                SecureField("", text: $model.keyDraft, prompt: Text(model.keyState == .saved ? String(localized: "저장된 키를 쓰고 있어요") : String(localized: "sk-or-… 붙여 넣기")))
                    .textFieldStyle(.plain)
                    .uiFont(13, design: .monospaced)
                    .padding(.horizontal, 10)
                    .frame(height: 32)
                    .background(Theme.ink.opacity(0.05), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .onChange(of: model.keyDraft) { model.keyChanged() }
                keyStatus
            }
            if model.keyState == .ok {
                // what the key turned on, so the card says what happens next (2026-11-24)
                Text("음성 인식, 회의 정리, 답 카드가 켜졌어요. 이제 회의를 시작하면 돼요.")
                    .uiFont(12.5).foregroundStyle(Theme.ink2)
                    .fixedSize(horizontal: false, vertical: true)
                    .transition(.opacity)
            } else if !inSettings || model.keyState == .empty || model.keyState == .rejected {
                // one paragraph, so a narrow window (onboarding, English) wraps it as a sentence
                Text(Self.noKeyHint)
                    .uiFont(12).foregroundStyle(Theme.ink3).tint(Theme.ink2)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    static var noKeyHint: AttributedString {
        var a = AttributedString(localized: "키가 없으면 [openrouter.ai에서 만들기](https://openrouter.ai/settings/keys) · 쓴 만큼 OpenRouter에 내요")
        for run in a.runs where run.link != nil { a[run.range].underlineStyle = .single }
        return a
    }

    @ViewBuilder private var keyStatus: some View {
        switch model.keyState {
        case .empty: EmptyView()
        case .checking:
            HStack(spacing: 6) { ProgressView().controlSize(.mini); Text("확인하는 중") }
                .uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize()
        case .ok: HStack(spacing: 10) { DoneMark(text: String(localized: "됐어요")).fixedSize(); forget }
        case .saved: HStack(spacing: 10) { status(String(localized: "쓰고 있어요")); forget }
        case .rejected: Text("이 키는 쓸 수 없어요").uiFont(12.5, weight: .medium).foregroundStyle(Theme.warn).fixedSize()
        case .unknown: Text("지금 확인할 수 없어요").uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize()
        }
    }

    /// Removes the saved key (2026-11-24: there was no way to take a key out of the app).
    private var forget: some View {
        Button(String(localized: "지우기")) { model.forgetKey() }
            .buttonStyle(.plain).uiFont(12.5).underline().foregroundStyle(Theme.ink3).fixedSize()
    }

    private func status(_ text: String) -> some View {
        HStack(spacing: 6) { DrawnCheck(size: 17); Text(text).uiFont(12.5).foregroundStyle(Theme.ink2) }.fixedSize()
    }

    // MARK: Mac 안에서만 쓰기

    private var localBody: some View {
        VStack(spacing: 0) {
            Divider().opacity(0.6).padding(.bottom, 6)
            row(String(localized: "음성 인식 모델")) { modelState }
            row(String(localized: "회의 정리")) { serverState }
            // 설정 only: the onboarding window is a fixed height and the block would run under its 계속 button; an
            // on-device meeting started without a model points here (`OpenAIMeeting.noModelAtStart`)
            if inSettings, let guide = LocalModelGuide.shown(for: model) { guide.padding(.top, 8) }
        }
    }

    private func row<V: View>(_ label: String, @ViewBuilder value: () -> V) -> some View {
        HStack(spacing: 10) {
            Text(label).uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink)
            Spacer(minLength: 8)
            value()
        }
        .frame(minHeight: 26)
    }

    @ViewBuilder private var modelState: some View {
        let stt = model.localSTT
        switch stt.phase {
        case .ready: status(String(localized: "받았어요"))
        case .downloading(let done, let total):
            if stt.waitingForSearch {
                Text("검색 모델을 받은 뒤에 받아요").uiFont(12.5).foregroundStyle(Theme.ink2)
            } else {
                HStack(spacing: 8) {
                    ProgressView(value: Double(done), total: Double(max(total, 1)))
                        .progressViewStyle(.linear).tint(Theme.ink).frame(width: 120)
                    Text("\(LocalSTTCopy.gb(total)) 중 \(LocalSTTCopy.gb(done))").uiFont(12.5).monospacedDigit()
                        .foregroundStyle(Theme.ink2)
                }
            }
        case .needsSpace(let more):
            Text("디스크 공간이 \(LocalSTTCopy.gb(more)) 더 필요해요").uiFont(12.5).foregroundStyle(Theme.warn)
        case .waiting: Text("인터넷이 연결되면 이어서 받아요").uiFont(12.5).foregroundStyle(Theme.ink2)
        case .failed:
            HStack(spacing: 8) {
                Text("받지 못했어요").uiFont(12.5).foregroundStyle(Theme.ink2)
                Button("다시 받기") { stt.turnOn(consented: true) }.buttonStyle(.plain)
                    .uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink)
            }
        case .off, .unsupported:
            HStack(spacing: 6) { ProgressView().controlSize(.mini); Text("준비하는 중") }.uiFont(12.5).foregroundStyle(Theme.ink2)
        }
    }

    @ViewBuilder private var serverState: some View {
        switch model.search {
        case .idle, .searching:
            HStack(spacing: 6) { ProgressView().controlSize(.mini); Text("이 Mac에서 Ollama나 LM Studio를 찾는 중") }
                .uiFont(12.5).foregroundStyle(Theme.ink2)
        case .found(let s): status(String(localized: "\(s.name)의 \(LocalModelGuide.shownName(s.model))로 정리해요"))
        case .none:
            HStack(spacing: 0) {
                Text("Ollama나 LM Studio를 켜면 돼요 · ")
                Link(String(localized: "설치 방법"), destination: URL(string: "https://ollama.com/download")!).underline()
            }
            .uiFont(12.5).foregroundStyle(Theme.ink2).tint(Theme.ink2)
        case .noModel(let name) where !inSettings && name != "LM Studio":
            // 온보딩: the fixed-height window has room for this row only, so 받기 sits in it (설정 has the full block)
            inlinePull
        case .noModel(let name):
            // on but empty (just installed): it needs a model, not to be turned on
            Text("\(name)에서 모델을 하나 받으면 돼요").uiFont(12.5).foregroundStyle(Theme.ink2)
        }
    }

    /// 받기 in one row, as tall as the other rows (온보딩).
    @ViewBuilder private var inlinePull: some View {
        switch model.pull {
        case .idle:
            HStack(spacing: 8) {
                Text(verbatim: "Gemma 4 E4B · \(ByteCountFormatter.string(fromByteCount: AIChecks.notesModelSize, countStyle: .file))")
                    .uiFont(12.5).foregroundStyle(Theme.ink2)
                smallButton(String(localized: "받기"))
            }
        case .pulling(let done, let total):
            HStack(spacing: 6) {
                ProgressView().controlSize(.mini)
                Text(verbatim: "\(ByteCountFormatter.string(fromByteCount: done, countStyle: .file)) / \(ByteCountFormatter.string(fromByteCount: total, countStyle: .file))")
                    .uiFont(12.5).foregroundStyle(Theme.ink2).monospacedDigit()
            }
        case .failed:
            HStack(spacing: 8) {
                Text("받지 못했어요").uiFont(12.5).foregroundStyle(Theme.ink2)
                smallButton(String(localized: "다시 받기"))
            }
        }
    }

    private func smallButton(_ label: String) -> some View {
        Button { model.pullNotesModel() } label: {
            Text(label).uiFont(12, weight: .semibold).foregroundStyle(Theme.ink)
                .padding(.horizontal, 10).padding(.vertical, 3)
                .background(Theme.ink.opacity(0.08), in: Capsule())
        }
        .buttonStyle(PressStyle())
    }
}

/// What to get for 회의 정리 when "Mac 안에서만" has no model yet (오픈소스판 e2e, 2026-11-25): one model, a 받기 that
/// has this Mac's Ollama download it (no Terminal), and what a model this small misses. The pick and the numbers come
/// from docs/24 7-8절 (2026-11-19): Gemma 4 E4B (Q4_K_M) was the best of four small models and kept 27 of 58 decisions
/// where the cloud notes kept 54. Tacie ships no model and serves none: Ollama downloads it (docs/41 0절, 2026-11-26).
struct LocalModelGuide: View {
    enum Server { case ollama, none, lmStudio }

    @Bindable var model: AIChoiceModel
    let server: Server

    /// The model's name as the card says it: the recommended download by its name, any other as Ollama lists it.
    static func shownName(_ id: String) -> String { id == AIChecks.notesModel ? "Gemma 4 E4B" : id }

    @MainActor static func shown(for model: AIChoiceModel) -> LocalModelGuide? {
        switch model.search {
        case .none: LocalModelGuide(model: model, server: .none)
        case .noModel(let name): LocalModelGuide(model: model, server: name == "LM Studio" ? .lmStudio : .ollama)
        default: nil
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Text("추천").uiFont(11, weight: .semibold).foregroundStyle(Theme.ink2)
                    .padding(.horizontal, 7).padding(.vertical, 1.5)
                    .background(Theme.ink.opacity(0.08), in: Capsule())
                Text(verbatim: "Gemma 4 E4B").uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink)
                Text(ByteCountFormatter.string(fromByteCount: AIChecks.notesModelSize, countStyle: .file))
                    .uiFont(12).foregroundStyle(Theme.ink3)
                Spacer(minLength: 8)
                if server == .ollama { action }
            }
            if case .pulling(let done, let total) = model.pull {
                ProgressView(value: Double(done), total: Double(max(total, 1))).progressViewStyle(.linear).tint(Theme.ink2)
                Text("\(ByteCountFormatter.string(fromByteCount: done, countStyle: .file)) / \(ByteCountFormatter.string(fromByteCount: total, countStyle: .file)) · 이 Mac의 Ollama로 받는 중")
                    .uiFont(12).foregroundStyle(Theme.ink2).monospacedDigit()
            } else if case .failed(let why) = model.pull {
                Text(why).uiFont(12).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            } else {
                Text(note).uiFont(12).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            }
            Text("회의 정리 평가에서 이 모델은 결정 58개 중 27개를 담았어요(클라우드는 54개).")
                .uiFont(12).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.fill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    }

    @ViewBuilder private var action: some View {
        switch model.pull {
        case .pulling: EmptyView()
        case .failed:
            Button { model.pullNotesModel() } label: { OutlineButton(label: String(localized: "다시 받기")) }.buttonStyle(PressStyle())
        case .idle:
            Button { model.pullNotesModel() } label: { OutlineButton(label: String(localized: "받기")) }.buttonStyle(PressStyle())
        }
    }

    private var note: String {
        switch server {
        case .ollama: String(localized: "받으면 이 Mac의 Ollama가 내려받고, 다 받으면 바로 회의 정리에 써요.")
        case .none: String(localized: "Ollama를 설치하고 켜면 여기서 바로 받을 수 있어요.")
        case .lmStudio: String(localized: "LM Studio에서 이 모델을 받은 뒤 이 화면을 다시 열면 찾아요.")
        }
    }
}
