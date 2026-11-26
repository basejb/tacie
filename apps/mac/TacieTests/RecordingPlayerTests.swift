import AVFoundation
import XCTest

@MainActor
final class RecordingPlayerTests: XCTestCase {
    private var tmp: URL!

    override func setUpWithError() throws {
        tmp = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
    }

    override func tearDown() {
        try? FileManager.default.removeItem(at: tmp)
    }

    private func tone(_ seconds: Double) -> [Float] {
        (0..<Int(seconds * 16_000)).map { 0.3 * sin(2 * .pi * 220 * Float($0) / 16_000) }
    }

    /// A finished one-file recording of `seconds`.
    private func recording(_ name: String, seconds: Double) throws -> URL {
        let url = tmp.appendingPathComponent("\(name).caf")
        let r = try MeetingRecorder(url: url)
        r.append(tone(seconds)); _ = r.finish()
        return url
    }

    /// A recording the app quit in: parts of 1 s, no final file.
    private func parts(_ name: String, seconds: Double) throws -> [URL] {
        let url = tmp.appendingPathComponent("\(name).caf")
        let r = try MeetingRecorder(url: url, partSeconds: 1)
        for _ in 0..<Int(seconds * 2) { r.append(tone(0.5)) }; _ = r.close()
        return MeetingRecorder.playableFiles(for: url)
    }

    private func line(_ key: String, audioAt: Int?, words: [SpokenWord]?, voice: String? = nil) -> TranscriptLine {
        TranscriptLine(key: key, at: Date(), speaker: nil, text: key, question: false, voice: voice, audioAt: audioAt, words: words)
    }

    private func w(_ t: String, _ s: Int, _ e: Int) -> SpokenWord { SpokenWord(text: t, start: s, end: e) }

    private func wait(_ what: String, timeout: Double = 5, _ ok: @MainActor () -> Bool) async {
        let end = Date().addingTimeInterval(timeout)
        while !ok(), Date() < end { try? await Task.sleep(for: .milliseconds(20)) }
        XCTAssertTrue(ok(), what)
    }

    func testCurrentWord() {
        let lines = [line("a", audioAt: 0, words: [w("하나", 0, 400), w("둘", 500, 900)], voice: "화자 1"),
                     line("b", audioAt: 2000, words: nil, voice: "화자 2")]
        XCTAssertEqual(RecordingPlayer.currentWord(in: lines, atMs: 450)?.wordIndex, 0)
        XCTAssertEqual(RecordingPlayer.currentWord(in: lines, atMs: 600)?.wordIndex, 1)
        XCTAssertEqual(RecordingPlayer.currentWord(in: lines, atMs: 2500)?.lineKey, "b")
        XCTAssertEqual(RecordingPlayer.currentWord(in: lines, atMs: 2500)?.wordIndex, -1)
        XCTAssertNil(RecordingPlayer.currentWord(in: lines, atMs: -5))
        XCTAssertEqual(RecordingPlayer.speaker(in: lines, atMs: 600), "화자 1")
        XCTAssertEqual(RecordingPlayer.speaker(in: lines, atMs: 2100), "화자 2")
        XCTAssertNil(RecordingPlayer.speaker(in: lines, atMs: -1))
    }

    func testNextRateCycles() {
        let p = RecordingPlayer()
        XCTAssertEqual(p.rate, 1)
        p.nextRate(); XCTAssertEqual(p.rate, 1.5)
        p.nextRate(); XCTAssertEqual(p.rate, 2)
        p.nextRate(); XCTAssertEqual(p.rate, 1)
    }

    func testPlayFromAndStopOnMeetingStart() async throws {
        let url = try recording("m", seconds: 3)
        let p = RecordingPlayer()
        p.volume = 0
        p.play(meeting: "m", urls: [url], fromMs: 1000)
        XCTAssertEqual(p.meetingID, "m")
        await wait("moves on") { p.positionMs >= 1100 }
        XCTAssertTrue(p.playing)
        XCTAssertEqual(p.durationMs, 3000, accuracy: 150)
        NotificationCenter.default.post(name: .meetingStarted, object: nil)
        await wait("stopped") { !p.playing }
        XCTAssertNil(p.meetingID)
    }

    func testPartsPlayAsOneTimeline() async throws {
        let files = try parts("q", seconds: 3.5)
        XCTAssertEqual(files.count, 4)
        let p = RecordingPlayer()
        p.volume = 0
        p.play(meeting: "q", urls: files, fromMs: 2800)   // inside the third part
        XCTAssertEqual(p.durationMs, 3500, accuracy: 150)
        await wait("crosses the boundary at 3 s") { p.positionMs >= 3100 }
        p.seek(toMs: 500)
        await wait("plays after a seek") { p.positionMs >= 600 && p.positionMs < 2000 }
        p.seek(toMs: 99_999)
        XCTAssertEqual(p.positionMs, p.durationMs)
        await wait("ends") { !p.playing }
        p.toggle()   // at the end: starts over
        XCTAssertTrue(p.playing)
        await wait("restarted") { p.positionMs < 1500 }
        p.rate = 2
        p.toggle()
        XCTAssertFalse(p.playing)
        p.stop()
    }

