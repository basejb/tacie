import Foundation

/// A calendar event as the matcher sees it (read from EventKit by `CalendarStore`).
struct CalendarEvent: Equatable {
    var id: String
    var title: String
    var start: Date
    var end: Date
    var allDay: Bool
    /// Where a call link may sit: the location, the notes and the event's URL.
    var texts: [String]
    var invited: [MeetingInvitee]
}

/// The video service a call link points at.
enum VideoHost: Equatable { case zoom, teams, meet }

/// Which calendar event a meeting belongs to (docs/35 3-3).
enum CalendarLink {
    /// An event counts when it starts this close to the meeting, before or after.
    static let window: TimeInterval = 600

    /// Not all-day, not over, starting within `window` of `at`. Among those: the one carrying the starting app's
    /// link, then any call link, then the closest start.
    static func pick(_ events: [CalendarEvent], at: Date, app: MeetingApp?) -> CalendarEvent? {
        let mine = app.flatMap(host(of:))
        return events
            .filter { !$0.allDay && $0.end > at && abs($0.start.timeIntervalSince(at)) <= window }
            .min { a, b in
                let ha = host(of: a), hb = host(of: b)
                let ma = mine != nil && ha == mine, mb = mine != nil && hb == mine
                if ma != mb { return ma }
                if (ha != nil) != (hb != nil) { return ha != nil }
                return abs(a.start.timeIntervalSince(at)) < abs(b.start.timeIntervalSince(at))
            }
    }

    /// The service of the first call link in the event's location, notes or URL.
    static func host(of event: CalendarEvent) -> VideoHost? {
        for text in event.texts {
            let t = text.lowercased()
            if t.contains("zoom.us/") { return .zoom }
            if t.contains("teams.microsoft.com/") || t.contains("teams.live.com/") { return .teams }
            if t.contains("meet.google.com/") { return .meet }
        }
        return nil
    }

    /// The service a meeting app joins calls on; Meet runs in the browser.
    static func host(of app: MeetingApp) -> VideoHost? {
        switch app {
        case .zoom: .zoom
        case .teams: .teams
        case .browser: .meet
        default: nil
        }
    }

    /// Titles that only name the tool or say nothing; the notes' own title reads better than these.
    static let generic: Set<String> = [
        "zoom", "zoom meeting", "zoom 회의", "teams", "teams meeting", "microsoft teams meeting", "google meet", "meet",
        "meeting", "회의", "미팅", "untitled", "제목 없음", "(no title)", "call", "통화",
    ]

    static func meaningful(_ title: String) -> Bool {
        let t = title.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return !t.isEmpty && !generic.contains(t)
    }
}
