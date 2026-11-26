import SwiftUI

/// The ways to sign in (docs/14): the first one wide, the others side by side under "or", so the choice shows at a
/// glance.
struct SignInButtons: View {
    var body: some View {
        let all = ASignInWith.allCases
        VStack(spacing: 0) {
            SignInButton(p: all[0], primary: true)
            HStack(spacing: 10) {
                Rectangle().fill(Theme.hairline).frame(height: 1)
                Text("또는").font(.system(size: 11.5)).foregroundStyle(Theme.ink3).fixedSize()
                Rectangle().fill(Theme.hairline).frame(height: 1)
            }
            .padding(.vertical, 12)
            HStack(spacing: 8) {
                ForEach(all.dropFirst(), id: \.self) { SignInButton(p: $0, primary: false) }
            }
            NewsConsentBox(on: Binding(get: { TacieAccount.shared.newsAtSignIn }, set: { TacieAccount.shared.newsAtSignIn = $0 }))
                .padding(.top, 14)
        }
    }
}

/// The optional news email consent (server mail-news.js, docs/14): a box, never ticked for the person, and signing in
/// works the same without it. Under the sign-in buttons and in the 새로 생긴 기능 window.
struct NewsConsentBox: View {
    @Binding var on: Bool

    var body: some View {
        Button { on.toggle() } label: {
            HStack(alignment: .firstTextBaseline, spacing: 7) {
                Image(systemName: on ? "checkmark.square.fill" : "square")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(on ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(Theme.ink3))
                Text("새 기능 소식을 메일로 받을게요 (선택)").uiFont(12).foregroundStyle(Theme.ink2)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

struct SignInButton: View {
    let p: ASignInWith
    let primary: Bool
    @State private var hover = false

    var body: some View {
        let account = TacieAccount.shared
        let busy = account.phase == .signingIn
        Button { account.signIn(with: p) } label: {
            HStack(spacing: 10) {
                logo
                Text(busy && account.tried == p ? String(localized: "로그인 중…") : primary ? String(localized: "\(p.label)로 계속하기") : p.label)
                    .font(.system(size: 13.5, weight: .semibold))
            }
            .foregroundStyle(primary ? Theme.onStrong : Theme.ink)
            .frame(maxWidth: .infinity).frame(height: 46)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(primary ? Theme.ink.opacity(hover ? 0.85 : 1) : (hover ? Theme.fill : Theme.paper)))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(primary ? .clear : Theme.ink.opacity(0.12)))
            .contentShape(Rectangle())
        }
        .buttonStyle(PressStyle())
        .disabled(busy)
        .onHover { hover = $0 }
    }

    @ViewBuilder private var logo: some View {
        switch p {
        case .google:
            // the colour mark on a white disc, so it reads on the ink button
            Image("ServiceGoogle").resizable().scaledToFit().frame(width: 14, height: 14)
                .frame(width: 20, height: 20).background(Circle().fill(.white))
        case .github:
            Image("ServiceGitHub").resizable().renderingMode(.template).scaledToFit().frame(width: 17, height: 17)
        case .apple:
            Image(systemName: "apple.logo").font(.system(size: 16, weight: .medium)).frame(width: 17, height: 17)
        }
    }
}

/// Why the sign-in sheet is up over the main window: the sidebar's card, or a meeting or a question that needs the account.
enum SignInAsk: Equatable { case card, meeting, ask }

/// The sign-in over the main window (docs/14): the same buttons as the onboarding. A meeting or a question that asked for
/// it goes on once the account is in; 나중에 drops it.
struct SignInSheet: View {
    let ask: SignInAsk
    let done: (_ signedIn: Bool) -> Void
    private var account: TacieAccount { TacieAccount.shared }

    var body: some View {
        let (title, sub) = Self.words(ask)
        VStack(spacing: 0) {
            LitHead(channels: EyeRig.target(for: .listening, remote: false), height: 34).padding(.bottom, 12)
            Text(title).uiFont(18, weight: .bold).tracking(-0.3).foregroundStyle(Theme.ink)
            Text(sub).uiFont(12.5).foregroundStyle(Theme.ink2).multilineTextAlignment(.center).lineSpacing(2)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 5).padding(.bottom, 18)
            if case .failed(let why) = account.phase {
                Text(why).uiFont(11.5).foregroundStyle(Theme.warn).multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true).padding(.bottom, 10)
            }
            SignInButtons()
            Text("이름과 이메일만 받아요. 회의 내용은 서버에 남기지 않아요.").uiFont(11).foregroundStyle(Theme.ink3)
                .multilineTextAlignment(.center).padding(.top, 12)
            Button {
                if account.phase == .signingIn { account.cancelSignIn() }
                done(false)
            } label: {
                Text("나중에").uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink3).padding(.horizontal, 10).frame(height: 28)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .padding(.top, 8)
        }
        .padding(.horizontal, 26).padding(.top, 26).padding(.bottom, 14)
        .frame(width: 400)
        .background(RoundedRectangle(cornerRadius: 22, style: .continuous).fill(Theme.paper))
        .overlay(RoundedRectangle(cornerRadius: 22, style: .continuous).strokeBorder(Theme.modalEdge))
        .shadow(color: Theme.modalShadow, radius: 30, y: 12)
        .appearanceScheme()
        .onChange(of: account.signedIn) { if account.signedIn { done(true) } }
    }

    static func words(_ ask: SignInAsk) -> (String, String) {
        switch ask {
        case .card: (String(localized: "Tacie에 로그인해요"), String(localized: "로그인하면 바로 회의를 듣고 답할 수 있어요."))
        case .meeting: (String(localized: "로그인하고 회의를 시작해요"), String(localized: "Tacie가 회의를 듣고 답하려면 로그인이 필요해요.\n로그인하면 바로 이어서 시작해요."))
        case .ask: (String(localized: "로그인하고 물어봐요"), String(localized: "Tacie가 답하려면 로그인이 필요해요.\n로그인하면 바로 이어서 물어볼 수 있어요."))
        }
    }
}

/// The sidebar's foot while Tacie AI waits for a sign-in, in place of the profile row: always in sight, out of the
/// way of the notes. 로그인 opens the sheet.
struct SignInCard: View {
    let signIn: () -> Void
    let settings: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Circle().fill(Theme.warn).frame(width: 6, height: 6)
                Text("로그인이 필요해요").uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink)
                Spacer(minLength: 0)
                Button(action: settings) { IconButton(symbol: "gearshape") }.buttonStyle(.plain).help("설정 (⌘,)")
            }
            Text("로그인해야 회의를 듣고 답할 수 있어요.").uiFont(11).foregroundStyle(Theme.ink3)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.bottom, 8)
            Button(action: signIn) {
                Text("로그인").uiFont(12, weight: .semibold).foregroundStyle(Theme.onStrong)
                    .frame(maxWidth: .infinity).frame(height: 28)
                    .background(Capsule().fill(Theme.ink))
            }
            .buttonStyle(PressStyle())
        }
        .padding(.leading, 12).padding(.trailing, 6).padding(.top, 4).padding(.bottom, 10)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Theme.paper))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.ink.opacity(0.1)))
        .padding(.horizontal, 10).padding(.vertical, 8)
    }
}
