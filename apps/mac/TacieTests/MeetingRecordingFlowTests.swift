import XCTest

/// MeetingModel's recording decisions, as the pure functions it calls (MeetingModel itself needs a running engine).
final class MeetingRecordingFlowTests: XCTestCase {
    private func heard(words: [HeardWord]?, fired: Bool = false) -> Heard {
        Heard(utteranceId: "s1u3", text: "배포는 언제 해요", fired: fired, pLookup: 0.2, skipped: nil, mergedFrom: nil,
              voice: "화자 1", words: words, error: nil, errorKind: nil)
    }

    func testRecordingWanted() {
        XCTAssertFalse(MeetingRecording.wanted(setting: false, practice: false, sample: false))
        XCTAssertTrue(MeetingRecording.wanted(setting: true, practice: false, sample: false))
        XCTAssertFalse(MeetingRecording.wanted(setting: true, practice: true, sample: false))
        XCTAssertFalse(MeetingRecording.wanted(setting: true, practice: false, sample: true))
    }

    func testRecordMeetingsDefaultsOn() {
        let before = UserDefaults.standard.object(forKey: "recordMeetings")
        defer { UserDefaults.standard.set(before, forKey: "recordMeetings") }
        UserDefaults.standard.removeObject(forKey: "recordMeetings")
        XCTAssertTrue(MeetingRecording.setting)
        UserDefaults.standard.set(false, forKey: "recordMeetings")
        XCTAssertFalse(MeetingRecording.setting)
    }

    func testSplitPiecesDecodeAndSitAtTheirOffset() throws {
        let h = try EngineClient.engineDecoder.decode(Heard.self, from: Data(#"""
        {"utterance_id": "s1u3", "text": "하나 둘", "fired": false, "p_lookup": 0.2, "skipped": null, "merged_from": null,
         "voice": "화자 1", "words": [{"text": "하나", "start": 0.3, "end": 1.5}],
         "then": [{"utterance_id": "s1u4", "text": "셋 넷", "fired": true, "p_lookup": 0.9, "skipped": null,
                   "merged_from": null, "voice": "화자 2", "offset_ms": 6950,
                   "words": [{"text": "셋", "start": 0.45, "end": 1.65}]}]}
        """#.utf8))
        let pieces = MeetingRecording.pieces(of: h, startMs: 10_000)
        XCTAssertEqual(pieces.map(\.heard.utteranceId), ["s1u3", "s1u4"])
        XCTAssertEqual(pieces.map(\.startMs), [10_000, 16_950])
        let second = MeetingRecording.line(for: pieces[1].heard, key: "e0:s1u4", at: Date(), startMs: pieces[1].startMs)
        XCTAssertEqual(second.voice, "화자 2")
        XCTAssertTrue(second.question)
        XCTAssertEqual(second.audioAt, 16_950)
        XCTAssertEqual(second.words?.first?.start, 17_400)
        XCTAssertEqual(MeetingRecording.pieces(of: h, startMs: nil).map(\.startMs), [nil, nil])
        XCTAssertEqual(MeetingRecording.pieces(of: heard(words: nil), startMs: 5).count, 1)
    }

    func testLineWithoutRecordingHasNoPlaces() {
        let at = Date(timeIntervalSince1970: 1_000)
        let l = MeetingRecording.line(for: heard(words: [HeardWord(text: "배포는", start: 0.1, end: 0.4)], fired: true),
                                      key: "e0:s1u3", at: at, startMs: nil)
        XCTAssertNil(l.audioAt)
        XCTAssertNil(l.words)
        XCTAssertEqual(l.key, "e0:s1u3")
        XCTAssertEqual(l.at, at)
        XCTAssertEqual(l.text, "배포는 언제 해요")
        XCTAssertTrue(l.question)
        XCTAssertEqual(l.voice, "화자 1")
        XCTAssertNil(l.speaker)
    }

    func testLineWithRecordingPlacesWords() {
        let l = MeetingRecording.line(for: heard(words: [HeardWord(text: "a", start: 0.1, end: 0.4)]),
                                      key: "e0:s1u3", at: Date(), startMs: 5000)
        XCTAssertEqual(l.audioAt, 5000)
        XCTAssertEqual(l.words, [SpokenWord(text: "a", start: 5100, end: 5400)])
    }

    func testLineWithRecordingButNoTimingKeepsItsPlaceOnly() {
        let l = MeetingRecording.line(for: heard(words: nil), key: "e0:s1u3", at: Date(), startMs: 5000)
        XCTAssertEqual(l.audioAt, 5000)
        XCTAssertNil(l.words)
    }

    /// 대화 기록 on the island puts the mic of a remote meeting on the right as "나": the line keeps its channel.
    func testLineKeepsItsChannel() {
        let me = MeetingRecording.line(for: heard(words: nil), key: "e0:s1u3", at: Date(), startMs: nil, speaker: "me")
        XCTAssertEqual(me.speaker, "me")
        XCTAssertNil(MeetingRecording.line(for: heard(words: nil), key: "e0:s1u4", at: Date(), startMs: nil).speaker)
    }

    func testUnheardSpanCoversTheUtterance() {
        // 44-byte header + 1.5 s of 16 kHz 16-bit mono
        let wav = Data(count: 44 + 24_000 * 2)
        XCTAssertEqual(MeetingRecording.span(wav: wav, at: 7000), AudioSpan(start: 7000, end: 8500))
        XCTAssertEqual(MeetingRecording.span(wav: Data(count: 10), at: 7000), AudioSpan(start: 7000, end: 7000))
    }

    func testMeetingStartedNotificationName() {
        XCTAssertEqual(Notification.Name.meetingStarted.rawValue, "ai.tacie.meetingStarted")
    }
}
