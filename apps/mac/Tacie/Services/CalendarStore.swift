import EventKit
import Foundation
import os

/// Where the calendar card stands (docs/35 3-2). `off`: the person has not connected it in Tacie (or disconnected it).
/// `denied` also covers a Mac whose rules forbid it and write-only access, which cannot read events.
enum CalendarAccess: Equatable { case off, notAsked, granted, denied }

/// What `CalendarStore` reads; EventKit in the app, a stub in tests.
protocol CalendarSource: Sendable {
    func events(from: Date, to: Date, calendars: Set<String>?) -> [CalendarEvent]
    func calendars() -> [CalendarInfo]
}

/// One calendar the picker lists, under its account ("iCloud", "minho@company.com").
struct CalendarInfo: Equatable, Identifiable {
    let id: String
    let title: String
    let account: String
    /// The colour the Calendar app shows it in (0xRRGGBB), for the picker's dot.
    var color: UInt32? = nil

    static func rgb(_ color: CGColor?) -> UInt32? {
        guard let c = color?.converted(to: CGColorSpace(name: CGColorSpace.sRGB)!, intent: .defaultIntent, options: nil),
              let p = c.components, p.count >= 3 else { return nil }
        let b = { (v: CGFloat) in UInt32(max(0, min(255, (v * 255).rounded()))) }
        return b(p[0]) << 16 | b(p[1]) << 8 | b(p[2])
    }
}

/// The calendar link on this Mac: whether it is on, which calendars, and finding the event a meeting starts in.
/// Event contents stay here; only counts are logged.
@MainActor
final class CalendarStore: ObservableObject {
    static let shared = CalendarStore()

    /// Whether the app offers the calendar link at all. Held back from shipped builds for now (2026-11-15, user call):
    /// off hides the settings row and its search entry and never looks up an event at meeting start, so a switch
    /// turned on earlier does nothing. The code stays; dev builds keep it, and it comes back by flipping this.
    static let offered = Build.isDev

    /// Per-account settings (AccountSpace, SpaceAdoption.defaultsKeys).
    static let onKey = "calendarOn"
    static let idsKey = "calendarIDs"

    @Published var access: CalendarAccess = .off
    /// The calendars meetings look in; nil is every calendar.
    @Published private(set) var chosen: Set<String>?

    private let store: EKEventStore
    private let source: CalendarSource
    private let defaults: UserDefaults
    private let status: () -> EKAuthorizationStatus
    private let resetStore: () -> Void
    private var changes: NSObjectProtocol?

