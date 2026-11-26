import SwiftUI
import AppKit

/// Developer: only in a development build, so vendor and model names may show here.
struct DeveloperPage: View {
    let context: SettingsContext
    @AppStorage(DevSettings.sttModel) private var sttModel = ""
    @AppStorage(DevSettings.answerModel) private var answerModel = ""
    @AppStorage(DevSettings.jevModel) private var jevModel = ""
    @AppStorage(DevSettings.gate) private var gate = ""
    @AppStorage(DevSettings.show) private var show = ""
    @AppStorage(DevSettings.debugText) private var debugText = false
    @AppStorage(DevSettings.sttPath) private var sttPath = "stream"
    @AppStorage(DevSettings.sonioxRegion) private var sonioxRegion = "auto"
    @AppStorage(Build.githubLoginKey) private var githubLogin = "gh"
    @State private var config: EngineConfig?
    @State private var engineRoot = EngineProcess.root
    @State private var checkout = EngineProcess.checkout
    @State private var slackToken = ""
    @State private var slackNote: String?
    @State private var addressURL = AIAddress.saved()?.url ?? ""
    @State private var addressModel = AIAddress.saved()?.model ?? ""
    @State private var addressKey = ""
    @State private var addressNote: String?
    @State private var sttName = SttProviderChoice.saved()?.name.rawValue ?? "none"
    @State private var sttRegion = SttProviderChoice.saved()?.region ?? "us"
    @State private var sttKey = ""
    @State private var sttNote: String?

    private let waiting = String(localized: "검색 엔진이 준비되면 보여요")

