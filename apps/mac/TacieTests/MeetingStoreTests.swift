import XCTest

final class MeetingStoreTests: XCTestCase {
    private var dir: URL!
    private var store: MeetingStore { MeetingStore(directory: dir) }
    private let seoul = TimeZone(identifier: "Asia/Seoul")!

    override func setUp() {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    }

    override func tearDown() {
        try? FileManager.default.removeItem(at: dir)
    }

    /// 2026-10-23 at the given KST time, whole seconds (ISO 8601 keeps no fractions).
    private func kst(_ h: Int, _ m: Int = 0, day: Int = 23, month: Int = 10, year: Int = 2026) -> Date {
        var c = DateComponents(year: year, month: month, day: day, hour: h, minute: m)
        c.timeZone = seoul
        return Calendar(identifier: .gregorian).date(from: c)!
    }

    private func meeting(start: Date, end: Date?) -> SavedMeeting {
        var m = SavedMeeting(mode: "remote", start: start)
        m.end = end
        return m
    }

    func testWriteThenLoadRoundTrips() throws {
        var m = SavedMeeting(mode: "remote", start: kst(14))
        m.end = kst(14, 47)
        m.title = "결제 실패 재시도 정책"
        m.titleEdited = true
        m.notes = .failed
        m.notesProblem = "인터넷 연결이 끊겼어요"
        m.summary = ["결제 실패는 3번까지 자동으로 다시 시도해요."]
        m.ledger = [LedgerEntry(kind: "commitment", text: "재시도 간격 PR 올리기", owner: "me", due: "금요일까지", done: true)]
        let card = Card(id: "c1", path: "apps/api/payment-retry.scheduler.ts", kind: "code", source: "repo", byline: nil, url: nil,
                        lineStart: 10, lineEnd: 30, title: "payment-retry.scheduler.ts", snippet: "const MAX = 2", score: 0.92,
                        githubUrl: "https://github.com/x/y/blob/dev/a.ts#L10", localPath: nil, chunk: 7, repeat: false)
        m.questions = [SavedQuestion(id: "v1", question: "지금 결제 재시도는 몇 번까지 하게 돼 있어요?", asker: "them", at: kst(14, 6),
                                     answer: "최대 2번 다시 시도해요. [1]", cards: [card], cited: [7], unanswered: false)]
        m.transcript = [TranscriptLine(key: "e0:s1u1", at: kst(14, 6), speaker: "them", text: "지금 결제 재시도는 몇 번까지 하게 돼 있어요?", question: true)]
        try store.write(m)
        XCTAssertEqual(store.loadAll(), [m])
    }

    /// Lines that are only sound tags ("[pause]") from meetings saved before 2026-11-10 are not shown in 대화 기록;
    /// a tag next to words ("[헛기침] 안녕하세요") is a spoken line and stays.
    func testSoundOnlyLinesAreDroppedOnLoad() throws {
        var m = SavedMeeting(mode: "inperson", start: kst(14))
        m.end = kst(14, 30)
        func line(_ n: Int, _ text: String) -> TranscriptLine {
            TranscriptLine(key: "e0:s1u\(n)", at: kst(14, n), speaker: nil, text: text, question: false)
        }
        m.transcript = [line(1, "[pause]"), line(2, "[헛기침] 안녕하세요."), line(3, "[배경 소음] [outro jingle]"),
                        line(4, "OpenRouter(오픈라우터)에서 보내요")]
        try store.write(m)
        XCTAssertEqual(store.loadAll().first?.transcript.map(\.key), ["e0:s1u2", "e0:s1u4"])
    }

    func testSpeakersRoundTrip() throws {
        var m = SavedMeeting(mode: "inperson", start: kst(14))
        m.end = kst(14, 30)
        m.transcript = [TranscriptLine(key: "e0:s1u1", at: kst(14, 1), speaker: nil, text: "재시도는요?", question: true, voice: "화자 2", name: "지수")]
        m.questions = [SavedQuestion(id: "v1", question: "재시도는요?", asker: nil, at: kst(14, 1), answer: nil, cards: [], cited: [],
                                     unanswered: true, voice: "화자 2")]
        m.voices = [VoiceEntry(key: "화자 1", name: "민재", person: "p1"), VoiceEntry(key: "화자 3", same: "화자 1")]
        m.ledger = [LedgerEntry(kind: "commitment", text: "PR 올리기", owner: "화자 1")]
        try store.write(m)
        XCTAssertEqual(store.loadAll(), [m])
    }

