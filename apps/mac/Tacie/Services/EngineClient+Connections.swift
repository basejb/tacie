import Foundation

/// The engine calls that hand over this Mac's connections (AI keys, Linear, Google Drive, Atlassian, Asana, Dropbox,
/// note folders). They stay in the Mac app: the iPhone engine has no sources (docs/27 7-11). The shared part of
/// `EngineClient` is in apps/shared.
extension EngineClient {
    /// Hands the engine the AI keys (memory only, never logged): a key, or nil to drop the one it holds.
    /// Answers which keys the engine can use now (a dev engine also counts its key files).
    /// `chat` is the AI address (AIAddress.payload: the url, model and key, or NSNull to drop it).
    /// `stt` is the speech-to-text provider (SttProviderChoice.payload, or NSNull).
    func setAIKeys(_ keys: [AIKeys.Name: String?], chat: Any = NSNull(), stt: Any = NSNull()) async throws -> [String: Bool] {
        var body: [String: Any] = [:]
        for (name, key) in keys { body[name.rawValue] = key ?? NSNull() }
        body["chat"] = chat
        body["stt"] = stt
        return try await post("/ai/credentials", body: body)
    }

    /// Hands the engine the Linear connection (memory only, never logged), or nil to disconnect: the engine then drops
    /// the found issues. Muted teams go along and the engine remembers them. A token-only change starts nothing.
    func setLinear(_ c: LinearConnection?) async throws -> Bool {
        struct Answer: Codable { let connected: Bool }
        let body: [String: Any] = [
            "token": c?.token ?? NSNull(), "workspace": c?.workspace ?? NSNull(),
            "workspace_id": c?.workspaceID ?? NSNull(), "muted_teams": c?.muted ?? [],
        ]
        let r: Answer = try await post("/linear/credentials", body: body)
        return r.connected
    }

    /// Hands the engine the Google Drive connection (memory only, never logged), or nil to disconnect: the engine then
    /// drops the found files. Muted shared drives go along and the engine remembers them. A token-only change starts nothing.
    func setGDrive(_ c: GDriveConnection?) async throws -> Bool {
        struct Answer: Codable { let connected: Bool }
        let body: [String: Any] = [
            "token": c?.token ?? NSNull(), "account": c?.account ?? NSNull(),
            "account_id": c?.accountID ?? NSNull(), "muted_drives": c?.muted ?? [],
        ]
        let r: Answer = try await post("/gdrive/credentials", body: body)
        return r.connected
    }

    /// Hands the engine the Jira · Confluence connection (memory only, never logged), or nil to disconnect: the engine
    /// then deletes what it fetched. Muted projects and spaces go along. A token-only change starts nothing.
    func setAtlassian(_ c: AtlassianConnection?) async throws -> Bool {
        struct Answer: Codable { let connected: Bool }
        let r: Answer = try await post("/atlassian/credentials", body: AtlassianConnection.engineBody(c))
        return r.connected
    }

    /// Hands the engine the Asana connection (memory only, never logged), or nil to disconnect: the engine then deletes
    /// what it fetched. Muted workspaces and projects go along. A token-only change starts nothing.
    func setAsana(_ c: AsanaConnection?) async throws -> Bool {
        struct Answer: Codable { let connected: Bool }
        let r: Answer = try await post("/asana/credentials", body: AsanaConnection.engineBody(c))
        return r.connected
    }

    /// Hands the engine the Dropbox connection (memory only, never logged), or nil to disconnect: the engine then
    /// deletes what it fetched. Muted folders go along and the engine remembers them. A token-only change starts nothing.
    func setDropbox(_ c: DropboxConnection?) async throws -> Bool {
        struct Answer: Codable { let connected: Bool }
        let r: Answer = try await post("/dropbox/credentials", body: DropboxConnection.engineBody(c))
        return r.connected
    }

    /// Every note folder the user chose (docs/34): the engine keeps exactly this list and reads them. Returns how many
    /// it got. The paths are never logged.
    func setNoteFolders(_ list: [NoteFolder]) async throws -> Int {
        struct Answer: Codable { let count: Int }
        let r: Answer = try await post("/notes/folders", body: ["folders": list.map(\.json)], timeout: 30)
        return r.count
    }
}
