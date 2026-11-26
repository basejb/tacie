import AppKit
import SwiftUI

/// The onboarding's Obsidian tile (SourcesStep), after Dropbox's.
struct NotesTile: View {
    let keeper: IndexKeeper
    @State private var picking = false

    var body: some View {
        NotesTileBody(status: NotesStatus(info: keeper.notesInfo), actions: NotesActions(pick: { picking = true }))
            .sheet(isPresented: $picking) { NotesFoldersSheet(keeper: keeper) }
    }
}

struct NotesSettingsCard: View {
    let keeper: IndexKeeper
    @State private var picking = false
    @State private var askRemove = false

    var body: some View {
        NotesCardBody(status: NotesStatus(info: keeper.notesInfo),
                      actions: NotesActions(pick: { picking = true }, removeAll: { askRemove = true }))
            .alert("고른 vault를 모두 뺄까요?", isPresented: $askRemove) {
                Button("모두 빼기", role: .destructive) {
                    Task { await keeper.setNoteFolders([]) }
                }
                Button("취소", role: .cancel) {}
            } message: {
                Text("노트에서 더는 답을 찾지 않아요. 노트 파일은 그대로예요.")
            }
            .sheet(isPresented: $picking) { NotesFoldersSheet(keeper: keeper) }
    }
}

/// vault 고르기: the vaults Obsidian has opened on this Mac, the folders chosen before, and 다른 폴더 추가.
struct NotesFoldersSheet: View {
    let keeper: IndexKeeper
    @Environment(\.dismiss) private var dismiss
    @State private var rows: [NotesPickRow] = []
    @State private var on: Set<String> = []

    var body: some View {
        NotesFoldersBody(rows: rows, on: $on, add: add, cancel: { dismiss() }, save: {
            let chosen = rows.filter { on.contains($0.path) }.map(\.folder)
            Task { await keeper.setNoteFolders(chosen) }
            dismiss()
        })
        .onAppear {
            let vaults = ObsidianVaults.found()
            // counts only, never paths: how many vaults the registry has and how many are shown
            let listed = (try? Data(contentsOf: ObsidianVaults.registry)).map { ObsidianVaults.parse($0).count } ?? 0
            Log.info("obsidian vaults: \(vaults.count) shown of \(listed)")
            let before = keeper.notesInfo?.folders ?? []
            var list = vaults.map { NotesPickRow(path: $0.path, name: $0.name, vaultID: $0.id, iCloud: $0.inICloud) }
            for f in before where !list.contains(where: { $0.path == f.path }) {
                list.append(NotesPickRow(path: f.path, name: f.name, vaultID: f.vaultId,
                                         iCloud: f.path.contains("/Library/Mobile Documents/")))
            }
            rows = list
            on = Set(before.map(\.path))
        }
    }

    /// 다른 폴더 추가: a folder of Markdown files (or a vault Obsidian has not opened here).
    private func add() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = String(localized: "추가")
        guard panel.runModal() == .OK, let url = panel.url else { return }
        let path = url.path
        if !rows.contains(where: { $0.path == path }) {
            let id = ObsidianVaults.id(for: path, in: ObsidianVaults.found())
            rows.append(NotesPickRow(path: path, name: url.lastPathComponent, vaultID: id,
                                     iCloud: path.contains("/Library/Mobile Documents/")))
        }
        on.insert(path)
    }
}
