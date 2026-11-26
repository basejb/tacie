import XCTest

/// What the app sends the engine of a finished meeting (docs/30).
final class MeetingDigestTests: XCTestCase {
    private let seoul = TimeZone(identifier: "Asia/Seoul")!
    private lazy var cal: Calendar = {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = seoul
        return c
    }()

    private func start() -> Date {
        var c = DateComponents(year: 2026, month: 11, day: 3, hour: 14, minute: 2, second: 10)
        c.timeZone = seoul
        return Calendar(identifier: .gregorian).date(from: c)!
    }

    private func done() -> SavedMeeting {
        var m = SavedMeeting(mode: "remote", start: start())
        m.end = start().addingTimeInterval(40 * 60)
        m.title = "결제 화면 정리"
        m.notes = .done
        m.summary = ["카드 결제만 먼저 내기로 했어요"]
        return m
    }

    func test_digest_skips_live_and_unfinished() {
        var live = done()
        live.end = nil
        XCTAssertNil(live.digest(calendar: cal))
        for notes in [SavedMeeting.Notes.waiting, .failed, .off] {
            var m = done()
            m.notes = notes
            XCTAssertNil(m.digest(calendar: cal), "\(notes)")
        }
        var empty = done()
        empty.summary = []
        XCTAssertNil(empty.digest(calendar: cal))
        empty.ledger = [LedgerEntry(kind: "decision", text: "카드 결제만 먼저 내요")]
        XCTAssertNotNil(empty.digest(calendar: cal))
    }

    func test_digest_fields() throws {
        var m = done()
        m.topics = [NoteTopic(title: "결제 버튼", points: ["버튼을 아래로 옮겨요"])]
        m.ledger = [LedgerEntry(kind: "commitment", text: "시안을 다시 그려요", owner: "지윤", due: "금요일까지", done: true)]
        let d = try XCTUnwrap(m.digest(calendar: cal))
        // the id is the meeting's own (made from the start in the Mac's time zone); CI runs in UTC (2026-11-26)
        XCTAssertEqual(d.id, m.id)
        XCTAssertEqual(d.title, "결제 화면 정리")
        XCTAssertEqual(d.start, "2026-11-03T14:02:10+09:00")
        XCTAssertEqual(d.end, "2026-11-03T14:42:10+09:00")
        XCTAssertEqual(d.summary, ["카드 결제만 먼저 내기로 했어요"])
        XCTAssertEqual(d.topics, [MeetingDigest.Topic(title: "결제 버튼", points: ["버튼을 아래로 옮겨요"])])
        XCTAssertEqual(d.ledger, [MeetingDigest.Entry(kind: "commitment", text: "시안을 다시 그려요", owner: "지윤",
                                                      due: "금요일까지", done: true)])
    }

    func test_digest_resolves_speaker_refs() throws {
        var m = done()
        m.voices = [VoiceEntry(key: "화자 2", name: "지윤", person: nil, same: nil)]
        m.summary = ["[[화자 2]]가 시안을 맡기로 했어요"]
        m.ledger = [LedgerEntry(kind: "commitment", text: "[[화자 2]]가 시안을 그려요", owner: "화자 2")]
        let d = try XCTUnwrap(m.digest(calendar: cal))
        XCTAssertEqual(d.summary, ["지윤이 시안을 맡기로 했어요"])
        XCTAssertEqual(d.ledger.first?.text, "지윤이 시안을 그려요")
        XCTAssertEqual(d.ledger.first?.owner, "지윤")
        XCTAssertFalse(String(describing: d).contains("[["))
    }

    func test_untitled_meeting_uses_its_shown_title() throws {
        var m = done()
        m.title = nil
        let d = try XCTUnwrap(m.digest(calendar: cal))
        XCTAssertEqual(d.title, m.displayTitle(calendar: cal))
        XCTAssertFalse(d.title.isEmpty)
    }

    func test_digest_encodes_engine_keys() throws {
        var m = done()
        m.ledger = [LedgerEntry(kind: "decision", text: "카드 결제만 먼저 내요")]
        let json = try XCTUnwrap(m.digest(calendar: cal)).json
        XCTAssertEqual(Set(json.keys), ["id", "title", "start", "end", "summary", "topics", "ledger"])
        let entry = try XCTUnwrap((json["ledger"] as? [[String: Any]])?.first)
        XCTAssertEqual(Set(entry.keys).subtracting(["owner", "due"]), ["kind", "text", "done"])
    }
}
