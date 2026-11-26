import SwiftUI

/// A meeting page's Summary when the month's Tacie AI ran out (NotesLimit): Tacie resting, why, what was kept and when
/// the notes can be written; once the month turns, Write summary. The account's month is the one the server last said.
struct LimitNotesBlock: View {
    let meeting: SavedMeeting
    let limit: NotesLimit
    /// 0…1 of the month left as the server last said; nil when it has not said.
    let left: Double?
    let write: () -> Void
    let usage: () -> Void
    private var ready: Bool { (left ?? 0) > 0 }
    private var recorded: Bool { meeting.recording == .kept }

    var body: some View {
        let until = UsageMonth.untilReset()
        HStack(alignment: .top, spacing: 16) {
            CueEyes(mood: ready ? .happy : .asleep, height: 30).padding(.top, 2)
            VStack(alignment: .leading, spacing: 0) {
                Text(NotesLimitText.kicker).uiFont(12, weight: .semibold).foregroundStyle(Theme.ink3)
                Text(NotesLimitText.title(limit, ready: ready)).uiFont(16, weight: .semibold).foregroundStyle(Theme.ink)
                    .padding(.top, 4)
                Text(NotesLimitText.detail(limit, ready: ready, recorded: recorded, until: until))
                    .uiFont(14).foregroundStyle(Theme.ink2).lineSpacing(3).fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 6)
                meter(until: until).padding(.top, 16)
                HStack(spacing: 4) {
                    if ready {
                        Button(NotesLimitText.write, action: write).buttonStyle(InkButtonStyle())
                    } else {
                        Button(action: usage) {
                            Text(NotesLimitText.usage).uiFont(13, weight: .medium).foregroundStyle(Theme.ink3)
                                .padding(.vertical, 6).contentShape(Rectangle())
                        }
                        .buttonStyle(PressStyle())
                    }
                }
                .padding(.top, 14)
            }
            .frame(maxWidth: 560, alignment: .leading)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 24).padding(.top, 22).padding(.bottom, 20)
        .background(Theme.fill, in: RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
    }

    private func meter(until: String) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                if let left {
                    Text(NotesLimitText.left(Int((left * 100).rounded()))).uiFont(12.5, weight: .medium, digits: true)
                        .foregroundStyle(Theme.ink)
                }
                Spacer()
                Text(NotesLimitText.reset(until, out: !ready)).uiFont(12.5, digits: true).foregroundStyle(Theme.ink3)
            }
            GeometryReader { g in
                ZStack(alignment: .leading) {
                    Capsule().fill(Theme.ink.opacity(0.08))
                    Capsule().fill(Theme.ink).frame(width: g.size.width * (1 - (left ?? 0)))
                }
            }
            .frame(height: 6)
            .padding(.top, 10)
            HStack(spacing: 14) {
                kept(NotesLimitText.transcript(minutes: meeting.limitMinutes))
                if recorded { kept(NotesLimitText.recording) }
            }
            .padding(.top, 10)
        }
        .padding(.horizontal, 16).padding(.vertical, 13)
        .background(Theme.raised, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.hairline))
    }

    private func kept(_ label: String) -> some View {
        HStack(spacing: 5) {
            Image(systemName: "checkmark").font(.system(size: 10, weight: .semibold)).foregroundStyle(Theme.ink3)
            Text(label).uiFont(12.5, digits: true).foregroundStyle(Theme.ink2)
        }
    }
}