    /// A file from before speakers (no voice, no voices) still opens.
    func testLoadsMeetingSavedBeforeSpeakers() throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let json = #"{"version": 1, "id": "20261023-140000", "mode": "remote", "start": "2026-10-23T05:00:00Z", "titleEdited": false, "notes": "done", "summary": [], "ledger": [], "questions": [], "transcript": [{"key": "e0:s1u1", "at": "2026-10-23T05:06:00Z", "speaker": "them", "text": "안녕하세요", "question": false}]}"#
        try Data(json.utf8).write(to: dir.appendingPathComponent("20261023-140000.json"))
        let loaded = store.loadAll()
        XCTAssertEqual(loaded.count, 1)
        XCTAssertFalse(loaded[0].hasSpeakers)
        XCTAssertNil(loaded[0].voices)
    }

    /// A file from before recording (none of its fields) reads with no recording.
    func testOldMeetingFileStillReads() throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let json = #"{"version": 1, "id": "20261023-140000", "mode": "remote", "start": "2026-10-23T05:00:00Z", "titleEdited": false, "notes": "done", "summary": [], "ledger": [], "questions": [], "transcript": [{"key": "e0:s1u1", "at": "2026-10-23T05:06:00Z", "speaker": "them", "text": "안녕하세요", "question": false}]}"#
        try Data(json.utf8).write(to: dir.appendingPathComponent("20261023-140000.json"))
        let loaded = store.loadAll()
        XCTAssertEqual(loaded.count, 1)
        XCTAssertNil(loaded[0].recording)
        XCTAssertNil(loaded[0].recordingSeconds)
        XCTAssertNil(loaded[0].unheard)
        XCTAssertNil(loaded[0].transcript[0].words)
        XCTAssertNil(loaded[0].transcript[0].audioAt)
    }

    /// A file from before the calendar link reads with no event and no invited people; a linked one round-trips.
    func testOldFileWithoutEventDecodesAndLinkRoundTrips() throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let json = #"{"version": 1, "id": "20261023-140000", "mode": "remote", "start": "2026-10-23T05:00:00Z", "titleEdited": false, "notes": "done", "summary": [], "ledger": [], "questions": [], "transcript": []}"#
        try Data(json.utf8).write(to: dir.appendingPathComponent("20261023-140000.json"))
        XCTAssertNil(store.loadAll()[0].event)
        XCTAssertNil(store.loadAll()[0].invited)
        var m = meeting(start: kst(15), end: kst(15, 40))
        m.link(MeetingEvent(id: "ev1", title: "주간 제품 회의", start: kst(15), end: kst(16)),
               invited: [MeetingInvitee(name: "정하늘", email: "sky@example.com")], meaningful: true)
        try store.write(m)
        let back = store.loadAll().first { $0.id == m.id }
        XCTAssertEqual(back?.event?.id, "ev1")
        XCTAssertEqual(back?.invited?.first?.email, "sky@example.com")
    }

    func testLoadAllSkipsBrokenFilesNewestFirst() throws {
        let m1 = meeting(start: kst(10), end: kst(10, 30))
        let m2 = meeting(start: kst(14), end: kst(14, 47))
        try store.write(m1)
        try store.write(m2)
        try Data("{".utf8).write(to: dir.appendingPathComponent("junk.json"))
        XCTAssertEqual(store.loadAll().map(\.id), [m2.id, m1.id])
    }

    func testDeleteAndDeleteAll() throws {
        let m1 = meeting(start: kst(10), end: kst(10, 30))
        let m2 = meeting(start: kst(14), end: kst(14, 47))
        try store.write(m1)
        try store.write(m2)
        try store.delete(m1.id)
        XCTAssertEqual(store.loadAll(), [m2])
        XCTAssertNoThrow(try store.delete("없음"))
        try store.deleteAll()
        XCTAssertEqual(store.loadAll(), [])
    }

    func testExpiredByRetention() {
        let now = kst(12)
        let day: TimeInterval = 86_400
        let recent = meeting(start: now - 29 * day - 3600, end: now - 29 * day)
        let month = meeting(start: now - 31 * day - 3600, end: now - 31 * day)
        let old = meeting(start: now - 400 * day, end: nil)  // no end: its start counts
        let all = [recent, month, old]
        XCTAssertEqual(MeetingStore.expired(all, keeping: .days30, now: now), [month.id, old.id])
        XCTAssertEqual(MeetingStore.expired(all, keeping: .year, now: now), [old.id])
        XCTAssertEqual(MeetingStore.expired(all, keeping: .forever, now: now), [])
    }

    func testIdComesFromStart() {
        XCTAssertEqual(SavedMeeting.id(for: kst(14), in: seoul), "20261023-140000")
    }

    func testDeleteAllRemovesUnreadableFilesToo() throws {
        try store.write(meeting(start: kst(9), end: kst(10)))
        try store.write(meeting(start: kst(11), end: kst(12)))
        try "{".write(to: dir.appendingPathComponent("junk.json"), atomically: true, encoding: .utf8)
        try "memo".write(to: dir.appendingPathComponent("notes.txt"), atomically: true, encoding: .utf8)
        try store.deleteAll()
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: dir.path), ["notes.txt"])
    }

    func testDeleteRemovesAudioToo() throws {
        let m = meeting(start: kst(14), end: kst(15))
        try store.write(m)
        try Data([1, 2, 3]).write(to: store.audioFile(m.id))
        try store.delete(m.id)
        XCTAssertFalse(FileManager.default.fileExists(atPath: dir.appendingPathComponent("\(m.id).json").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: store.audioFile(m.id).path))
    }

    func testDeleteAudioKeepsJSON() throws {
        let m = meeting(start: kst(14), end: kst(15))
        try store.write(m)
        try Data([1, 2, 3]).write(to: store.audioFile(m.id))
        try store.deleteAudio(m.id)
        try store.deleteAudio(m.id)   // already gone: quiet
        XCTAssertFalse(FileManager.default.fileExists(atPath: store.audioFile(m.id).path))
        XCTAssertEqual(store.loadAll().map(\.id), [m.id])
    }

    func testLoadAllSkipsAudioFiles() throws {
        let m = meeting(start: kst(14), end: kst(15))
        try store.write(m)
        try Data([1, 2, 3]).write(to: store.audioFile(m.id))
        XCTAssertEqual(store.loadAll().count, 1)
    }

    func testDeleteAllRemovesAudioToo() throws {
        let m = meeting(start: kst(14), end: kst(15))
        try store.write(m)
        try Data([1, 2, 3]).write(to: store.audioFile(m.id))
        try store.deleteAll()
        XCTAssertEqual((try FileManager.default.contentsOfDirectory(atPath: dir.path)).count, 0)
    }

    private func leftovers(_ id: String) throws {
        for name in ["\(id).caf", "\(id).part-0001.caf", "\(id).joining.caf"] {
            try Data([1]).write(to: dir.appendingPathComponent(name))
        }
    }

    private func audioFiles(_ id: String) -> [String] {
        ((try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []).filter { $0.hasPrefix("\(id).") && $0.hasSuffix(".caf") }
    }

    func testDeleteRemovesLeftoverPartsButNotOthers() throws {
        let m = meeting(start: kst(14), end: kst(15)), other = meeting(start: kst(16), end: kst(17))
        try store.write(m); try store.write(other)
        try leftovers(m.id); try leftovers(other.id)
        try store.delete(m.id)
        XCTAssertEqual(audioFiles(m.id), [])
        XCTAssertFalse(FileManager.default.fileExists(atPath: dir.appendingPathComponent("\(m.id).json").path))
        XCTAssertEqual(audioFiles(other.id).count, 3)
    }

    func testDeleteAudioRemovesLeftoverPartsKeepsJSON() throws {
        let m = meeting(start: kst(14), end: kst(15)), other = meeting(start: kst(16), end: kst(17))
        try store.write(m); try store.write(other)
        try leftovers(m.id); try leftovers(other.id)
        try store.deleteAudio(m.id)
        XCTAssertEqual(audioFiles(m.id), [])
        XCTAssertEqual(store.loadAll().count, 2)
        XCTAssertEqual(audioFiles(other.id).count, 3)
    }

    func testDeleteAllRemovesLeftoverParts() throws {
        let m = meeting(start: kst(14), end: kst(15))
        try store.write(m); try leftovers(m.id)
        try store.deleteAll()
        XCTAssertEqual(audioFiles(m.id), [])
    }
}
