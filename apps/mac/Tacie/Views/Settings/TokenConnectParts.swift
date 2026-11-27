import SwiftUI

/// The window itself without the keeper: logo and title, the body, Cancel (and Notion's Done). Render tests draw it.
struct TokenSheetFrame: View {
    @Bindable var model: TokenConnectModel
    let close: () -> Void

    private var source: ConnectionsScene.Source { model.source }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(spacing: 0) {
                TokenPathScene(source: source, state: model.state)
                Text(TokenCopy.title(source)).uiFont(18, weight: .bold).tracking(-0.3).foregroundStyle(Theme.ink)
                    .padding(.top, 16)
                Text(TokenCopy.desc(source)).uiFont(12.5).foregroundStyle(Theme.ink2).lineSpacing(2)
                    .multilineTextAlignment(.center).fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 5)
            }
            .frame(maxWidth: .infinity)
            .padding(.bottom, 16)
            TokenConnectBody(model: model, showsDesc: false, showsWhy: false)
                .padding(.horizontal, 14).padding(.vertical, 4)
                .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill))
                .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
            Text(TokenCopy.why(source)).uiFont(11.5).foregroundStyle(Theme.ink3).lineSpacing(1.5)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 12).padding(.horizontal, 4)
            HStack(spacing: 8) {
                Button(String(localized: "취소")) { model.cancel(); close() }
                    .buttonStyle(.plain).uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink3)
                    .keyboardShortcut(.cancelAction)
                Spacer(minLength: 8)
                if source == .notion {
                    Button(String(localized: "완료"), action: close)
                        .buttonStyle(InkButtonStyle())
                        .disabled(model.state != .ok)
                        .opacity(model.state == .ok ? 1 : 0.4)
                }
            }
            .padding(.top, 16)
        }
        .padding(.horizontal, 24).padding(.top, 22).padding(.bottom, 18)
        .frame(width: 460)
        .background(Theme.paper)
    }
}

/// The token window's scene (docs/29 3절 "연결이 보여요"): the service's logo, a line, and Tacie on this Mac. Nothing sits
/// between them, which is the window's point: the data comes straight to this Mac. While the token is checked the
/// light runs from the service to Tacie and Tacie thinks; once it is good Tacie is happy and the line stays lit.
/// Decorative: nothing in it is pressed. Reduced motion: no running light.
struct TokenPathScene: View {
    let source: ConnectionsScene.Source
    let state: TokenConnectModel.State
    @Environment(\.reducesMotion) private var reduceMotion

    private var mood: EyeMood {
        switch state {
        case .checking: .thinking
        case .ok: .happy
        default: .listening
        }
    }

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30, paused: reduceMotion || state != .checking)) { ctx in
            let f = ctx.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 1.4) / 1.4
            HStack(spacing: 0) {
                end(TokenCopy.vendor(source)) {
                    HStack(spacing: 2) {
                        ForEach(source.logos, id: \.self) { logo in
                            Image(logo).resizable().scaledToFit()
                                .frame(width: source.logos.count > 1 ? 15 : 20, height: source.logos.count > 1 ? 15 : 20)
                        }
                    }
                    .frame(width: 40, height: 40)
                    .background(RoundedRectangle(cornerRadius: 11, style: .continuous).fill(Theme.logoTile))
                    .overlay(RoundedRectangle(cornerRadius: 11, style: .continuous).strokeBorder(Theme.hairline))
                }
                Wire(light: state == .checking && !reduceMotion ? f : nil)
                    .overlay { if state == .ok { Capsule().fill(Theme.eyeGlow).frame(height: 2).padding(.horizontal, 4) } }
                    .padding(.bottom, 18)
                end(String(localized: "이 Mac")) {
                    LitHead(channels: EyeRig.target(for: mood, remote: false), height: 34).frame(height: 40)
                }
            }
            .frame(width: 250)
        }
        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: state)
    }

    private func end<V: View>(_ label: String, @ViewBuilder _ icon: () -> V) -> some View {
        VStack(spacing: 6) {
            icon()
            Text(label).uiFont(11, weight: .semibold).foregroundStyle(Theme.ink3).fixedSize()
        }
        .frame(width: 70)
    }
}

/// The steps and the field(s), with the check's answer beside the token. The sheet and the GitHub page both use it.
struct TokenConnectBody: View {
    @Bindable var model: TokenConnectModel
    /// The GitHub page says it in its own header.
    var showsDesc = true
    /// The token window says it under its scene.
    var showsWhy = true