    func testStateForSeveralFiles() throws {
        let store = MeetingStore(directory: tmp)
        let lib = MeetingLibrary(store: store, vault: VoiceVault(root: tmp.appendingPathComponent("people"), enabled: { true }))
        var m = SavedMeeting(mode: "remote", start: Date())
        m.recording = .kept
        let ps = try parts(m.id, seconds: 2.5)
        XCTAssertEqual(lib.recordingState(m), .available(ps))
    }

    /// The page checks the sound while the ended meeting's parts are still being joined; the join then moves them.
    /// What it plays at the tap must still be the meeting's sound, not the gone parts.
    func testPlaysRightAfterTheJoin() async throws {
        let store = MeetingStore(directory: tmp)
        let lib = MeetingLibrary(store: store, vault: VoiceVault(root: tmp.appendingPathComponent("people"), enabled: { true }))
        var m = SavedMeeting(mode: "remote", start: Date())
        m.recording = .kept
        let r = try MeetingRecorder(url: store.audioFile(m.id), partSeconds: 1)
        for _ in 0..<5 { r.append(tone(0.5)) }
        r.flushForTesting()
        guard case .available(let seen) = lib.recordingState(m) else { return XCTFail("parts should play") }
        XCTAssertGreaterThanOrEqual(seen.count, 2)
        let before = RecordingState.checkKey(live: false, meeting: m, audioRevision: lib.audioRevision(m.id))
        _ = r.finish()
        lib.audioChanged(m.id)   // what finishRecording does once the join is done
        XCTAssertFalse(seen.allSatisfy { FileManager.default.fileExists(atPath: $0.path) }, "the join moved the parts")
        XCTAssertNotEqual(RecordingState.checkKey(live: false, meeting: m, audioRevision: lib.audioRevision(m.id)), before)
        let now = lib.filesToPlay(m)
        XCTAssertEqual(now, [store.audioFile(m.id)])
        let p = RecordingPlayer()
        p.volume = 0
        p.play(meeting: m.id, urls: now, fromMs: 0)
        XCTAssertEqual(p.durationMs, 2500, accuracy: 150)
        await wait("plays") { p.positionMs >= 200 }
        XCTAssertTrue(p.playing)
        p.stop()
    }

    func testAudioRevisionChangesTheSoundKey() throws {
        let store = MeetingStore(directory: tmp)
        let lib = MeetingLibrary(store: store, vault: VoiceVault(root: tmp.appendingPathComponent("people"), enabled: { true }))
        var m = SavedMeeting(mode: "remote", start: Date())
        m.recording = .kept
        lib.put(m)
        let key = { RecordingState.checkKey(live: false, meeting: m, audioRevision: lib.audioRevision(m.id)) }
        let a = key()
        lib.audioChanged(m.id)
        let b = key()
        XCTAssertNotEqual(a, b)
        lib.deleteRecording(m.id)
        XCTAssertNotEqual(key(), b)
        XCTAssertEqual(lib.audioRevision("other"), 0)
    }

    func testStaleLoadNeverPlays() async throws {
        let a = try recording("a", seconds: 3), b = try recording("b", seconds: 3)
        let p = RecordingPlayer()
        p.volume = 0
        p.play(meeting: "a", urls: [a], fromMs: 0)
        p.play(meeting: "b", urls: [b], fromMs: 1500)
        XCTAssertEqual(p.meetingID, "b")
        await wait("b plays") { p.positionMs >= 1600 }
        XCTAssertEqual(p.meetingID, "b")
        XCTAssertTrue(p.playing)
        p.play(meeting: "a", urls: [a], fromMs: 0)
        p.stop()
        try await Task.sleep(for: .milliseconds(500))
        XCTAssertNil(p.meetingID)
        XCTAssertFalse(p.playing)
        XCTAssertEqual(p.positionMs, 0)
    }

    func testPauseDuringLoadSticks() async throws {
        let a = try recording("a", seconds: 3)
        let p = RecordingPlayer()
        p.volume = 0
        p.play(meeting: "a", urls: [a], fromMs: 1000)
        p.toggle()
        XCTAssertFalse(p.playing)
        try await Task.sleep(for: .milliseconds(500))
        XCTAssertFalse(p.playing)
        XCTAssertEqual(p.positionMs, 1000)
    }

    func testDroppedPlayerFreesItself() async throws {
        let a = try recording("a", seconds: 3)
        weak var weakP: RecordingPlayer?
        do {
            let p = RecordingPlayer()
            weakP = p
            p.volume = 0
            p.play(meeting: "a", urls: [a], fromMs: 0)
            await wait("running") { p.positionMs > 50 }
        }
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertNil(weakP)
    }
}
