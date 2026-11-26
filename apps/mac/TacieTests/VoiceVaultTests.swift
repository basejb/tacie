import XCTest

@MainActor
final class VoiceVaultTests: XCTestCase {
    private var root: URL!
    private var on = true
    private let base = Date(timeIntervalSince1970: 1_792_000_000)

    override func setUp() async throws {
        root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        on = true
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: root)
    }

    private func vault() -> VoiceVault { VoiceVault(root: root, enabled: { [unowned self] in self.on }) }

    /// A 512-long vector pointing along one axis (or a mix of two).
    private func axis(_ i: Int, _ j: Int? = nil) -> [Float] {
        var v = [Float](repeating: 0, count: 512)
        v[i] = 1
        if let j { v[j] = 1 }
        return v
    }

    private func meeting(_ offset: TimeInterval, voices: [VoiceEntry]? = nil) -> SavedMeeting {
        var m = SavedMeeting(mode: "inperson", start: base + offset)
        m.voices = voices
        return m
    }

    private func files() -> [String] {
        let e = FileManager.default.enumerator(atPath: root.path)
        return (e?.allObjects as? [String] ?? []).sorted()
    }

    func testDismissedSuggestionsStayWithTheMeeting() {
        let v = vault()
        let m = meeting(0)
        v.save(prints: [MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 10, person: nil)], meeting: m.id)
        let before = v.revision
        let shot = v.snapshot(meeting: m.id)
        v.dismiss(SpeakerSuggest.pair("화자 1", "p1"), meeting: m.id)
        XCTAssertGreaterThan(v.revision, before)
        XCTAssertEqual(vault().dismissed(meeting: m.id), [SpeakerSuggest.pair("화자 1", "p1")])
        XCTAssertEqual(v.prints(meeting: m.id).count, 1)
        // saving the voices again keeps what was dismissed; an undo of a naming from before puts it back as it was
        v.save(prints: v.prints(meeting: m.id), meeting: m.id)
        XCTAssertEqual(v.dismissed(meeting: m.id).count, 1)
        v.restore(shot)
        XCTAssertEqual(v.dismissed(meeting: m.id), [])
    }

    func testNothingIsWrittenWhenOff() {
        on = false
        let v = vault()
        let m = meeting(0)
        v.save(prints: [MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 10, person: nil)], meeting: m.id)
        XCTAssertNil(v.remember(key: "화자 1", name: "민재", person: nil, meeting: m))
        XCTAssertEqual(v.people, [])
        XCTAssertEqual(v.prints(meeting: m.id), [])
        XCTAssertEqual(files(), [])
    }

    func testTurningOffKeepsStoredPeople() {
        let v = vault()
        let m = meeting(0)
        v.save(prints: [MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 10, person: nil)], meeting: m.id)
        let id = v.remember(key: "화자 1", name: "민재", person: nil, meeting: m)
        XCTAssertNotNil(id)
        on = false
        XCTAssertNil(v.remember(key: "화자 1", name: "민재", person: id, meeting: meeting(3600)))
        XCTAssertEqual(v.people.map(\.name), ["민재"])
        XCTAssertEqual(vault().people.map(\.id), [id!])
        XCTAssertEqual(vault().people.first?.meetings, 1)
    }

    func testRememberNewThenUpdateWeightsBySeconds() throws {
        let v = vault()
        let a = meeting(0)
        v.save(prints: [MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 30, person: nil)], meeting: a.id)
        let id = try XCTUnwrap(v.remember(key: "화자 1", name: "민재", person: nil, meeting: a))
        XCTAssertNotNil(UUID(uuidString: id))
        var p = try XCTUnwrap(v.people.first)
        XCTAssertEqual(p.name, "민재")
        XCTAssertEqual(p.vector, axis(0))
        XCTAssertEqual(p.seconds, 30)
        XCTAssertEqual(p.meetings, 1)
        XCTAssertEqual(p.last, a.start)
        XCTAssertEqual(p.color, 0)

        // The second meeting has the person as 화자 2, with 화자 3 merged into it: 10 + 20 seconds along axis 1.
        let b = meeting(86_400, voices: [VoiceEntry(key: "화자 3", same: "화자 2")])
        v.save(prints: [
            MeetingPrint(key: "화자 2", channel: "mic", vector: axis(1), seconds: 10, person: nil),
            MeetingPrint(key: "화자 3", channel: "mic", vector: axis(1), seconds: 20, person: nil),
            MeetingPrint(key: "화자 4", channel: "mic", vector: axis(2), seconds: 99, person: nil),
        ], meeting: b.id)
        XCTAssertEqual(v.remember(key: "화자 2", name: "다른 이름", person: id, meeting: b), id)
        p = try XCTUnwrap(vault().people.first)
        XCTAssertEqual(p.name, "민재")  // the person's own name stays; the meeting keeps the one given there
        XCTAssertEqual(p.seconds, 60)
        XCTAssertEqual(p.meetings, 2)
        XCTAssertEqual(p.last, b.start)
        // 30·e0 + 30·e1, normalised
        XCTAssertEqual(p.vector[0], Float(0.5).squareRoot(), accuracy: 1e-5)
        XCTAssertEqual(p.vector[1], Float(0.5).squareRoot(), accuracy: 1e-5)
        XCTAssertEqual(p.vector[2], 0)

        // A new person takes the next colour.
        let other = try XCTUnwrap(v.remember(key: "화자 4", name: "서연", person: nil, meeting: b))
        XCTAssertEqual(v.people.first { $0.id == other }?.color, 1)
    }

    func testRememberCountsAMeetingOnce() throws {
        let v = vault()
        let m = meeting(0)
        v.save(prints: [MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 10, person: nil)], meeting: m.id)
        let id = try XCTUnwrap(v.remember(key: "화자 1", name: "민재", person: nil, meeting: m))
        XCTAssertEqual(v.remember(key: "화자 1", name: "민재", person: id, meeting: m), id)
        let p = try XCTUnwrap(v.people.first)
        XCTAssertEqual(p.meetings, 1)
        XCTAssertEqual(p.seconds, 10)  // the same speaker's voice is not added twice
    }

    func testForgetMeetingDeletesItsPrints() {
        let v = vault()
        let a = meeting(0), b = meeting(3600)
        let print = MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 10, person: "p")
        v.save(prints: [print], meeting: a.id)
        v.save(prints: [print], meeting: b.id)
        XCTAssertEqual(vault().prints(meeting: a.id), [print])
        v.forget(meeting: a.id)
        XCTAssertEqual(vault().prints(meeting: a.id), [])
        XCTAssertEqual(vault().prints(meeting: b.id), [print])
        v.forgetMeetings()
        XCTAssertEqual(vault().prints(meeting: b.id), [])
    }

    func testRenameAndForgetPerson() throws {
        let v = vault()
        let m = meeting(0)
        v.save(prints: [
            MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 10, person: nil),
            MeetingPrint(key: "화자 2", channel: "mic", vector: axis(1), seconds: 10, person: nil),
        ], meeting: m.id)
        let a = try XCTUnwrap(v.remember(key: "화자 1", name: "민재", person: nil, meeting: m))
        let b = try XCTUnwrap(v.remember(key: "화자 2", name: "서연", person: nil, meeting: m))
        v.rename(person: a, to: "  김민재 ")
        v.rename(person: a, to: " ")
        v.forget(person: b)
        XCTAssertEqual(vault().people.map(\.name), ["김민재"])
        // Colours do not come back when someone is forgotten: the next one is the third colour.
        let c = try XCTUnwrap(v.remember(key: "화자 2", name: "지훈", person: nil, meeting: m))
        XCTAssertEqual(vault().people.first { $0.id == c }?.color, 2)
    }

    func testRemoveAllDeletesEverything() {
        let v = vault()
        let m = meeting(0)
        v.save(prints: [MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 10, person: nil)], meeting: m.id)
        XCTAssertNotNil(v.remember(key: "화자 1", name: "민재", person: nil, meeting: m))
        XCTAssertTrue(v.removeAll())
        XCTAssertEqual(v.people, [])
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.path))
        XCTAssertEqual(vault().people, [])
        XCTAssertTrue(vault().removeAll())  // nothing there is fine
    }

    /// A person forgotten while a meeting still linked to them is not made again from that link.
    func testRememberingAForgottenPersonMakesNoOne() throws {
        let v = vault()
        let a = meeting(0)
        v.save(prints: [MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 10, person: nil)], meeting: a.id)
        let id = try XCTUnwrap(v.remember(key: "화자 1", name: "민재", person: nil, meeting: a))
        v.forget(person: id)
        let b = meeting(3600)
        v.save(prints: [MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 10, person: id)], meeting: b.id)
        XCTAssertNil(v.remember(key: "화자 1", name: "민재", person: id, meeting: b))
        XCTAssertEqual(v.people, [])
        XCTAssertEqual(vault().people, [])
    }

    /// 기억한 참석자 모두 지우기 has something to delete: people, or meetings' voices kept to name later.
    func testHasStoredData() throws {
        let v = vault()
        XCTAssertFalse(v.hasStoredData)
        let m = meeting(0)
        v.save(prints: [MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 10, person: nil)], meeting: m.id)
        XCTAssertTrue(v.hasStoredData)
        XCTAssertEqual(v.people, [])
        let id = try XCTUnwrap(v.remember(key: "화자 1", name: "민재", person: nil, meeting: m))
        v.forget(meeting: m.id)
        XCTAssertTrue(v.hasStoredData)  // the person
        v.forget(person: id)
        XCTAssertFalse(v.hasStoredData)
        v.save(prints: [MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 10, person: nil)], meeting: m.id)
        XCTAssertTrue(v.removeAll())
        XCTAssertFalse(v.hasStoredData)
    }

    /// A people file that cannot be read is moved aside before anything writes over it.
    func testUnreadablePeopleFileIsKeptAside() throws {
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let bad = Data("{\"created\": 1, \"people\": [{\"id\": ".utf8)
        try bad.write(to: root.appendingPathComponent("people.json"))
        let v = vault()
        XCTAssertEqual(v.people, [])
        let m = meeting(0)
        v.save(prints: [MeetingPrint(key: "화자 1", channel: "mic", vector: axis(0), seconds: 10, person: nil)], meeting: m.id)
        XCTAssertNotNil(v.remember(key: "화자 1", name: "민재", person: nil, meeting: m))
        XCTAssertEqual(try Data(contentsOf: root.appendingPathComponent("people.json.bak")), bad)
        XCTAssertEqual(vault().people.map(\.name), ["민재"])
    }
}
