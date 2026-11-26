import Foundation
import Observation

/// The meetings kept on this Mac, newest first, for the main window. Every change goes to disk at once.
@Observable
@MainActor
final class MeetingLibrary {
    private(set) var meetings: [SavedMeeting]
    /// Bumped when a meeting's sound files change on disk (parts joined, sound deleted), so a page that looked
    /// at them looks again: the join often leaves the meeting's record as it was.
    private(set) var audioRevisions: [String: Int] = [:]
    private var store: MeetingStore
    /// The folder of the account in use, to read one meeting from without the list (MCP `read_transcript`).
    var folder: MeetingStore { store }
    /// Each meeting's sound as last checked, and the key it was checked under (`RecordingState.checkKey`): a page shown
    /// again knows it at once instead of opening the files on the main thread and adding the player a frame later.
    @ObservationIgnored private var checkedSound: [String: (key: String, state: RecordingState)] = [:]
    /// Each meeting's voices go with the meeting; the remembered people stay.
    private let vault: VoiceVault
    /// After any change to the list or a meeting in it (the engine keeps the past meetings it searches, docs/30).
    @ObservationIgnored var onChange: (() -> Void)?
    /// After an iPhone meeting (`source == "iphone"`) was removed here by the person: it goes on the server too
    /// (IPhoneMeetings). Not for one the server deleted (`removeSynced`).
    @ObservationIgnored var onIPhoneRemove: ((String) -> Void)?
    /// When an edit to an iPhone meeting is stamped (tests fix it).
    @ObservationIgnored var now: () -> Date = { Date() }

    init(store: MeetingStore, vault: VoiceVault) {
        self.store = store
        self.vault = vault
        meetings = store.loadAll()
    }

    /// Another account's meetings (AccountSpace): the saves asked for so far reach the old folder first, then the list
    /// is the new folder's. Nothing of the previous account stays in memory.
    func reopen(store: MeetingStore) {
        self.store.flush()
        self.store = store
        meetings = store.loadAll()
        audioRevisions = [:]
        checkedSound = [:]
        onChange?()
    }

    /// How long meetings stay, from 개인정보 (kept until deleted unless the person chose otherwise).
    static var retention: Retention {
        UserDefaults.standard.string(forKey: "meetingRetention").flatMap(Retention.init(rawValue:)) ?? .forever
    }

    func put(_ m: SavedMeeting) { keep(m, stamped: true) }

    /// A merge from the server (IPhoneMeetings), written as it is: its stamps are already the server's and this Mac's,
    /// and writing it is no edit of its own to send back.
    func putSynced(_ m: SavedMeeting) { keep(m, stamped: false) }

    func update(_ id: String, _ change: (inout SavedMeeting) -> Void) {
        guard let i = meetings.firstIndex(where: { $0.id == id }) else { return }
        if meetings[i].source == "iphone" {
            // ids first, so the edit's copy and the one it is compared with name the same entries alike
            var old = meetings[i]
            old.ensureLedgerIds()
            var new = old
            change(&new)
            meetings[i] = stamped(old, new)
        } else {
            change(&meetings[i])
        }
        write(meetings[i])
        onChange?()
    }

    private func keep(_ m: SavedMeeting, stamped stamp: Bool) {
        var m = m
        if let i = meetings.firstIndex(where: { $0.id == m.id }) {
            if stamp && meetings[i].source == "iphone" { m = stamped(meetings[i], m) }
            meetings[i] = m
        } else {
            meetings.insert(m, at: meetings.firstIndex { $0.start < m.start } ?? meetings.endIndex)
        }
        write(m)
        onChange?()
    }

    /// An iPhone meeting edited here from `old` to `new`: what changed is stamped now (SyncStamps.touch), so it goes
    /// to the server and wins over older edits there. Entries added here (LedgerEditing) get their ids first.
    private func stamped(_ old: SavedMeeting, _ new: SavedMeeting) -> SavedMeeting {
        var before = old
        before.ensureLedgerIds()
        var after = new
        after.ensureLedgerIds()
        // the edit's own stamps, if it set any, are the ones to add to
        before.sync = new.sync
        after.sync = SyncStamps.touch(old: before, new: after, at: now())
        return after
    }

    /// Where the meeting's recording goes (it may not exist).
    func audioFile(_ id: String) -> URL { store.audioFile(id) }

    /// Only the recording's files; the meeting's record is not changed.
    func deleteAudio(_ id: String) {
        do { try store.deleteAudio(id) } catch { Log.info("meeting audio delete failed: \(error.localizedDescription)") }
        audioChanged(id)
    }

    func audioRevision(_ id: String) -> Int { audioRevisions[id] ?? 0 }

    /// The meeting's sound files moved (the recording was closed and joined): pages check them again.
    func audioChanged(_ id: String) { audioRevisions[id, default: 0] += 1 }