    /// `status` and `reset` stand in for EventKit in tests.
    init(defaults: UserDefaults = .standard, source: CalendarSource? = nil,
         status: (() -> EKAuthorizationStatus)? = nil, reset: (() -> Void)? = nil) {
        self.defaults = defaults
        let store = EKEventStore()
        self.store = store
        self.source = source ?? EventKitSource(store: store)
        self.status = status ?? { EKEventStore.authorizationStatus(for: .event) }
        self.resetStore = reset ?? { store.reset() }
        if source == nil {
            // calendars or accounts changed in the Calendar app, or the permission did
            changes = NotificationCenter.default.addObserver(forName: .EKEventStoreChanged, object: store, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.refresh() }
            }
        }
        reopen()
    }

    private var onKeyInUse: String { AccountSpace.inUse.key(Self.onKey) }
    private var idsKeyInUse: String { AccountSpace.inUse.key(Self.idsKey) }

    /// Reads the account's settings again (after an account switch) and the Mac's permission.
    func reopen() {
        chosen = (defaults.array(forKey: idsKeyInUse) as? [String]).map(Set.init)
        refresh()
    }

    /// The Mac's permission may change in System Settings while Tacie runs; called when the app comes forward.
    func refresh() {
        let was = access
        access = Self.access(on: defaults.bool(forKey: onKeyInUse), status: status())
        // a store made before access was granted (say, allowed later in System Settings) lists nothing until reset
        if access == .granted, was != .granted { resetStore() }
    }

    /// 연결: asks macOS the first time; afterwards only turns the link on.
    func connect() async {
        defaults.set(true, forKey: onKeyInUse)
        if status() == .notDetermined {
            _ = try? await store.requestFullAccessToEvents()
        }
        refresh()
    }

    /// 연결 끊기: Tacie stops reading the calendar. The macOS permission stays until the person turns it off there.
    func disconnect() {
        defaults.set(false, forKey: onKeyInUse)
        refresh()
    }

    func choose(_ ids: Set<String>?) {
        chosen = ids
        if let ids { defaults.set(Array(ids).sorted(), forKey: idsKeyInUse) } else { defaults.removeObject(forKey: idsKeyInUse) }
    }

    func calendars() -> [CalendarInfo] { access == .granted ? source.calendars() : [] }

    /// The event a meeting starting `at` belongs to, if any (CalendarLink.pick). Read off the main thread and given up
    /// after `limit` seconds: a slow calendar never holds a meeting up.
    func eventNow(at: Date, app: MeetingApp?, limit: Double = 1) async -> CalendarEvent? {
        refresh()  // the permission may have changed in System Settings meanwhile
        guard access == .granted else { return nil }
        let source = source, ids = chosen
        return await lookupWithin(limit) {
            let found = source.events(from: at.addingTimeInterval(-CalendarLink.window),
                                      to: at.addingTimeInterval(CalendarLink.window), calendars: ids)
            return CalendarLink.pick(found, at: at, app: app)
        }
    }

    /// The events of a meeting's day, for changing the linked one (the meeting page's event line).
    func events(on day: Date, calendar: Calendar = .current) -> [CalendarEvent] {
        guard access == .granted else { return [] }
        let start = calendar.startOfDay(for: day)
        return source.events(from: start, to: start.addingTimeInterval(86_400), calendars: chosen)
            .filter { !$0.allDay }.sorted { $0.start < $1.start }
    }

    // MARK: debug control (E2E on this Mac)

    static let testTitle = "Tacie 캘린더 시험"

    /// The only calendar end-to-end runs may write to; never the default calendar (it can be a shared work one).
    static let testCalendar = "Tacie Test"

    private var testCalendar: EKCalendar? {
        store.calendars(for: .event).first { $0.title == Self.testCalendar && $0.allowsContentModifications }
    }

    /// A plain event (no invitees, so nobody gets mail) in the "Tacie Test" calendar, for an end-to-end run.
    func debugMakeTestEvent(startingIn minutes: Double) throws {
        guard let calendar = testCalendar else { throw CocoaError(.fileNoSuchFile) }
        let e = EKEvent(eventStore: store)
        e.title = Self.testTitle
        e.startDate = Date().addingTimeInterval(minutes * 60)
        e.endDate = e.startDate.addingTimeInterval(1800)
        e.location = "https://zoom.us/j/0000000000"
        e.calendar = calendar
        try store.save(e, span: .thisEvent)
    }

    /// Removes the test events of the last and next day from the "Tacie Test" calendar only; returns how many.
    func debugDeleteTestEvents() -> Int {
        guard let calendar = testCalendar else { return 0 }
        let p = store.predicateForEvents(withStart: Date().addingTimeInterval(-86_400), end: Date().addingTimeInterval(86_400), calendars: [calendar])
        var n = 0
        for e in store.events(matching: p) where e.title == Self.testTitle {
            if (try? store.remove(e, span: .thisEvent)) != nil { n += 1 }
        }
        return n
    }

    // MARK: pure parts (tested)

    nonisolated static func access(on: Bool, status: EKAuthorizationStatus) -> CalendarAccess {
        guard on else { return .off }
        switch status {
        case .fullAccess: return .granted
        case .notDetermined: return .notAsked
        default: return .denied  // denied, restricted, write-only
        }
    }

    /// An attendee as the meeting keeps them: without the person using this Mac, the email from a `mailto:` address.
    nonisolated static func invitee(name: String?, url: URL?, isMe: Bool) -> MeetingInvitee? {
        guard !isMe else { return nil }
        let email = url.flatMap { $0.scheme?.lowercased() == "mailto" ? String($0.absoluteString.dropFirst(7)) : nil }
            .flatMap { $0.removingPercentEncoding }
        let n = name?.trimmingCharacters(in: .whitespacesAndNewlines)
        let shown = (n?.isEmpty ?? true) || n == email ? nil : n
        guard shown != nil || email != nil else { return nil }
        return MeetingInvitee(name: shown, email: email)
    }
}

/// What `work` gives within `seconds`; nil when it takes longer. The work is left to finish on its own: a task group
/// would wait for it, and an EventKit read does not stop when cancelled.
func lookupWithin<T: Sendable>(_ seconds: Double, _ work: @escaping @Sendable () async -> T?) async -> T? {
    let gate = OSAllocatedUnfairLock(initialState: false)
    return await withCheckedContinuation { (c: CheckedContinuation<T?, Never>) in
        let finish: @Sendable (T?) -> Void = { value in
            if gate.withLock({ done in defer { done = true }; return !done }) { c.resume(returning: value) }
        }
        Task.detached { finish(await work()) }
        Task.detached { try? await Task.sleep(for: .seconds(seconds)); finish(nil) }
    }
}

/// EKEventStore reads are safe from any thread.
private struct EventKitSource: CalendarSource, @unchecked Sendable {
    let store: EKEventStore

    func events(from: Date, to: Date, calendars ids: Set<String>?) -> [CalendarEvent] {
        let cals = ids.map { ids in store.calendars(for: .event).filter { ids.contains($0.calendarIdentifier) } }
        if let cals, cals.isEmpty { return [] }
        let predicate = store.predicateForEvents(withStart: from, end: to, calendars: cals)
        return store.events(matching: predicate).filter { $0.status != .canceled }.map { e in
            CalendarEvent(id: e.calendarItemIdentifier, title: e.title ?? "", start: e.startDate, end: e.endDate,
                          allDay: e.isAllDay, texts: [e.location, e.notes, e.url?.absoluteString].compactMap { $0 },
                          invited: (e.attendees ?? []).compactMap {
                              CalendarStore.invitee(name: $0.name, url: $0.url, isMe: $0.isCurrentUser)
                          })
        }
    }

    func calendars() -> [CalendarInfo] {
        store.calendars(for: .event)
            .filter { $0.type != .birthday && $0.type != .subscription }
            .map { CalendarInfo(id: $0.calendarIdentifier, title: $0.title, account: $0.source.title, color: CalendarInfo.rgb($0.cgColor)) }
            .sorted { ($0.account, $0.title) < ($1.account, $1.title) }
    }
}
