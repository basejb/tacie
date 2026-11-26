import XCTest
import SwiftUI
@testable import Tacie

/// The Obsidian vault list, the card's lines, and (when TACIE_NOTES_RENDER_DIR names a folder) PNGs of the card and
/// the vault picker (docs/34 5-6절). Nothing opens on screen.
@MainActor
final class NotesCardTests: XCTestCase {
    private typealias Info = IndexState.NotesInfo

    func testRegistryGivesVaultsNewestFirstAndSkipsOddEntries() {
        let json = """
        {"vaults": {
          "0123456789abcdef": {"path": "/Users/a/Library/Mobile Documents/iCloud~md~obsidian/Documents/일", "ts": 10, "open": true},
          "fedcba9876543210": {"path": "/Users/a/notes", "ts": 20},
          "short": {"path": "/Users/a/x", "ts": 30},
          "aaaaaaaaaaaaaaaa": {"path": "relative", "ts": 40}
        }}
        """
        let vaults = ObsidianVaults.parse(Data(json.utf8))
        XCTAssertEqual(vaults.map(\.id), ["fedcba9876543210", "0123456789abcdef"])
        XCTAssertEqual(vaults[1].name, "일")
        XCTAssertTrue(vaults[1].inICloud)
        XCTAssertFalse(vaults[0].inICloud)
        XCTAssertEqual(ObsidianVaults.id(for: "/Users/a/notes/", in: vaults), "fedcba9876543210")
        XCTAssertNil(ObsidianVaults.id(for: "/Users/a/other", in: vaults))
        XCTAssertTrue(ObsidianVaults.parse(Data("nope".utf8)).isEmpty)
    }

