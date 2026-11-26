import XCTest

@MainActor
final class MeetingLibraryTests: XCTestCase {
    private var dir: URL!
    private var people: URL!
    private var vault: VoiceVault!
    private var store: MeetingStore { MeetingStore(directory: dir) }
    private let base = Date(timeIntervalSince1970: 1_792_000_000)

    override func setUp() async throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        people = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        vault = VoiceVault(root: people, enabled: { true })
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: dir)
        try? FileManager.default.removeItem(at: people)
    }

    private func meeting(_ offset: TimeInterval, ended: Bool = true) -> SavedMeeting {
        var m = SavedMeeting(mode: "remote", start: base + offset)
        if ended { m.end = base + offset + 1800 }
        return m
    }

    func testPutUpdateRemovePersist() {
        let lib = MeetingLibrary(store: store, vault: vault)
        let a = meeting(0), b = meeting(3600)
        lib.put(a)
        lib.put(b)
        lib.update(a.id) { $0.title = "바꾼 제목" }
        XCTAssertEqual(lib.meetings.map(\.id), [b.id, a.id])
        lib.remove(b.id)
        let again = MeetingLibrary(store: store, vault: vault)
        XCTAssertEqual(again.meetings.map(\.id), [a.id])
        XCTAssertEqual(again.meetings.first?.title, "바꾼 제목")
        again.removeAll()
        XCTAssertEqual(MeetingLibrary(store: store, vault: vault).meetings, [])
    }

    func testRemoveAllReportsFailureAndReloads() throws {
        let lib = MeetingLibrary(store: store, vault: vault)
        lib.put(meeting(0))
        lib.put(meeting(3600))
        lib.flush()  // saves run in the background: on disk before the folder turns read-only
        try FileManager.default.setAttributes([.posixPermissions: 0o500], ofItemAtPath: dir.path)
        defer { try? FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: dir.path) }
        XCTAssertFalse(lib.removeAll())
        XCTAssertEqual(lib.meetings.count, 2)
    }

    func testRemoveAllWithoutFolderSucceeds() {
        let lib = MeetingLibrary(store: store, vault: vault)
        XCTAssertTrue(lib.removeAll())
        XCTAssertEqual(lib.meetings, [])
    }

    func testPruneDeletesFilesToo() {
        let lib = MeetingLibrary(store: store, vault: vault)
        let old = meeting(-31 * 86_400), recent = meeting(-86_400)
        lib.put(old)
        lib.put(recent)
        lib.prune(keeping: .days30, now: base)
        XCTAssertEqual(lib.meetings.map(\.id), [recent.id])
        XCTAssertEqual(store.loadAll().map(\.id), [recent.id])
    }

    func testRecoverUnfinishedMarksNotesFailed() {
        var died = meeting(0, ended: false)
        died.addLine(TranscriptLine(key: "e0:s1u1", at: base + 600, speaker: nil, text: "마지막 말", question: false), replacing: nil)
        var fine = meeting(-86_400)
        fine.notes = .done
        try? store.write(died)
        try? store.write(fine)
        let lib = MeetingLibrary(store: store, vault: vault)
        lib.recoverUnfinished(quietFor: 0)
        let r = lib.meetings.first { $0.id == died.id }
        XCTAssertEqual(r?.end, base + 600)
        XCTAssertEqual(r?.notes, .failed)
        XCTAssertEqual(r?.notesProblem, "앱이 꺼졌어요")
        XCTAssertEqual(lib.meetings.first { $0.id == fine.id }, fine)
        XCTAssertEqual(store.loadAll().first { $0.id == died.id }?.notes, .failed)
    }

    /// Quit while the notes were on their way: the meeting has an end but its notes never came.
    func testRecoverNotesThatNeverCame() {
        var quit = meeting(0)
        quit.notes = .waiting
        try? store.write(quit)
        let lib = MeetingLibrary(store: store, vault: vault)
        lib.recoverUnfinished(quietFor: 0)
        XCTAssertEqual(lib.meetings.first?.end, quit.end)
        XCTAssertEqual(lib.meetings.first?.notes, .failed)
        XCTAssertEqual(lib.meetings.first?.notesProblem, "앱이 꺼졌어요")
    }

    func testWouldExpireCountsWithoutDeleting() {
        let lib = MeetingLibrary(store: store, vault: vault)
        for d in [1, 31, 400] { lib.put(meeting(-Double(d) * 86_400)) }
        XCTAssertEqual(lib.wouldExpire(keeping: .days30, now: base), 2)
        XCTAssertEqual(lib.wouldExpire(keeping: .year, now: base), 1)
        XCTAssertEqual(lib.wouldExpire(keeping: .forever, now: base), 0)
        XCTAssertEqual(lib.meetings.count, 3)
        XCTAssertEqual(store.loadAll().count, 3)
    }

    func testLibraryRemoveAndPruneForgetPrints() throws {
        let lib = MeetingLibrary(store: store, vault: vault)
        let old = meeting(-31 * 86_400), recent = meeting(-86_400), gone = meeting(-2 * 86_400)
        for m in [old, recent, gone] {
            lib.put(m)
            vault.save(prints: [MeetingPrint(key: "화자 1", channel: "mic", vector: [1], seconds: 5, person: nil)], meeting: m.id)
        }
        var named = recent
        named.voices = [VoiceEntry(key: "화자 1")]
        XCTAssertNotNil(vault.remember(key: "화자 1", name: "민재", person: nil, meeting: named))
        lib.remove(gone.id)
        XCTAssertEqual(vault.prints(meeting: gone.id), [])
        lib.prune(keeping: .days30, now: base)
        XCTAssertEqual(vault.prints(meeting: old.id), [])
        XCTAssertEqual(vault.prints(meeting: recent.id).count, 1)
        XCTAssertTrue(lib.removeAll())
        XCTAssertEqual(VoiceVault(root: people, enabled: { true }).prints(meeting: recent.id), [])
        XCTAssertEqual(VoiceVault(root: people, enabled: { true }).people.map(\.name), ["민재"])  // people.json stays
    }

    // MARK: recording

    private func tone(_ seconds: Double) -> [Float] {
        (0..<Int(seconds * 16_000)).map { 0.3 * sin(2 * .pi * 220 * Float($0) / 16_000) }
    }

    private func recorded(_ m: SavedMeeting, seconds: Double) throws {
        let r = try MeetingRecorder(url: store.audioFile(m.id))
        r.append(tone(seconds)); _ = r.finish()
    }

    func testMissingFileReadsAsDeleted() throws {
        let lib = MeetingLibrary(store: store, vault: vault)
        var m = meeting(0)
        m.recording = .kept
        XCTAssertEqual(lib.recordingState(m), .deleted)   // no file
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try Data("not audio".utf8).write(to: store.audioFile(m.id))
        XCTAssertEqual(lib.recordingState(m), .deleted)   // garbage
        try? FileManager.default.removeItem(at: store.audioFile(m.id))
        try recorded(m, seconds: 1)
        XCTAssertEqual(lib.recordingState(m), .available([store.audioFile(m.id)]))
    }

    /// A page shown again knows the sound at once; a change of the files (joined, deleted) is checked again.
    func testCheckedRecordingStateIsKeptUntilTheSoundChanges() throws {
        var m = meeting(0)
        m.recording = .kept
        let lib = MeetingLibrary(store: store, vault: vault)
        lib.put(m)
        XCTAssertNil(lib.knownRecordingState(m))
        XCTAssertEqual(lib.checkedRecordingState(m), .deleted)   // no file yet
        XCTAssertEqual(lib.knownRecordingState(m), .deleted)
        try recorded(m, seconds: 1)
        XCTAssertEqual(lib.checkedRecordingState(m), .deleted)   // kept: the files did not say they changed
        lib.audioChanged(m.id)
        XCTAssertNil(lib.knownRecordingState(m))
        XCTAssertEqual(lib.checkedRecordingState(m), .available([store.audioFile(m.id)]))
        lib.deleteRecording(m.id)
        let after = try XCTUnwrap(lib.meetings.first)
        XCTAssertEqual(lib.checkedRecordingState(after), .deleted)
        lib.remove(m.id)
        XCTAssertNil(lib.knownRecordingState(after))
    }

    func testRecordingStateNoneForOldMeetings() {
        let lib = MeetingLibrary(store: store, vault: vault)
        XCTAssertEqual(lib.recordingState(meeting(0)), .none)
        var m = meeting(0)
        m.recording = .deleted
        XCTAssertEqual(lib.recordingState(m), .deleted)
    }

    func testDeleteRecordingKeepsTranscript() throws {
        let lib = MeetingLibrary(store: store, vault: vault)
        var m = meeting(0)
        m.recording = .kept
        m.addLine(TranscriptLine(key: "e0:s1u1", at: base, speaker: nil, text: "남는 말", question: false, audioAt: 0,
                                 words: [SpokenWord(text: "남는", start: 0, end: 300)]), replacing: nil)
        lib.put(m)
        try recorded(m, seconds: 1)
        lib.deleteRecording(m.id)
        XCTAssertFalse(FileManager.default.fileExists(atPath: store.audioFile(m.id).path))
        let r = MeetingLibrary(store: store, vault: vault).meetings.first
        XCTAssertEqual(r?.recording, .deleted)
        XCTAssertEqual(r?.transcript.map(\.text), ["남는 말"])
        XCTAssertNil(r?.transcript.first?.words)
    }

    func testLaunchSweepsAudioOfMeetingsWithoutRecord() throws {
        let keep = meeting(0)
        try? store.write(keep)
        try recorded(keep, seconds: 1)
        let orphan = meeting(-5000)
        try recorded(orphan, seconds: 1)
        for n in ["\(orphan.id).part-0001.caf", "\(orphan.id).joining.caf", "OTHER.part-0002.caf"] {
            try Data([1]).write(to: dir.appendingPathComponent(n))
        }
        MeetingLibrary(store: store, vault: vault).recoverUnfinished(quietFor: 0)
        let left = try FileManager.default.contentsOfDirectory(atPath: dir.path).sorted()
        XCTAssertEqual(left, ["\(keep.id).caf", "\(keep.id).json"].sorted())
    }

    func testRecoveryRefreshesRecordedSecondsAndDropsEmptyOnes() throws {
        var died = meeting(0, ended: false)
        died.recording = .kept
        died.recordingSeconds = 99
        died.addLine(TranscriptLine(key: "e0:s1u1", at: base + 5, speaker: nil, text: "말", question: false), replacing: nil)
        var empty = meeting(100, ended: false)   // no words, no sound
        empty.recording = .kept
        var soundOnly = meeting(200, ended: false)   // no words, but the sound plays
        soundOnly.recording = .kept
        for m in [died, empty, soundOnly] { try? store.write(m) }
        try recorded(died, seconds: 2)
        try recorded(soundOnly, seconds: 1)
        let lib = MeetingLibrary(store: store, vault: vault)
        lib.recoverUnfinished(quietFor: 0)
        XCTAssertEqual(Set(lib.meetings.map(\.id)), [died.id, soundOnly.id])
        XCTAssertEqual(lib.meetings.first { $0.id == died.id }?.recordingSeconds ?? 0, 2, accuracy: 0.1)
        XCTAssertEqual(store.loadAll().count, 2)
    }

    func testRecoveryDropsRecordingWhenOnlyTextIsLeft() {
        var m = meeting(0, ended: false)
        m.recording = .kept
        m.addLine(TranscriptLine(key: "e0:s1u1", at: base + 5, speaker: nil, text: "말", question: false, audioAt: 0,
                                 words: [SpokenWord(text: "말", start: 0, end: 100)]), replacing: nil)
        try? store.write(m)
        let lib = MeetingLibrary(store: store, vault: vault)
        lib.recoverUnfinished(quietFor: 0)
        XCTAssertEqual(store.loadAll().first?.recording, .deleted)
        XCTAssertNil(store.loadAll().first?.transcript.first?.words)
    }

    /// Another Tacie may be running these: recently touched records and sound are left alone.
    func testRecoveryAndSweepSkipRecentlyChangedFiles() throws {
        var live = meeting(0, ended: false)
        live.recording = .kept
        var stale = meeting(100, ended: false)
        stale.recording = .kept
        stale.addLine(TranscriptLine(key: "e0:s1u1", at: base + 105, speaker: nil, text: "말", question: false), replacing: nil)
        try? store.write(live); try? store.write(stale)
        try recorded(stale, seconds: 1)
        let freshOrphan = meeting(-5000), oldOrphan = meeting(-9000)
        try recorded(freshOrphan, seconds: 1); try recorded(oldOrphan, seconds: 1)
        let old = Date().addingTimeInterval(-600)
        for u in [dir.appendingPathComponent("\(stale.id).json"), store.audioFile(stale.id), store.audioFile(oldOrphan.id)] {
            try FileManager.default.setAttributes([.modificationDate: old], ofItemAtPath: u.path)
        }
        let lib = MeetingLibrary(store: store, vault: vault)
        lib.recoverUnfinished()
        XCTAssertEqual(lib.meetings.first { $0.id == live.id }?.notes, .failed)   // marked, but nothing deleted
        XCTAssertEqual(lib.meetings.first { $0.id == live.id }?.recording, .kept)
        XCTAssertEqual(lib.meetings.first { $0.id == stale.id }?.notes, .failed)
        XCTAssertTrue(FileManager.default.fileExists(atPath: store.audioFile(freshOrphan.id).path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: store.audioFile(oldOrphan.id).path))
    }

    func testFreshUnfinishedIsMarkedButNothingDeleted() throws {
        var withSound = meeting(0, ended: false)
        withSound.recording = .kept
        withSound.addLine(TranscriptLine(key: "e0:s1u1", at: base + 5, speaker: nil, text: "말", question: false), replacing: nil)
        var empty = meeting(100, ended: false)   // no text, no sound yet
        empty.recording = .kept
        try? store.write(withSound); try? store.write(empty)
        try recorded(withSound, seconds: 2)
        let lib = MeetingLibrary(store: store, vault: vault)
        lib.recoverUnfinished()   // default quiet time; both files were just written
        XCTAssertEqual(Set(lib.meetings.map(\.id)), [withSound.id, empty.id])
        XCTAssertTrue(lib.meetings.allSatisfy { $0.notes == .failed && $0.end != nil })
        XCTAssertEqual(lib.meetings.first { $0.id == withSound.id }?.recordingSeconds ?? 0, 2, accuracy: 0.1)
        XCTAssertEqual(lib.meetings.first { $0.id == empty.id }?.recording, .kept)
        XCTAssertTrue(FileManager.default.fileExists(atPath: store.audioFile(withSound.id).path))
    }

    /// Another account's space (AccountSpace): its meetings and people only; the first account's come back with it.
    func testReopenShowsOnlyTheOtherAccountsMeetingsAndPeople() throws {
        let lib = MeetingLibrary(store: store, vault: vault)
        lib.put(meeting(0))
        let prints = people.appendingPathComponent("meetings")
        try FileManager.default.createDirectory(at: prints, withIntermediateDirectories: true)
        try Data("{}".utf8).write(to: prints.appendingPathComponent("x.json"))
        XCTAssertTrue(vault.hasStoredData)
        let other = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let otherPeople = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: other); try? FileManager.default.removeItem(at: otherPeople) }

        lib.reopen(store: MeetingStore(directory: other))
        vault.reopen(root: otherPeople)
        XCTAssertTrue(lib.meetings.isEmpty)
        XCTAssertFalse(vault.hasStoredData)

        lib.reopen(store: store)
        vault.reopen(root: people)
        XCTAssertEqual(lib.meetings.count, 1)
        XCTAssertTrue(vault.hasStoredData)
    }

    // MARK: past meetings for the engine (docs/30)

    private func noted(_ offset: TimeInterval) -> SavedMeeting {
        var m = meeting(offset)
        m.notes = .done
        m.summary = ["카드 결제만 먼저 내기로 했어요"]
        return m
    }

    func testOnChangeFiresOnEveryChange() {
        let lib = MeetingLibrary(store: store, vault: vault)
        var calls = 0
        lib.onChange = { calls += 1 }
        let a = meeting(0), b = meeting(3600)
        lib.put(a)
        XCTAssertEqual(calls, 1)
        lib.update(a.id) { $0.title = "바꾼 제목" }
        XCTAssertEqual(calls, 2)
        lib.update("none") { $0.title = "없음" }
        XCTAssertEqual(calls, 2, "nothing changed")
        lib.put(b)
        lib.remove(b.id)
        XCTAssertEqual(calls, 4)
        lib.prune(keeping: .forever)
        XCTAssertEqual(calls, 4, "nothing past retention")
        lib.put(meeting(-400 * 86400))
        calls = 0
        lib.prune(keeping: .days30)
        XCTAssertGreaterThan(calls, 0)
        calls = 0
        lib.removeAll()
        XCTAssertEqual(calls, 1)
        calls = 0
        lib.reopen(store: store)
        XCTAssertEqual(calls, 1)
    }

    func testDigestsKeepOnlyFinishedNotesSortedById() {
        let lib = MeetingLibrary(store: store, vault: vault)
        let live = meeting(7200, ended: false)
        var waiting = noted(3600)
        waiting.notes = .waiting
        let a = noted(0), b = noted(10800)
        for m in [b, live, waiting, a] { lib.put(m) }
        XCTAssertEqual(lib.digests().map(\.id), [a.id, b.id])
        XCTAssertTrue(lib.hasDigest)
        lib.remove(a.id)
        lib.remove(b.id)
        XCTAssertFalse(lib.hasDigest)
    }

    func testMeetingsCountAsSourceOnlyWithNotes() {
        XCTAssertFalse(SourcePresence.searchable(facts: false, meetings: false))
        XCTAssertTrue(SourcePresence.searchable(facts: false, meetings: true))
        XCTAssertTrue(SourcePresence.searchable(facts: true, meetings: false))
    }

    // MARK: iPhone meetings (design notes 4-3)

    private func iphone(_ offset: TimeInterval) -> SavedMeeting {
        var m = SavedMeeting(id: "iphone-s\(Int(offset))", mode: "inperson", start: base + offset)  // MeetingSync.macId
        m.end = base + offset + 1800
        m.source = "iphone"
        m.notes = .done
        return m
    }

    func testPruneSkipsIPhoneMeetings() {
        let lib = MeetingLibrary(store: store, vault: vault)
        let mac = meeting(-31 * 86_400), phone = iphone(-400 * 86_400)
        lib.put(mac)
        lib.put(phone)
        lib.prune(keeping: .days30, now: base)
        XCTAssertEqual(lib.meetings.map(\.id), [phone.id])
        XCTAssertEqual(store.loadAll().map(\.id), [phone.id])
    }

    func testWouldExpireSkipsIPhoneMeetings() {
        let lib = MeetingLibrary(store: store, vault: vault)
        lib.put(meeting(-31 * 86_400))
        lib.put(iphone(-400 * 86_400))
        XCTAssertEqual(lib.wouldExpire(keeping: .days30, now: base), 1)
    }

    func testUpdateStampsOnlyIPhoneMeetings() {
        let lib = MeetingLibrary(store: store, vault: vault)
        lib.now = { Date(timeIntervalSince1970: 1_800_000_000) }
        let at = SyncStamps.string(from: Date(timeIntervalSince1970: 1_800_000_000))
        var phone = iphone(0)
        phone.ledger = [LedgerEntry(kind: "commitment", text: "a", id: "e1")]
        let mac = meeting(3600)
        lib.put(mac)
        lib.put(phone)
        XCTAssertNil(lib.meetings.first { $0.id == phone.id }?.sync, "a new meeting is not an edit")
        lib.update(mac.id) { $0.title = "맥" }
        XCTAssertNil(lib.meetings.first { $0.id == mac.id }?.sync)
        lib.update(phone.id) {
            $0.title = "폰"
            $0.ledger[0].done = true
            $0.ledger.append(LedgerEntry(kind: "commitment", text: "b"))  // LedgerEditing adds without an id
        }
        let m = lib.meetings.first { $0.id == phone.id }!
        XCTAssertEqual(m.sync?.fields["title"], at)
        XCTAssertNil(m.sync?.fields["summary"])
        XCTAssertEqual(m.sync?.ledger["e1"], at)
        let added = try! XCTUnwrap(m.ledger[1].id)
        XCTAssertEqual(m.sync?.ledger[added], at)
        XCTAssertEqual(store.loadAll().first { $0.id == phone.id }?.sync, m.sync, "the stamps go to disk")
    }

    /// The changed-decision mark (DecisionTrail) stays on this Mac: writing it is not an edit to send.
    func testMarkingReplacesDoesNotStamp() {
        let lib = MeetingLibrary(store: store, vault: vault)
        var phone = iphone(0)
        phone.ledger = [LedgerEntry(kind: "decision", text: "a", id: "e1")]
        phone.sync = SyncStamps(fields: ["title": "2026-11-16T00:00:00.000Z"])
        lib.putSynced(phone)
        lib.update(phone.id) { DecisionTrail.mark(&$0, ["a": ReplacedDecision(text: "old", meeting: "m", title: "t", at: base)]) }
        let m = lib.meetings.first { $0.id == phone.id }!
        XCTAssertEqual(m.ledger[0].replaces?.text, "old")
        XCTAssertEqual(m.sync, phone.sync)
    }

    func testPutSyncedNeverStamps() {
        let lib = MeetingLibrary(store: store, vault: vault)
        var phone = iphone(0)
        lib.putSynced(phone)
        phone.title = "서버에서"
        lib.putSynced(phone)
        XCTAssertEqual(lib.meetings.first?.title, "서버에서")
        XCTAssertNil(lib.meetings.first?.sync)
    }

    func testRemoveTellsOnlyForIPhoneMeetingsAndNotWhenSynced() {
        let lib = MeetingLibrary(store: store, vault: vault)
        var told: [String] = []
        lib.onIPhoneRemove = { told.append($0) }
        let mac = meeting(0), a = iphone(3600), b = iphone(7200)
        for m in [mac, a, b] { lib.put(m) }
        lib.remove(mac.id)
        lib.remove(a.id)
        lib.removeSynced(b.id)
        XCTAssertEqual(told, [a.id])
        XCTAssertEqual(lib.meetings, [])
        XCTAssertEqual(store.loadAll(), [])
    }
}
