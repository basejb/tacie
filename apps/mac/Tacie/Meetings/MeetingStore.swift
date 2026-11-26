import Foundation

/// Meetings on disk: one JSON file per meeting in Application Support, nowhere else.
struct MeetingStore {
    let directory: URL

    /// The meetings of the account in use (AccountSpace).
    static var standard: MeetingStore { MeetingStore(directory: AccountSpace.inUse.meetings) }

    private static let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.dateEncodingStrategy = .iso8601
        e.outputFormatting = [.prettyPrinted, .sortedKeys]
        return e
    }()

    private static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .iso8601
        return d
    }()

    /// Newest first. A file that does not read (cut off by a crash, edited by hand) is skipped, not fatal.
    func loadAll() -> [SavedMeeting] {
        flush()
        let files = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
        return files.filter { $0.pathExtension == "json" }.compactMap { url in
            guard let data = try? Data(contentsOf: url), var m = try? Self.decoder.decode(SavedMeeting.self, from: data) else {
                Log.info("meeting file skipped: \(url.lastPathComponent)")
                return nil
            }
            // meetings saved before 2026-11-10 kept lines that are only sound tags; 대화 기록 no longer shows them
            m.transcript.removeAll(where: \.isSoundOnly)
            return m
        }
        .sorted { $0.start > $1.start }
    }

    /// One meeting by id, without reading the rest of the folder (MCP `read_transcript`); nil when it is not there.
    func load(id: String) -> SavedMeeting? {
        // an id is a file name inside this folder, never a path
        guard !id.isEmpty, !id.contains("/"), !id.hasPrefix(".") else { return nil }
        flush()
        guard let data = try? Data(contentsOf: file(id)), var m = try? Self.decoder.decode(SavedMeeting.self, from: data) else {
            return nil
        }
        m.transcript.removeAll(where: \.isSoundOnly)
        return m
    }

    func write(_ m: SavedMeeting) throws {
        try Self.saves.sync { try put(m) }
    }

    /// Writes in the background, after the saves asked for before it: encoding a meeting with its transcript held the
    /// main thread 5–8 ms on every to-do checked or line edited. Whatever reads or deletes the folder waits for it first.
    func save(_ m: SavedMeeting) {
        Self.saves.async {
            do { try put(m) } catch { Log.info("meeting write failed: \(error.localizedDescription)") }
        }
    }

    /// Waits for the saves already asked for (quitting, tests).
    func flush() { Self.saves.sync {} }

    /// One queue for every folder: saves run in the order they were asked for.
    private static let saves = DispatchQueue(label: "ai.tacie.meeting-saves", qos: .userInitiated)

    private func put(_ m: SavedMeeting) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try Self.encoder.encode(m).write(to: file(m.id), options: .atomic)
    }

    /// The meeting's recording; the file may not exist (after a crash only `MeetingRecorder.parts(for:)` are left,
    /// see `playableFiles(for:)`).
    func audioFile(_ id: String) -> URL { directory.appendingPathComponent("\(id).caf") }

    /// A live meeting rewrites its record at least every minute; one touched within this long may belong to another process.
    static let liveWindow: TimeInterval = 180

    static func recentlyChanged(_ url: URL, within seconds: TimeInterval, now: Date) -> Bool {
        guard let m = (try? FileManager.default.attributesOfItem(atPath: url.path))?[.modificationDate] as? Date else { return false }
        return now.timeIntervalSince(m) < seconds
    }

    /// Whether the meeting's record changed so lately that another Tacie may still be running it.
    func maybeLive(_ id: String, quietFor: TimeInterval = MeetingStore.liveWindow, now: Date = Date()) -> Bool {
        flush()
        return Self.recentlyChanged(file(id), within: quietFor, now: now)
    }

    /// The record and its recording together.
    func delete(_ id: String) throws {
        flush()  // a save still waiting would write the record back
        var first: Error?
        for url in [file(id)] + MeetingRecorder.allFiles(for: audioFile(id)) {
            do { try FileManager.default.removeItem(at: url) } catch CocoaError.fileNoSuchFile {} catch { first = first ?? error }
        }
        if let first { throw first }
    }

    /// Only the recording; the transcript stays. Quiet when there is none.
    func deleteAudio(_ id: String) throws {
        var first: Error?
        for url in MeetingRecorder.allFiles(for: audioFile(id)) {
            do { try FileManager.default.removeItem(at: url) } catch CocoaError.fileNoSuchFile {} catch { first = first ?? error }
        }
        if let first { throw first }
    }

    /// Recording files (final, parts, half-made join) whose meeting has no record: the meeting is gone, so is its sound.
    /// Only for launch, when no meeting is being recorded.
    /// Files changed within `quietFor` seconds are left: another running Tacie may be recording that meeting.
    func deleteOrphanAudio(quietFor: TimeInterval = MeetingStore.liveWindow, now: Date = Date()) throws {
        flush()
        guard let files = try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil) else { return }
        let known = Set(files.filter { $0.pathExtension == "json" }.map { $0.deletingPathExtension().lastPathComponent })
        var first: Error?
        for url in files where url.pathExtension == "caf" {
            let id = String(url.lastPathComponent.prefix { $0 != "." })
            guard !known.contains(id), !Self.recentlyChanged(url, within: quietFor, now: now) else { continue }
            do { try FileManager.default.removeItem(at: url) } catch CocoaError.fileNoSuchFile {} catch { first = first ?? error }
        }
        if let first { throw first }
    }

    func deleteAll() throws {
        flush()
        // Every *.json (and recording), readable or not: a file half-written when the app died still holds transcript text.
        guard FileManager.default.fileExists(atPath: directory.path) else { return }
        let files = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
        var first: Error?
        for url in files where url.pathExtension == "json" || url.pathExtension == "caf" {
            do { try FileManager.default.removeItem(at: url) } catch CocoaError.fileNoSuchFile {} catch { first = first ?? error }
        }
        if let first { throw first }
    }

    /// Meetings older than the retention, by when they ended (or started, for one that never ended). iPhone meetings
    /// follow the iPhone's retention only: they go when the server says they were deleted (IPhoneMeetings).
    static func expired(_ meetings: [SavedMeeting], keeping r: Retention, now: Date) -> [String] {
        guard let days = r.days else { return [] }
        let cutoff = now.addingTimeInterval(-Double(days) * 86_400)
        return meetings.filter { $0.source != "iphone" && ($0.end ?? $0.start) < cutoff }.map(\.id)
    }

    private func file(_ id: String) -> URL { directory.appendingPathComponent("\(id).json") }
}
