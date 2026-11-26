import AVFoundation
import XCTest

final class MeetingRecorderTests: XCTestCase {
    private var tmp: URL!

    override func setUpWithError() throws {
        tmp = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
    }

    override func tearDown() {
        try? FileManager.default.removeItem(at: tmp)
    }

    /// A 16 kHz mono tone with some wobble so the encoder has something to chew on.
    private func tone(seconds: Double) -> [Float] {
        (0..<Int(seconds * 16_000)).map { i in
            let t = Float(i) / 16_000
            return 0.3 * sin(2 * .pi * 220 * t) * (0.6 + 0.4 * sin(2 * .pi * 3 * t))
        }
    }

    func testWritesReadableAAC() throws {
        let url = tmp.appendingPathComponent("a.caf")
        let r = try MeetingRecorder(url: url)
        r.append(tone(seconds: 3))
        XCTAssertEqual(r.finish(), 3, accuracy: 0.05)
        let f = try AVAudioFile(forReading: url)
        XCTAssertEqual(f.fileFormat.streamDescription.pointee.mFormatID, kAudioFormatMPEG4AAC)
        XCTAssertEqual(Double(f.length) / f.processingFormat.sampleRate, 3, accuracy: 0.1)
    }

    func testUnclosedFileStillPlays() throws {   // the app died mid-meeting
        let url = tmp.appendingPathComponent("b.caf")
        let r = try MeetingRecorder(url: url, partSeconds: 2)
        for _ in 0..<10 { r.append(tone(seconds: 0.5)) }; r.flushForTesting()   // as the audio thread feeds it; written to disk, never finished
        // A crash leaves the parts (the last one still open is empty); copy them as they are.
        let crash = tmp.appendingPathComponent("crash")
        try FileManager.default.createDirectory(at: crash, withIntermediateDirectories: true)
        for p in MeetingRecorder.parts(for: url) {
            try FileManager.default.copyItem(at: p, to: crash.appendingPathComponent(p.lastPathComponent))
        }
        let copy = crash.appendingPathComponent("b.caf")
        let playable = MeetingRecorder.playableFiles(for: copy)
        XCTAssertGreaterThanOrEqual(playable.count, 2)
        var total = 0.0
        for p in playable {
            let f = try AVAudioFile(forReading: p)
            total += Double(f.length) / f.processingFormat.sampleRate
            XCTAssertNoThrow(try AVAudioPlayer(contentsOf: p))
        }
        XCTAssertGreaterThan(total, 3.9)   // at most the open part is lost
    }

    func testFinishJoinsPartsIntoOneFile() throws {
        let url = tmp.appendingPathComponent("d.caf")
        let r = try MeetingRecorder(url: url, partSeconds: 1)
        r.append(tone(seconds: 3.5))
        XCTAssertEqual(r.finish(), 3.5, accuracy: 0.05)
        XCTAssertEqual(MeetingRecorder.parts(for: url), [])
        XCTAssertEqual(MeetingRecorder.playableFiles(for: url), [url])
        let f = try AVAudioFile(forReading: url)
        XCTAssertEqual(Double(f.length) / f.processingFormat.sampleRate, 3.5, accuracy: 0.1)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: tmp.path), ["d.caf"])
    }

    /// 이어서 듣기: the finished file becomes the first part and the new sound joins after it.
    func testResumingAppendsToTheFinishedRecording() throws {
        let url = tmp.appendingPathComponent("r.caf")
        let first = try MeetingRecorder(url: url)
        first.append(tone(seconds: 2))
        first.finish()
        let again = try MeetingRecorder(url: url, resuming: true)
        XCTAssertEqual(again.seconds, 2, accuracy: 0.1)  // the old sound counts from the start
        again.append(tone(seconds: 3))
        XCTAssertEqual(again.finish(), 5, accuracy: 0.2)
        let seconds = Double(try AVAudioFile(forReading: url).length) / MeetingRecorder.sampleRate
        XCTAssertEqual(seconds, 5, accuracy: 0.3)
        XCTAssertTrue(MeetingRecorder.parts(for: url).isEmpty)
    }

    func testCloseWithoutJoinLeavesEveryPartPlayable() throws {   // the app quits mid-meeting
        let url = tmp.appendingPathComponent("q.caf")
        let r = try MeetingRecorder(url: url, partSeconds: 1)
        for _ in 0..<7 { r.append(tone(seconds: 0.5)) }   // 3.5 s: the last part is still open
        XCTAssertEqual(r.close(), 3.5, accuracy: 0.05)
        XCTAssertFalse(FileManager.default.fileExists(atPath: url.path))   // no join
        let parts = MeetingRecorder.parts(for: url)
        XCTAssertEqual(parts.count, 4)
        let playable = MeetingRecorder.playableFiles(for: url)
        XCTAssertEqual(playable, parts)   // the open one was closed, so none is empty
        let total = try playable.reduce(0.0) { sum, p in
            let f = try AVAudioFile(forReading: p)
            return sum + Double(f.length) / f.processingFormat.sampleRate
        }
        XCTAssertEqual(total, 3.5, accuracy: 0.15)
        r.append(tone(seconds: 1)); r.flushForTesting()   // nothing more is written once closed
        XCTAssertEqual(r.seconds, 3.5, accuracy: 0.05)
        XCTAssertEqual(r.finish(), 3.5, accuracy: 0.05)   // and finish no longer joins
        XCTAssertEqual(MeetingRecorder.parts(for: url), parts)
    }

    func testSizePerHourIsSpeechSized() throws {    // spec: 10–15 MB per hour
        let url = tmp.appendingPathComponent("c.caf")
        let r = try MeetingRecorder(url: url)
        r.append(tone(seconds: 60)); _ = r.finish()
        let bytes = try FileManager.default.attributesOfItem(atPath: url.path)[.size] as! Int
        XCTAssertLessThan(Double(bytes) * 60, 16_000_000)
    }

    func testPlayableFallsBackToPartsWhenFinalIsBroken() throws {
        let url = tmp.appendingPathComponent("e.caf")
        let r = try MeetingRecorder(url: url, partSeconds: 1)
        for _ in 0..<8 { r.append(tone(seconds: 0.5)) }
        r.flushForTesting()
        let parts = MeetingRecorder.parts(for: url).filter { (try? AVAudioFile(forReading: $0).length) ?? 0 > 0 }
        XCTAssertGreaterThanOrEqual(parts.count, 2)
        try Data().write(to: url)   // a final file that does not read
        XCTAssertEqual(MeetingRecorder.playableFiles(for: url), parts)
    }
}
