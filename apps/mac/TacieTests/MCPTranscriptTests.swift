import XCTest

final class MCPTranscriptTests: XCTestCase {
    private let base = Date(timeIntervalSince1970: 1_800_000_000)

    private func line(_ i: Int, text: String? = nil, voice: String? = "화자 1") -> TranscriptLine {
        TranscriptLine(key: "k\(i)", at: base.addingTimeInterval(Double(i) * 60), speaker: nil, text: text ?? "line \(i)",
                       question: false, voice: voice)
    }

    private func meeting(lines: Int, ledger: [LedgerEntry] = []) -> SavedMeeting {
        var m = SavedMeeting(mode: "inperson", start: base)
        m.transcript = (0..<lines).map { line($0) }
        m.ledger = ledger
        return m
    }

    private func read(_ m: SavedMeeting, around: String? = nil, before: Int? = nil, after: Int? = nil,
                      offset: Int? = nil, limit: Int? = nil) -> [String: Any] {
        MCPTranscript.read(m, around: around, before: before, after: after, offset: offset, limit: limit,
                           name: { "N-" + ($0.voice ?? "") }, zone: TimeZone(identifier: "UTC")!)
    }

    private func keys(_ r: [String: Any]) -> [String] { (r["lines"] as! [[String: Any]]).map { $0["key"] as! String } }

    func testLineShape() {
        let r = read(meeting(lines: 3), limit: 1)
        let l = (r["lines"] as! [[String: Any]])[0]
        XCTAssertEqual(l["key"] as? String, "k0")
        XCTAssertEqual(l["speaker"] as? String, "N-화자 1")
        XCTAssertEqual(l["text"] as? String, "line 0")
        XCTAssertEqual((l["at"] as? String)?.count, 5)
        XCTAssertNil(l["translation"])
    }

    func testAroundLedgerTextUsesRefsWindow() {
        var e = LedgerEntry(kind: "decision", text: "Ship on Friday")
        e.refs = ["k20"]
        let r = read(meeting(lines: 40, ledger: [e]), around: "Ship on Friday")
        XCTAssertEqual(keys(r), (14...26).map { "k\($0)" })
        XCTAssertNil(r["next_offset"])
    }

    func testAroundLedgerContainsMatch() {
        var e = LedgerEntry(kind: "decision", text: "Ship on Friday for sure")
        e.refs = ["k10"]
        let r = read(meeting(lines: 40, ledger: [e]), around: "on friday for")
        XCTAssertEqual(keys(r).first, "k4")
    }

    func testAroundKeyCentersThere() {
        let r = read(meeting(lines: 40), around: "k20")
        XCTAssertEqual(keys(r), (14...26).map { "k\($0)" })
    }

    func testBeforeAfterClampTo20() {
        let r = read(meeting(lines: 100), around: "k50", before: 50, after: 2)
        XCTAssertEqual(keys(r), (30...52).map { "k\($0)" })
        let s = read(meeting(lines: 100), around: "k50", before: 0, after: 0)
        XCTAssertEqual(keys(s), ["k50"])
    }

    func testPagingReturnsNextOffsetUntilEnd() {
        let m = meeting(lines: 130)
        let p1 = read(m)
        XCTAssertEqual(keys(p1).count, 60)
        XCTAssertEqual(p1["next_offset"] as? Int, 60)
        let p2 = read(m, offset: 60, limit: 500)
        XCTAssertEqual(keys(p2).first, "k60")
        XCTAssertEqual(keys(p2).count, 60)
        XCTAssertEqual(p2["next_offset"] as? Int, 120)
        let p3 = read(m, offset: 120)
        XCTAssertEqual(keys(p3).count, 10)
        XCTAssertNil(p3["next_offset"])
    }

    func testSoundOnlyLinesAreDropped() {
        var m = meeting(lines: 4)
        m.transcript[1] = line(1, text: "[pause] [silence]")
        let r = read(m)
        XCTAssertEqual(keys(r), ["k0", "k2", "k3"])
    }

    func testUnknownAroundFallsBackToPagingMarkedNotFound() {
        let r = read(meeting(lines: 5), around: "nothing like it")
        XCTAssertEqual(keys(r).count, 5)
        XCTAssertEqual(r["found"] as? Bool, false)
    }

    func testFoundMarkerOnlyOnMiss() {
        XCTAssertNil(read(meeting(lines: 5), around: "k2")["found"])
        XCTAssertNil(read(meeting(lines: 5))["found"])
    }

    func testKeyWinsOverLedgerTextThatContainsIt() {
        var e = LedgerEntry(kind: "decision", text: "see k30 later")
        e.refs = ["k5"]
        let r = read(meeting(lines: 40, ledger: [e]), around: "k30", before: 0, after: 0)
        XCTAssertEqual(keys(r), ["k30"])
    }

    func testAroundWordsFromTranscriptFindsBestLine() {
        var m = meeting(lines: 40)
        m.transcript[17] = line(17, text: "let's retry the upload three times then give up")
        m.transcript[25] = line(25, text: "three is fine")
        let r = read(m, around: "why retry three times", before: 1, after: 1)
        XCTAssertEqual(keys(r), ["k16", "k17", "k18"])
        XCTAssertNil(r["found"])
    }

    func testAroundOneSharedWordIsNotAMatch() {
        var m = meeting(lines: 10)
        m.transcript[7] = line(7, text: "the upload failed")
        let r = read(m, around: "upload policy")
        XCTAssertEqual(r["found"] as? Bool, false)
        XCTAssertEqual(keys(r).first, "k0")
    }

    func testLedgerEntryWithoutRefsUsesItsOwnWords() {
        var m = meeting(lines: 30)
        m.transcript[12] = line(12, text: "we keep the cache for seven days")
        m.ledger = [LedgerEntry(kind: "decision", text: "Keep the cache seven days")]
        let r = read(m, around: "Keep the cache seven days", before: 0, after: 0)
        XCTAssertEqual(keys(r), ["k12"])
    }

    func testMultiRefWindowCutAt60IsMarkedTruncated() {
        var e = LedgerEntry(kind: "decision", text: "Big one")
        e.refs = ["k20", "k60", "k100"]
        let r = read(meeting(lines: 130, ledger: [e]), around: "Big one", before: 20, after: 20)
        XCTAssertEqual(keys(r).count, 60)
        XCTAssertEqual(r["truncated"] as? Bool, true)
        let s = read(meeting(lines: 40, ledger: [e]), around: "k20")
        XCTAssertNil(s["truncated"])
    }
}
