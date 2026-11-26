import Foundation

/// What the iPhone app hands its engine (docs/40 4-1, 6-1): the Tacie session (the engine's AI calls and its meeting
/// search go through the relay with it) and the meetings it searches. Both live in the engine's memory only, so a
/// restarted engine (the app was suspended, `EngineHost.ready`) gets them again.
protocol EngineFeeding {
    func setRelaySession(_ token: String) async throws
    func clearRelaySession() async throws
    func setMeetings(_ list: [MeetingDigest]) async throws -> Int
}

/// Keeps one engine up to date with the current token and meetings. Policy only; the app wires the sources
/// (apps/ios/Tacie/Engine/IPhoneEngineFeed.swift).
@MainActor
final class EngineFeed {
    private let token: () -> String?
    private let meetings: () -> [MeetingDigest]
    private let wait: Duration
    private var engine: EngineFeeding?
    /// What the current engine holds (nil token: none; nil meetings: not sent yet).
    private var heldToken: String?
    private var heldMeetings: [MeetingDigest]?
    private var pumping = false
    private var again = false
    /// Set by `signedOut` so the token and meetings read as gone before the app has cleared them.
    private var out = false
    /// Bumped when the engine changes, so a send that was under way does not write down for the new one.
    private var generation = 0
    private let signedIn: () -> Bool
    private let feedsMeetings: () -> Bool

    /// `signedIn` is whether an account is signed in, apart from the session to send: a development engine with key
    /// files has no session to send but is still signed in. It defaults to "there is a token". `feedsMeetings` is
    /// false where nothing reads the meetings in the engine (the App Store iPhone app): only the session goes, the
    /// meetings are never read or sent, and `ready` does not wait for them.
    init(wait: Duration = .seconds(5), signedIn: (() -> Bool)? = nil, feedsMeetings: @escaping () -> Bool = { true },
         token: @escaping () -> String?, meetings: @escaping () -> [MeetingDigest]) {
        self.wait = wait
        self.signedIn = signedIn ?? { token() != nil }
        self.feedsMeetings = feedsMeetings
        self.token = token
        self.meetings = meetings
    }

    /// A new engine (first start or after a restart): it holds nothing yet.
    func engineStarted(_ e: EngineFeeding) async {
        generation += 1
        engine = e
        heldToken = nil
        heldMeetings = nil
        await pump()
    }

    /// The engine went away (a restart is under way): nothing is ready until `engineStarted`.
    func engineLost() {
        generation += 1
        engine = nil
    }

    func tokenChanged() async {
        if signedIn() { out = false }
        await pump()
    }
    func meetingsChanged() async { await pump() }

    /// Signing out: the engine forgets the meetings and the session. Every send goes through `pump`, so this cannot
    /// race a send already under way (that one is followed by the clearing).
    func signedOut() async {
        out = true
        await pump()
    }

    /// True once the current engine holds the current token and meetings. Waits up to `wait` (5 s), trying again.
    func ready() async -> Bool {
        let clock = ContinuousClock()
        let deadline = clock.now + wait
        while true {
            if engine != nil, heldToken == currentToken(), !feedsMeetings() || heldMeetings == sorted() { return true }
            if clock.now >= deadline { return false }
            await pump()
            try? await Task.sleep(for: .milliseconds(20))
        }
    }

    private func currentToken() -> String? { out ? nil : token() }
    private func sorted() -> [MeetingDigest] { out ? [] : meetings().sorted { $0.id < $1.id } }

    /// Sends what differs. One run at a time; a call while one runs makes it go round once more, even after a failed
    /// send (so a sign-out that came during it still clears). A failed send stays unsent and is tried again at the
    /// next trigger. What a send changed is written down only if the engine is still
    /// the one it went to (`generation`); otherwise the new engine is served next.
    private func pump() async {
        if pumping { again = true; return }
        pumping = true
        defer { pumping = false }
        repeat {
            again = false
            guard let e = engine else { return }
            let gen = generation
            let steps: [Step] = out ? [.meetings, .token] : [.token, .meetings]
            round: for step in steps {
                switch step {
                case .token:
                    let want = currentToken()
                    guard want != heldToken else { continue }
                    do {
                        if let want { try await e.setRelaySession(want) } else { try await e.clearRelaySession() }
                    } catch {
                        // stop this round; a trigger that came meanwhile (`again`, a sign-out) still goes round
                        break round
                    }
                    if gen == generation { heldToken = want }
                case .meetings:
                    guard feedsMeetings() else { continue }
                    let list = sorted()
                    guard list != heldMeetings else { continue }
                    do {
                        _ = try await e.setMeetings(list)
                    } catch {
                        break round
                    }
                    if gen == generation { heldMeetings = list }
                }
                if gen != generation { break }
            }
            if gen != generation { again = true }
        } while again
    }

    private enum Step { case token, meetings }
}
