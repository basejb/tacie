import XCTest

final class MeetingVoicesTests: XCTestCase {
    private let base = Date(timeIntervalSince1970: 1_792_000_000)

    private func line(_ n: Int, _ voice: String?, _ text: String = "말", question: Bool = false, name: String? = nil) -> TranscriptLine {
        TranscriptLine(key: "e0:s1u\(n)", at: base + Double(n), speaker: nil, text: text, question: question, voice: voice, name: name)
    }

    private func meeting(_ lines: [TranscriptLine], voices: [VoiceEntry]? = nil) -> SavedMeeting {
        var m = SavedMeeting(mode: "inperson", start: base)
        m.transcript = lines
        m.voices = voices
        return m
    }

    /// A file saved before speakers: no `voice`, no `voices`, owners and askers as me/them.
    func testOldMeetingDecodesWithoutSpeakers() throws {
        let json = #"""
        {"version": 1, "id": "20261023-140000", "mode": "remote", "start": "2026-10-23T05:00:00Z", "end": "2026-10-23T05:47:00Z",
         "title": "결제 실패 재시도 정책", "titleEdited": false, "notes": "done", "summary": ["3번까지 다시 시도해요."],
         "ledger": [{"kind": "commitment", "text": "PR 올리기", "owner": "me", "due": "금요일까지", "done": false}],
         "questions": [{"id": "v1", "question": "재시도는 몇 번까지 해요?", "asker": "them", "at": "2026-10-23T05:06:00Z",
                        "answer": "최대 2번이에요.", "cards": [], "cited": [], "unanswered": false}],
         "transcript": [{"key": "e0:s1u1", "at": "2026-10-23T05:06:00Z", "speaker": "them", "text": "재시도는 몇 번까지 해요?", "question": true},
                        {"key": "e0:s1u2", "at": "2026-10-23T05:06:05Z", "speaker": "me", "text": "찾아볼게요", "question": false}]}
        """#
        let d = JSONDecoder()
        d.dateDecodingStrategy = .iso8601
        let m = try d.decode(SavedMeeting.self, from: Data(json.utf8))
        XCTAssertFalse(m.hasSpeakers)
        XCTAssertTrue(m.attendees.isEmpty)
        XCTAssertNil(m.voices)
        XCTAssertNil(m.transcript[0].voice)
        XCTAssertNil(m.questions[0].voice)
        XCTAssertNil(m.lineName(m.transcript[0]))
        XCTAssertEqual(m.ownerName(m.ledger[0].owner!), "나")
    }

    func testOtherSpeakerNoteSaysWhatPickingDoes() {
        XCTAssertEqual(namerOthersNote(lineOnly: false), "같은 사람이면 합쳐요")
        XCTAssertEqual(namerOthersNote(lineOnly: true), "이 말만 이 사람으로 바꿔요")
    }

    func testRenameAppliesToEveryLineOfTheKey() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 2"), line(3, "화자 1")])
        m.rename(key: "화자 1", to: " 민재 ", person: "p1")
        XCTAssertEqual(m.transcript.map { m.lineName($0) }, ["민재", "화자 2", "민재"])
        XCTAssertEqual(m.voices, [VoiceEntry(key: "화자 1", name: "민재", person: "p1", same: nil)])
        m.rename(key: "화자 1", to: "민재 님", person: nil)
        XCTAssertEqual(m.displayName(key: "화자 1"), "민재 님")
        XCTAssertNil(m.voices?.first?.person)
        XCTAssertEqual(m.voices?.count, 1)
    }

    func testRenameLineTouchesOnlyThatLine() {
        var m = meeting([line(1, "화자 2"), line(2, "화자 2"), line(3, "화자 2")])
        m.renameLine("e0:s1u2", to: "지수")
        XCTAssertEqual(m.transcript.map { m.lineName($0) }, ["화자 2", "지수", "화자 2"])
        XCTAssertEqual(m.displayName(key: "화자 2"), "화자 2")
        XCTAssertNil(m.voices)
        m.renameLine("e0:s1u2", to: "  ")  // blank: the line goes back to its speaker
        XCTAssertEqual(m.lineName(m.transcript[1]), "화자 2")
    }

    func testMergeFollowsSame() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 4"), line(3, "화자 2"), line(4, "화자 4")])
        m.rename(key: "화자 1", to: "민재", person: nil)
        m.merge("화자 4", into: "화자 1")
        XCTAssertEqual(m.lineName(m.transcript[1]), "민재")
        XCTAssertEqual(m.displayName(key: "화자 4"), "민재")
        XCTAssertEqual(m.attendees.filter { $0.name == "민재" }.count, 1)
        XCTAssertEqual(m.attendees.map(\.key), ["화자 1", "화자 2"])
        // A rename through the merged key names the one it joined; merging back is a no-op, not a loop.
        m.rename(key: "화자 4", to: "민재 님", person: nil)
        XCTAssertEqual(m.displayName(key: "화자 1"), "민재 님")
        m.merge("화자 1", into: "화자 4")
        XCTAssertEqual(m.displayName(key: "화자 4"), "민재 님")
        XCTAssertEqual(m.attendees.map(\.key), ["화자 1", "화자 2"])
    }

    func testAttendeesShareByCharacters() {
        var m = meeting([line(1, "화자 1", "배포는 금요일에"), line(2, "화자 1", "해요"), line(3, "화자 2", "네 좋아요"), line(4, nil, "잡음")])
        m.rename(key: "화자 1", to: "민재", person: nil)
        XCTAssertTrue(m.hasSpeakers)
        let a = m.attendees
        XCTAssertEqual(a.map(\.name), ["민재", "화자 2"])
        XCTAssertEqual(a.map(\.key), ["화자 1", "화자 2"])
        XCTAssertEqual(a.map(\.share), [69, 31])  // 9 and 4 characters; spaces and unvoiced lines don't count
    }

    func testAttendeesShareFloorsAtOne() {
        let m = meeting([line(1, "화자 1", String(repeating: "가", count: 300)), line(2, "화자 2", "네")])
        XCTAssertEqual(m.attendees.map(\.share), [100, 1])
    }

    func testOwnerNameKeyLegacyAndLiteral() {
        var m = meeting([line(1, "화자 2")])
        m.rename(key: "화자 2", to: "지수", person: nil)
        XCTAssertEqual(m.ownerName("화자 2"), "지수")
        XCTAssertEqual(m.ownerName("화자 7"), "화자 7")  // a key the model named without lines: still a key
        XCTAssertEqual(m.ownerName("me"), "나")
        XCTAssertEqual(m.ownerName("them"), "상대")
        XCTAssertEqual(m.ownerName("팀장"), "팀장")
    }

    func testTurnsSplitOnNameChangeAndMarkQuestions() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 1", question: true), line(3, "화자 2"), line(4, "화자 2", name: "지수"),
                         line(5, "화자 3"), line(6, nil)])
        m.merge("화자 3", into: "화자 2")
        let t = m.turns
        XCTAssertEqual(t.map(\.name), ["화자 1", "화자 2", "지수", "화자 2", nil])
        XCTAssertEqual(t.map(\.key), ["화자 1", "화자 2", "화자 2", "화자 2", nil])
        XCTAssertEqual(t.map { $0.lines.count }, [2, 1, 1, 1, 1])
        XCTAssertEqual(t.map(\.question), [true, false, false, false, false])
        XCTAssertEqual(t[0].at, base + 1)
        XCTAssertEqual(t[2].at, base + 4)
    }

    func testColorIndexByNumberAndPerson() {
        XCTAssertEqual(SavedMeeting.colorIndex(for: "화자 1", person: nil, personColor: nil), 0)
        XCTAssertEqual(SavedMeeting.colorIndex(for: "화자 6", person: nil, personColor: nil), 5)
        XCTAssertEqual(SavedMeeting.colorIndex(for: "화자 7", person: nil, personColor: nil), 0)
        XCTAssertEqual(SavedMeeting.colorIndex(for: "화자 2", person: "p1", personColor: 4), 4)
        XCTAssertEqual(SavedMeeting.colorIndex(for: "화자 2", person: nil, personColor: 4), 1)  // no person: by number
        XCTAssertEqual(SavedMeeting.colorIndex(for: "화자 2", person: "p1", personColor: nil), 1)
        XCTAssertEqual(SavedMeeting.colorIndex(for: "화자 2", person: "p1", personColor: 9), 3)
    }

    // MARK: the live meeting and the engine

    func testHeardDecodesVoiceAndPerson() throws {
        let json = #"{"utterance_id": "s1u3", "text": "배포 언제예요?", "fired": true, "p_lookup": 0.9, "skipped": null,"#
            + #" "merged_from": null, "voice": "화자 2", "person": "p1", "error": null, "error_kind": null}"#
        let h = try EngineClient.engineDecoder.decode(Heard.self, from: Data(json.utf8))
        XCTAssertEqual(h.voice, "화자 2")
        XCTAssertEqual(h.person, "p1")
        let old = #"{"utterance_id": "s1u3", "text": "네", "fired": false, "skipped": null, "merged_from": null, "error": null, "error_kind": null}"#
        let o = try EngineClient.engineDecoder.decode(Heard.self, from: Data(old.utf8))
        XCTAssertNil(o.voice)
        XCTAssertNil(o.person)
    }

    func testEngineVoiceDecodesToAPrint() throws {
        let v = (0..<512).map { _ in "0.0441" }.joined(separator: ",")
        let json = #"{"voices": [{"voice": "화자 2", "channel": "call", "vector": ["# + v + #"], "seconds": 12.5, "person": null, "near": "p1"}]}"#
        struct Voices: Decodable { let voices: [EngineVoice] }
        let e = try EngineClient.engineDecoder.decode(Voices.self, from: Data(json.utf8)).voices[0]
        XCTAssertEqual(e.near, "p1")
        XCTAssertEqual(e.print, MeetingPrint(key: "화자 2", channel: "call", vector: e.vector, seconds: 12.5, person: nil))
        XCTAssertEqual(e.vector.count, 512)
    }

    /// With the speaker-grouping model the engine also hands over that voice; it rides along in the print for a restore.
    func testEngineVoiceKeepsItsDiarVector() throws {
        let v = (0..<512).map { _ in "0.0441" }.joined(separator: ",")
        let d = (0..<192).map { _ in "0.0722" }.joined(separator: ",")
        let json = #"{"voices": [{"voice": "화자 1", "channel": "room", "vector": ["# + v + #"], "diar_vector": ["# + d
            + #"], "seconds": 30, "person": "p1", "near": null}]}"#
        struct Voices: Decodable { let voices: [EngineVoice] }
        let e = try EngineClient.engineDecoder.decode(Voices.self, from: Data(json.utf8)).voices[0]
        XCTAssertEqual(e.diarVector?.count, 192)
        XCTAssertEqual(e.print.diarVector, e.diarVector)
        // a print saved before this change has no diar vector and still decodes
        let old = #"{"key": "화자 1", "channel": "room", "vector": [1], "seconds": 3, "person": null}"#
        XCTAssertNil(try JSONDecoder().decode(MeetingPrint.self, from: Data(old.utf8)).diarVector)
    }

    /// Every key whose shown name is not the key itself: named ones, and merged ones (even into an unnamed key).
    func testNamesMapFromMeeting() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 2"), line(3, "화자 3"), line(4, "화자 4"), line(5, "화자 5", name: "민재")])
        m.rename(key: "화자 2", to: "지수", person: nil)
        m.merge("화자 3", into: "화자 2")
        m.merge("화자 4", into: "화자 1")
        XCTAssertEqual(m.engineNames(), ["화자 2": "지수", "화자 3": "지수", "화자 4": "화자 1"])
        XCTAssertEqual(meeting([line(1, "화자 1")]).engineNames(), [:])
    }

    /// New voices of a renewed session start after the highest number the transcript already used.
    func testHighestSpeakerNumber() {
        XCTAssertEqual(meeting([line(1, "화자 2"), line(2, "화자 11"), line(3, nil), line(4, "화자 3")]).highestSpeaker, 11)
        XCTAssertEqual(meeting([line(1, nil)]).highestSpeaker, 0)
    }

    /// Lines named on their own (이 줄만) stay out of their speaker's voice: only this session's, by the engine's id.
    func testOwnLinesOfThisEpoch() {
        var m = meeting([line(1, "화자 1", name: "민재"), line(2, "화자 1"), line(3, "화자 2", name: "지수")])
        m.transcript.append(TranscriptLine(key: "e1:s1u1", at: base + 9, speaker: nil, text: "말", question: false, voice: "화자 1", name: "지수"))
        XCTAssertEqual(m.ownLines(epoch: 0), ["s1u1", "s1u3"])
        XCTAssertEqual(m.ownLines(epoch: 1), ["s1u1"])
    }

    /// The engine knew a remembered person: their name goes on the key unless the key already has one.
    func testAdoptPersonOnlyWhenUnnamed() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 2"), line(3, "화자 3")])
        XCTAssertTrue(m.adopt(key: "화자 1", name: "민재", person: "p1"))
        XCTAssertEqual(m.displayName(key: "화자 1"), "민재")
        XCTAssertEqual(m.voices?.first { $0.key == "화자 1" }?.person, "p1")
        m.rename(key: "화자 2", to: "지수", person: nil)
        XCTAssertFalse(m.adopt(key: "화자 2", name: "서연", person: "p2"))
        XCTAssertEqual(m.displayName(key: "화자 2"), "지수")
        m.merge("화자 3", into: "화자 2")
        XCTAssertFalse(m.adopt(key: "화자 3", name: "서연", person: "p2"))
        XCTAssertFalse(m.adopt(key: "화자 1", name: "민재", person: "p1"))  // already theirs
    }

    /// Naming a line on its own during the meeting puts it on the list the next voices fetch leaves out.
    func testRenamedLineJoinsTheExcludeList() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 1"), line(3, "화자 2")])
        XCTAssertEqual(m.ownLines(epoch: 0), [])
        m.renameLine("e0:s1u2", to: "민재")
        XCTAssertEqual(m.ownLines(epoch: 0), ["s1u2"])
        m.renameLine("e0:s1u2", to: " ")  // given back to its speaker
        XCTAssertEqual(m.ownLines(epoch: 0), [])
    }
    // MARK: the meeting page

    private func person(_ id: String, color: Int) -> Person {
        Person(id: id, name: id, vector: [], seconds: 0, meetings: 1, last: base, color: color)
    }

    /// A remembered person keeps their colour; anyone else goes by number; a merged key takes its target's.
    func testSpeakerColorFollowsRememberedPerson() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 2"), line(3, "화자 3")],
                        voices: [VoiceEntry(key: "화자 2", name: "지수", person: "p1")])
        let people = [person("p1", color: 3)]
        XCTAssertEqual(m.speakerColor("화자 2", people: people), 3)
        XCTAssertEqual(m.speakerColor("화자 3", people: people), 2)
        XCTAssertEqual(m.speakerColor("화자 2", people: []), 1)  // forgotten person: by number
        m.merge("화자 3", into: "화자 2")
        XCTAssertEqual(m.speakerColor("화자 3", people: people), 3)
    }

    /// The letter in the dot: a name's first letter, or the speaker's number while unnamed.
    func testSpeakerMark() {
        XCTAssertEqual(SavedMeeting.speakerMark(name: "민재", key: "화자 1"), "민")
        XCTAssertEqual(SavedMeeting.speakerMark(name: "화자 2", key: "화자 2"), "2")
        XCTAssertEqual(SavedMeeting.speakerMark(name: "화자 12", key: "화자 12"), "12")
        XCTAssertEqual(SavedMeeting.speakerMark(name: "jisu", key: "화자 3"), "J")
        XCTAssertEqual(SavedMeeting.speakerMark(name: "지수", key: nil), "지")
    }

    /// A question line shows the answer found for it (same words); other lines and unanswered questions show none.
    func testFoundAnswerForQuestionLine() {
        var m = meeting([line(1, "화자 1", "배포 언제예요?"), line(2, "화자 3", "재시도 몇 번 해요?", question: true),
                         line(3, "화자 2", "환불은 어떻게 해요?", question: true)])
        m.questions = [
            SavedQuestion(id: "v1", question: "재시도 몇 번 해요?", asker: nil, at: base, answer: "세 번이에요. [1]", cards: [],
                          cited: [1], unanswered: false, voice: "화자 3"),
            SavedQuestion(id: "v2", question: "환불은 어떻게 해요?", asker: nil, at: base, answer: nil, cards: [], cited: [],
                          unanswered: true, voice: "화자 2"),
        ]
        XCTAssertEqual(m.foundAnswer(for: m.transcript[1]), "세 번이에요. [1]")
        XCTAssertNil(m.foundAnswer(for: m.transcript[2]))
        XCTAssertNil(m.foundAnswer(for: m.transcript[0]))
        // a line joined with the words before it still finds its question
        m.transcript[1] = line(2, "화자 3", "그런데 재시도 몇 번 해요?", question: true)
        XCTAssertEqual(m.foundAnswer(for: m.transcript[1]), "세 번이에요. [1]")
    }

    /// The answer's first part for the line under a question: citations and links out, cut at a word, then "…".
    func testAnswerStart() {
        XCTAssertEqual(SavedMeeting.answerStart("세 번이에요. 1분, 5분, 30분 간격으로 다시 시도해요. [1]"), "세 번이에요. 1분, 5분, 30분 간격으로…")
        XCTAssertEqual(SavedMeeting.answerStart("세 번이에요 [1][2]"), "세 번이에요…")
        XCTAssertEqual(SavedMeeting.answerStart("**두 번**이에요 (https://example.com/a)"), "두 번이에요…")
    }

    // MARK: the namer

    private func named(_ id: String, _ name: String, color: Int = 0) -> Person {
        Person(id: id, name: name, vector: [], seconds: 0, meetings: 1, last: base, color: color)
    }

    /// Typing keeps only the people whose name has the letters (case folded), and the first of them is chosen.
    func testNamerFiltersAndHighlights() {
        let people = [named("p1", "지수"), named("p2", "현우"), named("p3", "Jina")]
        let others = [(key: "화자 1", name: "민재"), (key: "화자 4", name: "화자 4")]
        let r = namerMatches(query: "지", people: people, others: others)
        XCTAssertTrue(r.others.isEmpty)
        XCTAssertEqual(r.people.map(\.id), ["p1"])
        XCTAssertEqual(r.highlighted, "p1")
        let latin = namerMatches(query: "jI", people: people, others: others)
        XCTAssertEqual(latin.people.map(\.id), ["p3"])
        XCTAssertEqual(latin.highlighted, "p3")
        // a speaker of this meeting comes before the remembered people
        let speaker = namerMatches(query: "화자", people: people, others: others)
        XCTAssertEqual(speaker.others.map(\.key), ["화자 4"])
        XCTAssertEqual(speaker.highlighted, "화자 4")
        let none = namerMatches(query: "  서연 ", people: people, others: others)
        XCTAssertTrue(none.others.isEmpty && none.people.isEmpty)
        XCTAssertNil(none.highlighted)
    }

    /// The person the engine says the voice sounds closest to goes to the top of the remembered people.
    func testNamerPutsNearFirst() {
        let people = [named("p1", "지수"), named("p2", "현우"), named("p3", "현아")]
        XCTAssertEqual(namerMatches(query: "", near: "p2", people: people, others: []).people.map(\.id), ["p2", "p1", "p3"])
        let typed = namerMatches(query: "현", near: "p3", people: people, others: [])
        XCTAssertEqual(typed.people.map(\.id), ["p3", "p2"])
        XCTAssertEqual(typed.highlighted, "p3")
        XCTAssertEqual(namerMatches(query: "", near: "gone", people: people, others: []).people.map(\.id), ["p1", "p2", "p3"])
    }

    func testNamerEmptyQueryHighlightsNothing() {
        let r = namerMatches(query: " ", near: "p1", people: [named("p1", "지수")], others: [(key: "화자 1", name: "민재")])
        XCTAssertEqual(r.others.map(\.key), ["화자 1"])
        XCTAssertEqual(r.people.map(\.id), ["p1"])
        XCTAssertNil(r.highlighted)
    }

    /// The calendar's invited people sit between the meeting's speakers and the remembered people (docs/35 3-4).
    func testInvitedListedBetweenOthersAndPeople() {
        let r = namerMatches(query: "", people: [named("p1", "지수")], others: [(key: "화자 1", name: "민재")],
                             invited: ["정하늘", "윤가람"])
        XCTAssertEqual(r.others.map(\.key), ["화자 1"])
        XCTAssertEqual(r.invited, ["정하늘", "윤가람"])
        XCTAssertEqual(r.people.map(\.id), ["p1"])
        XCTAssertNil(r.highlighted)
        // typed: a speaker first, then an invited name, then a person
        XCTAssertEqual(namerMatches(query: "하늘", people: [], others: [], invited: ["정하늘"]).highlighted, "invited:정하늘")
        XCTAssertEqual(namerMatches(query: "정", people: [named("p9", "정우")], others: [], invited: ["정하늘"]).highlighted,
                       "invited:정하늘")
    }

    /// Review I3: the person the voice sounds closest to stays the default over an invited name that also fits.
    func testNearPersonBeatsInvitedName() {
        let r = namerMatches(query: "민", near: "p1", people: [named("p1", "민호")], others: [], invited: ["민지"])
        XCTAssertEqual(r.highlighted, "p1")
        // without a near match the invited name still comes before the people
        XCTAssertEqual(namerMatches(query: "민", people: [named("p1", "민호")], others: [], invited: ["민지"]).highlighted, "invited:민지")
    }

    /// An invited name a speaker already carries, or a remembered person already has, is not offered twice.
    func testInvitedHidesNamesAlreadyUsed() {
        let r = namerMatches(query: "", people: [named("p1", "지수")], others: [(key: "화자 1", name: "민재")],
                             invited: ["민재", "지수", "정하늘"])
        XCTAssertEqual(r.invited, ["정하늘"])
    }

    func testInvitedFilteredByQuery() {
        XCTAssertEqual(namerMatches(query: "가", people: [], others: [], invited: ["정하늘", "윤가람"]).invited, ["윤가람"])
        XCTAssertEqual(namerMatches(query: "", people: [], others: []).invited, [])
    }

    /// The namer offers the meeting's other speakers, and remembered people no speaker of this meeting is already.
    func testNamerOffersOthersAndUnusedPeople() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 2"), line(3, "화자 4"), line(4, "화자 3")],
                        voices: [VoiceEntry(key: "화자 1", name: "민재", person: "p1")])
        m.merge("화자 3", into: "화자 1")
        XCTAssertEqual(m.namerOthers(for: "화자 2").map(\.key), ["화자 1", "화자 4"])
        XCTAssertEqual(m.namerOthers(for: "화자 2").map(\.name), ["민재", "화자 4"])
        XCTAssertEqual(m.namerOthers(for: "화자 3").map(\.key), ["화자 2", "화자 4"])  // merged: it is 화자 1
        XCTAssertEqual(m.namerPeople([named("p1", "민재"), named("p2", "지수")]).map(\.id), ["p2"])
    }

    /// Merging a named speaker into an unnamed one keeps the name: the typed name never disappears.
    func testMergeCarriesNameOntoUnnamedTarget() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 2")], voices: [VoiceEntry(key: "화자 2", name: "지수", person: "p1")])
        m.merge("화자 2", into: "화자 1")
        XCTAssertEqual(m.displayName(key: "화자 1"), "지수")
        XCTAssertEqual(m.displayName(key: "화자 2"), "지수")
        XCTAssertEqual(m.voices?.first { $0.key == "화자 1" }?.person, "p1")
        // a target with a name of its own keeps it
        var n = meeting([line(1, "화자 1"), line(2, "화자 2")],
                        voices: [VoiceEntry(key: "화자 1", name: "민재"), VoiceEntry(key: "화자 2", name: "지수", person: "p1")])
        n.merge("화자 2", into: "화자 1")
        XCTAssertEqual(n.displayName(key: "화자 2"), "민재")
        XCTAssertNil(n.voices?.first { $0.key == "화자 1" }?.person)
    }

    /// A speaker linked to a remembered person and given a different typed name was the wrong person: the link goes.
    /// The same name keeps it, and choosing a remembered person sets it.
    func testTypedNameClearsAWrongPerson() {
        let people = [named("p1", "지수"), named("p2", "현우")]
        var m = meeting([line(1, "화자 2")], voices: [VoiceEntry(key: "화자 2", name: "지수", person: "p1")])
        m.renameTyped(key: "화자 2", to: "지수", people: people)
        XCTAssertEqual(m.voices?.first?.person, "p1")
        m.renameTyped(key: "화자 2", to: "서연", people: people)
        XCTAssertEqual(m.displayName(key: "화자 2"), "서연")
        XCTAssertNil(m.voices?.first?.person)
        m.rename(key: "화자 2", to: "현우", person: "p2")
        XCTAssertEqual(m.voices?.first?.person, "p2")
    }

    /// A line named on its own after another speaker (or a remembered person) shows in their colour, as that speaker.
    func testLineNamedAfterSomeoneTakesTheirColour() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 2"), line(3, "화자 2"), line(4, "화자 2")],
                        voices: [VoiceEntry(key: "화자 1", name: "민재")])
        m.renameLine("e0:s1u2", to: "민재")
        m.renameLine("e0:s1u3", to: "현우")
        let people = [named("p2", "현우", color: 4)]
        let t = m.turns
        XCTAssertEqual(t.map(\.name), ["민재", "현우", "화자 2"])
        XCTAssertEqual(m.turnLook(t[0], people: people).key, "화자 1")
        XCTAssertEqual(m.turnLook(t[0], people: people).color, 0)
        XCTAssertEqual(m.turnLook(t[1], people: people).color, 4)
        XCTAssertEqual(m.turnLook(t[2], people: people).key, "화자 2")
        XCTAssertEqual(m.turnLook(t[2], people: people).color, 1)
        XCTAssertEqual(m.turnLook(t[1], people: people).key, "@현우")
        m.renameLine("e0:s1u4", to: "서연")  // nobody's name: a new person, in the colour after the speakers' and 현우's
        XCTAssertEqual(m.turnLook(m.turns[2], people: people).key, "@서연")
        XCTAssertEqual(m.turnLook(m.turns[2], people: people).color, 3)
    }

    /// Two people the voices heard as one: a line named on its own after nobody in the meeting is a person of their own,
    /// with their share taken from the speaker who said it.
    func testLineNamedAfterNobodyIsAnAttendee() {
        var m = meeting([line(1, "화자 1", "가나다라"), line(2, "화자 2", "가나다라"), line(3, "화자 2", "가나")],
                        voices: [VoiceEntry(key: "화자 2", name: "민재")])
        m.renameLine("e0:s1u3", to: "지수")
        XCTAssertEqual(m.attendees.map(\.key), ["화자 1", "화자 2", "@지수"])
        XCTAssertEqual(m.attendees.map(\.name), ["화자 1", "민재", "지수"])
        XCTAssertEqual(m.attendees.map(\.share), [40, 40, 20])
        XCTAssertEqual(m.lines(of: "@지수"), ["e0:s1u3"])
        XCTAssertEqual(m.namerOthers(for: "화자 1").map(\.name), ["민재", "지수"])  // offered for the next turn
        XCTAssertEqual(m.ownerKey("지수"), "지수")  // a to-do owner stays a plain name
        XCTAssertEqual(MeetingStats.speakers(m).all, 3)
        XCTAssertEqual(MeetingStats.speakers(m).named, 2)
        m.renameLine("e0:s1u3", to: "민재")  // its own speaker's name gives the line back
        XCTAssertNil(m.transcript[2].name)
        XCTAssertEqual(m.attendees.count, 2)
    }

    /// Naming lines after someone who exists only on lines joins them; a speaker merged into them takes the name.
    func testLinePersonJoinsAndMerges() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 1"), line(3, "화자 2")])
        m.renameLine("e0:s1u2", to: "지수")
        m.renameLine("e0:s1u3", to: "지수")
        XCTAssertEqual(m.lines(of: "@지수"), ["e0:s1u2", "e0:s1u3"])
        XCTAssertEqual(m.namerTarget(turn: m.turns[1]), NamerTarget(key: "@지수", lines: ["e0:s1u2", "e0:s1u3"]))
        XCTAssertFalse(m.speaks("@지수", beyond: ["e0:s1u2", "e0:s1u3"]))
        m.merge("화자 2", into: "@지수")
        XCTAssertEqual(m.displayName(key: "화자 2"), "지수")
        XCTAssertNil(m.transcript[2].name)  // now its own speaker's name
        XCTAssertEqual(m.attendees.map(\.key), ["화자 1", "화자 2"])
        XCTAssertEqual(m.lines(of: "화자 2"), ["e0:s1u2", "e0:s1u3"])
    }

    /// A line named after a speaker follows the speaker's new name instead of splitting off.
    func testLinesFollowARenamedSpeaker() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 2"), line(3, "화자 3")])
        m.renameLine("e0:s1u2", to: "화자 3")
        m.rename(key: "화자 3", to: "현우", person: nil)
        XCTAssertEqual(m.transcript[1].name, "현우")
        XCTAssertEqual(m.attendees.map(\.key), ["화자 1", "화자 3"])
        m.merge("화자 3", into: "화자 1")
        XCTAssertEqual(m.transcript[1].name, "현우")  // 화자 1 had no name: 현우 carried over
        XCTAssertEqual(m.attendees.map(\.key), ["화자 1"])
    }

    /// A turn's name names its first line's speaker (after merges), even when the turn mixes voices under one name,
    /// and carries the turn's lines for 이 발언만. 전체 and 이 발언만 differ only when the speaker spoke elsewhere too.
    func testNamerTargets() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 2"), line(3, "화자 2"), line(4, "화자 4")],
                        voices: [VoiceEntry(key: "화자 1", name: "민재")])
        m.renameLine("e0:s1u2", to: "민재")  // joins 민재's turn: the turn holds 화자 1 and 화자 2
        m.merge("화자 4", into: "화자 2")
        let t = m.turns
        XCTAssertEqual(t[0].lines.map(\.key), ["e0:s1u1", "e0:s1u2"])
        XCTAssertEqual(m.namerTarget(turn: t[0]), NamerTarget(key: "화자 1", lines: ["e0:s1u1", "e0:s1u2"]))
        XCTAssertEqual(m.namerTarget(turn: t[1]), NamerTarget(key: "화자 2", lines: ["e0:s1u3", "e0:s1u4"]))  // merged
        XCTAssertFalse(m.speaks("화자 1", beyond: ["e0:s1u1", "e0:s1u2"]))
        XCTAssertFalse(m.speaks("화자 2", beyond: ["e0:s1u3", "e0:s1u4"]))
        XCTAssertTrue(m.speaks("화자 2", beyond: ["e0:s1u3"]))
        let old = meeting([line(1, nil)])
        XCTAssertNil(old.namerTarget(turn: old.turns[0]))
    }

    /// The meeting page's to-do owner: an in-person meeting saved before speakers showed no "me"/"them" owner, and still
    /// does not. A remote one shows 나/상대, and a speaker key shows its name in either.
    func testShownOwnerHidesChannelsInPersonOnly() throws {
        let json = #"""
        {"version": 1, "id": "20261023-140000", "mode": "inperson", "start": "2026-10-23T05:00:00Z", "end": "2026-10-23T05:47:00Z",
         "title": "주간 회의", "titleEdited": false, "notes": "done", "summary": [],
         "ledger": [{"kind": "commitment", "text": "PR 올리기", "owner": "me", "done": false},
                    {"kind": "commitment", "text": "배포 확인", "owner": "them", "done": false}],
         "questions": [],
         "transcript": [{"key": "e0:s1u1", "at": "2026-10-23T05:06:00Z", "speaker": "me", "text": "제가 올릴게요", "question": false}]}
        """#
        let d = JSONDecoder()
        d.dateDecodingStrategy = .iso8601
        let old = try d.decode(SavedMeeting.self, from: Data(json.utf8))
        XCTAssertNil(old.shownOwner(old.ledger[0].owner!))
        XCTAssertNil(old.shownOwner(old.ledger[1].owner!))
        XCTAssertEqual(old.shownOwner("팀장"), "팀장")

        var remote = SavedMeeting(mode: "remote", start: base)
        remote.transcript = [line(1, "화자 2")]
        XCTAssertEqual(remote.shownOwner("me"), "나")
        XCTAssertEqual(remote.shownOwner("them"), "상대")

        var now = meeting([line(1, "화자 2")])
        now.rename(key: "화자 2", to: "지수", person: nil)
        XCTAssertEqual(now.shownOwner("화자 2"), "지수")
    }

    /// Notes written again from the transcript name owners by their shown name: a name some speaker of the meeting shows
    /// goes back to that speaker's key (after merges), anything else stays as it is.
    func testOwnerKeyMapsShownNamesBack() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 2"), line(3, "화자 4")])
        m.rename(key: "화자 1", to: "민재", person: nil)
        m.rename(key: "화자 4", to: "지수", person: nil)
        m.merge("화자 4", into: "화자 2")  // 화자 2 takes 지수
        XCTAssertEqual(m.ownerKey("민재"), "화자 1")
        XCTAssertEqual(m.ownerKey("지수"), "화자 2")
        XCTAssertEqual(m.ownerKey("화자 3"), "화자 3")
        XCTAssertEqual(m.ownerKey("팀장"), "팀장")
        XCTAssertEqual(m.ownerKey("me"), "me")
        // the engine's own notes already give keys: they stay
        XCTAssertEqual(m.ownerKey("화자 1"), "화자 1")
    }

    /// A speaker linked to a person who was forgotten loses the link; its name stays.
    func testUnlinkForgottenPeople() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 2")],
                        voices: [VoiceEntry(key: "화자 1", name: "민재", person: "p1"),
                                 VoiceEntry(key: "화자 2", name: "지수", person: "p2")])
        XCTAssertEqual(m.unlink(keeping: [named("p1", "민재")]), ["화자 2"])
        XCTAssertEqual(m.voices, [VoiceEntry(key: "화자 1", name: "민재", person: "p1"),
                                  VoiceEntry(key: "화자 2", name: "지수", person: nil)])
        XCTAssertEqual(m.unlink(keeping: [named("p1", "민재")]), [])
        XCTAssertEqual(m.unlink(keeping: []), ["화자 1"])
        XCTAssertEqual(m.displayName(key: "화자 1"), "민재")
    }

    /// A session that cannot open with the remembered people opens without them: they are a help, never a reason to
    /// lose the meeting. Without people there is nothing to drop, and the error stands.
    func testOpenSessionRetriesWithoutPeople() async throws {
        struct Refused: Error {}
        let people: [(id: String, vector: [Float])] = [("p1", [1, 0])]
        var asked: [Int] = []
        let id = try await openSession(people: people) { p in
            asked.append(p.count)
            if !p.isEmpty { throw Refused() }
            return "s2"
        }
        XCTAssertEqual(id, "s2")
        XCTAssertEqual(asked, [1, 0])

        asked = []
        let first = try await openSession(people: people) { p in asked.append(p.count); return "s1" }
        XCTAssertEqual(first, "s1")
        XCTAssertEqual(asked, [1])

        asked = []
        do {
            _ = try await openSession(people: []) { p in asked.append(p.count); throw Refused() }
            XCTFail("expected the error")
        } catch is Refused {}
        XCTAssertEqual(asked, [0])
    }

    func testParticleRoFollowsTheLastSound() {
        XCTAssertEqual(Korean.withRo("민재"), "민재로")
        XCTAssertEqual(Korean.withRo("민준"), "민준으로")
        XCTAssertEqual(Korean.withRo("서울"), "서울로")
        XCTAssertEqual(Korean.withRo("화자 2"), "화자 2로")
        XCTAssertEqual(Korean.withRo("화자 3"), "화자 3으로")
        XCTAssertEqual(Korean.withRo("화자 7"), "화자 7로")
        XCTAssertEqual(Korean.withRo("Alex"), "Alex로")
    }

    // MARK: what the namer says (NamingText, the Mac and the iPhone)

    func testNamingTextFitsTheParticles() {
        XCTAssertEqual(NamingText.done(.typed("민호"), old: "화자 2", new: "민호", lineOnly: false), "화자 2를 민호로 바꿨어요")
        XCTAssertEqual(NamingText.done(.typed(" 지윤 "), old: "민준", new: " 지윤 ", lineOnly: false), "민준을 지윤으로 바꿨어요")
        XCTAssertEqual(NamingText.done(.speaker("화자 1"), old: "화자 3", new: "서연", lineOnly: false), "화자 3을 서연과 합쳤어요")
        XCTAssertEqual(NamingText.done(.speaker("화자 1"), old: "지윤", new: "민호", lineOnly: false), "지윤을 민호와 합쳤어요")
        XCTAssertEqual(NamingText.done(.typed("서연"), old: "화자 2", new: "서연", lineOnly: true), "이 말을 서연이 한 말로 바꿨어요")
        XCTAssertEqual(NamingText.done(.speaker("화자 1"), old: "화자 2", new: "민호", lineOnly: true), "이 말을 민호가 한 말로 바꿨어요")
    }

    /// The iPhone namer's line above its button and the button itself, particles fitted to the quoted name.
    func testNamingPreviewAndButtonFitTheParticles() {
        XCTAssertEqual(NamingText.preview(speaker: "화자 1", lines: 48, target: "나", merge: false, lineOnly: false),
                       "화자 1의 말 48줄이 모두 \"나\"로 바뀌어요.")
        XCTAssertEqual(NamingText.preview(speaker: "화자 1", lines: 2, target: "민준", merge: false, lineOnly: false),
                       "화자 1의 말 2줄이 모두 \"민준\"으로 바뀌어요.")
        XCTAssertEqual(NamingText.preview(speaker: "화자 3", lines: 3, target: "나", merge: true, lineOnly: false),
                       "화자 3과 \"나\"를 한 사람으로 합쳐요. 화자 3의 말 3줄이 \"나\"로 바뀌어요.")
        XCTAssertEqual(NamingText.preview(speaker: "화자 2", lines: 5, target: "민준", merge: true, lineOnly: false),
                       "화자 2와 \"민준\"을 한 사람으로 합쳐요. 화자 2의 말 5줄이 \"민준\"으로 바뀌어요.")
        XCTAssertEqual(NamingText.preview(speaker: "화자 3", lines: 3, target: "나", merge: true, lineOnly: true),
                       "이 말 한 줄만 \"나\"가 한 말로 바꿔요. 화자 3의 다른 말은 그대로예요.")
        XCTAssertEqual(NamingText.button(target: "나", merge: false, lineOnly: false), "\"나\"로 바꾸기")
        XCTAssertEqual(NamingText.button(target: "지윤", merge: false, lineOnly: false), "\"지윤\"으로 바꾸기")
        XCTAssertEqual(NamingText.button(target: "나", merge: true, lineOnly: false), "\"나\"와 합치기")
        XCTAssertEqual(NamingText.button(target: "민준", merge: true, lineOnly: false), "\"민준\"과 합치기")
        XCTAssertEqual(NamingText.button(target: "나", merge: true, lineOnly: true), "\"나\"가 한 말로 바꾸기")
        XCTAssertEqual(NamingText.button(target: "서울", merge: false, lineOnly: true), "\"서울\"이 한 말로 바꾸기")
        XCTAssertEqual(Korean.ro("화자 3"), "으로")
        XCTAssertEqual(Korean.particle("화자 2", "과", "와"), "와")
    }

    /// The iPhone namer's rows: each speaker's line count and first line, after merges and lines named on their own.
    func testNamerLinesCountAndFirstLine() {
        var m = meeting([line(1, "화자 1", "처음"), line(2, "화자 2", "네"), line(3, "화자 1"), line(4, "화자 3", "응"), line(5, "화자 2")])
        XCTAssertEqual(m.namerLines(of: "화자 1").count, 2)
        XCTAssertEqual(m.namerLines(of: "화자 1").first?.text, "처음")
        XCTAssertEqual(m.namerLines(of: "화자 2").count, 2)
        m.merge("화자 3", into: "화자 2")
        XCTAssertEqual(m.namerLines(of: "화자 2").count, 3)
        XCTAssertEqual(m.namerLines(of: "화자 2").first?.text, "네")
        m.renameLine("e0:s1u2", to: "지수")
        XCTAssertEqual(m.namerLines(of: "화자 2").count, 2)
        XCTAssertEqual(m.namerLines(of: "화자 2").first?.key, "e0:s1u4")
        XCTAssertEqual(m.namerLines(of: SavedMeeting.linePersonKey("지수")).count, 1)
        XCTAssertEqual(m.namerLines(of: "화자 9").count, 0)
        XCTAssertNil(m.namerLines(of: "화자 9").first)
    }

    func testNamingFooter() {
        XCTAssertEqual(NamingText.footer(name: "화자 2", lineOnly: false, linePerson: false), "화자 2로 나온 말을 모두 바꿔요.")
        XCTAssertEqual(NamingText.footer(name: "지윤", lineOnly: false, linePerson: true), "지윤으로 이름 붙인 말을 모두 바꿔요.")
        XCTAssertEqual(NamingText.footer(name: "지윤", lineOnly: true, linePerson: false), "이 말만 바꿔요. 나머지는 그대로예요.")
    }

    /// The iPhone's namer on one line: 이 말만 renames the line and nothing else; 되돌리기 puts the names back.
    func testLineOnlyRenameAndUndo() {
        var m = meeting([line(1, "화자 1"), line(2, "화자 2"), line(3, "화자 1")], voices: [VoiceEntry(key: "화자 1", name: "민호")])
        let target = m.namerTarget(turn: SavedMeeting.Turn(key: "화자 1", name: "민호", at: base, lines: [m.transcript[2]], question: false))
        XCTAssertEqual(target, NamerTarget(key: "화자 1", lines: ["e0:s1u3"]))
        XCTAssertTrue(m.speaks("화자 1", beyond: ["e0:s1u3"]))
        let before = m.naming
        m.renameLine("e0:s1u3", to: "지윤")
        XCTAssertEqual(m.transcript.map { m.lineName($0) }, ["민호", "화자 2", "지윤"])
        XCTAssertEqual(m.displayName(key: "화자 1"), "민호")
        XCTAssertEqual(m.namerOthers(for: "화자 1").map(\.name), ["화자 2", "지윤"])
        m.restore(before)
        XCTAssertEqual(m.transcript.map { m.lineName($0) }, ["민호", "화자 2", "민호"])
    }
}
