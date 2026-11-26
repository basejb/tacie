import AppKit
import SwiftUI

/// What the Obsidian card says (docs/34 5-6절). Built from plain values so the render tests can draw every state
/// without an engine.
struct NotesStatus {
    let info: IndexState.NotesInfo?

    var chosen: [IndexState.NotesInfo.Folder] { info?.folders ?? [] }
    var on: Bool { !chosen.isEmpty }

    /// "vault 2개 · 노트 340개", or why not.
    var line: String {
        guard on else { return String(localized: "고른 vault가 없어요") }
        return [chosenLine, notesLine ?? String(localized: "노트를 읽는 중")].joined(separator: " · ")
    }

    /// "vault 2개 · 폴더 1개": what is chosen (the onboarding tile's first line).
    var chosenLine: String {
        let vaults = chosen.filter(\.obsidian).count, others = chosen.count - vaults
        var parts: [String] = []
        if vaults > 0 { parts.append(String(localized: "vault \(vaults)개")) }
        if others > 0 { parts.append(String(localized: "폴더 \(others)개")) }
        return parts.joined(separator: " · ")
    }

    /// "노트 340개" once the notes are in the search data; nil while they are read.
    var notesLine: String? { info?.notes.map { String(localized: "노트 \($0)개") } }

    /// Still reading: a chosen folder has not been read yet, or nothing is counted so far.
    var reading: Bool { on && (info?.notes == nil || chosen.contains { $0.notes == nil && !$0.missing }) }

    /// A second line when something needs the user's eye: a folder that is gone, or iCloud notes not here yet.
    var note: String? {
        if let gone = chosen.first(where: \.missing) {
            return String(localized: "\(gone.name) 폴더를 찾을 수 없어요. 옮겼다면 다시 골라 주세요.")
        }
        if let n = info?.notDownloaded, n > 0 {
            return String(localized: "iCloud에서 받지 못한 노트 \(n)개는 지금은 제목으로만 찾아요. 다음에 다시 받아요.")
        }
        return nil
    }
}

/// What the Obsidian card can do; the keeper-backed wrapper fills it.
struct NotesActions {
    var pick: () -> Void = {}
    var removeAll: () -> Void = {}
}

/// 설정 → 연결's Obsidian card, under Dropbox's.
struct NotesCardBody: View {
    let status: NotesStatus
    var actions = NotesActions()

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            LogoTile(logo: "ServiceObsidian")
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: "Obsidian").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                Text(status.line).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                if let note = status.note {
                    Text(note).uiFont(12).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 8)
            if status.on {
                Menu {
                    Button("vault 고르기", action: actions.pick)
                    Button("모두 빼기", role: .destructive, action: actions.removeAll)
                } label: {
                    Image(systemName: "ellipsis").uiFont(13, weight: .semibold).foregroundStyle(Theme.ink3)
                        .frame(width: 24, height: 20)
                        .contentShape(Rectangle())
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
                .padding(.top, 2)
            } else {
                Button(action: actions.pick) { OutlineButton(label: String(localized: "vault 고르기")) }.buttonStyle(PressStyle())
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)  // a cell of 설정 → 연결's two columns, which draw the row
    }
}

/// The onboarding's Obsidian tile (SourcesStep), after Dropbox's: the same size and two status lines as the other tiles.
struct NotesTileBody: View {
    let status: NotesStatus
    var actions = NotesActions()

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                // on the logo tile's own light ground, like Dropbox's, so the purple mark keeps its contrast in dark mode
                Image("ServiceObsidian").resizable().scaledToFit().frame(width: 13, height: 13)
                    .frame(width: 20, height: 20)
                    .background(Theme.logoTile, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 5, style: .continuous).stroke(Theme.ink.opacity(0.1)))
                VStack(alignment: .leading, spacing: 1) {
                    Text(verbatim: "Obsidian").uiFont(13.5, weight: .semibold).foregroundStyle(Theme.ink)
                    Text("이 Mac의 마크다운 노트").uiFont(11.5).foregroundStyle(Theme.ink3).lineLimit(1)
                }
                Spacer(minLength: 0)
                if status.on {
                    TileMenu { Button("vault 고르기", action: actions.pick) }
                }
            }
            if !status.on {
                Button("vault 고르기", action: actions.pick).buttonStyle(InkButtonStyle())
            } else if let gone = status.chosen.first(where: \.missing) {
                Text("\(gone.name) 폴더를 찾을 수 없어요. 옮겼다면 다시 골라 주세요.")
                    .uiFont(11.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
            } else if status.reading, status.notesLine == nil {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.mini)
                    Text("노트를 읽는 중").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink2)
                }
            } else {
                TileStatusLines(name: status.chosenLine, detail: status.notesLine, growing: status.reading)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 100, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.paper))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
            .strokeBorder(status.on ? Theme.ink.opacity(0.35) : Theme.ink.opacity(0.1)))
    }
}

/// One row of the picker: a vault Obsidian knows, or a folder the user added by hand.
struct NotesPickRow: Identifiable, Equatable {
    let path: String
    let name: String
    let vaultID: String?
    let iCloud: Bool
    var id: String { path }

    var folder: NoteFolder { NoteFolder(path: path, vaultID: vaultID, name: name) }
}

struct NotesFoldersBody: View {
    let rows: [NotesPickRow]
    @Binding var on: Set<String>
    /// Past six rows the list scrolls; the render tests draw it flat (ImageRenderer leaves a ScrollView blank).
    var scrolls = true
    var add: () -> Void = {}
    var cancel: () -> Void = {}
    var save: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("vault 고르기").uiFont(16, weight: .semibold).foregroundStyle(Theme.ink)
            Text("고른 vault의 노트에서도 답을 찾아요. 카드를 누르면 Obsidian에서 그 노트가 열려요.")
                .uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            VStack(spacing: 2) {
                if rows.isEmpty {
                    Text("이 Mac의 Obsidian에서 연 vault가 없어요. 노트 폴더를 직접 골라 주세요.")
                        .uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, minHeight: 50, alignment: .leading)
                } else if scrolls && rows.count > 6 {
                    // past six rows the list scrolls by itself, so 저장 stays inside a 620-point onboarding window
                    ScrollView { VStack(spacing: 2) { ForEach(rows) { row($0) } } }
                        .scrollIndicators(.automatic)
                        .frame(height: 204)
                } else {
                    ForEach(rows) { row($0) }
                }
            }
            .padding(.top, 14)
            Button(action: add) {
                Label("다른 폴더 추가", systemImage: "plus").uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink2)
            }
            .buttonStyle(.plain)
            .padding(.top, 8)
            .padding(.leading, 10)
            HStack(spacing: 8) {
                Spacer(minLength: 8)
                Button(action: cancel) { OutlineButton(label: String(localized: "취소")) }
                    .buttonStyle(PressStyle()).keyboardShortcut(.cancelAction)
                Button("저장", action: save)
                    .buttonStyle(InkButtonStyle())
                    .keyboardShortcut(.defaultAction)
            }
            .padding(.top, 18)
        }
        .padding(22)
        .frame(width: 420)
        .background(Theme.paper)
    }

    /// One vault: an ink check when it is searched, its name, and "iCloud" when its notes live there.
    private func row(_ r: NotesPickRow) -> some View {
        PickRow(title: r.name, detail: r.iCloud ? "iCloud Drive" : (r.path as NSString).abbreviatingWithTildeInPath,
                on: on.contains(r.path)) {
            if on.contains(r.path) { on.remove(r.path) } else { on.insert(r.path) }
        }
    }
}
