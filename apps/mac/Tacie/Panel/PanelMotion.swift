import SwiftUI

/// Tacie at work in the panel (docs/36 0-2절, mockup v9): no rainbow glow, only the head face's eyes, one shimmering
/// step line, and the answer coming in sources first, then in clauses. The numbers are the spec's.
enum PanelMotion {
    /// after this, the eyes look toward the question once and glow one step brighter; before it nothing shows
    static let lookAfter: TimeInterval = 1
    /// after this, the step line
    static let workingAfter: TimeInterval = 3
    /// after this, a request the engine reports no steps for (an ask) says it is writing
    static let writingAfter: TimeInterval = 6
    /// after this, "조금 더 걸려요" and one more glow step
    static let slowAfter: TimeInterval = 10

    static let lookSeconds = 0.45
    /// the eyes' glance (mockup `.look` transition)
    static let look = Animation.timingCurve(0.32, 0.72, 0, 1, duration: lookSeconds)
    /// the eyes' glow growing and settling (mockup box-shadow .6s ease)
    static let glow = Animation.easeInOut(duration: 0.6)
    /// 움직임 줄이기: the glow changes in one short fade
    static func glowAnimation(reduce: Bool) -> Animation { Theme.anim(glow, reduce: reduce) }
    /// the step line's brightness, ink3 ↔ ink and back
    static let shimmerPeriod = 1.6
    /// how often the step line's shimmer and an arriving answer are redrawn
    static let frame = 1.0 / 30

    /// The step line's brightness `elapsed` seconds after the question went out: 0 (ink3) when it appears at 3 s,
    /// 1 (ink) half a period later, back to 0 a period later.
    static func shimmer(at elapsed: TimeInterval) -> Double {
        (1 - cos(2 * .pi * (elapsed - workingAfter) / shimmerPeriod)) / 2
    }

    /// sources take their place one after another, before the answer
    static let chipStep = 0.11
    static let chipFade = 0.2
    /// the answer's clauses fade in one after another
    static let chunkStep = 0.14
    static let chunkFade = 0.28
    /// 움직임 줄이기 and answers back within a second: one short fade for everything
    static let reducedFade = 0.12

    /// The lights at rest, as the island's with no meeting (`EyeRig`, asleep): sizes swing ±4% against each other over
    /// 4 s, brightness a little over 4 s and 6.3 s. Drawn 15 times a second; slow enough that no frame shows a step.
    struct Breath: Equatable {
        var size = 0.0, glow = 0.0, both = 0.0
        static let still = Breath()
    }
    static let breathPeriods = (4.0, 6.3)
    /// the conversation list's card coming up and going (quick, no bounce)
    static let listCard = Animation.spring(response: 0.22, dampingFraction: 0.92)
    /// the breath is slow (4–6.3 s periods): 8 frames a second is enough and keeps an idle panel cheap
    static let breathFrame = 1.0 / 8
    static func breath(at t: TimeInterval) -> Breath {
        let s = sin(2 * .pi * t / breathPeriods.0)
        let b = 0.03 * sin(2 * .pi * t / breathPeriods.1)
        return Breath(size: 0.04 * s, glow: 0.05 * s + b, both: b)
    }

    /// When the working phase can change, from the time the question went out: the view samples the phase only then.
    static func schedule(from start: Date) -> [Date] {
        [0, lookAfter, workingAfter, writingAfter, slowAfter].map { start.addingTimeInterval($0 + ($0 > 0 ? 0.01 : 0)) }
    }

    /// The answer in clauses to fade in one after another: cut after "." "," "?" "!" (a citation mark right after one
    /// stays with it) where a space follows, never inside **bold** or `code`. Joined back they are the text.
    static func chunks(_ text: String) -> [String] {
        var out: [String] = []
        var cur = ""
        var i = text.startIndex
        while i < text.endIndex {
            let c = text[i]
            cur.append(c)
            let next = text.index(after: i)
            if c.isWhitespace, next < text.endIndex, !text[next].isWhitespace, endsClause(cur) {
                out.append(cur)
                cur = ""
            }
            i = next
        }
        if !cur.isEmpty { out.append(cur) }
        return out
    }

    private static func endsClause(_ s: String) -> Bool {
        let bare = s.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: #"(\[\d+\])+$"#, with: "", options: .regularExpression)
        guard let last = bare.last, ".,?!。".contains(last) else { return false }
        // a chunk starts at a cut, so its own marks tell whether the cut would fall inside bold or code
        return s.components(separatedBy: "**").count % 2 == 1 && s.filter { $0 == "`" }.count % 2 == 0
    }
}

/// What the panel shows of a request in flight, by the seconds since it went out.
enum PanelPhase {
    enum Phase: Equatable {
        /// the first second: only the question bubble
        case quiet
        /// 1–3 s: the eyes look toward the question and glow brighter
        case looking
        /// from 3 s: the step line
        case working(String)
        /// from 10 s: the step line says it takes a little longer, the eyes one more step brighter
        case slow(String)
        /// the answer is here
        case done

