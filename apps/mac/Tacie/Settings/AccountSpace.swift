import Foundation

/// Where one Tacie account's data lives on this Mac (docs/04 4절 "계정마다 따로"): its meetings, remembered people, search
/// data (the engine's `index` and `data`) and connections (GitHub, Notion, Linear, Google Drive, Dropbox). Signing out
/// leaves them where they are and nothing shows them; signing in again brings them back.
///
/// Signed in: `Application Support/Tacie/accounts/<id>/` (`id` is the account's `analytics_id` from GET /me: the same
/// on every Mac, not the Google, GitHub or Apple id). Signed out: `Application Support/Tacie/` itself, the layout from
/// before accounts had their own space. What was made there (everything from before this change, or meetings made
/// without a sign-in) joins the next account signed in (`SpaceAdoption.merge`).
///
/// The downloaded models (`Tacie/models`) and the settings of this Mac stay shared.
struct AccountSpace: Equatable, Sendable {
    /// nil: signed out.
    let id: String?

    init(id: String?) {
        self.id = id.flatMap(Self.valid)
    }

    static let signedOut = AccountSpace(id: nil)

    /// The space the app uses now. Set at launch and by AppDelegate's switch only (main thread); the keychain stores
    /// read it so a connection is always saved where the shown data lives.
    nonisolated(unsafe) static var inUse = AccountSpace.signedOut

    /// `Application Support/Tacie` (`Tacie Dev` for a development build).
    static var base: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent(Build.supportFolder, isDirectory: true)
    }

    func folder(base: URL = AccountSpace.base) -> URL {
        guard let id else { return base }
        return base.appendingPathComponent("accounts", isDirectory: true).appendingPathComponent(id, isDirectory: true)
    }

    var meetings: URL { folder().appendingPathComponent("meetings", isDirectory: true) }
    var people: URL { folder().appendingPathComponent("people", isDirectory: true) }

    /// A keychain account name or settings key of this space: the old name when signed out, `name@id` otherwise.
    func key(_ name: String) -> String {
        guard let id else { return name }
        return "\(name)@\(id)"
    }

    /// Some account has its own space on this Mac (signed out now, its meetings wait for the next sign-in).
    static func anyAccount(base: URL = AccountSpace.base) -> Bool {
        let accounts = base.appendingPathComponent("accounts", isDirectory: true)
        let names = (try? FileManager.default.contentsOfDirectory(atPath: accounts.path)) ?? []
        return names.contains { valid($0) != nil }
    }

    /// Only plain ids become folder names (the server sends 32 hex characters).
    static func valid(_ id: String) -> String? {
        let ok = !id.isEmpty && id.count <= 64 && id.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber) }
        return ok ? id : nil
    }
}

/// Moving what the signed-out space holds into an account's space. Runs only while nothing uses either space:
/// the engine stopped, no meeting, the meeting list about to be read again (AppDelegate.switchSpace).
struct SpaceAdoption {
    /// The folders and files of a space, as the app and the engine name them.
    /// `index.new` is the engine's swap place: a sync cut short leaves it, and the engine puts it back on its next start
    /// (tacie-index `recover`).
    static let items = ["meetings", "people", "index", "index.old", "index.new", "data", "linear-refresh.pending",
                        "atlassian-refresh.pending", "asana-refresh.pending", "panel", "terms.json"]
    /// The connections' keychain items: (service, account name when signed out).
    static let keychain = [("ai.tacie.github", "token"), ("ai.tacie.notion", "connection"),
                           ("ai.tacie.linear", "connection"), ("ai.tacie.gdrive", "connection"),
                           ("ai.tacie.atlassian", "connection"), ("ai.tacie.asana", "connection"),
                           ("ai.tacie.dropbox", "connection")]
    /// The settings that belong to a space.
    static let defaultsKeys = [GitHubCredentials.disconnectedKey, FirstStepsStore.key, CalendarStore.onKey, CalendarStore.idsKey,
                               MCPAccess.onKey]
        + SpaceAdoption.syncKeys
    #if TACIE_OPEN
    static let syncKeys: [String] = []  // no iPhone sync in the open-source edition (docs/41 8절)
    #else
    static let syncKeys = IPhoneMeetings.keys
    #endif
    /// Set while a move runs: an app that stopped half way finishes the move on the next switch to that account.
    static let pendingKey = "account.adopting"

    var base: URL = AccountSpace.base
    var fm: FileManager = .default
    var defaults: UserDefaults = .standard
    var store: (_ service: String, _ account: String) -> any TokenStore = { s, a in KeychainTokenStore(service: s, account: a) }

    /// The engine's search data: an account that has any of it keeps its own, and the signed-out copy is dropped (a
    /// signed-out `index.new` must never be swapped into the account's index).
    static let searchData = [["index", "index.old", "index.new"], ["data"]]


