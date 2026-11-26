import Foundation

/// Which GitHub login the engine gets (pure, so tests can pin it). Never logged.
enum GitHubCredentials {
    /// UserDefaults key: true from 연결 해제 until the next device-flow connection is saved, so a token the
    /// keychain could not delete is never handed to the engine again.
    static let disconnectedKey = "githubDisconnected"
    /// The key of the account in use (AccountSpace): each account has its own connection.
    static var disconnectedKeyInUse: String { AccountSpace.inUse.key(disconnectedKey) }

    /// `tacie`: the Tacie connection is the login (Build.usesTacieLogin); otherwise a development build uses `gh`.
    static func resolve(tacie: Bool, token: String?, disconnected: Bool) -> (mode: String, token: String?) {
        guard tacie else { return ("gh", nil) }
        if !disconnected, let token, !token.isEmpty { return ("token", token) }
        return ("none", nil)
    }
}
