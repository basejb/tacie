import SwiftUI

extension AtlassianStatus {
    init(keeper: IndexKeeper) {
        self.init(link: keeper.atlassian, info: keeper.atlassianInfo, stepLine: keeper.stepLine)
    }
}

/// The onboarding's Jira · Confluence tile (SourcesStep), next to Google Drive's.
struct AtlassianTile: View {
    let keeper: IndexKeeper
    @State private var pickScopes = false
    @State private var pasting = false

    var body: some View {
        AtlassianTileBody(status: AtlassianStatus(keeper: keeper),
                          actions: AtlassianActions(connect: { TokenCopy.start(.atlassian, keeper: keeper, paste: $pasting) },
                                                    cancel: { keeper.atlassian.cancel() },
                                                    pickScopes: { pickScopes = true }))
            .sheet(isPresented: $pickScopes) { AtlassianScopesSheet(keeper: keeper) }
            .sheet(isPresented: $pasting) { TokenConnectSheet(source: .atlassian, keeper: keeper) }
    }
}

/// 설정 → 연결's Jira · Confluence card, under Google Drive's.
struct AtlassianSettingsCard: View {
    let keeper: IndexKeeper
    @State private var askDisconnect = false
    @State private var pickScopes = false
    @State private var pasting = false
    @State private var disconnected = false

    var body: some View {
        AtlassianCardBody(status: AtlassianStatus(keeper: keeper),
                          actions: AtlassianActions(connect: { disconnected = false; TokenCopy.start(.atlassian, keeper: keeper, paste: $pasting) },
                                                    cancel: { keeper.atlassian.cancel() },
                                                    pickScopes: { pickScopes = true },
                                                    disconnect: { askDisconnect = true }),
                          disconnected: disconnected)
            // the note belongs to the disconnect just made: any other state (connecting again, a new connection) ends it
            .onChange(of: AtlassianStatus(keeper: keeper).state) { _, state in
                if state != .off { disconnected = false }
            }
            .alert("Jira · Confluence 연결을 해제할까요?", isPresented: $askDisconnect) {
                Button("연결 해제", role: .destructive) {
                    Analytics.capture(.sourceDisconnected(tool: .atlassian))
                    disconnected = true
                    Task { await keeper.atlassian.disconnect() }
                }
                Button("취소", role: .cancel) {}
            } message: {
                Text("이 Mac에 받아 둔 Jira · Confluence 자료를 지워요.")
            }
            .sheet(isPresented: $pickScopes) { AtlassianScopesSheet(keeper: keeper) }
            .sheet(isPresented: $pasting) { TokenConnectSheet(source: .atlassian, keeper: keeper) }
    }
}

/// 프로젝트·스페이스 고르기 with the engine's projects and spaces; saving hands the turned-off ones to the engine.
struct AtlassianScopesSheet: View {
    let keeper: IndexKeeper
    @Environment(\.dismiss) private var dismiss
    @State private var scopes: AtlassianScopes?
    @State private var onProjects: Set<String> = []
    @State private var onSpaces: Set<String> = []
    @State private var unreachable = false

    var body: some View {
        AtlassianScopesBody(scopes: scopes, onProjects: $onProjects, onSpaces: $onSpaces, unreachable: unreachable, cancel: { dismiss() }, save: {
            guard let scopes else { return }
            let saved = keeper.atlassian.saved
            let projects = AtlassianLink.muted(shown: scopes.projects.map(\.id), on: onProjects, before: saved?.mutedProjects ?? [])
            let spaces = AtlassianLink.muted(shown: scopes.spaces.map(\.id), on: onSpaces, before: saved?.mutedSpaces ?? [])
            Task { await keeper.atlassian.setMuted(projects: projects, spaces: spaces) }
            dismiss()
        })
        .task {
            let found: AtlassianScopes
            do { found = try await keeper.atlassianScopes() } catch {
                // the engine could not ask Atlassian: say so instead of "nothing to choose"
                unreachable = true
                scopes = AtlassianScopes(projects: [], spaces: [])
                return
            }
            let saved = keeper.atlassian.saved
            onProjects = Set(found.projects.map(\.id)).subtracting(saved?.mutedProjects ?? [])
            onSpaces = Set(found.spaces.map(\.id)).subtracting(saved?.mutedSpaces ?? [])
            scopes = found
        }
    }
}