    /// Moves what the signed-out space holds into the account's space, on every sign-in (#56: meetings made without an
    /// account join the next account signed in). A name the account already has stays where it was: meetings and
    /// people file by file, connections and settings by key. Search data the account has wins (`searchData`). Each
    /// item moves on its own (a rename on the same disk); one that cannot move stays, is logged, and the next sign-in
    /// tries again. Returns whether any meeting moved.
    @discardableResult
    func merge(into id: String) -> Bool {
        // what an unfinished merge left in the signed-out space goes to whoever signs in next, like anything made
        // there since: the pending marker only says a retry is due
        guard AccountSpace.valid(id) != nil else { return false }
        let from = AccountSpace.signedOut.folder(base: base)
        let to = AccountSpace(id: id).folder(base: base)
        defaults.set(id, forKey: Self.pendingKey)
        var left = 0
        do {
            try fm.createDirectory(at: to, withIntermediateDirectories: true)
        } catch {
            Log.info("account space not made: \(error.localizedDescription)")
            defaults.removeObject(forKey: Self.pendingKey)
            return false
        }
        let meetingsBefore = count(from.appendingPathComponent("meetings"))
        for group in Self.searchData {
            let accountHas = group.contains { fm.fileExists(atPath: to.appendingPathComponent($0).path) }
            for name in group where fm.fileExists(atPath: from.appendingPathComponent(name).path) {
                let src = from.appendingPathComponent(name)
                if accountHas {
                    do { try fm.removeItem(at: src) } catch { left += 1 }
                } else if !move(src, to: to.appendingPathComponent(name)) {
                    left += 1
                }
            }
        }
        let searchNames = Set(Self.searchData.joined())
        for name in Self.items where !searchNames.contains(name) && fm.fileExists(atPath: from.appendingPathComponent(name).path) {
            if !move(from.appendingPathComponent(name), to: to.appendingPathComponent(name)) { left += 1 }
        }
        for (service, account) in Self.keychain {
            let old = store(service, account)
            let new = store(service, AccountSpace(id: id).key(account))
            do {
                guard let secret = try old.readChecked() else { continue }
                // the account's own connection wins; the signed-out one stays signed out
                guard try new.readChecked() == nil else { continue }
                try new.save(secret)
                try old.delete()
            } catch {
                left += 1  // a locked or refusing keychain: the move stays pending and is tried again
            }
        }
        for key in Self.defaultsKeys {
            guard let value = defaults.object(forKey: key) else { continue }
            let newKey = AccountSpace(id: id).key(key)
            guard defaults.object(forKey: newKey) == nil else { continue }
            defaults.set(value, forKey: newKey)
            defaults.removeObject(forKey: key)
        }
        if left == 0 { defaults.removeObject(forKey: Self.pendingKey) }
        let moved = meetingsBefore - count(from.appendingPathComponent("meetings"))
        Log.info(left == 0 ? "signed-out data merged into the account's space (\(moved) meeting files)"
                           : "signed-out data merged, \(left) left for the next try")
        return moved > 0
    }

    private func count(_ folder: URL) -> Int {
        (try? fm.contentsOfDirectory(atPath: folder.path).count) ?? 0
    }

    /// The deleted accounts whose space is still to be erased (kept so a quit in between does not leave it).
    static let eraseKey = "account.erase"

    /// The account was deleted on the server: its space goes once nothing uses it (`eraseRequested`).
    func requestErase(_ id: String) {
        guard AccountSpace.valid(id) != nil else { return }
        var ids = defaults.stringArray(forKey: Self.eraseKey) ?? []
        if !ids.contains(id) { ids.append(id) }
        defaults.set(ids, forKey: Self.eraseKey)
    }

    /// Erases every requested space except the one in use: its folder (meetings, recordings, people, search data),
    /// its connections in the keychain and its settings. One that cannot be erased fully stays requested.
    func eraseRequested(keeping inUse: AccountSpace) {
        let ids = defaults.stringArray(forKey: Self.eraseKey) ?? []
        guard !ids.isEmpty else { return }
        var left: [String] = []
        for id in ids where id != inUse.id {
            if !erase(id) { left.append(id) }
        }
        if let current = inUse.id, ids.contains(current) { left.append(current) }
        defaults.set(left.isEmpty ? nil : left, forKey: Self.eraseKey)
    }

    private func erase(_ id: String) -> Bool {
        let space = AccountSpace(id: id)
        var ok = true
        let folder = space.folder(base: base)
        if fm.fileExists(atPath: folder.path) {
            do { try fm.removeItem(at: folder) } catch {
                ok = false
                Log.info("deleted account's space not erased: \(error.localizedDescription)")
            }
        }
        for (service, account) in Self.keychain {
            do { try store(service, space.key(account)).delete() } catch { ok = false }
        }
        for key in Self.defaultsKeys { defaults.removeObject(forKey: space.key(key)) }
        if defaults.string(forKey: Self.pendingKey) == id { defaults.removeObject(forKey: Self.pendingKey) }
        if ok { Log.info("deleted account's space erased") }
        return ok
    }

    /// A folder whose name is free at the destination moves whole. One that is there already moves its entries one by
    /// one. A file whose name is taken at the destination stays where it was: that is a kept clash, not a failure.
    private func move(_ src: URL, to dst: URL) -> Bool {
        if !fm.fileExists(atPath: dst.path) {
            do { try fm.moveItem(at: src, to: dst); return true } catch {
                Log.info("account space: \(src.lastPathComponent) not moved: \(error.localizedDescription)")
                return false
            }
        }
        var srcDir: ObjCBool = false, dstDir: ObjCBool = false
        guard fm.fileExists(atPath: src.path, isDirectory: &srcDir), srcDir.boolValue,
              fm.fileExists(atPath: dst.path, isDirectory: &dstDir), dstDir.boolValue else { return true }
        var ok = true
        for name in (try? fm.contentsOfDirectory(atPath: src.path)) ?? [] {
            ok = move(src.appendingPathComponent(name), to: dst.appendingPathComponent(name)) && ok
        }
        if ok, ((try? fm.contentsOfDirectory(atPath: src.path)) ?? []).isEmpty { try? fm.removeItem(at: src) }
        return ok
    }
}
