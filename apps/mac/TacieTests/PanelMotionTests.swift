import XCTest

/// docs/36 0-2절: what the panel shows while Tacie works, by the seconds since the question went out.
final class PanelMotionTests: XCTestCase {
    private typealias Phase = PanelPhase.Phase

    func testPhaseBoundaries() {
        XCTAssertEqual(PanelPhase.at(elapsed: 0, steps: [], done: false), .quiet)
        XCTAssertEqual(PanelPhase.at(elapsed: 0.9, steps: [], done: false), .quiet)
        XCTAssertEqual(PanelPhase.at(elapsed: 1.0, steps: [], done: false), .looking)
        XCTAssertEqual(PanelPhase.at(elapsed: 2.9, steps: [], done: false), .looking)
        XCTAssertEqual(PanelPhase.at(elapsed: 3.0, steps: [], done: false), .working("회의 기록 찾는 중…"))
        XCTAssertEqual(PanelPhase.at(elapsed: 9.9, steps: [], done: false), .working("정리하는 중…"))
        XCTAssertEqual(PanelPhase.at(elapsed: 10.0, steps: [], done: false), .slow("정리하는 중…"))
        // with the engine's steps (prep) the line follows them, not the clock
        XCTAssertEqual(PanelPhase.at(elapsed: 9.9, steps: [.meetings(["a"])], done: false), .working("회의 1개 읽는 중…"))
        XCTAssertEqual(PanelPhase.at(elapsed: 10.0, steps: [.meetings(["a"])], done: false), .slow("회의 1개 읽는 중…"))
    }

    func testAskWithoutStepsSaysWritingAfterSixSeconds() {
        XCTAssertEqual(PanelPhase.at(elapsed: 5.9, steps: [], done: false), .working("회의 기록 찾는 중…"))
        XCTAssertEqual(PanelPhase.at(elapsed: 6.0, steps: [], done: false), .working("정리하는 중…"))
    }

    /// Every moment the phase (or its line) changes has a redraw in the schedule, a hair after it.
    func testScheduleHasEveryPhaseChange() {
        let start = Date(timeIntervalSinceReferenceDate: 0)
        let marks = PanelMotion.schedule(from: start).map { $0.timeIntervalSince(start) }
        var last = PanelPhase.at(elapsed: 0, steps: [], done: false)
        for i in 1...1200 {
            let t = Double(i) / 100
            let p = PanelPhase.at(elapsed: t, steps: [], done: false)
            if p != last {
                XCTAssertTrue(marks.contains { $0 >= t - 0.01 && $0 <= t + 0.011 }, "no redraw near \(t) s")
                last = p
            }
        }
    }

    func testDoneIsDoneAtAnyTime() {
        for t in [0, 0.9, 1.0, 3.0, 10.0, 60.0] {
            XCTAssertEqual(PanelPhase.at(elapsed: t, steps: [.writing], done: true), .done, "\(t)")
        }
    }

    func testStepLineFollowsWhatTheEngineDid() {
        XCTAssertEqual(PanelPhase.stepText([]), "회의 기록 찾는 중…")
        XCTAssertEqual(PanelPhase.stepText([.meetings(["a"])]), "회의 1개 읽는 중…")
        XCTAssertEqual(PanelPhase.stepText([.meetings(["a"]), .related(["b", "a"]), .sources(1)]), "회의 2개와 연결한 서비스 1개 읽는 중…")
        XCTAssertEqual(PanelPhase.stepText([.related([]), .sources(3)]), "연결한 서비스 3개 읽는 중…")
        XCTAssertEqual(PanelPhase.stepText([.meetings(["a"]), .sources(1), .writing]), "정리하는 중…")
    }

    func testWhatEachPhaseShows() {
        XCTAssertNil(Phase.quiet.stepLine)
        XCTAssertNil(Phase.looking.stepLine)
        XCTAssertNil(Phase.done.stepLine)
        XCTAssertEqual(Phase.working("정리하는 중…").stepLine, "정리하는 중…")
        XCTAssertEqual(Phase.slow("정리하는 중…").stepLine, "정리하는 중… 조금 더 걸려요")
        // the eyes: one glow step brighter while working, one more after 10 s, back at rest when done
        XCTAssertEqual([Phase.quiet, .looking, .working("x"), .slow("x"), .done].map(\.glow), [0, 1, 1, 2, 0])
        XCTAssertEqual([Phase.quiet, .looking, .working("x"), .slow("x"), .done].map { $0.looks(reduce: false) },
                       [false, true, false, false, false])
        XCTAssertEqual([Phase.quiet, .looking, .working("x"), .slow("x"), .done].map { $0.shimmers(reduce: false) },
                       [false, false, true, true, false])
    }

