import SwiftUI

/// How the main window shows an iPhone meeting's copy (design notes
/// 4-3): notes only, so nothing that plays, takes the meeting up, writes the notes again or names speakers shows. Its
/// title, to-dos, decisions and pin stay editable; the list and the delete dialog say iPhone.
enum IPhoneMeetingUI {
    /// What the page offers that needs the transcript or the recording.
    enum Action { case listen, resume, rewrite, rename, deleteRecording }

    static func allows(_ action: Action, for m: SavedMeeting) -> Bool { !m.fromIPhone }

    /// The 대화 기록 tab's one line; nil for this Mac's own meetings.
    static func transcriptNote(for m: SavedMeeting) -> String? {
        m.fromIPhone ? String(localized: "대화 기록은 iPhone에 있어요") : nil
    }

    /// Under 이 회의를 지울까요?: an iPhone meeting goes on the iPhone too.
    static func deleteMessage(for m: SavedMeeting) -> String {
        m.fromIPhone ? String(localized: "iPhone의 녹음과 대화 기록도 함께 지워져요")
            : String(localized: "받아쓴 대화와 정리가 이 Mac에서 사라져요.")
    }

    /// The iPhone meetings among these: 모두 지우기 deletes them on the iPhone too.
    static func iphoneCount(_ meetings: [SavedMeeting]) -> Int { meetings.filter(\.fromIPhone).count }

    /// The second line under 회의 기록을 모두 지울까요?; nil when there is no iPhone meeting.
    static func removeAllNote(for meetings: [SavedMeeting]) -> String? {
        let n = iphoneCount(meetings)
        return n > 0 ? String(localized: "iPhone에서 기록한 회의 \(n)개는 iPhone의 녹음과 대화 기록까지 함께 지워져요.") : nil
    }

    /// Under 회의 기록을 모두 지울까요?: as before with only this Mac's meetings; with iPhone meetings, all of them and
    /// then how many go on the iPhone too.
    static func removeAllMessage(for meetings: [SavedMeeting]) -> String {
        guard let note = removeAllNote(for: meetings) else {
            return String(localized: "이 Mac에 저장한 회의 \(meetings.count)개의 대화 기록과 회의 정리가 사라져요. 되돌릴 수 없어요.")
        }
        return String(localized: "이 Mac의 회의 기록 \(meetings.count)개가 모두 지워져요. 되돌릴 수 없어요.") + "\n" + note
    }

    /// What the small iPhone mark stands for: its name to VoiceOver; nil for this Mac's own meetings.
    static func rowTag(for m: SavedMeeting) -> String? { m.fromIPhone ? String(localized: "iPhone") : nil }

    /// The chip's tooltip.
    static var markHelp: String { String(localized: "iPhone에서 기록했어요") }

    /// Under the 대화 기록 tab's line: what is here and where the rest is.
    static var transcriptDetail: String {
        String(localized: "회의 정리만 이 Mac에 와요. 녹음과 대화 기록은 iPhone의 Tacie에서 이 회의를 열면 볼 수 있어요.")
    }

    // 설정 → 계정 → 기기: only while the account has iPhone meetings here.

    static func showsDevices(_ meetings: [SavedMeeting]) -> Bool { iphoneCount(meetings) > 0 }

    /// This Mac's row: how many of its own meetings are here.
    static func macLine(_ meetings: [SavedMeeting]) -> String {
        String(localized: "회의 \(meetings.count - iphoneCount(meetings))개")
    }

    /// The iPhone's row: how many, and when they last came.
    static func iphoneLine(_ meetings: [SavedMeeting], pulledAt: Date?, now: Date = Date(), calendar: Calendar = .current) -> String {
        let count = String(localized: "iPhone 회의 \(iphoneCount(meetings))개")
        guard let pulledAt else { return count }
        let when = pulledAt.formatted(date: calendar.isDate(pulledAt, inSameDayAs: now) ? .omitted : .abbreviated, time: .shortened)
        return count + " · " + String(localized: "\(when)에 받아 왔어요")
    }

    static var devicesFoot: String {
        String(localized: "iPhone 회의는 회의 정리만 이 Mac에 와요. 대화 기록과 녹음은 iPhone에만 있어요.")
    }
}

/// "[iPhone] iPhone" in a small pill after an iPhone meeting's time or date, like a label.
/// `compact`: the picture alone, where the row is narrow (the sidebar's 최근에 연 회의).
struct IPhoneChip: View {
    var compact = false
    var body: some View {
        HStack(spacing: 3) {
            Image(systemName: "iphone").font(.system(size: 9.5, weight: .semibold))
            if !compact { Text(verbatim: "iPhone").uiFont(10.5, weight: .semibold) }
        }
        .foregroundStyle(Theme.ink2)
        .padding(.horizontal, compact ? 5 : 6).frame(height: 17)
        .background(Theme.fill, in: Capsule())
        .fixedSize()
        .help(IPhoneMeetingUI.markHelp)
    }
}

/// The 대화 기록 tab when the transcript is not on this Mac: the iPhone, the line, and where to find the rest.
struct TranscriptElsewhere: View {
    let text: String
    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            Image(systemName: "iphone")
                .font(.system(size: 19, weight: .regular))
                .foregroundStyle(Theme.ink2)
                .frame(width: 40, height: 40)
                .background(Theme.fill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            VStack(alignment: .leading, spacing: 3) {
                Text(text).uiFont(14, weight: .semibold).foregroundStyle(Theme.ink)
                Text(IPhoneMeetingUI.transcriptDetail).uiFont(12.5).foregroundStyle(Theme.ink2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.top, 2)
        }
        .padding(.top, 4)
    }
}
