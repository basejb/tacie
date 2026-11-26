import XCTest
@testable import Tacie

final class LiveLineTests: XCTestCase {
    private func meeting() -> SavedMeeting {
        var m = SavedMeeting(mode: "inperson", start: Date(timeIntervalSince1970: 0))
        m.transcript = [TranscriptLine(key: "e0:s1u1", at: Date(timeIntervalSince1970: 10), speaker: nil, text: "앞에서 한 말", question: false)]
        return m
    }

    func testTheSentenceBeingSaidEndsTheTranscript() {
        let doc = TranscriptDoc.make(meeting: meeting(), people: [], calendar: .current, unheard: [], scale: 1,
                                     live: LiveLine(said: "예전에", partial: " 왜 이렇게"))
        XCTAssertTrue(doc.text.string.hasSuffix("예전에 왜 이렇게"))
        // the part not final yet is lighter than the rest
        let s = doc.text.string as NSString
        let said = (doc.text.attribute(.foregroundColor, at: s.range(of: "예전에").location, effectiveRange: nil) as? NSColor)?.alphaComponent ?? 0
        let partial = (doc.text.attribute(.foregroundColor, at: s.range(of: "이렇게").location, effectiveRange: nil) as? NSColor)?.alphaComponent ?? 1
        XCTAssertGreaterThan(said, partial)
    }

    func testNoLiveLineLeavesTheTranscriptAsItWas() {
        let a = TranscriptDoc.make(meeting: meeting(), people: [], calendar: .current, unheard: [], scale: 1)
        let b = TranscriptDoc.make(meeting: meeting(), people: [], calendar: .current, unheard: [], scale: 1, live: LiveLine(said: "", partial: ""))
        XCTAssertEqual(a.text.string, b.text.string)
    }

    /// The sentence being said continues the last line, so it stays in place when its line arrives (no jump down a
    /// paragraph and back).
    func testTheSentenceContinuesTheLastLine() {
        let doc = TranscriptDoc.make(meeting: meeting(), people: [], calendar: .current, unheard: [], scale: 1,
                                     live: LiveLine(said: "예전에", partial: " 왜"))
        XCTAssertTrue(doc.text.string.hasSuffix("앞에서 한 말 예전에 왜"), doc.text.string)
    }

    /// Sentences Soniox finished but whose lines have not come back stay, before the one being said.
    func testHeldSentencesComeFirst() {
        let line = LiveLine(held: ["끝난 문장."], said: "", partial: " 다음")
        XCTAssertFalse(line.isEmpty)
        let doc = TranscriptDoc.make(meeting: meeting(), people: [], calendar: .current, unheard: [], scale: 1, live: line)
        XCTAssertTrue(doc.text.string.hasSuffix("앞에서 한 말 끝난 문장. 다음"), doc.text.string)
    }
}
