import XCTest

/// Putting a naming of speakers back (Undo), and the particles its toast uses.
final class NamingUndoTests: XCTestCase {
    private let base = Date(timeIntervalSince1970: 1_792_000_000)

    private func line(_ n: Int, _ voice: String?, name: String? = nil) -> TranscriptLine {
        TranscriptLine(key: "e0:s1u\(n)", at: base + Double(n), speaker: nil, text: "말 \(n)", question: false, voice: voice, name: name)
    }

    private func meeting(_ lines: [TranscriptLine]) -> SavedMeeting {
        var m = SavedMeeting(mode: "inperson", start: base)
        m.transcript = lines
        return m
    }

    func testNamingIsPutBack() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 1"), line(3, "화자 2")])
        let before = m.naming
        m.rename(key: "화자 1", to: "해리", person: nil)
        m.renameLine("e0:s1u2", to: "앤디")
        m.merge("화자 2", into: "화자 1")
        XCTAssertNotEqual(m.naming, before)
        let after = m.naming
        m.restore(before)
        XCTAssertEqual(m.naming, before)
        XCTAssertEqual(m.lineName(m.transcript[1]), "화자 1")
        m.restore(after)
        XCTAssertEqual(m.lineName(m.transcript[1]), "앤디")
        XCTAssertEqual(m.lineName(m.transcript[2]), "해리")
    }

    /// A line heard after the naming keeps no name of its own when the naming is undone.
    func testRestoreLeavesNewLinesUnnamed() {
        var m = meeting([line(1, "화자 1")])
        let before = m.naming
        m.renameLine("e0:s1u1", to: "앤디")
        m.transcript.append(line(2, "화자 1", name: "앤디"))
        m.restore(before)
        XCTAssertNil(m.transcript[0].name)
        XCTAssertNil(m.transcript[1].name)
    }

    /// The engine moved two lines named on their own (앤디) out of 화자 1 into a new voice, 화자 3: the lines are 화자 3's,
    /// 화자 3 is 앤디, and the undo puts the lines back on 화자 1.
    func testMovedLinesBecomeTheirOwnSpeaker() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 1"), line(3, "화자 1"), line(4, "화자 2")])
        m.rename(key: "화자 1", to: "해리", person: nil)
        let before = m.naming
        m.renameLine("e0:s1u2", to: "앤디")
        m.renameLine("e0:s1u3", to: "앤디")
        m.moved(["e0:s1u2", "e0:s1u3"], to: "화자 3", name: "앤디", person: nil)
        XCTAssertEqual(m.transcript.map(\.voice), ["화자 1", "화자 3", "화자 3", "화자 2"])
        XCTAssertEqual(m.transcript.map(\.name), [nil, nil, nil, nil])
        XCTAssertEqual(m.transcript.map { m.lineName($0) }, ["해리", "앤디", "앤디", "화자 2"])
        XCTAssertEqual(m.attendees.map(\.name), ["해리", "앤디", "화자 2"])
        m.restore(before)
        XCTAssertEqual(m.transcript.map(\.voice), ["화자 1", "화자 1", "화자 1", "화자 2"])
        XCTAssertEqual(m.transcript.map { m.lineName($0) }, ["해리", "해리", "해리", "화자 2"])
    }

    /// Moved into a speaker who already has the name: the lines join it and keep no name of their own.
    func testMovedIntoANamedSpeaker() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 1"), line(3, "화자 2")])
        m.rename(key: "화자 2", to: "앤디", person: nil)
        m.renameLine("e0:s1u2", to: "앤디")
        m.moved(["e0:s1u2"], to: "화자 2", name: "앤디", person: nil)
        XCTAssertEqual(m.transcript[1].voice, "화자 2")
        XCTAssertNil(m.transcript[1].name)
        XCTAssertEqual(m.lineName(m.transcript[1]), "앤디")
    }

    func testParticles() {
        XCTAssertEqual(Korean.with("화자 2", "을", "를"), "화자 2를")
        XCTAssertEqual(Korean.with("화자 1", "을", "를"), "화자 1을")
        XCTAssertEqual(Korean.with("해리", "과", "와"), "해리와")
        XCTAssertEqual(Korean.with("민준", "과", "와"), "민준과")
        XCTAssertEqual(Korean.with("Andy", "을", "를"), "Andy를")
    }
}