        var stepLine: String? {
            switch self {
            case .working(let t): t
            case .slow(let t): "\(t) \(PanelText.slow)"
            default: nil
            }
        }

        /// the eyes' glow steps above rest
        var glow: Int {
            switch self {
            case .quiet, .done: 0
            case .looking, .working: 1
            case .slow: 2
            }
        }

        func looks(reduce: Bool) -> Bool { !reduce && self == .looking }
        func shimmers(reduce: Bool) -> Bool { !reduce && stepLine != nil }
    }

    static func at(elapsed: TimeInterval, steps: [PanelStep], done: Bool) -> Phase {
        if done { return .done }
        if elapsed < PanelMotion.lookAfter { return .quiet }
        // a chat reports each tool as it starts: its line shows then, not at a set time (docs/38 7절)
        let working = steps.contains { if case .tool(let name, _, _) = $0 { name != "start" } else { false } }
        if elapsed < PanelMotion.workingAfter && !working { return .looking }
        let text = stepText(steps, elapsed: elapsed)
        return elapsed < PanelMotion.slowAfter ? .working(text) : .slow(text)
    }

    /// What the head face shows for a conversation's last turn: the phase while it runs, and after its answer arrives
    /// until the done line shows (`doneAt` seconds later); an answer that came with the turn is done at once.
    static func face(at now: Date, turn: PanelTurn, doneAt: Double) -> Phase {
        let done = turn.body != nil && (turn.answeredAt.map { now >= $0.addingTimeInterval(doneAt) } ?? true)
        return at(elapsed: now.timeIntervalSince(turn.at), steps: turn.steps, done: done)
    }

    /// The step line from what the engine reported doing: finding, reading what it found, writing. With no steps
    /// reported (an ask), by time: finding, then writing after 6 s.
    static func stepText(_ steps: [PanelStep], elapsed: TimeInterval = 0) -> String {
        if steps.contains(where: { if case .tool = $0 { true } else { false } }) { return toolText(steps) }
        if steps.isEmpty, elapsed >= PanelMotion.writingAfter { return PanelText.stepWriting }
        var ids: [String] = []
        var sources = 0
        for s in steps {
            switch s {
            case .meetings(let x), .related(let x): ids += x.filter { !ids.contains($0) }
            case .sources(let n): sources = n
            case .writing: return PanelText.stepWriting
            case .tool: break
            }
        }
        return PanelText.stepReading(meetings: ids.count, sources: sources) ?? PanelText.stepFinding
    }
}

extension PanelPhase {
    /// A chat's step line (`PanelSteps`, shared with the iPhone).
    static func toolText(_ steps: [PanelStep]) -> String { PanelSteps.toolText(steps) }

    static func period(from: String?, to: String?, calendar: Calendar = .current, today: Date = Date(),
                       locale: Locale = PanelText.appLocale) -> String? {
        PanelSteps.period(from: from, to: to, calendar: calendar, today: today, locale: locale)
    }
}

/// An answer coming in: `t` seconds since it arrived (nil: all shown, as for an answer that was already there).
/// Sources first, `chipStep` apart, then the clauses `chunkStep` apart, then the done line. Not `staggered`
/// (움직임 줄이기, or back within a second), everything fades in at once, briefly.
struct PanelReveal: Equatable {
    var t: Double?
    var staggered: Bool

    static let shown = PanelReveal(t: nil, staggered: false)

    static func staggers(elapsed: TimeInterval, reduce: Bool) -> Bool { !reduce && elapsed >= PanelMotion.lookAfter }

    /// The reveal of `turn`'s answer at `now` (`t` from 0 when it arrived, unknown arrival: all shown).
    static func of(_ turn: PanelTurn, at now: Date, reduce: Bool) -> PanelReveal {
        guard turn.body != nil, let a = turn.answeredAt else { return .shown }
        return PanelReveal(t: max(0, now.timeIntervalSince(a)), staggered: staggers(elapsed: a.timeIntervalSince(turn.at), reduce: reduce))
    }

    func chipStart(_ i: Int) -> Double { staggered ? Double(i) * PanelMotion.chipStep : 0 }

    func chunkStart(chips: Int, _ j: Int) -> Double {
        staggered ? Double(chips) * PanelMotion.chipStep + Double(j) * PanelMotion.chunkStep : 0
    }

    /// when the last clause is in: the step line folds and the done line fades in
    func doneAt(chips: Int, chunks: Int) -> Double {
        staggered ? chunkStart(chips: chips, max(chunks - 1, 0)) + PanelMotion.chunkFade : 0
    }

