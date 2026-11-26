import SwiftUI

/// The linked calendar event under the meeting page's title (docs/35 3-3, layout B): "일정 오후 2:00–3:00 · 초대된 사람 5명 ⌄"
/// (event 2:00–3:00 PM · 5 invited). Pressing it opens `MeetingEventMenu` (not while the meeting goes on).
struct MeetingEventLine: View {
    let event: MeetingEvent
    let invited: Int
    let calendar: Calendar
    var lit = false

    static func text(_ event: MeetingEvent, invited: Int, calendar: Calendar) -> String {
        let when = span(event, calendar: calendar)
        return invited > 0 ? String(localized: "일정 \(when) · 초대된 사람 \(invited)명") : String(localized: "일정 \(when)")
    }

    /// "오후 2:00–3:00"; the second 오전/오후 only when it changes ("오전 11:30–오후 12:30"), the date when the day does.
    static func span(_ e: MeetingEvent, calendar: Calendar) -> String {
        let start = SavedMeeting.format("a h:mm", e.start, calendar)
        let sameHalf = calendar.component(.hour, from: e.start) / 12 == calendar.component(.hour, from: e.end) / 12
        let end: String
        if !calendar.isDate(e.start, inSameDayAs: e.end) {
            end = SavedMeeting.format("M월 d일 a h:mm", e.end, calendar)
        } else {
            end = SavedMeeting.format(sameHalf ? "h:mm" : "a h:mm", e.end, calendar)
        }
        return "\(start)–\(end)"
    }

    var body: some View {
        HStack(spacing: 5) {
            Image(systemName: "calendar").uiFont(11.5, weight: .medium)
            Text(Self.text(event, invited: invited, calendar: calendar)).lineLimit(1)
            Image(systemName: "chevron.down").uiFont(8.5, weight: .semibold)
        }
        .uiFont(12.5)
        .foregroundStyle(lit ? AnyShapeStyle(Theme.ink2) : AnyShapeStyle(Theme.ink3))
        .contentShape(Rectangle())
    }
}

/// The day's events to link the meeting to instead, the linked one checked, and 일정 떼어 내기.
struct MeetingEventMenu: View {
    let day: Date
    let linked: String
    let events: [CalendarEvent]
    let calendar: Calendar
    var pick: (CalendarEvent) -> Void = { _ in }
    var unlink: () -> Void = {}
    @State private var hover: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !events.isEmpty {
                Text(verbatim: String(localized: "\(SavedMeeting.format("M월 d일", day, calendar)) 일정")).uiFont(11.5, weight: .semibold)
                    .foregroundStyle(Theme.ink3)
                    .padding(.leading, 10).padding(.bottom, 4)
                ForEach(events, id: \.id) { e in
                    let on = e.id == linked
                    HStack(spacing: 10) {
                        Image(systemName: "checkmark").uiFont(11, weight: .bold).foregroundStyle(Theme.ink).opacity(on ? 1 : 0)
                            .frame(width: 12)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(verbatim: e.title).uiFont(13.5, weight: .medium).foregroundStyle(Theme.ink).lineLimit(1)
                            Text(MeetingEventLine.span(MeetingEvent(id: e.id, title: e.title, start: e.start, end: e.end), calendar: calendar))
                                .uiFont(11.5).foregroundStyle(Theme.ink3)
                        }
                        Spacer(minLength: 8)
                    }
                    .padding(.horizontal, 10).frame(height: 42)
                    .background(on || hover == e.id ? Theme.fill : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .contentShape(Rectangle())
                    .onHover { hover = $0 ? e.id : (hover == e.id ? nil : hover) }
                    .onTapGesture { if !on { pick(e) } }
                }
                Rectangle().fill(Theme.hairline).frame(height: 1).padding(.vertical, 6).padding(.horizontal, 6)
            }
            Text("일정 떼어 내기").uiFont(13).foregroundStyle(Theme.ink2)
                .padding(.horizontal, 10).frame(maxWidth: .infinity, minHeight: 30, alignment: .leading)
                .background(hover == "unlink" ? Theme.fill : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .contentShape(Rectangle())
                .onHover { hover = $0 ? "unlink" : (hover == "unlink" ? nil : hover) }
                .onTapGesture(perform: unlink)
        }
        .padding(8)
        .frame(width: 300)
        .background(Theme.raised)
    }
}
