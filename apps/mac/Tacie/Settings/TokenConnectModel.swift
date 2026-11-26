import Foundation
import Observation

/// The open edition's step 3: the flow of one token window. A paste is checked with that service after 0.7 s; if it
/// works it is saved and passed to the engine, "Done" shows for a moment, and the window closes. Only Notion has a step
/// 3 left (adding the pages Tacie reads), so it closes with "Done". If the check fails nothing is saved (Asana · Jira
/// need values that only the check returns).
@Observable
@MainActor
final class TokenConnectModel {
    enum State: Equatable { case empty, checking, ok, rejected, unknown, noSite }

    typealias Checker = (ConnectionsScene.Source, String, String?, String?) async -> PastedTokens.Check
    /// (what the check found, the token, the email): saves and hands the connection to the engine.
    typealias Connector = (PastedTokens.Found, String, String) async -> Void

    let source: ConnectionsScene.Source
    var token = ""
    var email = ""
    var site = ""
    private(set) var state: State = .empty

    var delay: Duration = .milliseconds(700)
    var closeAfter: Duration = .milliseconds(900)

    private let check: Checker
    private let connect: Connector
    private let done: () -> Void
    private var task: Task<Void, Never>?
    private var generation = 0

    init(source: ConnectionsScene.Source, check: @escaping Checker, connect: @escaping Connector,
         done: @escaping () -> Void) {
        self.source = source
        self.check = check
        self.connect = connect
        self.done = done
    }

    private func clean(_ s: String) -> String { s.trimmingCharacters(in: .whitespacesAndNewlines) }

    /// Every field the check needs is filled in.
    var ready: Bool {
        guard !clean(token).isEmpty else { return false }
        return source != .atlassian || (!clean(email).isEmpty && !clean(site).isEmpty)
    }

    /// A field changed: the last check is dropped and a new one starts after a pause.
    func changed() { start(after: delay) }

    /// 취소, or the window went away: a check under way is dropped, so nothing is saved after the person left.
    func cancel() {
        task?.cancel()
        task = nil
        generation += 1
        if state == .checking { state = .empty }
    }

    /// "다시 확인" after the service could not be reached.
    func recheck() { start(after: .zero) }

    private func start(after pause: Duration) {
        task?.cancel()
        generation += 1
        guard ready else {
            state = .empty
            return
        }
        state = .checking
        let g = generation
        task = Task {
            if pause > .zero { try? await Task.sleep(for: pause) }
            guard !Task.isCancelled, g == generation else { return }
            await run(g)
        }
    }

    private func run(_ g: Int) async {
        let token = clean(token), email = clean(email)
        let result = await check(source, token, email.isEmpty ? nil : email, clean(site))
        guard !Task.isCancelled, g == generation else { return }
        switch result {
        case .ok(let found):
            await connect(found, token, email)
            guard g == generation else { return }
            state = .ok
            guard source != .notion else { return }
            if closeAfter > .zero { try? await Task.sleep(for: closeAfter) }
            done()
        case .rejected: state = .rejected
        case .unknown: state = .unknown
        case .noSite: state = .noSite
        }
    }
}
