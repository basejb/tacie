import SwiftUI

/// Home (2026-11-16): where the main window opens. One sentence on today, then the latest meetings, the open to-dos,
/// the latest decisions, the questions to write down, and where Tacie looks. Each part opens its full page.
struct HomePage: View {
    let meetings: [SavedMeeting]  // the meeting going on first, then the kept ones
    let live: String?
    let now: Date
    @Bindable var selection: MainSelection
    let library: MeetingLibrary
    var people: [Person] = []
    var panel: PanelStore? = nil
    /// The connected sources by name, and when the search data was last synced.
    var sources: [String] = []
    var synced: String? = nil
    var openConnections: () -> Void = {}

    private var cal: Calendar { MainView.calendar }
    private var ended: [SavedMeeting] { meetings.filter { $0.end != nil } }

    var body: some View {
        let todos = Collected.allTodos(meetings, calendar: cal).filter { !$0.entry.done }
        let due = Collected.byDue(todos, now: now, calendar: cal)
        let late = due.first { $0.bucket == .overdue }?.todos.count ?? 0
        TitleRowPage {
            HomeTopRow(panel: panel)
        } content: {
            VStack(alignment: .leading, spacing: 0) {
                Text(SavedMeeting.format("M월 d일 EEEE", now, cal)).uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink3)
                    .padding(.top, 8)
                Text("오늘").uiFont(30, weight: .bold).retinaTracking(-0.6).padding(.top, 2)
                Text(todayLine(late: late)).uiFont(13.5).foregroundStyle(Theme.ink2).lineSpacing(3)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 8)

                if SignedOutNotice.shows(signedIn: TacieAccount.shared.signedIn, hasAccount: Build.hasAccount,
                                         accountsHere: AccountSpace.anyAccount()) {
                    SignedOutNotice { selection.settings = .account }.padding(.top, 18)
                }
                OpenImportNotice { selection.settings = .ai }
                OneAppNotice()

                section(String(localized: "최근 회의"), more: { selection.view = .meetings }) {
                    ForEach(Array(meetings.prefix(3))) { m in
                        MeetingCard(meeting: m, live: m.id == live, now: now) {
                            if m.id != live { Analytics.capture(.meetingNotesOpened(from: .list, hasLedger: !m.ledger.isEmpty)) }
                            selection.id = m.id
                        }
                    }
                }

                if !todos.isEmpty {
                    section(String(localized: "할 일"), note: late > 0 ? String(localized: "기한 지남 \(late)") : nil,
                            more: { selection.view = .todos }) {
                        VStack(alignment: .leading, spacing: 0) {
                            ForEach(Array(due.flatMap(\.todos).prefix(4))) { t in
                                TodoLine(meeting: t.meeting, index: t.index, library: library, people: people, day: t.day,
                                         openMeeting: { selection.id = t.meeting.id })
                                    .padding(.vertical, 6)
                            }
                        }
                        .todoColumns()
                    }
                }

                let decisions = latestDecisions
                if !decisions.isEmpty {
                    section(String(localized: "최근 결정"), more: { selection.view = .decisions }) {
                        VStack(alignment: .leading, spacing: 10) {
                            ForEach(decisions, id: \.key) { d in
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(d.meeting.shownRefs(d.meeting.ledger[d.index].text)).uiFont(14.5)
                                        .fixedSize(horizontal: false, vertical: true)
                                    if let was = d.meeting.ledger[d.index].replaces {
                                        ReplacedLine(was: was, open: library.meetings.contains { $0.id == was.meeting } ? { selection.id = $0 } : nil)
                                    } else {
                                        from(d.meeting)
                                    }
                                }
                            }
                        }
                    }
                }

