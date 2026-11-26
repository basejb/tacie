import Foundation
import Observation

/// The main window's Getting started (docs/11 14절): four things to try once, checked by doing them for real.
enum FirstStep: String, Codable, CaseIterable, CodingKeyRepresentable {
    case source, meeting, card, ask
}

/// What has been tried and when. Only that it happened, never which meeting or card.
struct FirstStepsState: Codable, Equatable {
    var done: [FirstStep: Date] = [:]
    /// The list was put away with ✕ (처음 안내 다시 보기 brings it back).
    var hidden = false
    /// The last step was done and the list said so once; it does not come back.
    var finishedShown = false
    /// Steps someone who used Tacie before this list existed already did, checked once.
    var backfilled = false
    /// Steps put away one by one with 넘기기; nil in state saved before it existed.
    var skipped: Set<FirstStep>? = nil
}

enum FirstSteps {
    static func isDone(_ s: FirstStepsState, _ step: FirstStep) -> Bool { s.done[step] != nil }

    /// The steps the list still shows, in order: all but the skipped ones.
    static func steps(_ s: FirstStepsState) -> [FirstStep] { FirstStep.allCases.filter { !(s.skipped ?? []).contains($0) } }

    static func total(_ s: FirstStepsState) -> Int { steps(s).count }

    static func count(_ s: FirstStepsState) -> Int { steps(s).filter { isDone(s, $0) }.count }

    static func visible(_ s: FirstStepsState) -> Bool {
        !s.hidden && !s.finishedShown && count(s) < total(s)
    }

    static func skip(_ s: inout FirstStepsState, _ step: FirstStep) {
        s.skipped = (s.skipped ?? []).union([step])
    }

    /// The last step just got done and the list has not said so yet: "처음 써 보기를 다 했어요" once, then gone.
    static func showsFinish(_ s: FirstStepsState) -> Bool {
        !s.hidden && !s.finishedShown && total(s) > 0 && count(s) == total(s)
    }

    /// An answer that counts for 처음 써 보기: not the onboarding's practice meeting, sample data or its step 3.
    static func realAnswer(practice: Bool, sample: Bool, onboarding: Bool) -> Bool {
        !practice && !sample && !onboarding
    }

    /// "눌러서 근거를 열어 보세요" under the first cards of a real meeting, once ever, and not once a card was opened.
    static func wantsCardHint(_ s: FirstStepsState, cards: Int, inMeeting: Bool, practice: Bool, sample: Bool,
                              hintShown: Bool) -> Bool {
        cards > 0 && inMeeting && !practice && !sample && !hintShown && !isDone(s, .card) && !(s.skipped ?? []).contains(.card)
    }

    /// Done once and for good: true only the first time, which keeps that first moment.
    @discardableResult
    static func mark(_ s: inout FirstStepsState, _ step: FirstStep, at: Date) -> Bool {
        guard s.done[step] == nil else { return false }
        s.done[step] = at
        return true
    }

    /// For someone updating into this list: a kept meeting and a connection count, a card or a question asked in a
    /// meeting left no trace and start empty.
    static func backfill(_ s: inout FirstStepsState, hasMeeting: Date?, hasSource: Bool, now: Date) {
        guard !s.backfilled else { return }
        s.backfilled = true
        if let hasMeeting { mark(&s, .meeting, at: hasMeeting) }
        if hasSource { mark(&s, .source, at: now) }
    }
}

/// The saved list, one JSON value in UserDefaults like the onboarding's progress. Each account has its own
/// (AccountSpace: `firstSteps.state@<id>`): the steps say when that account first did them.
@Observable
@MainActor
final class FirstStepsStore {
    static let shared = FirstStepsStore()
    nonisolated static let key = "firstSteps.state"

    private(set) var state: FirstStepsState
    @ObservationIgnored private let defaults: UserDefaults
    /// A step done for the first time (the app counts it, docs/16). Not called by `backfill`.
    @ObservationIgnored var onDone: ((FirstStep) -> Void)?

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        state = Self.load(defaults)
    }

    /// The key of the account in use.
    private static var spaceKey: String { AccountSpace.inUse.key(key) }

    private static func load(_ defaults: UserDefaults) -> FirstStepsState {
        defaults.data(forKey: spaceKey).flatMap { try? JSONDecoder().decode(FirstStepsState.self, from: $0) } ?? FirstStepsState()
    }

    /// Another account's space is open: its own list.
    func reopen() { state = Self.load(defaults) }

    func mark(_ step: FirstStep, at: Date = .now) {
        guard FirstSteps.mark(&state, step, at: at) else { return }
        save()
        onDone?(step)
    }

    func backfill(hasMeeting: Date?, hasSource: Bool) {
        guard !state.backfilled else { return }
        FirstSteps.backfill(&state, hasMeeting: hasMeeting, hasSource: hasSource, now: .now)
        save()
    }

    func hide() { state.hidden = true; save() }

    func skip(_ step: FirstStep) { FirstSteps.skip(&state, step); save() }

    func finishShown() { state.finishedShown = true; save() }

    func reset() { state = FirstStepsState(); save() }

    private func save() {
        if let data = try? JSONEncoder().encode(state) { defaults.set(data, forKey: Self.spaceKey) }
    }
}
