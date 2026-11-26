import XCTest

final class SmallSpeakersTests: XCTestCase {
    private let base = Date(timeIntervalSince1970: 1_792_000_000)

    private func line(_ n: Int, _ voice: String) -> TranscriptLine {
        TranscriptLine(key: "e0:s1u\(n)", at: base + Double(n), speaker: nil, text: "말", question: false, voice: voice)
    }

    /// 화자 1 and 2 talk a lot; 화자 3 said one line. `lines` per speaker in order.
    private func meeting(_ counts: [(String, Int)], voices: [VoiceEntry]? = nil) -> SavedMeeting {
        var m = SavedMeeting(mode: "inperson", start: base)
        var n = 0
        for (v, c) in counts {
            for _ in 0..<c {
                n += 1
                m.transcript.append(line(n, v))
            }
        }
        m.voices = voices
        return m
    }

    private func print(_ key: String, _ diar: [Float]?, seconds: Float, channel: String = "room", person: String? = nil) -> MeetingPrint {
        MeetingPrint(key: key, channel: channel, vector: [1, 0, 0], seconds: seconds, person: person, diarVector: diar)
    }

    // cos(a, near) = 0.8, cos(a, far) = 0
    private let a: [Float] = [1, 0, 0]
    private let b: [Float] = [0, 1, 0]
    private let near: [Float] = [0.8, 0.6, 0]
    private let far: [Float] = [0, 0, 1]

    func testSmallCloseSpeakerJoinsNearest() {
        var m = meeting([("화자 1", 10), ("화자 2", 10), ("화자 3", 1)])
        let prints = [print("화자 1", a, seconds: 60), print("화자 2", b, seconds: 50), print("화자 3", near, seconds: 4)]
        XCTAssertTrue(m.foldSmallSpeakers(prints))
        XCTAssertEqual(m.resolved("화자 3"), "화자 1")
        XCTAssertEqual(m.voices?.first { $0.key == "화자 3" }?.auto, true)
        XCTAssertEqual(m.attendees.map(\.key), ["화자 1", "화자 2"])
        XCTAssertFalse(m.foldSmallSpeakers(prints), "nothing changes the second time")
    }

    func testSmallFarSpeakerStays() {
        var m = meeting([("화자 1", 10), ("화자 2", 10), ("화자 3", 1)])
        let prints = [print("화자 1", a, seconds: 60), print("화자 2", b, seconds: 50), print("화자 3", far, seconds: 4)]
        XCTAssertFalse(m.foldSmallSpeakers(prints))
        XCTAssertEqual(m.resolved("화자 3"), "화자 3")
    }

    func testSpeakerWhoSaidEnoughStays() {
        // long enough, or too many lines
        var m = meeting([("화자 1", 10), ("화자 3", 2), ("화자 4", 4)])
        let prints = [print("화자 1", a, seconds: 60), print("화자 3", near, seconds: 12), print("화자 4", near, seconds: 8)]
        XCTAssertFalse(m.foldSmallSpeakers(prints))
    }

    func testNamedLinkedOrHandMergedSpeakersAreLeftAlone() {
        let prints = [print("화자 1", a, seconds: 60), print("화자 3", near, seconds: 4)]
        var named = meeting([("화자 1", 10), ("화자 3", 1)], voices: [VoiceEntry(key: "화자 3", name: "지수")])
        XCTAssertFalse(named.foldSmallSpeakers(prints))
        var linked = meeting([("화자 1", 10), ("화자 3", 1)])
        XCTAssertFalse(linked.foldSmallSpeakers([print("화자 1", a, seconds: 60), print("화자 3", near, seconds: 4, person: "p1")]))
        var merged = meeting([("화자 1", 10), ("화자 2", 10), ("화자 3", 1)], voices: [VoiceEntry(key: "화자 3", same: "화자 2")])
        XCTAssertFalse(merged.foldSmallSpeakers(prints + [print("화자 2", b, seconds: 50)]))
        XCTAssertEqual(merged.resolved("화자 3"), "화자 2")
    }

    func testOtherChannelOrNoSpeakerModelNeverJoins() {
        var m = meeting([("화자 1", 10), ("화자 3", 1)])
        XCTAssertFalse(m.foldSmallSpeakers([print("화자 1", a, seconds: 60, channel: "mic"), print("화자 3", near, seconds: 4, channel: "call")]))
        XCTAssertFalse(m.foldSmallSpeakers([print("화자 1", nil, seconds: 60), print("화자 3", nil, seconds: 4)]))
    }

    func testJoinedSpeakerLetsGoWhenItSaysMore() {
        var m = meeting([("화자 1", 10), ("화자 3", 1)])
        XCTAssertTrue(m.foldSmallSpeakers([print("화자 1", a, seconds: 60), print("화자 3", near, seconds: 4)]))
        m.transcript += (0..<4).map { line(100 + $0, "화자 3") }
        XCTAssertTrue(m.foldSmallSpeakers([print("화자 1", a, seconds: 60), print("화자 3", near, seconds: 20)]))
        XCTAssertEqual(m.resolved("화자 3"), "화자 3")
        XCTAssertNil(m.voices?.first { $0.key == "화자 3" }?.auto)
    }

    func testJoinedSpeakerFollowsAHandMergeOfItsTarget() {
        var m = meeting([("화자 1", 10), ("화자 2", 10), ("화자 3", 1)])
        let prints = [print("화자 1", a, seconds: 60), print("화자 2", b, seconds: 50), print("화자 3", near, seconds: 4)]
        XCTAssertTrue(m.foldSmallSpeakers(prints))
        m.merge("화자 1", into: "화자 2")
        XCTAssertEqual(m.resolved("화자 3"), "화자 2")
        _ = m.foldSmallSpeakers(prints)
        XCTAssertEqual(m.resolved("화자 3"), "화자 2")
        XCTAssertEqual(m.resolved("화자 1"), "화자 2")
        XCTAssertFalse(m.foldSmallSpeakers(prints))
    }

    func testReleaseFolded() {
        var m = meeting([("화자 1", 10), ("화자 3", 1)])
        XCTAssertTrue(m.foldSmallSpeakers([print("화자 1", a, seconds: 60), print("화자 3", near, seconds: 4)]))
        XCTAssertTrue(m.releaseFolded("화자 3"))
        XCTAssertEqual(m.resolved("화자 3"), "화자 3")
        XCTAssertFalse(m.releaseFolded("화자 1"))
    }
}
