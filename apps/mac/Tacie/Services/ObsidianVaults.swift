import Foundation

/// The Obsidian vaults this Mac's Obsidian has opened (docs/34 2절): `~/Library/Application Support/obsidian/
/// obsidian.json` is `{"vaults": {"<16-character ID>": {"path", "ts", "open"}}}`. The format is not documented, so a
/// file that cannot be read gives no vaults and the user picks the folder instead. Only paths and IDs are read here;
/// no note is opened.
struct ObsidianVault: Equatable, Identifiable {
    let id: String  // the vault ID obsidian:// links take
    let path: String
    var name: String { (path as NSString).lastPathComponent }
    /// In iCloud Drive: its notes may not be on this Mac yet and come down when Tacie reads them.
    var inICloud: Bool { path.contains("/Library/Mobile Documents/") }
}

enum ObsidianVaults {
    static var registry: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/obsidian/obsidian.json")
    }

    /// The vaults in the registry, most recently opened first. Folders that are surely gone are left out; a folder
    /// macOS will not let this app look into yet (iCloud Drive before the user allows it) stays in the list.
    static func found(at url: URL = registry, exists: (String) -> Bool = mayExist) -> [ObsidianVault] {
        guard let data = try? Data(contentsOf: url) else { return [] }
        return parse(data).filter { exists($0.path) }
    }

    /// False only when the folder is not there (ENOENT, ENOTDIR) or is a file: any other failure (EPERM from a
    /// protected folder) says nothing about whether it exists.
    static func mayExist(_ path: String) -> Bool {
        var st = stat()
        if stat(path, &st) == 0 { return (st.st_mode & S_IFMT) == S_IFDIR }
        return errno != ENOENT && errno != ENOTDIR
    }

    static func parse(_ data: Data) -> [ObsidianVault] {
        guard let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let vaults = root["vaults"] as? [String: Any] else { return [] }
        let list: [(ObsidianVault, Double)] = vaults.compactMap { id, v in
            guard id.count == 16, id.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber) }),
                  let v = v as? [String: Any], let path = v["path"] as? String, path.hasPrefix("/") else { return nil }
            return (ObsidianVault(id: id, path: path), (v["ts"] as? NSNumber)?.doubleValue ?? 0)
        }
        return list.sorted { $0.1 != $1.1 ? $0.1 > $1.1 : $0.0.path < $1.0.path }.map(\.0)
    }

    /// The vault a folder is, when the user picked it by hand: the registry's entry for the same path.
    static func id(for path: String, in vaults: [ObsidianVault]) -> String? {
        let p = (path as NSString).standardizingPath
        return vaults.first { ($0.path as NSString).standardizingPath == p }?.id
    }
}

/// A note folder the engine reads (POST /notes/folders): an Obsidian vault or any folder of Markdown files.
struct NoteFolder: Equatable {
    let path: String
    var vaultID: String?
    var name: String?

    var json: [String: Any] {
        var j: [String: Any] = ["path": path]
        if let vaultID { j["vault_id"] = vaultID }
        if let name { j["name"] = name }
        return j
    }
}
