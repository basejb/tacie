import Foundation

/// The developer tab's overrides. Only a development build hands them to the engine.
enum DevSettings {
    static let sttModel = "dev.sttModel", answerModel = "dev.answerModel", jevModel = "dev.jevModel",
               gate = "dev.gateThreshold", show = "dev.showThreshold", debugText = "dev.debugText",
               sttPath = "dev.sttPath",  // app side only (docs/25): nil/"stream", "shadow" or "server"
               sonioxRegion = "dev.sonioxRegion"  // "jp", "us" or "eu" when picked; unset: where the Mac is

    /// "0,75" reads as 0.75; anything outside 0...1 or not a number is nil.
    static func threshold(_ s: String) -> Double? {
        let t = s.trimmingCharacters(in: .whitespacesAndNewlines).replacingOccurrences(of: ",", with: ".")
        guard let v = Double(t), v.isFinite, (0...1).contains(v) else { return nil }
        return v
    }

    /// A Release build shares the bundle id and would read a dev build's leftovers, so it passes nothing.
    static func environment(_ d: UserDefaults, dev: Bool) -> [String: String] {
        guard dev else { return [:] }
        var env: [String: String] = [:]
        for (key, name) in [(sttModel, "TACIE_STT_MODEL"), (answerModel, "TACIE_ANSWER_MODEL"), (jevModel, "TACIE_JEV_MODEL")] {
            let v = (d.string(forKey: key) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if !v.isEmpty { env[name] = v }
        }
        for (key, name) in [(gate, "TACIE_GATE_THR"), (show, "TACIE_SHOW_THR")] {
            if let v = threshold(d.string(forKey: key) ?? "") { env[name] = String(v) }
        }
        if d.bool(forKey: debugText) { env["TACIE_LOG_TEXT"] = "1" }
        // a region picked on the developer page; otherwise the engine gets where the Mac is (EngineProcess)
        if let r = d.string(forKey: sonioxRegion), ["jp", "us", "eu"].contains(r) { env["TACIE_SONIOX_REGION"] = r }
        return env
    }
}

enum SlackConfig {
    static let url = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".config/slack/config.json")

    struct BadConfig: Error {}

    /// Keeps the other keys (such as "me") and sets "bot_token". Broken JSON throws rather than being overwritten.
    static func merged(_ existing: Data?, token: String) throws -> Data {
        var obj: [String: Any] = [:]
        if let existing {
            guard let parsed = try JSONSerialization.jsonObject(with: existing) as? [String: Any] else { throw BadConfig() }
            obj = parsed
        }
        obj["bot_token"] = token.trimmingCharacters(in: .whitespacesAndNewlines)
        return try JSONSerialization.data(withJSONObject: obj, options: [.prettyPrinted, .sortedKeys])
    }

    /// The temp file is opened O_EXCL with mode 0600 next to the target, so the token is never world-readable.
    static func save(token: String, to url: URL = SlackConfig.url) throws {
        let fm = FileManager.default
        let dir = url.deletingLastPathComponent()
        try fm.createDirectory(at: dir, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let data = try merged(try? Data(contentsOf: url), token: token)
        let temp = dir.appendingPathComponent(".config.json.\(UUID().uuidString)")
        let fd = open(temp.path, O_CREAT | O_EXCL | O_WRONLY, 0o600)
        guard fd >= 0 else { throw BadConfig() }
        do {
            let h = FileHandle(fileDescriptor: fd, closeOnDealloc: false)
            defer { try? h.close() }
            try h.write(contentsOf: data)
            if fm.fileExists(atPath: url.path) {
                _ = try fm.replaceItemAt(url, withItemAt: temp, backupItemName: nil, options: .usingNewMetadataOnly)
            } else {
                try fm.moveItem(at: temp, to: url)
            }
            try fm.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
        } catch {
            try? fm.removeItem(at: temp)
            throw error
        }
    }
}
