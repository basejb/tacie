import SwiftUI

/// Meetings (2026-11-16): every meeting by day in the page, out of the sidebar. A search typed in the sidebar shows
/// here too, each meeting with the line the words were found in.
struct MeetingListPage: View {
    let meetings: [SavedMeeting]  // the meeting going on first, then the kept ones
    let live: String?
    let now: Date
    @Bindable var selection: MainSelection
    let library: MeetingLibrary
    var panel: PanelStore? = nil
    @State private var opened: Set<String> = []
    @State private var deleting: SavedMeeting?

    private var cal: Calendar { MainView.calendar }
    private var query: String { selection.query.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        let found = meetings.filter { $0.matches(query) }
        TitleRowPage {
            HomeTopRow(panel: panel)
        } content: {
            VStack(alignment: .leading, spacing: 0) {
                Text(query.isEmpty ? String(localized: "mainView.meetings", defaultValue: "회의") : String(localized: "'\(query)' 검색 결과"))
                    .uiFont(30, weight: .bold).retinaTracking(-0.6)
                    .padding(.top, 8)
                Text(lead(found.count))
                    .uiFont(13.5).foregroundStyle(Theme.ink3)
                    .padding(.top, 10)
                if found.isEmpty {
                    Text(query.isEmpty ? String(localized: "회의가 끝나면 여기에 쌓여요.") : String(localized: "찾는 회의가 없어요."))
                        .uiFont(14).foregroundStyle(Theme.ink2).padding(.top, 34)
                }
                if let live, let m = found.first(where: { $0.id == live }) {
                    day(String(localized: "지금"))
                    card(m)
                }
                ForEach(MeetingGroups.group(found.filter { $0.id != live }, now: now, calendar: cal), id: \.title) { g in
                    // older months fold behind their title, as the list did; a search opens everything
                    let folds = query.isEmpty && !MeetingGroups.open.contains(g.kind)
                    if folds {
                        Button { withAnimation(Theme.move) { opened.formSymmetricDifference([g.title]) } } label: {
                            HStack(spacing: 6) {
                                Text(g.title).uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink3)
                                Text(verbatim: "\(g.meetings.count)").uiFont(12.5, digits: true).foregroundStyle(Theme.ink4)
                                Image(systemName: "chevron.right").uiFont(9, weight: .bold).foregroundStyle(Theme.ink4)
                                    .rotationEffect(.degrees(opened.contains(g.title) ? 90 : 0))
                            }
                            .padding(.top, 26).padding(.bottom, 4)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        if opened.contains(g.title) { ForEach(g.meetings) { card($0) } }
                    } else {
                        day(g.title)
                        ForEach(g.meetings) { card($0) }
                    }
                }
            }
            .padding(.horizontal, 56)
            .padding(.top, 14)
            .padding(.bottom, 60)
            .frame(maxWidth: 792, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .confirmationDialog("이 회의를 지울까요?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
                            presenting: deleting) { m in
            Button("지우기", role: .destructive) {
                Analytics.capture(.meetingDeleted)
                library.remove(m.id)
            }
            Button("취소", role: .cancel) {}
        } message: { m in
            Text(IPhoneMeetingUI.deleteMessage(for: m))
        }
    }

    private func lead(_ n: Int) -> String {
        if !query.isEmpty { return n == 0 ? String(localized: "다른 말로 찾아보세요.") : String(localized: "회의 \(n)개에서 찾았어요. 찾은 말이 들어간 줄을 같이 보여 줘요.") }
        return String(localized: "회의 \(meetings.count)개 · 최근 회의부터 보여 줘요.")
    }

    private func day(_ title: String) -> some View {
        Text(title).uiFont(12.5, weight: .semibold).foregroundStyle(Theme.ink3)
            .padding(.top, 26).padding(.bottom, 4)
    }

    private func card(_ m: SavedMeeting) -> some View {
        MeetingCard(meeting: m, live: m.id == live, now: now, query: query) {
            if m.id != live { Analytics.capture(.meetingNotesOpened(from: .list, hasLedger: !m.ledger.isEmpty)) }
            selection.id = m.id
        }
        .contextMenu {
            Button(m.pinned == true ? String(localized: "고정 풀기") : String(localized: "목록 위에 고정")) {
                Analytics.capture(.meetingPinned(on: m.pinned != true))
                library.update(m.id) { $0.pinned = $0.pinned == true ? nil : true }
            }
            if m.id != live { Button("이 회의 지우기…", role: .destructive) { deleting = m } }
        }
    }
}

/// A meeting in 회의 and on 홈: what kind, its title and when, the summary's first line (or, in a search, the line
/// the words were found in), and how many decisions and to-dos it left.
struct MeetingCard: View {
    let meeting: SavedMeeting
    var live = false
    let now: Date
    var query = ""
    let open: () -> Void
    @State private var hover = false

    private var cal: Calendar { MainView.calendar }

    var body: some View {
        Button(action: open) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: meeting.mode == "inperson" ? "person.2" : "display")
                    .uiFont(11, weight: .medium).foregroundStyle(Theme.ink2)
                    .frame(width: 24, height: 24)
                    .background(Theme.fill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                VStack(alignment: .leading, spacing: 3) {
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        if live { LiveDot(warn: false) }
                        Text(marked(meeting.displayTitle(calendar: cal))).uiFont(14.5, weight: .semibold).lineLimit(1)
                        Spacer(minLength: 8)
                        HStack(spacing: 6) {
                            let meta = meeting.meta(now: now, calendar: cal)
                            Text(meta).uiFont(12, digits: true).foregroundStyle(Theme.ink3).lineLimit(1)
                            if meeting.fromIPhone { IPhoneChip() }
                        }
                    }
                    if let line {
                        Text(marked(line)).uiFont(13).foregroundStyle(Theme.ink2).lineLimit(1)
                    }
                    if let counts {
                        Text(counts).uiFont(12).foregroundStyle(Theme.ink3).padding(.top, 2)
                    }
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 11)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(hover ? Theme.ink.opacity(0.035) : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(PressStyle())
        .padding(.horizontal, -12)
        .onHover { inside in withAnimation(.easeOut(duration: inside ? 0.12 : 0.2)) { hover = inside } }
    }

    /// The found line in a search; otherwise the summary's first line, or what is going on.
    private var line: String? {
        if !query.isEmpty, let hit = meeting.foundLine(query) { return hit }
        if live { return String(localized: "듣는 중이에요") }
        if meeting.notes == .waiting { return String(localized: "요약을 쓰는 중이에요") }
        return meeting.shownSummary.first
    }

    private var counts: String? {
        let d = meeting.ledger.filter { $0.kind == "decision" }.count
        let t = meeting.ledger.filter { $0.kind == "commitment" }.count
        guard d + t > 0 else { return nil }
        return String(localized: "결정 \(d) · 할 일 \(t)")
    }

    /// The searched words marked with the face's light, as a highlighter would.
    private func marked(_ s: String) -> AttributedString {
        var a = AttributedString(s)
        guard !query.isEmpty else { return a }
        var from = a.startIndex
        while from < a.endIndex, let r = a[from...].range(of: query, options: .caseInsensitive) {
            a[r].backgroundColor = Theme.eyeGlow.opacity(0.35)
            from = r.upperBound
        }
        return a
    }
}

extension SavedMeeting {
    /// The first line holding `query`: the summary, topics, decisions and to-dos, questions, then what was said; long
    /// lines are cut around the words so they show in one line.
    func foundLine(_ query: String) -> String? {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return nil }
        var texts = shownSummary
        texts += (shownTopics ?? []).flatMap { [$0.title] + $0.points }
        texts += ledger.map { shownRefs($0.text) }
        texts += questions.flatMap { [$0.question, $0.answer ?? ""] }
        texts += transcript.map(\.text)
        guard let t = texts.first(where: { $0.localizedCaseInsensitiveContains(q) }),
              let r = t.range(of: q, options: .caseInsensitive) else { return nil }
        let lead = t.distance(from: t.startIndex, to: r.lowerBound)
        guard lead > 24 else { return t }
        return "…" + String(t[t.index(r.lowerBound, offsetBy: -20)...])
    }
}

/// The top row of 홈 and 회의: Tacie AI on the right, where the meeting page has it.
struct HomeTopRow: View {
    var panel: PanelStore?
    var body: some View {
        HStack {
            Spacer()
            if let panel { AskEntryButton(store: panel).padding(.trailing, 6) }
        }
        .padding(.horizontal, 14)
        .frame(height: 52)
    }
}

