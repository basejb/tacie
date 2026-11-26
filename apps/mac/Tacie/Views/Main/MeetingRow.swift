import SwiftUI

/// One meeting in the main window's list.
struct MeetingRow: View {
    let meeting: SavedMeeting
    let live: Bool
    var warning: String? = nil
    let now: Date
    let selected: Bool
    let ns: Namespace.ID
    /// The time and the summary's first line under the title; false: one line, the title and the date.
    var full = true
    /// The chosen wash glides here; false for a second row of the same meeting (고정한 회의).
    var glide = true
    /// Set by the list, which also holds the row's •••: the highlight stays while the pointer is on the •••.
    var hovering: Bool? = nil
    @State private var hovered = false

    private var waiting: Bool { !live && meeting.notes == .waiting }

    private var meta: String { meeting.meta(now: now, calendar: MainView.calendar) }

    /// Under the title: the first summary line; while live or being written, what is going on.
    private var line: String? {
        if live, let warning { return warning }
        if live {
            return meeting.questions.isEmpty ? String(localized: "듣는 중이에요")
                : String(localized: "질문 \(meeting.questions.count)개를 잡았어요")
        }
        if waiting { return String(localized: "요약을 쓰는 중이에요") }
        return meeting.shownSummary.first
    }

    var body: some View {
        Group {
            if full { twoLines } else { oneLine }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background { RowHighlight(selected: selected, hovered: hovering ?? hovered, ns: ns, glide: glide) }
        .contentShape(Rectangle())
        .onHover { inside in withAnimation(.easeOut(duration: inside ? 0.12 : 0.2)) { hovered = inside } }
    }

    private var oneLine: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            if live { LiveDot(warn: warning != nil) }
            Text(meeting.displayTitle(calendar: MainView.calendar)).uiFont(13, weight: .medium).lineLimit(1)
            Spacer(minLength: 6)
            HStack(spacing: 5) {
                if meeting.fromIPhone { IPhoneChip(compact: true) }
                Text(SavedMeeting.format("M월 d일", meeting.start, MainView.calendar))
                    .uiFont(11.5).foregroundStyle(Theme.ink3).monospacedDigit()
                    .lineLimit(1).fixedSize()
            }
            .opacity(hovering == true ? 0 : 1)  // the ••• takes its place
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
    }

    private var twoLines: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                if live { LiveDot(warn: warning != nil) }
                Text(meeting.displayTitle(calendar: MainView.calendar)).uiFont(13.5, weight: .semibold).lineLimit(1)
            }
            HStack(spacing: 4) {
                Text(meta).uiFont(11.5).foregroundStyle(Theme.ink3).monospacedDigit()
                if meeting.fromIPhone { IPhoneChip() }
            }
            if let line {
                Group {
                    if waiting {
                        ShimmerText(text: line)
                    } else {
                        Text(line).uiFont(12.5).foregroundStyle(live && warning != nil ? AnyShapeStyle(Theme.warn) : live ? AnyShapeStyle(Theme.ink3) : AnyShapeStyle(Theme.ink2))
                            .lineLimit(2).lineSpacing(1.5)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .padding(.top, 2)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 9)
    }
}

/// The chosen row's wash, which glides from row to row, or the lighter one under the pointer.
struct RowHighlight: View {
    let selected: Bool
    let hovered: Bool
    let ns: Namespace.ID
    var glide = true

    var body: some View {
        if selected {
            if glide {
                RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.ink.opacity(0.065))
                    .matchedGeometryEffect(id: "row-on", in: ns)
            } else {
                RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.ink.opacity(0.065))
            }
        } else if hovered {
            // a lighter wash than the chosen row's, so the two never read alike
            RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.ink.opacity(0.035))
                .transition(.opacity)
        }
    }
}