    private func bad(_ s: String) -> Bool {
        !s.trimmingCharacters(in: .whitespaces).isEmpty && DevSettings.threshold(s) == nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            PageHeader(title: String(localized: "개발자"), desc: String(localized: "개발 빌드에만 있어요. 바꾼 값은 엔진을 다시 띄우면 써요."))
            models
            thresholds
            keys
            // the open edition picks its AI and its speech-to-text in 설정 → AI (2-D); these test fields are the
            // official development build's
            if !Build.isOpen {
                aiAddress
                sttProvider
            }
            paths
            SettingsSection(title: String(localized: "검색 데이터")) {
                SettingsRow(title: String(localized: "심볼 이름표가 빠진 파일"), desc: String(localized: "그래프를 만든 뒤 바뀐 파일이에요. 많아지면 그래프를 다시 만들어요.")) {
                    Text(String(localized: "파일 \(context.keeper.info?.symbolsMissing ?? 0)개")).uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink2)
                }
            }
            SettingsSection(title: "GitHub") {
                SettingsRow(title: String(localized: "GitHub 로그인"), desc: String(localized: "gh 로그인을 고르면 터미널의 gh 계정으로 받아요.")) {
                    Menu {
                        Button("Tacie 연결") { githubLogin = "tacie" }
                        Button("gh 로그인") { githubLogin = "gh" }
                    } label: {
                        MenuText(label: githubLogin == "tacie" ? String(localized: "Tacie 연결") : String(localized: "gh 로그인"))
                    }
                    .menuStyle(.button)
                    .buttonStyle(.plain)
                    .menuIndicator(.hidden)
                    .fixedSize()
                }
            }
            SettingsSection(title: String(localized: "로그")) {
                SettingsRow(title: String(localized: "디버그 글 로그"), desc: String(localized: "받아쓴 글과 질문 점수를 engine.log에 남겨요. 문제 신고에는 넣지 않아요.")) {
                    InkToggle(on: $debugText)
                }
                // the Soniox shadow uses the Tacie server's Soniox, which the open edition does not have
                if !Build.isOpen {
                    SettingsRow(title: String(localized: "받아쓰기 경로"), desc: String(localized: "Soniox: 배포 앱과 같아요. 그림자: 지금 경로로 받아쓰고 Soniox 결과는 비교용으로 회의 기록에 남겨요. 다음 회의부터 적용돼요.")) {
                        ChoiceStrip(choices: [("stream", "Soniox"), ("shadow", String(localized: "그림자")), ("server", String(localized: "지금 경로"))], value: $sttPath)
                    }
                    SettingsRow(title: String(localized: "Soniox 리전"), desc: String(localized: "그림자 받아쓰기의 소리를 처리할 곳이에요. 앱을 다시 켜면 적용돼요.")) {
                        ChoiceStrip(choices: [("auto", String(localized: "자동")), ("jp", String(localized: "일본")), ("us", String(localized: "미국")), ("eu", "EU")], value: $sonioxRegion)
                    }
                }
            }
            slack
            restart
        }
        .onChange(of: githubLogin) { Task { await context.keeper.pushCredentials() } }
        .task(id: context.engine.state) {
            config = try? await context.engine.client?.config()
        }
    }

    private func field(_ text: Binding<String>, prompt: String, width: CGFloat) -> some View {
        TextField("", text: text, prompt: Text(prompt))
            .textFieldStyle(.plain)
            .uiFont(12.5, design: .monospaced)
            .multilineTextAlignment(.trailing)
            .padding(.horizontal, 8)
            .frame(width: width, height: 26)
            .background(Theme.ink.opacity(0.045), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
    }

    private var models: some View {
        let anySet = ![sttModel, answerModel, jevModel].allSatisfy { $0.trimmingCharacters(in: .whitespaces).isEmpty }
        return SettingsSection(title: String(localized: "모델"), desc: config == nil ? waiting : (anySet ? String(localized: "평가 숫자는 기본 조합 기준이에요.") : nil)) {
            if let c = config {
                SettingsRow(title: String(localized: "받아쓰기 모델"), desc: String(localized: "지금: \(c.models.stt)")) {
                    field($sttModel, prompt: c.modelDefaults.stt, width: 240)
                }
                SettingsRow(title: String(localized: "답 모델"), desc: String(localized: "지금: \(c.models.answer)")) {
                    field($answerModel, prompt: c.modelDefaults.answer, width: 240)
                }
                SettingsRow(title: String(localized: "Jev 모델"), desc: c.models.jev.map { String(localized: "지금: \($0)") } ?? String(localized: "키 파일이 없어서 쓰지 않아요")) {
                    field($jevModel, prompt: c.modelDefaults.jev ?? "", width: 240)
                }
            }
        }
    }

    private var thresholds: some View {
        SettingsSection(title: String(localized: "기준값"), desc: config == nil ? waiting : nil) {
            if let c = config {
                SettingsRow(title: String(localized: "질문으로 볼 기준"), desc: bad(gate) ? String(localized: "0과 1 사이 숫자를 넣어 주세요") : String(localized: "지금: \(String(c.thresholds.gate))"), descColor: bad(gate) ? AnyShapeStyle(Theme.warn) : AnyShapeStyle(Theme.ink3)) {
                    field($gate, prompt: "\(c.thresholdDefaults.gate)", width: 80)
                }
                SettingsRow(title: String(localized: "답을 보여 줄 기준"), desc: bad(show) ? String(localized: "0과 1 사이 숫자를 넣어 주세요") : String(localized: "지금: \(String(c.thresholds.show))"), descColor: bad(show) ? AnyShapeStyle(Theme.warn) : AnyShapeStyle(Theme.ink3)) {
                    field($show, prompt: "\(c.thresholdDefaults.show)", width: 80)
                }
            }
        }
    }

    private func keyState(_ s: String) -> String {
        switch s {
        case "ok": String(localized: "있음")
        case "missing": String(localized: "없음")
        case "refused": String(localized: "거절됨")
        case "credit": String(localized: "잔액 부족")
        default: s
        }
    }

    private func keyTitle(_ name: String) -> String {
        switch name.lowercased() {
        case "openrouter": "OpenRouter"
        case "typesafe": "TypeSafe"
        case "slack": "Slack"
        default: name
        }
    }

    private var keys: some View {
        SettingsSection(title: String(localized: "키 파일"), desc: config == nil ? waiting : nil) {
            if let c = config {
                ForEach(c.keys, id: \.name) { k in
                    SettingsRow(title: keyTitle(k.name), desc: k.path) {
                        Text(keyState(k.state)).uiFont(12.5, weight: .medium)
                            .foregroundStyle(k.state == "ok" ? Theme.ink2 : Theme.warn)
                    }
                }
                // the open edition has no Tacie server to call instead
                if !Build.isOpen {
                    SettingsRow(title: String(localized: "Tacie 서버 대신 직접 부르기"), desc: String(localized: "Tacie 서버가 생기면 고를 수 있어요.")) {
                        Text("켜짐").uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink3)
                    }
                }
            }
        }
    }

    /// 오픈소스판의 AI 주소(2-A)를 시험하는 임시 칸이에요. AI 탭(2-D)이 생기면 그리로 옮겨요. 로그인해서 중계를 쓰는
    /// 동안은 엔진에 넘기지 않아요.
    private var aiAddress: some View {
        SettingsSection(title: String(localized: "AI 주소"), desc: addressNote ?? String(localized: "OpenAI 호환 주소(…/v1)와 모델이에요. 비우고 넘기면 지금 길로 돌아가요.")) {
            SettingsRow(title: String(localized: "주소")) { field($addressURL, prompt: "http://127.0.0.1:11434/v1", width: 260) }
            SettingsRow(title: String(localized: "모델")) { field($addressModel, prompt: "gemma4:e4b", width: 260) }
            SettingsRow(title: String(localized: "키"), desc: String(localized: "로컬 서버는 비워 둬요.")) {
                HStack(spacing: 8) {
                    SecureField("", text: $addressKey, prompt: Text("sk-…"))
                        .textFieldStyle(.plain)
                        .uiFont(12.5, design: .monospaced)
                        .padding(.horizontal, 8)
                        .frame(width: 180, height: 26)
                        .background(Theme.ink.opacity(0.045), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    Button(action: saveAddress) { OutlineButton(label: String(localized: "넘기기")) }.buttonStyle(PressStyle()).fixedSize()
                }
            }
        }
    }

    /// 오픈소스판의 받아쓰기 업체(2-B)를 시험하는 임시 칸이에요. AI 탭(2-D)이 생기면 그리로 옮겨요. 온디바이스는
    /// 회의 설정의 "음성 인식" 그대로예요. 로그인해서 중계를 쓰는 동안은 엔진에 넘기지 않아요.
    private var sttProvider: some View {
        SettingsSection(title: String(localized: "받아쓰기 업체"), desc: sttNote ?? String(localized: "자기 키로 받아써요. 없음이면 지금 길이에요.")) {
            SettingsRow(title: String(localized: "업체")) {
                ChoiceStrip(choices: [("none", String(localized: "없음")), ("soniox", "Soniox"), ("elevenlabs", "ElevenLabs")], value: $sttName)
            }
            if sttName == "soniox" {
                SettingsRow(title: String(localized: "Soniox 리전"), desc: String(localized: "키를 만든 콘솔의 리전이에요.")) {
                    ChoiceStrip(choices: [("us", String(localized: "미국")), ("jp", String(localized: "일본")), ("eu", "EU")], value: $sttRegion)
                }
            }
            SettingsRow(title: String(localized: "키"), desc: String(localized: "비워 두면 저장된 키를 그대로 써요.")) {
                HStack(spacing: 8) {
                    SecureField("", text: $sttKey, prompt: Text("키"))
                        .textFieldStyle(.plain)
                        .uiFont(12.5, design: .monospaced)
                        .padding(.horizontal, 8)
                        .frame(width: 180, height: 26)
                        .background(Theme.ink.opacity(0.045), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    Button(action: saveSttProvider) { OutlineButton(label: String(localized: "넘기기")) }.buttonStyle(PressStyle()).fixedSize()
                }
            }
        }
    }

    private func saveSttProvider() {
        let choice = SttProviderName(rawValue: sttName).map { SttProviderChoice(name: $0, region: sttRegion) }
        do {
            try SttProviderChoice.save(choice, key: sttKey)
            if Build.isOpen { AIMode.save(.custom) }  // per-provider picks are 직접 고르기 (2-D)
            sttKey = ""
            let hasKey = choice.map { !(SttProviderChoice.keyStore(for: $0.name).read() ?? "").isEmpty } ?? false
            sttNote = choice == nil ? String(localized: "업체를 지웠어요. 키는 키체인에 남아 있어요.")
                : !hasKey ? String(localized: "저장했어요. 이 업체의 키가 없어서 넘기지 않았어요.")
                : Relay.on ? String(localized: "저장했어요. 중계를 쓰는 동안은 넘기지 않아요.") : String(localized: "저장하고 엔진에 넘겼어요. 다음 회의부터 써요.")
            Task { await context.keeper.pushAIKeys() }
        } catch {
            sttNote = String(localized: "키체인에 저장하지 못했어요.")
        }
    }

    private func saveAddress() {
        let url = addressURL.trimmingCharacters(in: .whitespaces), model = addressModel.trimmingCharacters(in: .whitespaces)
        let address = url.isEmpty || model.isEmpty ? nil : AIAddress(url: url, model: model)
        do {
            try AIAddress.save(address, key: addressKey)
            if Build.isOpen { AIMode.save(.custom) }  // per-provider picks are 직접 고르기 (2-D)
            addressKey = ""
            addressNote = address == nil ? String(localized: "지웠어요.") : (Relay.on ? String(localized: "저장했어요. 중계를 쓰는 동안은 넘기지 않아요.") : String(localized: "저장하고 엔진에 넘겼어요."))
            Task { await context.keeper.pushAIKeys() }
        } catch {
            addressNote = String(localized: "키체인에 저장하지 못했어요.")
        }
    }

    private var paths: some View {
        SettingsSection(title: String(localized: "경로")) {
            SettingsRow(title: String(localized: "엔진 폴더"), desc: engineRoot.path) {
                Button { pick("engineRoot") { engineRoot = $0 } } label: { OutlineButton(label: String(localized: "바꾸기")) }.buttonStyle(PressStyle())
            }
            SettingsRow(title: String(localized: "내 체크아웃"), desc: checkout ?? String(localized: "고르지 않았어요. 카드는 GitHub에서 열려요.")) {
                Button { pick("checkoutRoot") { checkout = $0.path } } label: { OutlineButton(label: String(localized: "고르기")) }.buttonStyle(PressStyle())
            }
        }
    }

    private func pick(_ key: String, done: (URL) -> Void) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        UserDefaults.standard.set(url.path, forKey: key)
        done(url)
    }

    private var slack: some View {
        SettingsSection(title: String(localized: "슬랙")) {
            SettingsRow(title: String(localized: "슬랙 내부 앱 토큰"),
                        desc: slackNote ?? String(localized: "~/.config/slack/config.json에 저장해요. 슬랙은 --slack으로 만든 검색 데이터에만 들어가요.")) {
                HStack(spacing: 8) {
                    SecureField("", text: $slackToken, prompt: Text("xoxb-…"))
                        .textFieldStyle(.plain)
                        .uiFont(12.5, design: .monospaced)
                        .padding(.horizontal, 8)
                        .frame(width: 200, height: 26)
                        .background(Theme.ink.opacity(0.045), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    Button(action: saveToken) { OutlineButton(label: String(localized: "저장")) }.buttonStyle(PressStyle()).fixedSize()
                        .disabled(slackToken.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
    }

    private func saveToken() {
        do {
            try SlackConfig.save(token: slackToken)
            slackToken = ""
            slackNote = String(localized: "저장했어요. --slack으로 다시 만든 검색 데이터부터 써요.")
        } catch {
            slackNote = String(localized: "저장하지 못했어요.")
        }
    }

    private var restart: some View {
        let badThreshold = bad(gate) || bad(show)
        let reason = context.restartBlocker ?? (badThreshold ? String(localized: "기준값을 고쳐 주세요.") : nil)
        let blocked = reason != nil
        return VStack(alignment: .leading, spacing: 8) {
            Button(action: context.actions.restartEngine) { StrongButton(label: String(localized: "적용하고 엔진 다시 띄우기")) }
                .buttonStyle(PressStyle())
                .disabled(blocked)
                .opacity(blocked ? 0.4 : 1)
            if let reason { Text(reason).uiFont(12.5).foregroundStyle(Theme.ink3) }
        }
    }
}
