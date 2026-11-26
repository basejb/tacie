import Foundation

/// Words the person taught Tacie by fixing a misheard word in the panel (docs/38 12-4절), per account. They go to Soniox
/// as `context.terms` ahead of the team terms the engine picks, so the next meeting hears them.
enum MeetingTerms {
    /// Most words kept: the newest stay.
    static let limit = 30

    static func file(_ folder: URL) -> URL { folder.appendingPathComponent("terms.json") }

    /// Newest first.
    static func words(_ folder: URL = AccountSpace.inUse.folder()) -> [String] {
        guard let data = try? Data(contentsOf: file(folder)),
              let list = try? JSONDecoder().decode([String].self, from: data) else { return [] }
        return list
    }

    static func remember(_ word: String, in folder: URL = AccountSpace.inUse.folder()) {
        let w = word.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !w.isEmpty else { return }
        let list = Array(([w] + words(folder).filter { $0.lowercased() != w.lowercased() }).prefix(limit))
        let url = file(folder)
        do {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try JSONEncoder().encode(list).write(to: url, options: .atomic)
        } catch {
            Log.info("terms not saved: \(error.localizedDescription)")
        }
    }

}
