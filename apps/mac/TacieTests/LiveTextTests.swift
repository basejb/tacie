import XCTest
@testable import Tacie

/// The iPhone's live meeting text (LiveText): Soniox's echo of a cut sentence is hidden only until its `<fin>`.
final class LiveTextTests: XCTestCase {
    private func held(_ text: String, awaitingFin: Bool, id: Int = 1) -> LiveText.Held {
        LiveText.Held(id: id, text: text, awaitingFin: awaitingFin)
    }

    /// After `<fin>` the next speaker's "네" is a new sentence, not an echo of "네 맞아요": it shows.
    func testNewSpeechAfterFinShowsEvenWhenItRepeatsTheHeldOne() {
        let r = LiveText.compose(held: [held("네 맞아요", awaitingFin: false)], said: "네", partial: "")
        XCTAssertEqual(r.final, "네 맞아요 네")
        let grey = LiveText.compose(held: [held("네 맞아요", awaitingFin: false)], said: "", partial: "맞아요")
        XCTAssertEqual(grey.partial, "맞아요")
    }

    /// Before `<fin>` Soniox re-sends the cut sentence: it stays hidden.
    func testEchoBeforeFinIsHidden() {
        let r = LiveText.compose(held: [held("네 맞아요", awaitingFin: true)], said: "네 맞아요", partial: "")
        XCTAssertEqual(r.final, "네 맞아요")
        let tail = LiveText.compose(held: [held("네 맞아요", awaitingFin: true)], said: "네", partial: " 맞아요")
        XCTAssertEqual(tail.final, "네 맞아요")
        XCTAssertEqual(tail.partial, "")
    }

    /// The grey "회 사" was corrected to "회사" before `<fin>`: still the same sentence, not shown twice.
    func testCorrectedEchoIsNotDoubled() {
        let r = LiveText.compose(held: [held("회 사 일정이요", awaitingFin: true)], said: "회사", partial: "일정 이요")
        XCTAssertEqual(r.final, "회 사 일정이요")
        XCTAssertEqual(r.partial, "")
    }

    /// Words that are not the cut sentence show while it waits for `<fin>`.
    func testOtherSpeechShowsWhileAwaitingFin() {
        let r = LiveText.compose(held: [held("네 맞아요", awaitingFin: true)], said: "다음 안건은", partial: "뭐죠")
        XCTAssertEqual(r.final, "네 맞아요 다음 안건은")
        XCTAssertEqual(r.partial, "뭐죠")
    }

    /// The reported bug: the next speaker's "네" vanished the moment it became final.
    func testTheTrigger() {
        var live = LiveText()
        live.heard(said: "네", partial: "맞아요")
        live.cut(id: 1)
        live.heard(said: "네 맞아요", partial: "")
        XCTAssertEqual(live.shown.final, "네 맞아요")
        live.heard(said: "", partial: "")  // <fin>
        live.finished(id: 1, transcript: "네 맞아요")
        // the engine's line is still on its way; the next speaker says "네"
        live.heard(said: "", partial: "네")
        XCTAssertEqual(live.shown.partial, "네")
        live.heard(said: "네", partial: "")
        XCTAssertEqual(live.shown.final, "네 맞아요 네")
        live.done(id: 1)
        XCTAssertEqual(live.shown.final, "네")
    }

    /// The transcript replaces the cut text.
    func testTranscriptReplacesTheCutText() {
        var live = LiveText()
        live.heard(said: "회 사", partial: "일정")
        live.cut(id: 1)
        live.finished(id: 1, transcript: "회사 일정")
        XCTAssertEqual(live.held, [held("회사 일정", awaitingFin: false)])
    }

    /// `<fin>` with no transcript but a live connection (Soniox heard nothing more): the cut text stays until the
    /// engine's line comes, and the `<fin>` itself emptied the live words.
    func testNilTranscriptKeepsTheCutText() {
        var live = LiveText()
        live.heard(said: "네", partial: "맞아요")
        live.cut(id: 1)
        live.heard(said: "", partial: "")
        live.finished(id: 1, transcript: nil)
        XCTAssertEqual(live.shown.final, "네 맞아요")
        XCTAssertFalse(live.held[0].awaitingFin)
        live.done(id: 1)
        XCTAssertEqual(live.shown.final, "")
    }

    /// The connection stopped (no `<fin>` in time, a receive error) with the cut sentence's echo as its last words:
    /// the cut text stays, once.
    func testStoppedStreamDoesNotShowTheSentenceTwice() {
        var live = LiveText()
        live.heard(said: "네", partial: "맞아요")
        live.cut(id: 1)
        live.heard(said: "네 맞아요", partial: "")
        live.finished(id: 1, transcript: nil, streamStopped: true)
        XCTAssertEqual(live.shown.final, "네 맞아요")
        XCTAssertEqual(live.shown.partial, "")
    }

    /// The 9-minute reconnect while a cut sentence still waits for the old connection's `<fin>`: the old connection's
    /// echo leaves at the swap, and neither its `<fin>` nor its result shows the sentence twice.
    func testSwapWithAPendingFinDoesNotShowTheSentenceTwice() {
        var live = LiveText()
        live.heard(said: "네", partial: "맞아요")
        live.cut(id: 1)
        live.heard(said: "네 맞아요", partial: "")
        live.swapped()
        XCTAssertEqual(live.shown.final, "네 맞아요")
        // the new connection hears the next speaker; the old one's <fin> then closes the cut sentence
        live.heard(said: "", partial: "다음은")
        live.finished(id: 1, transcript: "네 맞아요")
        XCTAssertEqual(live.shown.final, "네 맞아요")
        XCTAssertEqual(live.shown.partial, "다음은")
        // or it gave nothing (closed before its <fin>)
        var gone = LiveText()
        gone.heard(said: "네 맞아요", partial: "")
        gone.cut(id: 1)
        gone.swapped()
        gone.finished(id: 1, transcript: nil)
        XCTAssertEqual(gone.shown.final, "네 맞아요")
    }

    /// Nothing on screen at the cut: nothing held, and a transcript that comes anyway is held until the line arrives.
    func testEmptyCutHoldsTheTranscript() {
        var live = LiveText()
        live.heard(said: " ", partial: "")
        live.cut(id: 1)
        XCTAssertTrue(live.held.isEmpty)
        live.finished(id: 1, transcript: "네")
        XCTAssertEqual(live.shown.final, "네")
    }

    /// Held sentences stay in the order they were cut, whichever finishes first.
    func testHeldKeepCutOrder() {
        var live = LiveText()
        live.heard(said: "하나", partial: "")
        live.cut(id: 1)
        live.heard(said: "", partial: "둘")
        live.cut(id: 2)
        live.heard(said: "", partial: "")
        live.finished(id: 2, transcript: "둘이요")
        live.finished(id: 1, transcript: "하나요")
        XCTAssertEqual(live.shown.final, "하나요 둘이요")
    }
}