    private var source: ConnectionsScene.Source { model.source }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if showsDesc {
                Text(TokenCopy.desc(source)).uiFont(13).foregroundStyle(Theme.ink2)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.bottom, 14)
            }
            step(1) {
                linkRow(String(localized: "토큰 만들기"), go: TokenCopy.make(source), url: PastedTokens.makeURL(source))
                note(TokenCopy.makeSub(source))
            }
            step(2, done: model.state == .ok) {
                Text(source == .atlassian ? String(localized: "사이트 주소, 이메일, 토큰 넣기") : String(localized: "여기에 붙여 넣기"))
                    .uiFont(13, weight: .semibold).foregroundStyle(Theme.ink)
                if source == .atlassian {
                    field($model.site, prompt: String(localized: "사이트 주소 (예: myteam.atlassian.net)"))
                    field($model.email, prompt: String(localized: "Atlassian 로그인 이메일"))
                }
                HStack(spacing: 10) {
                    field($model.token, prompt: TokenCopy.placeholder(source), secure: true)
                    status
                }
                if let after = TokenCopy.after(source) { note(after) }
            }
            if source == .notion {
                step(3) {
                    linkRow(String(localized: "Tacie가 읽을 페이지 더하기"), go: String(localized: "노션에서 더하기"),
                            url: PastedTokens.makeURL(.notion))
                    note(String(localized: "만든 통합의 Content access 탭에서 페이지를 더해요. 더한 페이지만 읽어요."))
                }
            }
            // why a token and not the official app's one-button sign-in (docs/41 7-4)
            if showsWhy { note(TokenCopy.why(source))
                .padding(.top, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .overlay(alignment: .top) { Rectangle().fill(Theme.hairline).frame(height: 1) } }
        }
    }

    private func step<Content: View>(_ n: Int, done: Bool = false, @ViewBuilder content: () -> Content) -> some View {
        HStack(alignment: .top, spacing: 10) {
            ZStack {
                Circle().fill(done ? Theme.live : Theme.ink.opacity(0.08))
                if done {
                    Image(systemName: "checkmark").uiFont(9, weight: .bold).foregroundStyle(.white)
                } else {
                    Text("\(n)").uiFont(11, weight: .semibold).foregroundStyle(Theme.ink2)
                }
            }
            .frame(width: 19, height: 19)
            .padding(.top, 1)
            VStack(alignment: .leading, spacing: 4) { content() }
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 9)
        // the window's panel draws its own edge above the first step
        .overlay(alignment: .top) { Rectangle().fill(n == 1 && !showsWhy ? .clear : Theme.hairline).frame(height: 1) }
    }

    private func linkRow(_ title: String, go: String, url: URL) -> some View {
        HStack(spacing: 10) {
            Text(title).uiFont(13, weight: .semibold).foregroundStyle(Theme.ink)
            Spacer(minLength: 8)
            Link(destination: url) {
                HStack(spacing: 3) {
                    Text(go).uiFont(12.5, weight: .semibold).underline()
                    Image(systemName: "arrow.up.right").uiFont(9.5, weight: .semibold)
                }
                .foregroundStyle(Theme.ink)
            }
        }
    }

    private func note(_ text: String) -> some View {
        Text(text).uiFont(12).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
    }

    private func field(_ text: Binding<String>, prompt: String, secure: Bool = false) -> some View {
        Group {
            if secure {
                SecureField("", text: text, prompt: Text(prompt))
            } else {
                TextField("", text: text, prompt: Text(prompt))
            }
        }
        .textFieldStyle(.plain)
        .uiFont(13, design: secure ? .monospaced : .default)
        .padding(.horizontal, 10)
        .frame(height: 32)
        .background(Theme.ink.opacity(0.05), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .onChange(of: text.wrappedValue) { model.changed() }
        .padding(.top, 3)
    }

    @ViewBuilder private var status: some View {
        switch model.state {
        case .empty: EmptyView()
        case .checking:
            HStack(spacing: 6) { ProgressView().controlSize(.mini); Text("확인하는 중") }
                .uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize()
        case .ok: DoneMark(text: String(localized: "됐어요")).fixedSize()
        case .rejected:
            Text("이 토큰은 쓸 수 없어요").uiFont(12.5, weight: .medium).foregroundStyle(Theme.warn).fixedSize()
        case .noSite:
            Text("이 주소에서 찾지 못했어요").uiFont(12.5, weight: .medium).foregroundStyle(Theme.warn).fixedSize()
        case .unknown:
            HStack(spacing: 6) {
                Text("지금 확인할 수 없어요").foregroundStyle(Theme.ink2)
                Button(String(localized: "다시 확인")) { model.recheck() }.buttonStyle(.plain)
                    .foregroundStyle(Theme.ink).underline()
            }
            .uiFont(12.5).fixedSize()
        }
    }
}

