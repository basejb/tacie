import Foundation
import PostHog

/// Usage numbers to PostHog (docs/16). App code calls only this; the events themselves are in AnalyticsEvent.swift.
/// On from the start (no switch, docs/16 0절); nothing is captured by the SDK on its own, and `beforeSend` drops
/// any event or key outside the allow-list.
extension Analytics {
    private static var started = false
    static let lastVersionKey = "analytics.lastVersion"

    /// Starts PostHog once signed in (launch, and again after each sign-in); signed out it stops sending.
    @MainActor static func start() {
        let token = BuildValues.postHogToken ?? ""
        let host = BuildValues.postHogHost ?? ""
        let signedIn = TacieAccount.shared.signedIn
        if started {
            if signedIn { PostHogSDK.shared.optIn() } else { PostHogSDK.shared.optOut() }
            return
        }
        // a test run (even one with an app host) never reports
        guard allowed(signedIn: signedIn, hasToken: !token.isEmpty && !host.isEmpty),
              ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] == nil else { return }
        let config = PostHogConfig(projectToken: token, host: host)
        config.captureApplicationLifecycleEvents = false  // app_opened is ours
        config.captureScreenViews = false
        config.enableSwizzling = false
        config.preloadFeatureFlags = false
        config.errorTrackingConfig.autoCapture = false
        config.setDefaultPersonProperties = false
        config.personProfiles = .identifiedOnly
        config.setBeforeSend { event in
            guard let kept = AnalyticsFilter.filter(event: event.event, properties: event.properties) else { return nil }
            event.properties = kept
            return event
        }
        PostHogSDK.shared.setup(config)
        // the SDK keeps an opt-out from a sign-out on disk; a sign-in now must undo it
        PostHogSDK.shared.optIn()
        started = true
        sink = { e in PostHogSDK.shared.capture(e.name, properties: e.properties.mapValues(\.json)) }
        // `account` stays as the last run left it until /me answers (AccountAnalytics.refreshed)
        PostHogSDK.shared.register(["build": (Build.isDev ? ABuild.dev : ABuild.product).rawValue, "platform": APlatform.mac.rawValue])
    }

    static func identify(analyticsId: String) {
        guard started else { return }
        PostHogSDK.shared.identify(analyticsId)
    }

    /// Signed out: the next events belong to a new anonymous id.
    static func reset() {
        guard started else { return }
        PostHogSDK.shared.reset()
        PostHogSDK.shared.register(["build": (Build.isDev ? ABuild.dev : ABuild.product).rawValue, "platform": APlatform.mac.rawValue])
    }

    static func setAccount(_ a: AAccount) {
        guard started else { return }
        PostHogSDK.shared.register(["account": a.rawValue])
    }

    /// At launch: app_opened, then app_updated when this Mac last ran another version.
    static func launched(loginItem: Bool, firstLaunch: Bool) {
        capture(.appOpened(firstLaunch: firstLaunch, loginItem: loginItem))
        let current = (Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String) ?? ""
        let defaults = UserDefaults.standard
        if let u = VersionCheck.update(last: defaults.string(forKey: lastVersionKey), current: current) {
            capture(.appUpdated(from: u.from, to: u.to))
        }
        defaults.set(current, forKey: lastVersionKey)
    }
}

extension AccountAnalytics {
    @MainActor static let live = AccountAnalytics(
        signedIn: { status, id, with in
            Analytics.setAccount(status)
            Analytics.capture(.signedIn(status: status, with: with))
            if let id { Analytics.identify(analyticsId: id) }
        },
        refreshed: { status, id in
            Analytics.setAccount(status)
            if let id { Analytics.identify(analyticsId: id) }
        },
        signedOut: { reason in
            Analytics.capture(.signedOut(reason: reason))
            Analytics.reset()
            Analytics.setAccount(.none)
        })
}
