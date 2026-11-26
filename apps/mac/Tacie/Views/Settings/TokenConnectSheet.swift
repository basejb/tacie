import SwiftUI

/// The open edition's step 3: the window that connects one service with a personal token (mockup 2026-11-23). ① a link
/// to make a token in that service ② the field to paste into (Jira · Confluence: site address, email, token) ③ Notion
/// only: adding the pages Tacie reads. A paste is checked at once; if it works it is saved and the window closes.
/// GitHub uses the same body inside the GitHub connect page.
struct TokenConnectSheet: View {
    let source: ConnectionsScene.Source
    let keeper: IndexKeeper
    @Environment(\.dismiss) private var dismiss
    @State private var model: TokenConnectModel?

    var body: some View {
        Group {
            if let model { TokenSheetFrame(model: model, close: { dismiss() }) } else { Color.clear.frame(width: 440, height: 300) }
        }
        // closed any way (취소, Esc, done): a check still under way must not save after the window is gone
        .onDisappear { model?.cancel() }
        .onAppear {
            if model == nil {
                let close = dismiss
                model = TokenConnectModel(source: source,
                                          check: { s, t, e, site in await PastedTokens.check(s, token: t, email: e, site: site) },
                                          connect: TokenCopy.connector(for: source, keeper: keeper), done: { close() })
            }
        }
    }
}

extension TokenCopy {
    /// "연결" and "다시 연결" on a card: the token window in the open edition, the sign-in otherwise.
    @MainActor
    static func start(_ s: ConnectionsScene.Source, keeper: IndexKeeper, paste: Binding<Bool>) {
        if PastedTokens.offered {
            paste.wrappedValue = true
            return
        }
        switch s {
        case .notion: keeper.notion.connect()
        case .linear: keeper.linear.connect()
        case .asana: keeper.asana.connect()
        case .atlassian: keeper.atlassian.connect()
        case .github, .gdrive, .dropbox, .notes: break
        }
    }

    /// 노션 "페이지 고르기": a pasted integration picks its pages on Notion's integrations page (Content access).
    @MainActor
    static func pickNotionPages(keeper: IndexKeeper) {
        if PastedTokens.offered { NSWorkspace.shared.open(PastedTokens.makeURL(.notion)) } else { keeper.notion.connect() }
    }

    /// Saves what the check found and hands it to the engine. GitHub's token goes to the keychain only: its page
    /// hands it over and moves on to the repo picker.
    @MainActor
    static func connector(for s: ConnectionsScene.Source, keeper: IndexKeeper) -> TokenConnectModel.Connector {
        { found, token, email in
            switch s {
            case .github:
                try? KeychainTokenStore.github.save(token)
                UserDefaults.standard.set(false, forKey: GitHubCredentials.disconnectedKeyInUse)
            case .notion: await keeper.notion.connectPasted(token: token, workspace: found.name)
            case .linear: await keeper.linear.connectPasted(token: token, org: found)
            case .asana: await keeper.asana.connectPasted(token: token, found: found)
            case .atlassian: await keeper.atlassian.connectPasted(token: token, email: email, found: found)
            case .gdrive, .dropbox, .notes: break
            }
        }
    }
}
