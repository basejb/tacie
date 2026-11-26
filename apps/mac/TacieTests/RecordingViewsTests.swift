import XCTest

/// The meeting page's recording pieces that are plain functions: time labels, where unheard stretches sit among the
/// lines, which time a shown word plays from, and turns broken at an unheard stretch.
final class RecordingViewsTests: XCTestCase {
    private func line(_ key: String, _ text: String = "말", voice: String? = nil, audioAt: Int? = nil,
                      words: [SpokenWord]? = nil) -> TranscriptLine {
        TranscriptLine(key: key, at: Date(timeIntervalSince1970: 0), speaker: nil, text: text, question: false,
                       voice: voice, audioAt: audioAt, words: words)
    }

    private func kinds(_ rows: [TranscriptRow]) -> [String] {
        rows.map {
            switch $0 {
            case .line(let l): return l.key
            case .unheard(let s): return "u\(s.start)"
            }
        }
    }

    // MARK: timeLabel

    func testTimeLabel() {
        XCTAssertEqual(RecordingPlayer.timeLabel(0), "0:00")
        XCTAssertEqual(RecordingPlayer.timeLabel(372_000), "6:12")
        XCTAssertEqual(RecordingPlayer.timeLabel(3_723_000), "1:02:03")
        XCTAssertEqual(RecordingPlayer.timeLabel(42_999), "0:42")  // a word at 42.999 s reads as 0:42
        XCTAssertEqual(RecordingPlayer.timeLabel(-5), "0:00")
    }

    // MARK: rows

    func testUnheardSitsBetweenTheLinesAroundIt() {
        let rows = TranscriptRow.rows(lines: [line("a", audioAt: 0), line("b", audioAt: 6000)],
                                      unheard: [AudioSpan(start: 3000, end: 5000)])
        XCTAssertEqual(kinds(rows), ["a", "u3000", "b"])
    }

    func testLinesWithoutAPlaceKeepTheirOrder() {
        let rows = TranscriptRow.rows(lines: [line("a", audioAt: 0), line("x"), line("b", audioAt: 6000), line("y")],
                                      unheard: [AudioSpan(start: 3000, end: 5000)])
        XCTAssertEqual(kinds(rows), ["a", "x", "u3000", "b", "y"])
    }

    func testUnheardAfterTheLastLineAndInOrder() {
        let rows = TranscriptRow.rows(lines: [line("a", audioAt: 1000)],
                                      unheard: [AudioSpan(start: 9000, end: 9500), AudioSpan(start: 0, end: 500)])
        XCTAssertEqual(kinds(rows), ["u0", "a", "u9000"])
    }

    func testNoUnheardIsTheLines() {
        XCTAssertEqual(kinds(TranscriptRow.rows(lines: [line("a"), line("b")], unheard: [])), ["a", "b"])
    }

    // MARK: word times

    func testShownWordsTakeTheirSpokenWordsTime() {
        let l = line("a", "우선 인증 쪽은", audioAt: 1000, words: [
            SpokenWord(text: "우선", start: 1000, end: 1200),
            SpokenWord(text: "인증", start: 1300, end: 1500),
            SpokenWord(text: "쪽은", start: 1600, end: 1800),
        ])
        XCTAssertEqual(l.shownWords.map(\.text), ["우선", "인증", "쪽은"])
        XCTAssertEqual(l.shownWords.map(\.ms), [1000, 1300, 1600])
    }

    func testUnpairedShownWordsTakeTheWordBefore() {
        let l = line("a", "하나 둘 셋 넷", audioAt: 500, words: [
            SpokenWord(text: "하나", start: 700, end: 800),
            SpokenWord(text: "둘", start: 900, end: 1000),
        ])
        XCTAssertEqual(l.shownWords.map(\.ms), [700, 900, 900, 900])
    }

    func testALineWithoutWordsPlaysFromItsStart() {
        XCTAssertEqual(line("a", "한 줄  글", audioAt: 4000).shownWords.map(\.ms), [4000, 4000, 4000])
        XCTAssertEqual(line("a", "한 줄").shownWords.map(\.ms), [nil, nil])
        XCTAssertEqual(line("a", "한 줄", audioAt: 4000, words: []).shownWords.map(\.ms), [4000, 4000])
    }

    func testCurrentShownWordStaysInsideTheLine() {
        let l = line("a", "하나 둘", audioAt: 0, words: [
            SpokenWord(text: "하나", start: 0, end: 1), SpokenWord(text: "둘", start: 2, end: 3), SpokenWord(text: "셋", start: 4, end: 5),
        ])
        XCTAssertEqual(l.shownIndex(ofWord: 1), 1)
        XCTAssertEqual(l.shownIndex(ofWord: 2), 1)  // more spoken words than shown ones: the last shown one
        XCTAssertNil(l.shownIndex(ofWord: -1))  // the line, not a word
    }

    // MARK: turns broken at unheard stretches

    func testTurnsBreakAtUnheard() {
        var m = SavedMeeting(mode: "remote", start: Date(timeIntervalSince1970: 0))
        m.transcript = [line("a", voice: "화자 1", audioAt: 0), line("b", voice: "화자 1", audioAt: 6000),
                        line("c", voice: "화자 1", audioAt: 8000)]
        let pieces = m.transcriptPieces(unheard: [AudioSpan(start: 3000, end: 5000)])
        let shape: [String] = pieces.map {
            switch $0 {
            case .turn(let t): return t.lines.map(\.key).joined()
            case .unheard(let s): return "u\(s.start)"
            }
        }
        XCTAssertEqual(shape, ["a", "u3000", "bc"])
        XCTAssertEqual(m.transcriptPieces(unheard: []).count, 1)
    }
}

@MainActor
final class PlaybackFocusTests: XCTestCase {
    func testOnlyTheLinesLosingAndGettingTheWordChange() {
        let f = PlaybackFocus()
        let a = f.line("a"), b = f.line("b"), c = f.line("c")
        f.set(CurrentWord(line: "a", word: 0))
        XCTAssertEqual(a.word, 0)
        f.set(CurrentWord(line: "a", word: 1))
        XCTAssertEqual(a.word, 1)
        f.set(CurrentWord(line: "b", word: 0))
        XCTAssertNil(a.word)
        XCTAssertEqual(b.word, 0)
        XCTAssertNil(c.word)
        XCTAssertTrue(f.line("b") === b)
        f.set(nil)
        XCTAssertNil(b.word)
        XCTAssertNil(f.current)
    }
}
