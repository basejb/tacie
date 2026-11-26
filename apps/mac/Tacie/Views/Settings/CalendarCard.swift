import AppKit
import SwiftUI

/// Settings → General → Meetings' calendar row (docs/35 3-2; moved out of Connections, whose cards are places answers
/// come from): connect, choose calendars, disconnect. Only this Mac's permission; no server login.
struct CalendarSettingsRow: View {
    @ObservedObject var store = CalendarStore.shared
    @State private var picking = false

    var body: some View {
        let total = store.calendars().count
        CalendarRowBody(status: CalendarStatus(access: store.access, chosen: store.chosen?.count, total: total),
                         actions: CalendarActions(
                             connect: {
                                 Task {
                                     await store.connect()
                                     Analytics.capture(.settingChanged(setting: .calendar, value: .on))
                                 }
                             },
                             pick: { picking = true },
                             openSettings: { OnboardingModel.openPrivacy("Privacy_Calendars") },
                             disconnect: {
                                 store.disconnect()
                                 Analytics.capture(.settingChanged(setting: .calendar, value: .off))
                             }))
            .sheet(isPresented: $picking) { CalendarPickerSheet(store: store) }
            .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
                store.refresh()
            }
            .onAppear { store.refresh() }
    }
}

/// 캘린더 고르기 over the Calendar app's calendars; every calendar chosen is saved as "all" (new ones join too).
struct CalendarPickerSheet: View {
    let store: CalendarStore
    @Environment(\.dismiss) private var dismiss
    @State private var calendars: [CalendarInfo] = []
    @State private var on: Set<String> = []

    var body: some View {
        CalendarPickerBody(calendars: calendars, on: $on, cancel: { dismiss() }, save: {
            store.choose(on.count == calendars.count ? nil : on)
            dismiss()
        })
        .onAppear {
            calendars = store.calendars()
            on = store.chosen.map { $0.intersection(calendars.map(\.id)) } ?? Set(calendars.map(\.id))
        }
    }
}
