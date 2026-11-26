import Foundation

/// The values only the official build carries (apps/mac/Official.xcconfig), read from Info.plist where the build put
/// them (`$(TACIE_…)`). The open edition leaves them empty, so each is nil there (docs/41 0절).
enum BuildValues {
    /// A non-empty value; an empty one, or a `$(…)` the build did not fill in, is nil.
    static func value(_ key: String, in info: [String: Any] = Bundle.main.infoDictionary ?? [:]) -> String? {
        guard let raw = info[key] as? String else { return nil }
        let v = raw.trimmingCharacters(in: .whitespaces)
        return v.isEmpty || v.hasPrefix("$(") ? nil : v
    }

    private static func url(_ key: String) -> URL? { value(key).flatMap(URL.init(string:)) }

    static var gitHubClientID: String? { value("TacieGitHubClientID") }
    static var linearClientID: String? { value("TacieLinearClientID") }
    static var dropboxAppKey: String? { value("TacieDropboxAppKey") }
    /// Sign-in, `/me` and the connections the server exchanges (auth.tacie.app).
    static var accountServer: URL? { url("TacieAccountServer") }
    static var relaySeoul: URL? { url("TacieRelaySeoul") }
    /// A Tacie server address, or one that cannot resolve (RFC 2606 `.invalid`) in a build without it: a call the
    /// open edition forgot to switch off fails instead of reaching our servers.
    static func server(_ url: URL?) -> URL { url ?? URL(string: "https://unset.invalid")! }
    static var relayUS: URL? { url("TacieRelayUS") }
    static var updateFeed: URL? { url("SUFeedURL") }
    static var postHogToken: String? { value("TaciePostHogToken") }
    static var postHogHost: String? { value("TaciePostHogHost") }

    /// Where the models and update notes are downloaded from; the open edition uses the same place (docs/41 13번).
    static func modelsBase(in info: [String: Any] = Bundle.main.infoDictionary ?? [:]) -> URL {
        value("TacieModelsBase", in: info).flatMap(URL.init(string:)) ?? URL(string: "https://models.tacie.app")!
    }
    static var modelsBase: URL { modelsBase() }
}