    /// when the done line is in too, and the answer can be drawn as it is
    func endAt(chips: Int, chunks: Int) -> Double {
        doneAt(chips: chips, chunks: chunks) + (staggered ? PanelMotion.chunkFade : PanelMotion.reducedFade)
    }

    func opacity(from start: Double, fade: Double) -> Double {
        guard let t else { return 1 }
        let x = min(max((t - start) / (staggered ? fade : PanelMotion.reducedFade), 0), 1)
        return x * x * (3 - 2 * x)
    }
}

private struct PanelRevealKey: EnvironmentKey { static let defaultValue = PanelReveal.shown }

extension EnvironmentValues {
    /// The answer being revealed, for the pieces of an answer view to read their opacity from.
    var panelReveal: PanelReveal {
        get { self[PanelRevealKey.self] }
        set { self[PanelRevealKey.self] = newValue }
    }
}

/// One piece of an answer fading in at `start` (seconds after the answer arrived), rising `rise` points with motion.
private struct PanelRevealItem: ViewModifier {
    let start: Double
    let fade: Double
    let rise: CGFloat
    @Environment(\.panelReveal) private var reveal

    func body(content: Content) -> some View {
        let o = reveal.opacity(from: start, fade: fade)
        content.opacity(o).offset(y: reveal.staggered ? rise * (1 - o) : 0)
    }
}

extension View {
    func panelReveal(at start: Double, fade: Double = PanelMotion.chunkFade, rise: CGFloat = 0) -> some View {
        modifier(PanelRevealItem(start: start, fade: fade, rise: rise))
    }
}

/// When a panel view redraws: at `marks`, and every frame inside `spans` (once a second in low-frequency mode).
/// Nothing else, so an idle panel never ticks.
struct PanelTicks: TimelineSchedule {
    struct Span { let from: Date; let to: Date }
    var marks: [Date] = []
    var spans: [Span] = []

    func entries(from start: Date, mode: TimelineScheduleMode) -> AnyIterator<Date> {
        let step = mode == .lowFrequency ? 1 : PanelMotion.frame
        var cur: Date?
        return AnyIterator {
            guard let c = cur else { cur = start; return start }
            var next = marks.filter { $0 > c }.min()
            for s in spans where s.to > c {
                let n = c < s.from ? s.from : s.to.timeIntervalSince(c) <= step + 1e-6 ? s.to : c.addingTimeInterval(step)
                next = min(next ?? n, n)
            }
            cur = next
            return next
        }
    }

    /// A turn's redraws: its phase changes while it runs, with every frame from 3 s for the shimmer; every frame while
    /// its answer comes in.
    static func turn(_ turn: PanelTurn, reduce: Bool, endAt: Double) -> PanelTicks {
        if turn.body == nil {
            let shimmer = reduce ? [] : [Span(from: turn.at.addingTimeInterval(PanelMotion.workingAfter), to: .distantFuture)]
            return PanelTicks(marks: PanelMotion.schedule(from: turn.at), spans: shimmer)
        }
        guard let a = turn.answeredAt else { return PanelTicks() }
        return PanelTicks(spans: [Span(from: a, to: a.addingTimeInterval(endAt))])
    }
}

/// The step line while Tacie works (docs/36 0-2절 4): its words' brightness alone goes ink3 → ink → ink3 over 1.6 s.
/// `brightness` is drawn as given (`PanelMotion.shimmer`); nil (움직임 줄이기) is a still ink2 line.
struct PanelStepLine: View {
    let text: String
    let brightness: Double?

    var body: some View {
        Text(text).uiFont(12.5).foregroundStyle(brightness == nil ? AnyShapeStyle(Theme.ink2) : AnyShapeStyle(Theme.ink3))
            .lineLimit(2)
            .overlay(alignment: .topLeading) {
                if let brightness {
                    Text(text).uiFont(12.5).foregroundStyle(Theme.ink).lineLimit(2).opacity(brightness)
                }
            }
    }
}

/// The head's face: at rest it breathes; while the last question runs it looks toward it and its glow grows
/// by the phase, back to rest when the answer's done line shows. Redrawn only at those moments.
struct PanelHeadFace: View {
    /// the conversation's last turn
    let turn: PanelTurn?
    /// seconds from its answer arriving to its done line
    let doneAt: Double
    @Environment(\.reducesMotion) private var reduce

    var body: some View {
        let marks = turn.map { t in PanelMotion.schedule(from: t.at) + [t.answeredAt?.addingTimeInterval(doneAt + 0.01)].compactMap { $0 } } ?? []
        TimelineView(PanelTicks(marks: marks)) { _ in
            let phase = turn.map { PanelPhase.face(at: Date(), turn: $0, doneAt: doneAt) } ?? .done
            PanelFace(size: .head, look: phase.looks(reduce: reduce), glow: phase.glow, breathes: phase == .done)
        }
    }
}
