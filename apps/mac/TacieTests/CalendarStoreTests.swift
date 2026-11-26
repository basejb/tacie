import EventKit
import XCTest

@MainActor
final class CalendarStoreTests: XCTestCase {
    func testAccessMapping() {
        XCTAssertEqual(CalendarStore.access(on: true, status: .fullAccess), .granted)
        XCTAssertEqual(CalendarStore.access(on: true, status: .denied), .denied)
        XCTAssertEqual(CalendarStore.access(on: true, status: .restricted), .denied)
        XCTAssertEqual(CalendarStore.access(on: true, status: .writeOnly), .denied)
        XCTAssertEqual(CalendarStore.access(on: true, status: .notDetermined), .notAsked)
        XCTAssertEqual(CalendarStore.access(on: false, status: .fullAccess), .off)
        XCTAssertEqual(CalendarStore.access(on: false, status: .denied), .off)
    }

    func testInviteeDropsMeAndReadsMailto() {
        XCTAssertNil(CalendarStore.invitee(name: "나", url: URL(string: "mailto:me@x.com"), isMe: true))
        XCTAssertEqual(CalendarStore.invitee(name: "정하늘", url: URL(string: "mailto:sky@x.com"), isMe: false),
                       MeetingInvitee(name: "정하늘", email: "sky@x.com"))
        // a name that only repeats the address is no name
        XCTAssertEqual(CalendarStore.invitee(name: "b@x.com", url: URL(string: "mailto:b@x.com"), isMe: false),
                       MeetingInvitee(name: nil, email: "b@x.com"))
        XCTAssertEqual(CalendarStore.invitee(name: "윤가람", url: URL(string: "https://x.com/u"), isMe: false),
                       MeetingInvitee(name: "윤가람", email: nil))
        XCTAssertNil(CalendarStore.invitee(name: " ", url: nil, isMe: false))
    }

    private struct Stub: CalendarSource {
        var delay: Double = 0
        var found: [CalendarEvent] = []
        func events(from: Date, to: Date, calendars: Set<String>?) -> [CalendarEvent] {
            if delay > 0 { Thread.sleep(forTimeInterval: delay) }
            return found
        }
        func calendars() -> [CalendarInfo] { [] }
    }

    func testEventNowPicksAndSlowCalendarGivesUp() async {
        let at = Date()
        let e = CalendarEvent(id: "ev", title: "주간 제품 회의", start: at, end: at + 1800, allDay: false, texts: [], invited: [])
        let defaults = UserDefaults(suiteName: "calendar-test-\(UUID().uuidString)")!
        defaults.set(true, forKey: AccountSpace.inUse.key(CalendarStore.onKey))
        let fast = CalendarStore(defaults: defaults, source: Stub(found: [e]), status: { .fullAccess }, reset: {})
        let picked = await fast.eventNow(at: at, app: nil)
        XCTAssertEqual(picked?.id, "ev")
        let slow = CalendarStore(defaults: defaults, source: Stub(delay: 3, found: [e]), status: { .fullAccess }, reset: {})
        let started = Date()
        let none = await slow.eventNow(at: at, app: nil, limit: 0.2)
        XCTAssertNil(none)
        XCTAssertLessThan(Date().timeIntervalSince(started), 1.5)
        let denied = CalendarStore(defaults: defaults, source: Stub(found: [e]), status: { .denied }, reset: {})
        let nothing = await denied.eventNow(at: at, app: nil)
        XCTAssertNil(nothing)
    }

    /// Review I2: a store made before access was granted lists nothing until reset, so a grant made later resets it.
    func testGrantLaterResetsTheStore() {
        var status = EKAuthorizationStatus.denied
        var resets = 0
        let defaults = UserDefaults(suiteName: "calendar-test-\(UUID().uuidString)")!
        defaults.set(true, forKey: AccountSpace.inUse.key(CalendarStore.onKey))
        let store = CalendarStore(defaults: defaults, source: Stub(), status: { status }, reset: { resets += 1 })
        XCTAssertEqual(store.access, .denied)
        status = .fullAccess
        store.refresh()
        XCTAssertEqual(store.access, .granted)
        XCTAssertEqual(resets, 1)
        store.refresh()
        XCTAssertEqual(resets, 1)  // only on the change
    }

    func testCalendarKeysMoveWithAccount() {
        XCTAssertTrue(SpaceAdoption.defaultsKeys.contains(CalendarStore.onKey))
        XCTAssertTrue(SpaceAdoption.defaultsKeys.contains(CalendarStore.idsKey))
    }
}