    func testOnlyAFolderThatIsSurelyGoneIsLeftOut() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent("a.md")
        try Data().write(to: file)
        XCTAssertTrue(ObsidianVaults.mayExist(dir.path))
        XCTAssertFalse(ObsidianVaults.mayExist(file.path))
        XCTAssertFalse(ObsidianVaults.mayExist(dir.appendingPathComponent("gone").path))
        XCTAssertFalse(ObsidianVaults.mayExist(file.appendingPathComponent("below").path))
    }

    func testFolderBodyCarriesTheVaultIDOnlyWhenKnown() {
        XCTAssertEqual(NSDictionary(dictionary: NoteFolder(path: "/a", vaultID: "0123456789abcdef", name: "일").json),
                       ["path": "/a", "vault_id": "0123456789abcdef", "name": "일"])
        XCTAssertEqual(NSDictionary(dictionary: NoteFolder(path: "/a").json), ["path": "/a"])
    }

    private func folder(_ name: String, obsidian: Bool = true, notes: Int? = 120, missing: Bool = false) -> Info.Folder {
        Info.Folder(id: name, path: "/Users/a/\(name)", name: name, vaultId: obsidian ? "0123456789abcdef" : nil,
                    obsidian: obsidian, notes: notes, missing: missing)
    }

    private func states() -> [(String, NotesStatus)] {
        [
            ("01-off", NotesStatus(info: Info(chosen: 0, folders: [], notes: nil, notDownloaded: nil, readAt: nil))),
            ("02-reading", NotesStatus(info: Info(chosen: 1, folders: [folder("일 노트", notes: nil)], notes: nil,
                                                  notDownloaded: nil, readAt: nil))),
            ("03-on", NotesStatus(info: Info(chosen: 2, folders: [folder("일 노트"), folder("회의록")], notes: 340,
                                             notDownloaded: 0, readAt: "2026-11-13T05:00:00Z"))),
            ("04-mixed", NotesStatus(info: Info(chosen: 2, folders: [folder("일 노트"), folder("메모", obsidian: false)],
                                                notes: 210, notDownloaded: 0, readAt: nil))),
            ("05-icloud", NotesStatus(info: Info(chosen: 1, folders: [folder("일 노트")], notes: 120, notDownloaded: 14,
                                                 readAt: nil))),
            ("06-missing", NotesStatus(info: Info(chosen: 1, folders: [folder("일 노트", missing: true)], notes: 0,
                                                  notDownloaded: 0, readAt: nil))),
        ]
    }

    func testLines() {
        let s = Dictionary(uniqueKeysWithValues: states())
        XCTAssertFalse(s["01-off"]!.on)
        XCTAssertEqual(s["02-reading"]!.line, "vault 1개 · 노트를 읽는 중")
        XCTAssertEqual(s["03-on"]!.line, "vault 2개 · 노트 340개")
        XCTAssertNil(s["03-on"]!.note)
        XCTAssertEqual(s["04-mixed"]!.line, "vault 1개 · 폴더 1개 · 노트 210개")
        XCTAssertEqual(s["05-icloud"]!.note, "iCloud에서 받지 못한 노트 14개는 지금은 제목으로만 찾아요. 다음에 다시 받아요.")
        XCTAssertEqual(s["06-missing"]!.note, "일 노트 폴더를 찾을 수 없어요. 옮겼다면 다시 골라 주세요.")
    }

    /// The onboarding tile's two lines and when it shows a spinner instead.
    func testTileLines() {
        let s = Dictionary(uniqueKeysWithValues: states())
        XCTAssertFalse(s["01-off"]!.reading)
        XCTAssertTrue(s["02-reading"]!.reading)
        XCTAssertNil(s["02-reading"]!.notesLine)
        XCTAssertEqual(s["03-on"]!.chosenLine, "vault 2개")
        XCTAssertEqual(s["03-on"]!.notesLine, "노트 340개")
        XCTAssertFalse(s["03-on"]!.reading)
        XCTAssertEqual(s["04-mixed"]!.chosenLine, "vault 1개 · 폴더 1개")
        // a gone folder is never read, so it does not keep the tile reading
        XCTAssertFalse(s["06-missing"]!.reading)
    }

    /// Note folders let the onboarding go on only once a note is in the search data.
    func testNotesCountAsASourceOnceFound() {
        XCTAssertFalse(OnboardingFlow.notesFound(notes: nil))
        XCTAssertFalse(OnboardingFlow.notesFound(notes: 0))
        XCTAssertTrue(OnboardingFlow.notesFound(notes: 3))
        XCTAssertTrue(OnboardingFlow.hasSource(github: false, notion: false, linear: false, gdrive: false, atlassian: false,
                                               notes: OnboardingFlow.notesFound(notes: 3)))
    }

    private func png<V: View>(_ view: V, width: CGFloat, _ name: String, dark: Bool = false, to out: URL) {
        AppearanceMode.isDark = dark
        defer { AppearanceMode.isDark = false }
        let framed = view.frame(width: width).padding(20).background(dark ? Color(white: 0.11) : Theme.fill)
            .environment(\.colorScheme, dark ? .dark : .light)
        let r = ImageRenderer(content: framed)
        r.scale = 2
        guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
        try? data.write(to: out.appendingPathComponent("\(name).png"))
    }

    func testRenderStates() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_NOTES_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let rows = [NotesPickRow(path: "/Users/a/Library/Mobile Documents/iCloud~md~obsidian/Documents/일 노트", name: "일 노트",
                                 vaultID: "0123456789abcdef", iCloud: true),
                    NotesPickRow(path: "/Users/a/Documents/회의록", name: "회의록", vaultID: "fedcba9876543210", iCloud: false)]
        for dark in [false, true] {
            let tail = dark ? "-dark" : ""
            for (name, status) in states() {
                png(NotesCardBody(status: status), width: 560, "card-\(name)\(tail)", dark: dark, to: out)
            }
            png(NotesFoldersBody(rows: rows, on: .constant([rows[0].path])), width: 420, "sheet-1-vaults\(tail)", dark: dark, to: out)
            png(NotesFoldersBody(rows: [], on: .constant([])), width: 420, "sheet-2-none\(tail)", dark: dark, to: out)
            for (name, status) in states() {
                png(NotesTileBody(status: status), width: 233, "tile-\(name)\(tail)", dark: dark, to: out)
            }
        }
    }
}
