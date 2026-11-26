import AppKit

/// A meeting app whose use of the microphone means a meeting has begun (docs/04 "회의 시작 알아채기").
enum MeetingApp: String, CaseIterable, Codable {
    case zoom, teams, slack, webex, facetime, discord, browser

    /// Its name in 설정 → 일반.
    var name: String {
        switch self {
        case .zoom: "Zoom"
        case .teams: "Microsoft Teams"
        case .slack: String(localized: "Slack 허들")
        case .webex: "Webex"
        case .facetime: "FaceTime"
        case .discord: "Discord"
        case .browser: String(localized: "브라우저(Meet 등)")
        }
    }

    /// What the pill says when it began.
    var started: String {
        switch self {
        case .zoom: String(localized: "Zoom 회의가 시작됐어요")
        case .teams: String(localized: "Teams 회의가 시작됐어요")
        case .slack: String(localized: "Slack 허들이 시작됐어요")
        case .webex: String(localized: "Webex 회의가 시작됐어요")
        case .facetime: String(localized: "FaceTime 통화가 시작됐어요")
        case .discord: String(localized: "Discord 통화가 시작됐어요")
        case .browser: String(localized: "브라우저에서 회의가 시작됐어요")
        }
    }

    /// Bundle ids of the app and its helpers (a helper's id starts with one of these and a dot). Safari's
    /// microphone runs in WebKit's GPU process, Chrome's and Edge's in their helpers, FaceTime's in avconferenced.
    private var bundleIDs: [String] {
        switch self {
        case .zoom: ["us.zoom.xos", "us.zoom"]
        case .teams: ["com.microsoft.teams2", "com.microsoft.teams"]
        case .slack: ["com.tinyspeck.slackmacgap"]
        case .webex: ["Cisco-Systems.Spark", "com.webex.meetingmanager", "com.cisco.webexmeetingsapp"]
        case .facetime: ["com.apple.FaceTime", "com.apple.avconferenced"]
        case .discord: ["com.hnc.Discord"]
        case .browser: ["com.google.Chrome", "com.apple.Safari", "com.apple.WebKit.GPU", "company.thebrowser.Browser",
                        "com.microsoft.edgemac", "org.mozilla.firefox", "com.brave.Browser", "com.vivaldi.Vivaldi",
                        "com.operasoftware.Opera", "com.kagi.kagimacOS"]
        }
    }

    /// The app's own icon for the pill, from the first of its bundle ids installed here; nil for the browser (any of
    /// several) and for an app not found.
    var icon: NSImage? {
        guard self != .browser else { return nil }
        for id in bundleIDs {
            if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: id) { return NSWorkspace.shared.icon(forFile: url.path) }
        }
        return nil
    }

    /// The meeting app a process belongs to, by its bundle id; nil for anything else (Tacie, dictation, music apps).
    static func of(bundleID id: String) -> MeetingApp? {
        allCases.first { app in app.bundleIDs.contains { id == $0 || id.hasPrefix($0 + ".") } }
    }

    // MARK: settings

    static let promptKey = "meetingPrompt"
    static let offKey = "meetingPromptOff"

    /// 설정 → 일반 "회의가 시작되면 알려 주기" (on unless turned off).
    static var promptOn: Bool {
        get { UserDefaults.standard.object(forKey: promptKey) as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: promptKey) }
    }

    /// The apps turned off one by one under it.
    static var off: Set<MeetingApp> {
        get { Set((UserDefaults.standard.stringArray(forKey: offKey) ?? []).compactMap(MeetingApp.init(rawValue:))) }
        set { UserDefaults.standard.set(newValue.map(\.rawValue).sorted(), forKey: offKey) }
    }
}

/// When a meeting app's microphone means "ask", and when its release means "the meeting is over". Pure: it is told
/// which apps hold the microphone and the time, and says what to do (tested in MeetingStartWatchTests).
struct MeetingStartWatch {
    /// The microphone held this long is a meeting, not a sound check or a voice message.
    static let hold: TimeInterval = 3
    /// Let go this long, the meeting in that app is over: a mute or a reconnect is shorter. A new one is asked again.
    static let gone: TimeInterval = 30
    /// Let go this long while Tacie listens to that app's meeting: the end countdown (MeetingModel.proposeEnd).
    static let release: TimeInterval = 8

    enum Event: Equatable {
        /// Ask whether to listen along (once per meeting of that app).
        case ask(MeetingApp)
        /// The app Tacie was listening along with let the microphone go.
        case ended(MeetingApp)
        /// An app asked about let the microphone go for `gone`: its meeting is over, the question goes away.
        case over(MeetingApp)
    }

    private var heldSince: [MeetingApp: Date] = [:]
    private var releasedAt: [MeetingApp: Date] = [:]
    private var asked: Set<MeetingApp> = []
    /// The app whose meeting Tacie listens to now.
    private(set) var linked: MeetingApp?
    private var endSent = false

    /// `holding`: the meeting apps using the microphone now. `listening`: Tacie is in a meeting, `remote` a remote one.
    /// `allowed`: the apps it may ask about (empty with the setting off).
    mutating func update(holding: Set<MeetingApp>, now: Date, listening: Bool, remote: Bool,
                         allowed: Set<MeetingApp>) -> [Event] {
        for app in holding {
            if heldSince[app] == nil { heldSince[app] = now }
            releasedAt[app] = nil
        }
        var events: [Event] = []
        for app in Array(heldSince.keys) where !holding.contains(app) {
            let since = releasedAt[app] ?? now
            releasedAt[app] = since
            if now.timeIntervalSince(since) >= Self.gone {
                heldSince[app] = nil
                releasedAt[app] = nil
                if asked.remove(app) != nil { events.append(.over(app)) }
            }
        }
        if listening {
            // a remote meeting is linked to the app holding the microphone: asked or not, its release ends it. An
            // in-person meeting is not: a call taken in the room does not end it.
            if linked == nil, remote { linked = MeetingApp.allCases.first { held($0, now: now) } }
            // a meeting that began while one was already on is not asked about afterwards
            for app in holding { asked.insert(app) }
            if let l = linked {
                if holding.contains(l) {
                    endSent = false
                } else if !endSent, let r = releasedAt[l], now.timeIntervalSince(r) >= Self.release {
                    endSent = true
                    events.append(.ended(l))
                }
            }
        } else {
            linked = nil
            endSent = false
            if let app = MeetingApp.allCases.first(where: { held($0, now: now) && !asked.contains($0) && allowed.contains($0) }) {
                asked.insert(app)
                events.append(.ask(app))
            }
        }
        return events
    }

    /// The next moment `update` would say something new without any change in who holds the microphone.
    func nextCheck(now: Date) -> Date? {
        var times: [Date] = []
        for (app, since) in heldSince where releasedAt[app] == nil && !asked.contains(app) { times.append(since + Self.hold) }
        for (app, at) in releasedAt {
            times.append(at + Self.gone)
            if app == linked && !endSent { times.append(at + Self.release) }
        }
        return times.filter { $0 > now }.min()
    }

    private func held(_ app: MeetingApp, now: Date) -> Bool {
        guard let since = heldSince[app], releasedAt[app] == nil else { return false }
        return now.timeIntervalSince(since) >= Self.hold
    }
}
