import Foundation

enum Build {
    static let isDev: Bool = {
        #if DEBUG
        true
        #else
        false
        #endif
    }()
    /// A source build (docs/41 0절, #56): no official values, so no Tacie sign-in; its data lives in `Tacie Open`. Set by
    /// apps/mac/Build.xcconfig and taken away by Official.xcconfig, so a target that reads neither (the iPhone app) is
    /// never one. Behaviour follows the AI choice (`AIMode`) and sign-in, not this flag.
    static let isOpen: Bool = {
        #if TACIE_OPEN
        true
        #else
        false
        #endif
    }()
    /// A development build (bundle id `ai.tacie.app.dev`) keeps its keychain items, logs, meetings and remembered
    /// people apart from the installed app, so a development build never reads, replaces or deletes the installed app's
    /// connections. The open edition keeps apart from both the same way. Only the downloaded models
    /// (`Application Support/Tacie/models`) stay shared.
    static func names(dev: Bool, open: Bool) -> BuildNames {
        switch (dev, open) {
        case (false, false): BuildNames(supportFolder: "Tacie", keychainSuffix: "", logFolder: "tacie", mcpName: "tacie")
        case (true, false): BuildNames(supportFolder: "Tacie Dev", keychainSuffix: ".dev", logFolder: "tacie-dev", mcpName: "tacie-dev")
        case (false, true): BuildNames(supportFolder: "Tacie Open", keychainSuffix: ".open", logFolder: "tacie-open", mcpName: "tacie-open")
        case (true, true): BuildNames(supportFolder: "Tacie Open Dev", keychainSuffix: ".open.dev", logFolder: "tacie-open-dev", mcpName: "tacie-open-dev")
        }
    }
    static let current = names(dev: isDev, open: isOpen)
    /// Tacie sign-in exists: a build with the official values (the account server), never a source build.
    static func hasAccount(open: Bool, accountServer: URL?) -> Bool { !open && accountServer != nil }
    /// The compile flag decides it: Official.xcconfig both clears `TACIE_OPEN` and sets the account server, and the test
    /// bundle cannot read the app's Info.plist values.
    static let hasAccount = !isOpen
    static let keychainSuffix = current.keychainSuffix
    static let logFolder = current.logFolder
    static let supportFolder = current.supportFolder
    /// The dev setting that picks which GitHub login a development build uses ("gh" or "tacie").
    static let githubLoginKey = "dev.githubLogin"
    /// The Tacie connection (device code, keychain) is the GitHub login: always in a product build, by setting in a development one.
    static var usesTacieLogin: Bool {
        !isDev || UserDefaults.standard.string(forKey: githubLoginKey) == "tacie"
    }
}

/// The names a build keeps apart by (`Build.names`).
struct BuildNames: Equatable {
    let supportFolder: String
    let keychainSuffix: String
    let logFolder: String
    let mcpName: String
}
