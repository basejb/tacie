import SwiftUI

extension DropboxStatus {
    init(keeper: IndexKeeper) {
        self.init(link: keeper.dropbox, info: keeper.dropboxInfo, stepLine: keeper.stepLine)
    }
}

/// The onboarding's Dropbox tile (SourcesStep), after Asana's.
struct DropboxTile: View {
    let keeper: IndexKeeper
    @State private var pickFolders = false

    var body: some View {
        DropboxTileBody(status: DropboxStatus(keeper: keeper),
                        actions: DropboxActions(connect: { keeper.dropbox.connect() },
                                                cancel: { keeper.dropbox.cancel() },
                                                pickFolders: { pickFolders = true }))
            .sheet(isPresented: $pickFolders) { DropboxFoldersSheet(keeper: keeper) }
    }
}

/// 설정 → 연결's Dropbox card, under Asana's.
struct DropboxSettingsCard: View {
    let keeper: IndexKeeper
    @State private var askDisconnect = false
    @State private var pickFolders = false

    var body: some View {
        DropboxCardBody(status: DropboxStatus(keeper: keeper),
                        actions: DropboxActions(connect: { keeper.dropbox.connect() },
                                                cancel: { keeper.dropbox.cancel() },
                                                pickFolders: { pickFolders = true },
                                                disconnect: { askDisconnect = true }))
            // an alert like Drive's: the revoke ends this grant on Dropbox's side too, so there is no link to give
            .alert("Dropbox 연결을 해제할까요?", isPresented: $askDisconnect) {
                Button("연결 해제", role: .destructive) {
                    Analytics.capture(.sourceDisconnected(tool: .dropbox))
                    Task { await keeper.dropbox.disconnect() }
                }
                Button("취소", role: .cancel) {}
            } message: {
                Text("이 Mac에 받아 둔 Dropbox 자료를 지우고, Dropbox에서 이 Mac의 승인을 취소해요. 다른 Mac의 Tacie는 그대로예요.")
            }
            .sheet(isPresented: $pickFolders) { DropboxFoldersSheet(keeper: keeper) }
    }
}

/// 폴더 고르기 with the engine's top folders; saving hands the turned-off ones to the engine.
struct DropboxFoldersSheet: View {
    let keeper: IndexKeeper
    @Environment(\.dismiss) private var dismiss
    @State private var folders: [DropboxFolder]?
    @State private var on: Set<String> = []
    @State private var unreachable = false

    var body: some View {
        DropboxFoldersBody(folders: folders, on: $on, unreachable: unreachable, cancel: { dismiss() }, save: {
            guard let folders else { return }
            let muted = DropboxLink.muted(shown: folders.map(\.id), on: on, before: keeper.dropbox.saved?.mutedFolders ?? [])
            Task { await keeper.dropbox.setMuted(muted) }
            dismiss()
        })
        .task {
            let found: [DropboxFolder]
            do { found = try await keeper.dropboxFolders() } catch {
                // the engine could not answer: say so instead of "nothing to choose"
                unreachable = true
                folders = []
                return
            }
            on = Set(found.map(\.id)).subtracting(keeper.dropbox.saved?.mutedFolders ?? [])
            folders = found
        }
    }
}
