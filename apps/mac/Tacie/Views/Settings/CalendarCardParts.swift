import AppKit
import SwiftUI

/// What the calendar card says (docs/35 3-2). Built from plain values so the render tests can draw every state.
struct CalendarStatus {
    let access: CalendarAccess
    /// How many calendars are chosen; nil is every calendar.
    let chosen: Int?
    /// How many calendars the Calendar app has (birthdays and subscriptions left out).
    let total: Int

    var on: Bool { access == .granted }

    var line: String {
        switch access {
        case .off, .notAsked: String(localized: "회의를 시작하면 지금 열리는 일정의 제목과 초대된 사람을 채워요")
        case .denied: String(localized: "캘린더 접근이 허용되지 않았어요")
        case .granted:
            if total == 0 { String(localized: "캘린더 앱에 계정이 없어요") }
            else if let chosen, chosen < total { String(localized: "캘린더 \(chosen)개") }
            else { String(localized: "모든 캘린더") }
        }
    }

    /// A second line: where the event goes, or what to do.
    var note: String? {
        switch access {
        case .off, .notAsked: nil
        case .denied: String(localized: "시스템 설정에서 Tacie의 캘린더 접근을 허용해 주세요.")
        case .granted:
            total == 0 ? String(localized: "Google이나 Microsoft 일정을 보려면 Mac의 캘린더 앱에 그 계정을 더해 주세요.")
                : String(localized: "일정은 이 Mac 밖으로 보내지 않아요. 초대된 사람의 이름만 받아쓰기에 보내요.")
        }
    }
}

/// What the calendar card can do; the store-backed wrapper fills it.
struct CalendarActions {
    var connect: () -> Void = {}
    var pick: () -> Void = {}
    var openSettings: () -> Void = {}
    var disconnect: () -> Void = {}
}

/// 설정 → 연결's calendar card, under Obsidian's.
struct CalendarCardBody: View {
    let status: CalendarStatus
    var actions = CalendarActions()

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            LogoTile(symbol: "calendar")
            VStack(alignment: .leading, spacing: 3) {
                Text("캘린더").uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                Text(status.line).uiFont(12.5).foregroundStyle(Theme.ink2).fixedSize(horizontal: false, vertical: true)
                if let note = status.note {
                    Text(note).uiFont(12).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 8)
            switch status.access {
            case .off, .notAsked:
                Button(action: actions.connect) { OutlineButton(label: String(localized: "연결")) }.buttonStyle(PressStyle())
            case .denied:
                Button(action: actions.openSettings) { OutlineButton(label: String(localized: "시스템 설정 열기")) }
                    .buttonStyle(PressStyle())
            case .granted:
                Menu {
                    Button("캘린더 고르기", action: actions.pick).disabled(status.total == 0)
                    Button("시스템 설정 열기", action: actions.openSettings)
                    Button("연결 끊기", role: .destructive, action: actions.disconnect)
                } label: {
                    Image(systemName: "ellipsis").uiFont(13, weight: .semibold).foregroundStyle(Theme.ink3)
                        .frame(width: 24, height: 20)
                        .contentShape(Rectangle())
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
                .padding(.top, 2)
            }
        }
        .padding(14)
        .background(Theme.paper, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Theme.ink.opacity(0.1)))
        .shadow(color: Theme.cardShadow, radius: 3, y: 1)
    }
}

/// 캘린더 고르기: the Calendar app's calendars by account, each with an ink check.
struct CalendarPickerBody: View {
    let calendars: [CalendarInfo]
    @Binding var on: Set<String>
    /// Past eight calendars the list scrolls; the render tests draw it flat (ImageRenderer leaves a ScrollView blank).
    var scrolls = true
    var cancel: () -> Void = {}
    var save: () -> Void = {}

    private var accounts: [(String, [CalendarInfo])] {
        var order: [String] = []
        var by: [String: [CalendarInfo]] = [:]
        for c in calendars {
            if by[c.account] == nil { order.append(c.account) }
            by[c.account, default: []].append(c)
        }
        return order.map { ($0, by[$0] ?? []) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("캘린더 고르기").uiFont(16, weight: .semibold).foregroundStyle(Theme.ink)
            Text("고른 캘린더에서 회의에 맞는 일정을 찾아요.")
                .uiFont(12.5).foregroundStyle(Theme.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            if scrolls && calendars.count > 8 {
                // a long list scrolls by itself so 저장 stays on screen
                ScrollView { list }.scrollIndicators(.automatic).frame(height: 320)
            } else {
                list
            }
            HStack(spacing: 8) {
                Spacer(minLength: 8)
                Button(action: cancel) { OutlineButton(label: String(localized: "취소")) }
                    .buttonStyle(PressStyle()).keyboardShortcut(.cancelAction)
                Button("저장", action: save)
                    .buttonStyle(InkButtonStyle())
                    .keyboardShortcut(.defaultAction)
                    .disabled(on.isEmpty)
            }
            .padding(.top, 18)
        }
        .padding(22)
        .frame(width: 420)
        .background(Theme.paper)
    }

    private var list: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(accounts, id: \.0) { account, list in
                Text(verbatim: account).uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink3)
                    .padding(.leading, 10).padding(.top, 14).padding(.bottom, 2)
                ForEach(list) { c in
                    PickRow(title: c.title, on: on.contains(c.id), toggle: { flip(c.id) }) { CalendarDot(color: c.color) }
                }
            }
        }
    }

    private func flip(_ id: String) {
        if on.contains(id) { on.remove(id) } else { on.insert(id) }
    }
}

/// A calendar's own colour as a small dot, the only colour in the picker; grey when the Calendar app gives none.
struct CalendarDot: View {
    let color: UInt32?

    var body: some View {
        Circle().fill(color.map { Color(hex: $0) } ?? Theme.ink4).frame(width: 8, height: 8)
    }
}

/// 설정 → 일반 → 회의's 캘린더 row (moved out of 연결, where the cards are places answers come from): the state on the
/// left, and on the right 연결, the chosen calendars as a menu, or 시스템 설정 열기.
struct CalendarRowBody: View {
    let status: CalendarStatus
    var actions = CalendarActions()

    /// One line under the title: what it does while off, what leaves this Mac while on, what to do otherwise.
    private var desc: String {
        switch status.access {
        case .off, .notAsked: status.line
        case .granted: status.note ?? status.line
        case .denied: status.note ?? status.line
        }
    }

    var body: some View {
        SettingsRow(title: String(localized: "캘린더"), desc: desc) {
            switch status.access {
            case .off, .notAsked:
                Button(action: actions.connect) { OutlineButton(label: String(localized: "연결")) }.buttonStyle(PressStyle())
            case .denied:
                Button(action: actions.openSettings) { OutlineButton(label: String(localized: "시스템 설정 열기")) }
                    .buttonStyle(PressStyle())
            case .granted:
                Menu {
                    Button("캘린더 고르기", action: actions.pick).disabled(status.total == 0)
                    Button("시스템 설정 열기", action: actions.openSettings)
                    Button("연결 끊기", role: .destructive, action: actions.disconnect)
                } label: {
                    MenuText(label: status.total == 0 ? String(localized: "캘린더 없음") : status.line)
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
            }
        }
    }
}