    func testShimmerStartsDimAndPeaksHalfAPeriodLater() {
        XCTAssertEqual(PanelMotion.shimmer(at: 3), 0, accuracy: 1e-9)
        XCTAssertEqual(PanelMotion.shimmer(at: 3.8), 1, accuracy: 1e-9)
        XCTAssertEqual(PanelMotion.shimmer(at: 4.6), 0, accuracy: 1e-9)
    }

    func testGlowChangeIsAShortFadeWithReduceMotion() {
        XCTAssertEqual(PanelMotion.glowAnimation(reduce: true), Theme.reduced)
        XCTAssertEqual(PanelMotion.glowAnimation(reduce: false), PanelMotion.glow)
    }

    // MARK: Head face

    private func turn(at: Date, answered: Date?, body: Bool = true) -> PanelTurn {
        PanelTurn(id: UUID(), question: "q", at: at, steps: [],
                  body: body ? .ask(PanelAsk(answer: "a", cited: [], cards: [], errorKind: nil)) : nil, answeredAt: answered)
    }

    func testFaceGlowStaysUntilTheDoneLine() {
        let at = Date(timeIntervalSinceReferenceDate: 0)
        let answered = at.addingTimeInterval(4)
        let t = turn(at: at, answered: answered)
        XCTAssertEqual(PanelPhase.face(at: answered.addingTimeInterval(0.5), turn: t, doneAt: 0.8).glow, 1)
        XCTAssertEqual(PanelPhase.face(at: answered.addingTimeInterval(0.8), turn: t, doneAt: 0.8), .done)
        XCTAssertEqual(PanelPhase.face(at: at.addingTimeInterval(2), turn: turn(at: at, answered: nil, body: false), doneAt: 0).glow, 1)
        // an answer that came with the turn is done at once
        XCTAssertEqual(PanelPhase.face(at: at, turn: turn(at: at, answered: nil), doneAt: 0.8), .done)
    }

    func testRevealOfATurn() {
        let at = Date(timeIntervalSinceReferenceDate: 0)
        let late = PanelReveal.of(turn(at: at, answered: at.addingTimeInterval(4)), at: at.addingTimeInterval(4.3), reduce: false)
        XCTAssertEqual(late.t!, 0.3, accuracy: 1e-9)
        XCTAssertTrue(late.staggered)
        XCTAssertFalse(PanelReveal.of(turn(at: at, answered: at.addingTimeInterval(0.5)), at: at, reduce: false).staggered)
        XCTAssertFalse(PanelReveal.of(turn(at: at, answered: at.addingTimeInterval(4)), at: at, reduce: true).staggered)
        XCTAssertEqual(PanelReveal.of(turn(at: at, answered: nil), at: at, reduce: false), .shown)
    }

    // MARK: Ticks

    func testTicksOnlyAtMarksAndInsideSpans() {
        let s = Date(timeIntervalSinceReferenceDate: 0)
        let ticks = PanelTicks(marks: [s.addingTimeInterval(1), s.addingTimeInterval(5)],
                               spans: [.init(from: s.addingTimeInterval(2), to: s.addingTimeInterval(2.1))])
        let got = Array(ticks.entries(from: s, mode: .normal)).map { $0.timeIntervalSince(s) }
        XCTAssertEqual(got.first, 0)
        XCTAssertEqual(got[1], 1)
        XCTAssertEqual(got[2], 2)
        XCTAssertEqual(got.last, 5)
        XCTAssertTrue(got.filter { $0 > 2 && $0 < 5 }.allSatisfy { $0 <= 2.1 + 1e-9 })
        XCTAssertEqual(got.filter { $0 > 2 && $0 <= 2.1 + 1e-9 }.count, 3)  // 1/30 s apart, the last at the span's end
        XCTAssertEqual(Array(PanelTicks().entries(from: s, mode: .normal)), [s])
    }

    func testReduceMotionHasNoLookNoShimmer() {
        for p in [Phase.quiet, .looking, .working("x"), .slow("x"), .done] {
            XCTAssertFalse(p.looks(reduce: true))
            XCTAssertFalse(p.shimmers(reduce: true))
        }
        XCTAssertEqual(Phase.slow("x").glow, 2)  // brightness is not motion: it stays
    }

    func testTimingsMatchTheSpec() {
        XCTAssertEqual(PanelMotion.lookSeconds, 0.45)
        XCTAssertEqual(PanelMotion.shimmerPeriod, 1.6)
        XCTAssertEqual(PanelMotion.chipStep, 0.11)
        XCTAssertEqual(PanelMotion.chunkStep, 0.14)
        XCTAssertEqual(PanelMotion.breathPeriods.0, 4.0)
        XCTAssertEqual(PanelMotion.breathPeriods.1, 6.3)
    }

