import Foundation

/// Brings an earlier open-source build's meetings and settings into this app once (#56: one app). The open build kept
/// its data in `Application Support/Tacie Open` and its settings under `ai.tacie.open`. Meetings, remembered people
/// and downloaded models move into this app's signed-out space (a name already here stays where it was, like
/// `SpaceAdoption.merge`). The AI choice, addresses and speech settings are copied only into an app never used before
/// (no meetings, no account here, not signed in), and only where ours are unset: someone who already used this app
/// keeps the AI they chose. Keys are never read: the person pastes them again in Settings → AI.
enum OpenEditionImport {
    /// Set once the import ran (moved anything or not), so it never runs twice.
    static let markerKey = "open-imported"
    /// Set when meetings came over, until the home page's line is closed.
    static let noticeKey = "open-imported.notice"
    /// The open build used a key (API key or own providers): it has to be pasted again here.
    static let usedKeyKey = "open-imported.usedKey"
    /// Folders that move, file by file.
    static let folders = ["meetings", "people", "models"]
    /// Settings that are copied when ours are unset. No keychain item and no account data is among them.
    static let settingsKeys = [AIMode.defaultsKey, AIAddress.urlKey, AIAddress.modelKey, LocalServer.nameKey,
                               LocalServer.urlKey, LocalServer.modelKey, CardsChoice.defaultsKey,
                               SttProviderChoice.providerKey, SttProviderChoice.regionKey, LocalSTT.key]

    /// The open build's folder and settings on this Mac (release builds only: a development build has its own names).
    static var openFolder: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent(Build.names(dev: false, open: true).supportFolder, isDirectory: true)
    }
    static let openSuite = "ai.tacie.open"

    /// Runs once. Returns whether any meeting, person, model or setting came over.
    @discardableResult
    static func run(base: URL, from: URL, defaults: UserDefaults, openDefaults: UserDefaults?, signedIn: Bool = false,
                    fm: FileManager = .default) -> Bool {
        guard !defaults.bool(forKey: markerKey) else { return false }
        let usedBefore = signedIn || fm.fileExists(atPath: base.appendingPathComponent("meetings").path)
            || fm.fileExists(atPath: base.appendingPathComponent("accounts").path)
        var moved = false, meetingsMoved = false
        if fm.fileExists(atPath: from.path) {
            for name in folders {
                let src = from.appendingPathComponent(name)
                guard fm.fileExists(atPath: src.path) else { continue }
                if merge(src, to: base.appendingPathComponent(name), fm: fm) {
                    moved = true
                    if name == "meetings" { meetingsMoved = true }
                }
            }
        }
        let theirMode = openDefaults?.string(forKey: AIMode.defaultsKey).flatMap(AIMode.init(rawValue:))
        if let openDefaults, !usedBefore {
            for key in settingsKeys {
                guard defaults.object(forKey: key) == nil, let value = openDefaults.object(forKey: key) else { continue }
                defaults.set(value, forKey: key)
                moved = true
            }
        }
        defaults.set(true, forKey: markerKey)
        if meetingsMoved {
            defaults.set(true, forKey: noticeKey)
            defaults.set(theirMode == .key || theirMode == .custom, forKey: usedKeyKey)
        }
        if moved {
            Log.info("open edition's meetings and settings brought over")
        }
        return moved
    }

    /// The home page's line, set by `run`: whether the person used a key in the open build (it has to be pasted again).
    static func usedKey(_ d: UserDefaults = .standard) -> Bool { d.bool(forKey: usedKeyKey) }

    /// Moves each entry whose name is free at `dst`; a folder on both sides merges inside. Returns whether anything moved.
    private static func merge(_ src: URL, to dst: URL, fm: FileManager) -> Bool {
        var srcDir: ObjCBool = false
        guard fm.fileExists(atPath: src.path, isDirectory: &srcDir) else { return false }
        // an empty folder brings nothing
        if srcDir.boolValue, ((try? fm.contentsOfDirectory(atPath: src.path)) ?? []).isEmpty { return false }
        if !fm.fileExists(atPath: dst.path) {
            do {
                try fm.createDirectory(at: dst.deletingLastPathComponent(), withIntermediateDirectories: true)
                try fm.moveItem(at: src, to: dst)
                return true
            } catch {
                Log.info("open edition: \(src.lastPathComponent) not moved: \(error.localizedDescription)")
                return false
            }
        }
        var dstDir: ObjCBool = false
        guard srcDir.boolValue, fm.fileExists(atPath: dst.path, isDirectory: &dstDir), dstDir.boolValue else { return false }
        var moved = false
        for name in (try? fm.contentsOfDirectory(atPath: src.path)) ?? [] {
            if merge(src.appendingPathComponent(name), to: dst.appendingPathComponent(name), fm: fm) { moved = true }
        }
        return moved
    }
}