/// The words of each service's token window (copy-reviewed with the mockup, 2026-11-23) and what saving does.
enum TokenCopy {
    static func title(_ s: ConnectionsScene.Source) -> String { String(localized: "\(s.name) 연결") }

    static func what(_ s: ConnectionsScene.Source) -> String {
        switch s {
        case .github: String(localized: "코드, 이슈, PR")
        case .notion: String(localized: "문서와 위키")
        case .linear: String(localized: "이슈와 프로젝트")
        case .asana: String(localized: "작업과 프로젝트")
        case .atlassian: String(localized: "이슈와 문서")
        case .gdrive, .dropbox, .notes: ""
        }
    }

    static func desc(_ s: ConnectionsScene.Source) -> String {
        String(localized: "토큰을 붙여 넣으면 회의 중에 \(what(s))에서 답을 찾아요. Tacie는 읽기만 하고, 토큰은 이 Mac의 키체인에 저장해요.")
    }

    static func make(_ s: ConnectionsScene.Source) -> String {
        switch s {
        case .github: String(localized: "GitHub에서 토큰 만들기")
        case .notion: String(localized: "노션에서 통합 만들기")
        case .linear: String(localized: "Linear에서 토큰 만들기")
        case .asana: String(localized: "Asana에서 토큰 만들기")
        case .atlassian: String(localized: "Atlassian에서 토큰 만들기")
        case .gdrive, .dropbox, .notes: ""
        }
    }

    static func makeSub(_ s: ConnectionsScene.Source) -> String {
        switch s {
        case .github: String(localized: "필요한 권한이 채워진 화면이 열려요. 확인하고 Generate token을 눌러요.")
        case .notion: String(localized: "New integration을 Tacie라는 이름으로 만들고, Configuration 탭의 토큰을 복사해요.")
        case .linear: String(localized: "Personal API keys에서 New API key를 눌러 만들어요.")
        case .asana: String(localized: "Personal access tokens에서 새 토큰을 만들어요.")
        case .atlassian: String(localized: "Create API token을 눌러요. 이름은 Tacie로 지어요.")
        case .gdrive, .dropbox, .notes: ""
        }
    }

    static func placeholder(_ s: ConnectionsScene.Source) -> String {
        switch s {
        case .github: String(localized: "github_pat_… 붙여 넣기")
        case .notion: String(localized: "ntn_… 붙여 넣기")
        case .linear: String(localized: "lin_api_… 붙여 넣기")
        case .asana: String(localized: "2/… 붙여 넣기")
        case .atlassian, .gdrive, .dropbox, .notes: String(localized: "토큰 붙여 넣기")
        }
    }

    /// The service the app would be registered with: Jira and Confluence share one Atlassian app.
    static func vendor(_ s: ConnectionsScene.Source) -> String { s == .atlassian ? "Atlassian" : s.name }

    /// Why a token is pasted here: a source build has no app registered with the service; a build with the official
    /// values has one, but its connect needs a Tacie sign-in (#56).
    static func why(_ s: ConnectionsScene.Source, hasAccount: Bool = Build.hasAccount) -> String {
        let v = vendor(s)
        return hasAccount
            ? String(localized: "로그인하지 않은 상태에서는 토큰을 직접 만들어 연결해요. 로그인하면 버튼 하나로 연결할 수 있어요. 자료는 \(v)에서 이 Mac으로 바로 와요.")
            : String(localized: "직접 빌드한 Tacie에는 \(v)에 등록한 앱이 없어서, 토큰을 직접 만들어 연결해요. 자료는 \(v)에서 이 Mac으로 바로 와요.")
    }

    static func after(_ s: ConnectionsScene.Source) -> String? {
        s == .github ? String(localized: "토큰을 확인하면 찾아볼 저장소를 골라요.") : nil
    }
}