    /// What the person may do with the meeting's sound: nothing recorded, gone, or the files to play.
    func recordingState(_ m: SavedMeeting) -> RecordingState {
        switch m.recording {
        case nil: return .none
        case .deleted?: return .deleted
        case .kept?:
            let files = RecordingState.files(for: m.recording, audio: store.audioFile(m.id))
            return files.isEmpty ? .deleted : .available(files)
        }
    }

    /// The meeting's sound as a page shows it: checked once per `RecordingState.checkKey`, then kept.
    func checkedRecordingState(_ m: SavedMeeting) -> RecordingState {
        let key = RecordingState.checkKey(live: false, meeting: m, audioRevision: audioRevision(m.id))
        if let c = checkedSound[m.id], c.key == key { return c.state }
        let state = recordingState(m)
        checkedSound[m.id] = (key, state)
        return state
    }

    /// What `checkedRecordingState` found under the meeting's current key, without looking at the disk.
    func knownRecordingState(_ m: SavedMeeting) -> RecordingState? {
        guard let c = checkedSound[m.id],
              c.key == RecordingState.checkKey(live: false, meeting: m, audioRevision: audioRevision(m.id)) else { return nil }
        return c.state
    }

    /// The files to play right now, read again at the tap: what a page checked earlier may have been joined since.
    func filesToPlay(_ m: SavedMeeting) -> [URL] {
        if case .available(let files) = recordingState(m) { return files }
        return []
    }

    /// Deletes only the sound; the transcript stays and the meeting reads as "recording deleted".
    func deleteRecording(_ id: String) {
        deleteAudio(id)
        update(id) { $0.dropRecording() }
    }

    func remove(_ id: String) {
        let phone = meetings.first { $0.id == id }?.source == "iphone"
        drop(id)
        if phone { onIPhoneRemove?(id) }
    }

    /// An iPhone meeting the server deleted (on the iPhone, or past its 회의 기록 보관 there): gone here, nothing sent.
    func removeSynced(_ id: String) { drop(id) }

    private func drop(_ id: String) {
        meetings.removeAll { $0.id == id }
        checkedSound[id] = nil
        vault.forget(meeting: id)
        do { try store.delete(id) } catch { Log.info("meeting delete failed: \(error.localizedDescription)") }
        onChange?()
    }

    /// False when some file could not be deleted; the list is then read from disk again, so it shows what is still there.
    /// The iPhone meetings it deleted go on the server too, as a single `remove` (the person was told on the dialog).
    @discardableResult
    func removeAll() -> Bool {
        let phones = meetings.filter { $0.source == "iphone" }.map(\.id)
        var gone = Set(phones)
        defer {
            onChange?()
            for id in phones where gone.contains(id) { onIPhoneRemove?(id) }
        }
        do {
            try store.deleteAll()
            meetings = []
            checkedSound = [:]
            vault.forgetMeetings()
            return true
        } catch {
            Log.info("meetings delete failed: \(error.localizedDescription)")
            let before = meetings.map(\.id)
            meetings = store.loadAll()
            let left = Set(meetings.map(\.id))
            for id in Set(before).subtracting(left) { vault.forget(meeting: id) }
            gone.subtract(left)
            return false
        }
    }

    /// How many meetings `prune(keeping:)` would delete, so the person can be asked first.
    func wouldExpire(keeping r: Retention, now: Date = Date()) -> Int {
        MeetingStore.expired(meetings, keeping: r, now: now).count
    }

    func prune(keeping r: Retention, now: Date = Date()) {
        let old = MeetingStore.expired(meetings, keeping: r, now: now)
        for id in old { remove(id) }
        if !old.isEmpty { Log.info("meetings past retention removed: \(old.count)") }
    }

    /// A meeting the app died in has no end: close it where its words stop, and say why there are no notes.
    /// One that ended while the app was quitting waits for notes that will never come: the same.
    func recoverUnfinished(quietFor: TimeInterval = MeetingStore.liveWindow, now: Date = Date()) {
        do { try store.deleteOrphanAudio(quietFor: quietFor, now: now) } catch { Log.info("orphan audio delete failed: \(error.localizedDescription)") }
        for m in meetings where m.end == nil || m.notes == .waiting {
            // Deleting is only for what has been quiet a while: another Tacie may still be running a fresh one.
            let quiet = !store.maybeLive(m.id, quietFor: quietFor, now: now)
            if m.recording == .kept {
                let files = MeetingRecorder.playableFiles(for: store.audioFile(m.id))
                if files.isEmpty && m.transcript.isEmpty && quiet { remove(m.id); continue }   // nothing left of it
                if files.isEmpty { if quiet && !m.transcript.isEmpty { deleteRecording(m.id) } }
                else { update(m.id) { $0.recordingSeconds = RecordingState.seconds(of: files) } }
            }
            update(m.id) {
                $0.end = $0.end ?? $0.transcript.last?.at ?? $0.start
                $0.notes = .failed
                $0.notesProblem = String(localized: "앱이 꺼졌어요")
                $0.notesLimit = nil
            }
        }
    }

    private func write(_ m: SavedMeeting) { store.save(m) }

    /// The saves still being written reach the disk (quitting).
    func flush() { store.flush() }
}
