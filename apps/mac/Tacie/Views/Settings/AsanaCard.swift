import SwiftUI

extension AsanaStatus {
    init(keeper: IndexKeeper) {
        self.init(link: keeper.asana, info: keeper.asanaInfo, stepLine: keeper.stepLine)
    }
}

/// The onboarding's Asana tile (SourcesStep), after Jira · Confluence's.
struct AsanaTile: View {
    let keeper: IndexKeeper
    @State private var pickScopes = false
    @State private var pasting = false

    var body: some View {
        AsanaTileBody(status: AsanaStatus(keeper: keeper),
                      actions: AsanaActions(connect: { TokenCopy.start(.asana, keeper: keeper, paste: $pasting) },
                                            cancel: { keeper.asana.cancel() },
                                            pickScopes: { pickScopes = true }))
            .sheet(isPresented: $pickScopes) { AsanaScopesSheet(keeper: keeper) }
            .sheet(isPresented: $pasting) { TokenConnectSheet(source: .asana, keeper: keeper) }
    }
}

/// 설정 → 연결's Asana card, under Jira · Confluence's.
struct AsanaSettingsCard: View {
    let keeper: IndexKeeper
    @State private var askDisconnect = false
    @State private var pickScopes = false
    @State private var pasting = false

    var body: some View {
        AsanaCardBody(status: AsanaStatus(keeper: keeper),
                      actions: AsanaActions(connect: { TokenCopy.start(.asana, keeper: keeper, paste: $pasting) },
                                            cancel: { keeper.asana.cancel() },
                                            pickScopes: { pickScopes = true },
                                            disconnect: { askDisconnect = true }))
            // a sheet rather than an alert: it carries the way to end the grant in Asana too, with a link
            .sheet(isPresented: $askDisconnect) {
                AsanaDisconnectBody(cancel: { askDisconnect = false }, confirm: {
                    askDisconnect = false
                    Analytics.capture(.sourceDisconnected(tool: .asana))
                    Task { await keeper.asana.disconnect() }
                })
            }
            .sheet(isPresented: $pickScopes) { AsanaScopesSheet(keeper: keeper) }
            .sheet(isPresented: $pasting) { TokenConnectSheet(source: .asana, keeper: keeper) }
    }
}

/// 워크스페이스·프로젝트 고르기 with the engine's workspaces and projects; saving hands the turned-off ones to the engine.
struct AsanaScopesSheet: View {
    let keeper: IndexKeeper
    @Environment(\.dismiss) private var dismiss
    @State private var scopes: AsanaScopes?
    @State private var onWorkspaces: Set<String> = []
    @State private var onProjects: Set<String> = []
    @State private var unreachable = false

    var body: some View {
        AsanaScopesBody(scopes: scopes, onWorkspaces: $onWorkspaces, onProjects: $onProjects, unreachable: unreachable,
                        cancel: { dismiss() }, save: {
            guard let scopes else { return }
            let saved = keeper.asana.saved
            let workspaces = AsanaLink.muted(shown: scopes.workspaces.map(\.gid), on: onWorkspaces,
                                             before: saved?.mutedWorkspaces ?? [])
            let projects = AsanaLink.muted(shown: scopes.projects.map(\.gid), on: onProjects, before: saved?.mutedProjects ?? [])
            Task { await keeper.asana.setMuted(workspaces: workspaces, projects: projects) }
            dismiss()
        })
        .task {
            let found: AsanaScopes
            do { found = try await keeper.asanaScopes() } catch {
                // the engine could not answer: say so instead of "nothing to choose"
                unreachable = true
                scopes = AsanaScopes(workspaces: [], projects: [])
                return
            }
            let saved = keeper.asana.saved
            onWorkspaces = Set(found.workspaces.map(\.gid)).subtracting(saved?.mutedWorkspaces ?? [])
            onProjects = Set(found.projects.map(\.gid)).subtracting(saved?.mutedProjects ?? [])
            scopes = found
        }
    }
}
