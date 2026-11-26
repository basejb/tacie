import XCTest

final class SpeakerSuggestTests: XCTestCase {
    private let base = Date(timeIntervalSince1970: 1_792_000_000)

    /// A 4-long unit vector at `cos` from axis 0, leaning toward axis `toward`.
    private func near(_ cos: Float, toward: Int = 1) -> [Float] {
        var v = [Float](repeating: 0, count: 4)
        v[0] = cos
        v[toward] = (1 - cos * cos).squareRoot()
        return v
    }

    private func person(_ id: String, _ vector: [Float]) -> Person {
        Person(id: id, name: id, vector: vector, seconds: 30, meetings: 1, last: base, color: 0)
    }

    private func meeting(_ keys: [String], voices: [VoiceEntry]? = nil) -> SavedMeeting {
        var m = SavedMeeting(mode: "inperson", start: base)
        m.transcript = keys.enumerated().map { i, k in
            TranscriptLine(key: "e0:s1u\(i)", at: base + Double(i), speaker: nil, text: "말", question: false, voice: k, name: nil)
        }
        m.voices = voices
        return m
    }

    private func print(_ key: String, _ vector: [Float], seconds: Float = 20) -> MeetingPrint {
        MeetingPrint(key: key, channel: "mic", vector: vector, seconds: seconds, person: nil)
    }

    private let speaker: [Float] = [1, 0, 0, 0]

    func testCloseEnoughAndClearlyAheadIsSuggested() {
        let m = meeting(["화자 1"])
        let got = SpeakerSuggest.make(meeting: m, prints: [print("화자 1", speaker)],
                                      people: [person("지윤", near(0.58)), person("서연", near(0.3, toward: 2))], dismissed: [])
        XCTAssertEqual(got, ["화자 1": "지윤"])
    }

    func testBelowTheFloorOrTooCloseToTheNextIsNot() {
        let m = meeting(["화자 1"])
        let p = [print("화자 1", speaker)]
        XCTAssertEqual(SpeakerSuggest.make(meeting: m, prints: p, people: [person("a", near(0.45))], dismissed: []), [:])
        // 0.58 and 0.52: 0.06 apart, under the 0.10 margin
        XCTAssertEqual(SpeakerSuggest.make(meeting: m, prints: p,
                                           people: [person("a", near(0.58)), person("b", near(0.52, toward: 2))], dismissed: []), [:])
    }

    func testShortSpeakerGetsNone() {
        let m = meeting(["화자 1"])
        XCTAssertEqual(SpeakerSuggest.make(meeting: m, prints: [print("화자 1", speaker, seconds: 5)],
                                           people: [person("a", near(0.6))], dismissed: []), [:])
    }

    func testNamedSpeakersAndPeopleAlreadyThereAreLeftOut() {
        // 화자 2 is named 지윤 (linked): 지윤 is not suggested to 화자 1, and 화자 2 gets nothing
        let m = meeting(["화자 1", "화자 2"], voices: [VoiceEntry(key: "화자 2", name: "지윤", person: "지윤")])
        let got = SpeakerSuggest.make(meeting: m, prints: [print("화자 1", speaker), print("화자 2", speaker)],
                                      people: [person("지윤", near(0.6))], dismissed: [])
        XCTAssertEqual(got, [:])
        // a typed name that is a remembered person's name also counts as them being there
        let typed = meeting(["화자 1", "화자 2"], voices: [VoiceEntry(key: "화자 2", name: "지윤")])
        XCTAssertEqual(SpeakerSuggest.make(meeting: typed, prints: [print("화자 1", speaker)],
                                           people: [person("지윤", near(0.6))], dismissed: []), [:])
    }

    func testOnePersonGoesToTheClosestSpeaker() {
        let m = meeting(["화자 1", "화자 2"])
        let got = SpeakerSuggest.make(meeting: m, prints: [print("화자 1", speaker), print("화자 2", near(0.9, toward: 2))],
                                      people: [person("지윤", near(0.6, toward: 3))], dismissed: [])
        // 화자 1·지윤 0.60, 화자 2·지윤 0.54: only 화자 1 gets her
        XCTAssertEqual(got, ["화자 1": "지윤"])
    }

    func testMergedSpeakerCountsAsOneWithItsVoices() {
        // 화자 3 merged into 화자 1: their prints make one voice, 12 s together
        let m = meeting(["화자 1", "화자 3"], voices: [VoiceEntry(key: "화자 3", same: "화자 1")])
        let got = SpeakerSuggest.make(meeting: m, prints: [print("화자 1", speaker, seconds: 6), print("화자 3", speaker, seconds: 6)],
                                      people: [person("지윤", near(0.6))], dismissed: [])
        XCTAssertEqual(got, ["화자 1": "지윤"])
    }

    func testDismissedSpeakerGetsNoSuggestionAgain() {
        let m = meeting(["화자 1"])
        let got = SpeakerSuggest.make(meeting: m, prints: [print("화자 1", speaker)], people: [person("지윤", near(0.6))],
                                      dismissed: [SpeakerSuggest.pair("화자 1", "지윤")])
        XCTAssertEqual(got, [:])
    }
}