    func testBreathStaysSmallAndStillIsRest() {
        for t in stride(from: 0.0, to: 13, by: 0.37) {
            let b = PanelMotion.breath(at: t)
            XCTAssertLessThanOrEqual(abs(b.size), 0.04 + 1e-9)
            XCTAssertLessThanOrEqual(abs(b.glow), 0.08 + 1e-9)
        }
        let rest = PanelFace.channels(look: 0, glow: 0)
        XCTAssertEqual(rest.left, rest.right)
        XCTAssertEqual(rest.left.size, 1)
    }

    // MARK: Answer chunks

    func testChunksSplitAtClausesAndKeepCitations() {
        let text = "부분 환불 수수료를 누가 낼지가 아직 안 정해졌어요[1]. 10월 31일 회의에서도 같은 이유로 미뤄졌고[2], Linear 이슈도 그 결정을 기다리고 있어요[3]."
        let chunks = PanelMotion.chunks(text)
        XCTAssertEqual(chunks.joined(), text)
        XCTAssertEqual(chunks.map { $0.trimmingCharacters(in: .whitespaces) },
                       ["부분 환불 수수료를 누가 낼지가 아직 안 정해졌어요[1].", "10월 31일 회의에서도 같은 이유로 미뤄졌고[2],",
                        "Linear 이슈도 그 결정을 기다리고 있어요[3]."])
    }

    func testCitationAfterThePunctuationStaysWithItsChunk() {
        let chunks = PanelMotion.chunks("세 번으로 정했어요.[1] 한도는 지윤 님이 봐요.[2]")
        XCTAssertEqual(chunks, ["세 번으로 정했어요.[1] ", "한도는 지윤 님이 봐요.[2]"])
    }

    func testShortAnswerIsOneChunk() {
        XCTAssertEqual(PanelMotion.chunks("세 번이에요[1]"), ["세 번이에요[1]"])
        XCTAssertEqual(PanelMotion.chunks(""), [])
    }

    func testNoSplitInsideBoldOrCode() {
        XCTAssertEqual(PanelMotion.chunks("**재시도, 환불** 둘 다 남았어요."), ["**재시도, 환불** 둘 다 남았어요."])
        XCTAssertEqual(PanelMotion.chunks("`retry, refund` 둘 다 남았어요."), ["`retry, refund` 둘 다 남았어요."])
    }

    // MARK: Reveal

    func testRevealOrderChipsThenChunksThenDone() {
        let r = PanelReveal(t: 0, staggered: true)
        XCTAssertEqual(r.chipStart(2), 0.22, accuracy: 1e-9)
        XCTAssertEqual(r.chunkStart(chips: 3, 1), 0.33 + 0.14, accuracy: 1e-9)
        XCTAssertEqual(r.doneAt(chips: 3, chunks: 3), 0.33 + 0.28 + PanelMotion.chunkFade, accuracy: 1e-9)
        XCTAssertEqual(r.endAt(chips: 3, chunks: 3), 0.33 + 0.28 + 2 * PanelMotion.chunkFade, accuracy: 1e-9)
        // a chip half-way in its fade, a later one not yet
        let mid = PanelReveal(t: 0.1, staggered: true)
        XCTAssertEqual(mid.opacity(from: mid.chipStart(0), fade: PanelMotion.chipFade), 0.5, accuracy: 1e-9)
        XCTAssertEqual(mid.opacity(from: mid.chipStart(1), fade: PanelMotion.chipFade), 0)
    }

    func testUnstaggeredRevealIsOneShortFade() {
        let r = PanelReveal(t: 0.06, staggered: false)
        XCTAssertEqual(r.chipStart(5), 0)
        XCTAssertEqual(r.chunkStart(chips: 5, 4), 0)
        XCTAssertEqual(r.opacity(from: r.chunkStart(chips: 5, 4), fade: PanelMotion.chunkFade), 0.5, accuracy: 1e-9)
        XCTAssertEqual(r.doneAt(chips: 5, chunks: 5), 0)
        XCTAssertEqual(r.endAt(chips: 5, chunks: 5), PanelMotion.reducedFade)
        XCTAssertEqual(PanelReveal.shown.opacity(from: 9, fade: 1), 1)
    }

    func testRevealIsStaggeredOnlyAfterASecondAndWithMotion() {
        XCTAssertFalse(PanelReveal.staggers(elapsed: 0.9, reduce: false))
        XCTAssertTrue(PanelReveal.staggers(elapsed: 1.0, reduce: false))
        XCTAssertFalse(PanelReveal.staggers(elapsed: 5, reduce: true))
    }
}
