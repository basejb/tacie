import AppKit
import AuthenticationServices
import Foundation

/// A sign-in page (Notion, Linear, Atlassian) in a sign-in window over Tacie (it shares Safari's sign-in), ending at
/// `tacie://<tool>?state=…&k=…` (issue #38, docs/31 10-2절). Only the window this app opened sees that return, so
/// the one-time key reaches the Mac that started the sign-in and no one else. Closing the window throws
/// `CancellationError`, as does cancelling the task; anything else that ends it throws `WebSignIn.Failed`.
@MainActor
final class WebSignIn: NSObject, ASWebAuthenticationPresentationContextProviding {
    struct Failed: Error {}

    private var session: ASWebAuthenticationSession?
    private var done: CheckedContinuation<URL, Error>?

    func run(_ url: URL) async throws -> URL {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (c: CheckedContinuation<URL, Error>) in
                done = c
                let s = ASWebAuthenticationSession(url: url, callbackURLScheme: "tacie") { [weak self] url, error in
                    Task { @MainActor in
                        if let url { self?.finish(.success(url)) } else if
                            (error as? ASWebAuthenticationSessionError)?.code == .canceledLogin {
                            self?.finish(.failure(CancellationError()))
                        } else {
                            self?.finish(.failure(Failed()))
                        }
                    }
                }
                s.presentationContextProvider = self
                s.prefersEphemeralWebBrowserSession = false
                session = s
                NSApp.activate(ignoringOtherApps: true)
                if !s.start() { finish(.failure(Failed())) }
            }
        } onCancel: {
            Task { @MainActor in
                self.session?.cancel()
                self.finish(.failure(CancellationError()))
            }
        }
    }

    private func finish(_ r: Result<URL, Error>) {
        session = nil
        guard let c = done else { return }
        done = nil
        c.resume(with: r)
    }

    nonisolated func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        MainActor.assumeIsolated { NSApp.keyWindow ?? NSApp.windows.first { $0.isVisible } ?? NSWindow() }
    }

    /// The return's query values when it is `tacie://<host>` for this sign-in's `state`. Any other return (another
    /// host, another state) throws `CancellationError`: it is ignored, with nothing on the card.
    nonisolated static func returned(_ url: URL, host: String, state: String) throws -> (String) -> String? {
        guard url.scheme == "tacie", url.host == host,
              let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
              items.first(where: { $0.name == "state" })?.value == state else { throw CancellationError() }
        return { name in items.first { $0.name == name }?.value }
    }

    /// `body`, or `expired` once `seconds` have passed (the relay keeps the code five minutes).
    nonisolated static func within(_ seconds: TimeInterval, expired: Error,
                                   _ body: @escaping @Sendable () async throws -> URL) async throws -> URL {
        try await withThrowingTaskGroup(of: URL.self) { group in
            group.addTask { try await body() }
            group.addTask {
                try await Task.sleep(for: .seconds(seconds))
                throw expired
            }
            defer { group.cancelAll() }
            return try await group.next()!
        }
    }
}