                let questions = Collected.questions(meetings).flatMap { q in q.questions.map { (q.meeting, $0) } }
                if !questions.isEmpty {
                    section(String(localized: "문서로 남길 질문"), note: "\(questions.count)", more: { selection.view = .questions }) {
                        VStack(alignment: .leading, spacing: 10) {
                            ForEach(Array(questions.prefix(2).enumerated()), id: \.offset) { _, q in
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(q.1.question).uiFont(14.5).fixedSize(horizontal: false, vertical: true)
                                    from(q.0)
                                }
                            }
                        }
                    }
                }

                sourcesLine.padding(.top, 38)
            }
            .padding(.horizontal, 56)
            .padding(.top, 14)
            .padding(.bottom, 60)
            .frame(maxWidth: 792, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
    }

    /// "오늘 회의 2개에서 결정 3개, 할 일 4개가 나왔어요. 기한이 지난 할 일이 2개 있어요."
    private func todayLine(late: Int) -> String {
        let today = ended.filter { cal.isDate($0.start, inSameDayAs: now) }
        var parts: [String] = []
        if today.isEmpty {
            parts.append(String(localized: "오늘은 아직 끝난 회의가 없어요."))
        } else {
            let d = today.reduce(0) { $0 + $1.ledger.filter { $0.kind == "decision" }.count }
            let t = today.reduce(0) { $0 + $1.ledger.filter { $0.kind == "commitment" }.count }
            parts.append(String(localized: "오늘 회의 \(today.count)개에서 결정 \(d)개, 할 일 \(t)개가 나왔어요."))
        }
        if late > 0 { parts.append(String(localized: "기한이 지난 할 일이 \(late)개 있어요.")) }
        return parts.joined(separator: " ")
    }

    private struct Decision { let meeting: SavedMeeting; let index: Int; var key: String { "\(meeting.id)#\(index)" } }

    /// The two latest decisions; one already said in a later meeting in the same words shows once.
    private var latestDecisions: [Decision] {
        var seen = Set<String>(), out: [Decision] = []
        for e in Collected.decisions(meetings) {
            for i in e.indices where seen.insert(e.meeting.ledger[i].text).inserted {
                out.append(Decision(meeting: e.meeting, index: i))
                if out.count == 2 { return out }
            }
        }
        return out
    }

    private func from(_ m: SavedMeeting) -> some View {
        Button { selection.id = m.id } label: {
            Text(verbatim: "\(m.displayTitle(calendar: cal)) · \(SavedMeeting.format("M월 d일", m.start, cal))")
                .uiFont(12.5).foregroundStyle(Theme.ink3)
        }
        .buttonStyle(.plain)
    }

    private func section<C: View>(_ title: String, note: String? = nil, more: @escaping () -> Void, @ViewBuilder content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(title).uiFont(15, weight: .semibold)
                if let note { Text(note).uiFont(12.5, digits: true).foregroundStyle(Theme.ink3) }
                Spacer()
                Button(action: more) { Text("모두 보기").uiFont(12.5).foregroundStyle(Theme.ink2) }
                    .buttonStyle(.plain)
            }
            content()
        }
        .padding(.top, 34)
    }

    /// Where Tacie looks, at the foot: the sources and the last sync; a click opens 설정 → 연결.
    private var sourcesLine: some View {
        Button(action: openConnections) {
            HStack(spacing: 10) {
                Text(sources.isEmpty ? String(localized: "서비스를 연결하면 회의 중에 거기서 답을 찾아요.")
                     : String(localized: "\(sources.joined(separator: ", "))에서 찾고 있어요."))
                    .uiFont(12.5).foregroundStyle(Theme.ink2).lineLimit(1)
                Spacer(minLength: 8)
                if let synced { Text(synced).uiFont(12).foregroundStyle(Theme.ink3).lineLimit(1) }
            }
            .padding(.horizontal, 14).padding(.vertical, 11)
            .background(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).fill(Theme.fill))
            .overlay(RoundedRectangle(cornerRadius: Theme.radiusInner, style: .continuous).strokeBorder(Theme.hairline))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("설정 → 연결에서 자세히 봐요")
    }
}
