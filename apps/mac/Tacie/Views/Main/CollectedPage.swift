import AppKit
import SwiftUI

/// To-dos, Decisions, Questions to write up from every meeting, newest meeting first, each under the meeting it came
/// from. A click on the meeting line opens that meeting; a to-do checked here is checked in its meeting.
struct CollectedPage: View {
    let view: SideView
    let meetings: [SavedMeeting]
    let library: MeetingLibrary
    @Bindable var selection: MainSelection
    var people: [Person] = []
    /// The main window's panel, for 'Tacie AI'; nil leaves the button out.
    var panel: PanelStore? = nil

    @State private var copied = false
    @AppStorage("todosByMeeting") private var byMeeting = false
    @State private var finishedOpen = false
    /// To-dos just checked: saved as done but kept in place until the checking stops, then they leave together.
    @State private var lingering: Set<Collected.Todo.Key> = []
    @State private var settle: Task<Void, Never>?

    private var cal: Calendar { MainView.calendar }

    var body: some View {
        TitleRowPage {
            // Tacie AI on the top row's right, where 홈, 회의 and the meeting page have it (2026-11-16)
            HomeTopRow(panel: view != .questions ? panel : nil)
        } content: {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .firstTextBaseline) {
                    Text(view.title).uiFont(30, weight: .bold).retinaTracking(-0.6)
                    Spacer(minLength: 16)
                    if view == .questions { copyButton }
                    if view == .todos, !allTodos.isEmpty { groupingPicker }
                }
                .padding(.top, 8)
                if view != .todos {
                    Text(lead).uiFont(13.5).foregroundStyle(Theme.ink3).lineSpacing(3)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.top, 10)
                }
                switch view {
                case .todos: todos
                case .decisions: decisions
                case .questions: questions
                case .home, .meetings: EmptyView()  // their own pages (HomePage, MeetingListPage)
                }
            }
            .padding(.horizontal, 56)
            .padding(.top, 14)
            .padding(.bottom, 60)
            .frame(maxWidth: 792, alignment: .leading)
            .todoColumns()
            .frame(maxWidth: .infinity)
        }
    }

    private var lead: String {
        switch view {
        case .todos: String(localized: "여기서 체크하면 그 회의에도 체크돼요.")
        case .decisions: String(localized: "회의에서 정한 것을 최근 회의부터 모아 봐요.")
        case .questions: String(localized: "답을 못 찾았거나 여러 곳에 흩어져 있던 질문이에요. 문서로 남겨 두면 다음에 찾을 수 있어요.")
        case .home, .meetings: ""
        }
    }

    // MARK: 할 일

    private var allTodos: [Collected.Todo] { Collected.allTodos(meetings, calendar: cal) }

    @ViewBuilder private var todos: some View {
        let all = allTodos, now = Date()
        let open = all.filter { !$0.entry.done || lingering.contains($0.id) }
        let finished = Collected.finished(all.filter { $0.entry.done && !lingering.contains($0.id) })
        if all.isEmpty {
            Text(lead).uiFont(13.5).foregroundStyle(Theme.ink3).padding(.top, 10)
            empty("회의에서 누가 무엇을 하기로 하면 여기에 모여요.")
        } else {
            summary(open, now: now).padding(.top, 18)
            Text(lead).uiFont(12.5).foregroundStyle(Theme.ink3).padding(.top, 14)
            VStack(alignment: .leading, spacing: 0) {
                if open.isEmpty { empty("남은 할 일이 없어요.").transition(.opacity) }
                if byMeeting {
                    ForEach(byMeetingGroups(open), id: \.meeting.id) { g in
                        VStack(alignment: .leading, spacing: 0) {
                            from(g.meeting)
                            ForEach(g.todos) { t in todoLine(t, showFrom: false).transition(.opacity) }
                        }
                        .transition(.opacity)
                    }
                } else {
                    ForEach(Collected.byDue(open, now: now, calendar: cal), id: \.bucket) { g in
                        VStack(alignment: .leading, spacing: 0) {
                            HStack(alignment: .firstTextBaseline, spacing: 7) {
                                Text(g.bucket.title).uiFont(12.5, weight: .semibold)
                                    .foregroundStyle(g.bucket == .overdue ? AnyShapeStyle(Theme.warn) : AnyShapeStyle(Theme.ink3))
                                Text(verbatim: "\(g.todos.count)").uiFont(12.5, weight: .medium, digits: true).foregroundStyle(Theme.ink4)
                                    .contentTransition(.numericText())
                            }
                            .padding(.top, 30)
                            .padding(.bottom, 6)
                            ForEach(g.todos) { t in todoLine(t, showFrom: true).transition(.opacity) }
                        }
                        .transition(.opacity)
                    }
                }
            }
            finishedFold(finished)
        }
    }

    /// 기한순 or 회의별, kept between visits: two segments on a grey track, the chosen one on white paper.
    private var groupingPicker: some View {
        HStack(spacing: 0) {
            segment("기한순", on: !byMeeting) { byMeeting = false }
            segment("회의별", on: byMeeting) { byMeeting = true }
        }
        .padding(2)
        .background(Theme.fill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private func segment(_ label: LocalizedStringKey, on: Bool, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label).uiFont(12, weight: on ? .semibold : .medium)
                .foregroundStyle(on ? Theme.ink : Theme.ink2)
                .padding(.horizontal, 10)
                .frame(height: 24)
                .background {
                    if on {
                        RoundedRectangle(cornerRadius: 6, style: .continuous).fill(Theme.chipOn)
                            .shadow(color: .black.opacity(0.06), radius: 1, y: 1)
                            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(Theme.hairline, lineWidth: 0.5))
                    }
                }
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    /// 남은 일 · 기한 지남 · 오늘까지 · 이번 주, at a glance. A count of nothing goes faint; past due turns red.
    private func summary(_ open: [Collected.Todo], now: Date) -> some View {
        let buckets = open.map { DueBucket($0.day, now: now, calendar: cal) }
        let n = { (b: DueBucket) in buckets.filter { $0 == b }.count }
        return HStack(alignment: .top, spacing: 28) {
            stat(open.count, Text(String(localized: "collected.openTodos", defaultValue: "남은 일")), warn: false)
            stat(n(.overdue), Text("기한 지남"), warn: true)
            stat(n(.today), Text("오늘까지"), warn: false)
            stat(n(.week), Text("이번 주"), warn: false)
        }
    }

    private func stat(_ n: Int, _ label: Text, warn: Bool) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(verbatim: "\(n)").uiFont(22, weight: .semibold, digits: true).retinaTracking(-0.4)
                .foregroundStyle(n == 0 ? Theme.ink4 : warn ? Theme.warn : Theme.ink)
                .contentTransition(.numericText())
            label.uiFont(12).foregroundStyle(Theme.ink3)
        }
    }

    private func byMeetingGroups(_ open: [Collected.Todo]) -> [(meeting: SavedMeeting, todos: [Collected.Todo])] {
        var order: [String] = [], groups: [String: [Collected.Todo]] = [:]
        for t in open {
            if groups[t.meeting.id] == nil { order.append(t.meeting.id) }
            groups[t.meeting.id, default: []].append(t)
        }
        return order.map { (groups[$0]![0].meeting, groups[$0]!) }
    }

    private func todoLine(_ t: Collected.Todo, showFrom: Bool) -> some View {
        TodoLine(meeting: t.meeting, index: t.index, library: library, people: people, day: t.day,
                 from: showFrom ? fromText(t.meeting) : nil,
                 openMeeting: { open(t.meeting) },
                 willToggle: { done in toggled(t.id, done: done) }, editable: true)
            .padding(.vertical, showFrom ? 8 : 6)
    }

    /// A check keeps the row where it is and starts the wait over; when the checking stops the checked rows leave
    /// together, so rows never move under the pointer between checks. An uncheck before that just stays.
    private func toggled(_ key: Collected.Todo.Key, done: Bool) {
        if done { lingering.insert(key) } else { lingering.remove(key) }
        settle?.cancel()
        guard !lingering.isEmpty else { return }
        settle = Task { @MainActor in
            try? await Task.sleep(for: .seconds(1.5))
            guard !Task.isCancelled else { return }
            withAnimation(Theme.move) { lingering.removeAll() }
        }
    }

    /// 끝낸 일 N at the foot, folded; open, the latest checked first. Unchecking one sends it back up.
    @ViewBuilder private func finishedFold(_ finished: [Collected.Todo]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Rectangle().fill(Theme.hairline).frame(height: 1)
            Button { withAnimation(Theme.move) { finishedOpen.toggle() } } label: {
                HStack(spacing: 7) {
                    Image(systemName: "chevron.right").uiFont(9, weight: .bold).foregroundStyle(Theme.ink4)
                        .rotationEffect(.degrees(finishedOpen ? 90 : 0))
                    Text("끝낸 일").uiFont(13, weight: .semibold).foregroundStyle(Theme.ink2)
                    Text(verbatim: "\(finished.count)").uiFont(13, weight: .medium, digits: true).foregroundStyle(Theme.ink3)
                        .contentTransition(.numericText())
                    Spacer(minLength: 12)
                    if finishedOpen, !finished.isEmpty {
                        Text("최근에 끝낸 순서").uiFont(12, weight: .medium).foregroundStyle(Theme.ink4)
                    }
                }
                .frame(height: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if finishedOpen {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if finished.isEmpty {
                        Text("아직 끝낸 일이 없어요.").uiFont(13.5).foregroundStyle(Theme.ink3).padding(.vertical, 8)
                    }
                    ForEach(finished) { t in todoLine(t, showFrom: true).transition(.opacity) }
                }
                .transition(.opacity)
            }
        }
        .padding(.top, 34)
    }

    // MARK: 결정 사항

    @ViewBuilder private var decisions: some View {
        let groups = Collected.decisions(meetings)
        if groups.isEmpty { empty("회의에서 정한 것이 생기면 여기에 모여요.") }
        LazyVStack(alignment: .leading, spacing: 0) {
            ForEach(groups) { g in
                from(g.meeting)
                VStack(alignment: .leading, spacing: 9) {
                    ForEach(g.indices, id: \.self) { i in Bullet(text: g.meeting.ledger[i].text) }
                }
            }
        }
    }

    // MARK: 문서로 남길 질문

    @ViewBuilder private var questions: some View {
        let groups = Collected.questions(meetings)
        if groups.isEmpty {
            empty("Tacie가 답을 못 찾은 질문이 생기면 여기에 모여요.")
        }
        LazyVStack(alignment: .leading, spacing: 0) {
            ForEach(groups) { g in
                // one stack per meeting: question ids (v1, v2…) repeat across meetings, and in one lazy list the
                // repeats drew as blank rows
                VStack(alignment: .leading, spacing: 0) {
                    from(g.meeting)
                    ForEach(g.questions) { q in
                        VStack(spacing: 0) {
                            Rectangle().fill(Theme.hairline).frame(height: 1)
                            HStack(alignment: .firstTextBaseline) {
                                VStack(alignment: .leading, spacing: 5) {
                                    Text(q.shownQuestion).uiFont(15, weight: .medium).fixedSize(horizontal: false, vertical: true)
                                        .textSelection(.enabled)
                                    if let a = g.meeting.heardAnswer(q) {
                                        Text("회의에서 나온 답 · \(a)").uiFont(13.5).foregroundStyle(Theme.ink2)
                                            .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                                    }
                                }
                                Spacer(minLength: 16)
                                QuestionAsker(meeting: g.meeting, question: q, people: people)
                            }
                            .padding(.vertical, 12)
                        }
                    }
                }
            }
        }
    }

    /// Every question on the page as a Markdown list, the same as a meeting page's 목록 복사.
    @ViewBuilder private var copyButton: some View {
        let groups = Collected.questions(meetings)
        if !groups.isEmpty {
            Button {
                let text = groups.flatMap(\.meeting.toDocumentLines).joined(separator: "\n")
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(text, forType: .string)
                copied = true
                Analytics.capture(.notesExported(how: .copy, part: .questions))
            } label: {
                OutlineButton(label: copied ? String(localized: "복사했어요") : String(localized: "목록 복사"), icon: copied ? "checkmark" : "doc.on.doc")
            }
            .buttonStyle(PressStyle())
            .copiedFlash($copied)
        }
    }

    // MARK: parts

    /// The meeting a group came from: its date and title. A click opens it.
    private func from(_ m: SavedMeeting) -> some View {
        MeetingLink(text: "\(SavedMeeting.format("M월 d일 EEEE", m.start, cal)) · \(m.displayTitle(calendar: cal))") { open(m) }
        .padding(.top, 30)
        .padding(.bottom, 10)
    }

    /// A to-do's meeting under its text where the page is not grouped by meeting: "10월 27일 · 주간 제품 회의".
    private func fromText(_ m: SavedMeeting) -> String {
        "\(SavedMeeting.format("M월 d일", m.start, cal)) · \(m.displayTitle(calendar: cal))"
    }

    private func open(_ m: SavedMeeting) {
        Analytics.capture(.meetingNotesOpened(from: .collected, hasLedger: !m.ledger.isEmpty))
        selection.id = m.id
    }

    private func empty(_ text: LocalizedStringKey) -> some View {
        Text(text).uiFont(14).foregroundStyle(Theme.ink3).padding(.top, 36)
    }
}

/// A group's meeting line on a collected page: grey, darker under the pointer, with an arrow that says it opens.
private struct MeetingLink: View {
    let text: String
    var small = false
    let action: () -> Void
    @State private var hovered = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                Text(text).uiFont(small ? 12 : 12.5, weight: small ? .regular : .semibold).lineLimit(1)
                Image(systemName: "arrow.up.right").uiFont(small ? 8.5 : 9.5, weight: .semibold).opacity(hovered ? 1 : 0)
            }
            .foregroundStyle(hovered ? AnyShapeStyle(Theme.ink2) : AnyShapeStyle(Theme.ink3))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovered = $0 }
        .help("이 회의 열기")
    }
}

/// One to-do of a meeting's ledger: a circle that checks it (and saves), the task, who and by when. Checking fills
/// the circle and draws a line through the task from the left; the row itself stays (the 할 일 page decides when it
/// leaves). On the 할 일 page the meeting it came from can sit under the task.
struct TodoLine: View {
    let meeting: SavedMeeting
    let index: Int
    let library: MeetingLibrary
    var people: [Person] = []
    /// The day the due points at; nil reads it here.
    var day: Date?
    var from: String?
    var openMeeting: () -> Void = {}
    /// Called with the new value just before a check or uncheck is saved.
    var willToggle: (Bool) -> Void = { _ in }
    /// On a meeting page: the text is a paragraph of the page's one selection, under this id.
    var selection: (id: String, group: TextSelectionGroup)? = nil
    /// On a meeting page: the words, who and when are written over in place, and the line can go.
    var editable = false
    /// A to-do just added: the caret starts in it.
    var focusOnAppear = false
    /// On a meeting page: opens the transcript at these lines (대화 기록에서 보기).
    var source: (([String]) -> Void)? = nil
    @State private var hovered = false
    @State private var rowHover = false
    @Environment(\.displayScale) private var displayScale

    private var cal: Calendar { MainView.calendar }
    /// The row's end is columns shared by the list (`TodoColumns`): who and by when start at the same place on every
    /// row, as wide as the list's widest, up to these.
    static let ownerCap: CGFloat = 120
    static let dueCap: CGFloat = 110
    @Environment(\.todoColumns) private var columns

    var body: some View {
        let e = meeting.ledger[index]
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Button {
                let done = !e.done
                Analytics.capture(.todoChecked(done: done))
                willToggle(done)
                withAnimation(Theme.settle) {
                    library.update(meeting.id) {
                        $0.ledger[index].done = done
                        $0.ledger[index].doneAt = done ? Date() : nil
                    }
                }
            } label: {
                // two marks crossfading, not `.symbolEffect(.replace)`: the symbol animation held the page's main
                // thread for up to 400 ms on each check (measured on a meeting page, docs/04)
                ZStack {
                    Image(systemName: "circle").uiFont(15)
                        .foregroundStyle(hovered ? AnyShapeStyle(Theme.ink3) : AnyShapeStyle(Theme.ink4))
                        .opacity(e.done ? 0 : 1)
                    Image(systemName: "checkmark.circle.fill").uiFont(15)
                        .foregroundStyle(Theme.ink)
                        .opacity(e.done ? 1 : 0)
                        .scaleEffect(e.done ? 1 : 0.7)
                }
                .offset(y: 1)
            }
            .buttonStyle(.plain)
            .onHover { hovered = $0 }
            VStack(alignment: .leading, spacing: 2) {
                words(e)
                    .overlay(alignment: .leading) {
                        Text(e.text).uiFont(15).strikethrough(true, color: Theme.ink4).foregroundStyle(.clear)
                            .mask(alignment: .leading) {
                                Rectangle().scaleEffect(x: e.done ? 1 : 0, anchor: .leading)
                            }
                            .allowsHitTesting(false)
                    }
                if let from { MeetingLink(text: from, small: true, action: openMeeting) }
            }
            Spacer(minLength: 12)
            HStack(alignment: editable ? .center : .firstTextBaseline, spacing: editable ? 6 : 10) {
                if editable {
                    OwnerChip(meeting: meeting, owner: e.owner, people: people, pick: { LedgerEdit.owner(library, meeting.id, index, $0) }) {
                        ownerChip(e)
                    }
                    .allowsHitTesting(e.owner != nil || rowHover)
                    .column(\.owner, width: columns.map { min($0.owner, Self.ownerCap) }) { ownerChip(e) }
                    DueChip(due: e.due.flatMap { day ?? DueDate.day($0, from: meeting.start, calendar: cal) },
                            said: e.due.flatMap { $0.range(of: #"^\d{1,2}월 \d{1,2}일까지$"#, options: .regularExpression) == nil ? $0 : nil },
                            commit: { LedgerEdit.due(library, meeting.id, index, $0) }) {
                        dueChip(e)
                    }
                    .allowsHitTesting(e.due != nil || rowHover)
                    .column(\.due, width: columns.map { min($0.due, Self.dueCap) }) { dueChip(e) }
                    // the page's source column is kept on every row, so a line added by hand lines up with the rest
                    if let source {
                        Group {
                            if let line = meeting.sourceLine(e), let refs = e.refs {
                                LedgerSourceChip(at: line.at, shown: rowHover) { source(refs) }
                            } else {
                                Color.clear
                            }
                        }
                        .frame(width: 24)
                    }
                    LedgerBin(shown: rowHover) { LedgerEdit.remove(library, meeting.id, index) }
                } else {
                    owner(e)
                    due(e)
                }
            }
            .opacity(e.done ? 0.6 : 1)
        }
        .modifier(EditableRow(on: editable, hover: $rowHover))
    }

    @ViewBuilder private func ownerChip(_ e: LedgerEntry) -> some View {
        if let raw = e.owner, let name = meeting.shownOwner(raw) {
            let key = meeting.ownerKey(raw)
            LedgerChipBody(words: Text(name)) {
                SpeakerDot(name: name, key: key, color: meeting.speakerColor(key, people: people), size: 16)
            }
        } else {
            LedgerChipBody(words: nil, faint: true) {
                Image(systemName: "person").uiFont(10.5, weight: .medium)
            }
            .opacity(rowHover ? 1 : 0)
        }
    }

    @ViewBuilder private func dueChip(_ e: LedgerEntry) -> some View {
        if let words = dueWords(e) {
            LedgerChipBody(words: words) { Image(systemName: "calendar").uiFont(10.5, weight: .medium) }
                .help(dueHelp(e))
        } else {
            LedgerChipBody(words: nil, faint: true) { Image(systemName: "calendar").uiFont(10.5, weight: .medium) }
                .opacity(rowHover ? 1 : 0)
        }
    }

    /// The due as the chip says it, short so the task keeps its width: "오늘", "내일", else "11월 10일" (red once past).
    /// The weekday, the days left and the words said are in the tooltip.
    private func dueWords(_ e: LedgerEntry) -> Text? {
        guard let said = e.due else { return nil }
        guard let d = day ?? DueDate.day(said, from: meeting.start, calendar: cal) else { return Text(said) }
        let n = DueBucket.days(to: d, now: Date(), calendar: cal)
        let sameYear = cal.component(.year, from: d) == cal.component(.year, from: Date())
        let date = SavedMeeting.format(sameYear ? "M월 d일" : "yyyy년 M월 d일", d, cal)
        if e.done { return Text(date) }
        if n == 0 { return Text("오늘").foregroundStyle(Theme.ink).fontWeight(.semibold) }
        if n == 1 { return Text("내일").foregroundStyle(Theme.ink).fontWeight(.medium) }
        return n < 0 ? Text(date).foregroundStyle(Theme.warn).fontWeight(.medium) : Text(date)
    }

    private func dueHelp(_ e: LedgerEntry) -> String {
        guard let said = e.due, let d = day ?? DueDate.day(said, from: meeting.start, calendar: cal) else { return String(localized: "기한 바꾸기") }
        let n = DueBucket.days(to: d, now: Date(), calendar: cal)
        let rel = n > 0 ? String(localized: "\(n)일 남음") : n == 0 ? String(localized: "오늘까지") : String(localized: "\(-n)일 지남")
        return String(localized: "\(SavedMeeting.format("M월 d일 (E)", d, cal)) · \(rel) · 회의에서 들은 말 “\(said)”")
    }

    @ViewBuilder private func words(_ e: LedgerEntry) -> some View {
        if editable {
            LedgerText(text: e.text, done: e.done, focusOnAppear: focusOnAppear) { LedgerEdit.text(library, meeting.id, index, $0) }
        } else if let selection {
            SelectableText(id: selection.id, text: e.text, group: selection.group, size: 15, lineSpacing: 0,
                           color: e.done ? Theme.ink.opacity(displayScale >= 2 ? 0.5 : 0.62) : Theme.ink)
        } else {
            Text(e.text).uiFont(15)
                .foregroundStyle(e.done ? AnyShapeStyle(Theme.ink3) : AnyShapeStyle(Theme.ink))
                .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
        }
    }

    @ViewBuilder private func owner(_ e: LedgerEntry) -> some View {
        if let raw = e.owner, let name = meeting.shownOwner(raw) {
            let key = meeting.ownerKey(raw)
            let speaker = key.range(of: #"^화자 \d+$"#, options: .regularExpression) != nil || SavedMeeting.isLinePerson(key)
            HStack(spacing: 5) {
                if speaker { SpeakerDot(name: name, key: key, color: meeting.speakerColor(key, people: people), size: 16) }
                Text(name).uiFont(12).foregroundStyle(Theme.ink2)
            }
        }
    }

    /// "10월 30일 (금) · 3일 지남" when the due reads as a day (the said words in the tooltip), else the words as said.
    /// A finished to-do keeps only the day.
    @ViewBuilder private func due(_ e: LedgerEntry) -> some View {
        if let said = e.due {
            if let d = day ?? DueDate.day(said, from: meeting.start, calendar: cal) {
                let n = DueBucket.days(to: d, now: Date(), calendar: cal)
                let sameYear = cal.component(.year, from: d) == cal.component(.year, from: Date())
                let date = Text(SavedMeeting.format(sameYear ? "M월 d일 (E)" : "yyyy년 M월 d일 (E)", d, cal))
                    .foregroundStyle(Theme.ink3)
                let rel = n < 0 ? Text("\(-n)일 지남").foregroundStyle(Theme.warn).fontWeight(.medium)
                    : n == 0 ? Text("오늘까지").foregroundStyle(Theme.ink).fontWeight(.semibold)
                    : Text("\(n)일 남음").foregroundStyle(Theme.ink3).fontWeight(.medium)
                (e.done ? date : date + Text(" · ").foregroundStyle(Theme.ink3) + rel)
                    .uiFont(12, digits: true)
                    .help("회의에서 들은 말 · “\(said)”")
            } else {
                Text(said).uiFont(12).foregroundStyle(Theme.ink3)
            }
        }
    }
}

/// Who asked a question and when: the speaker's dot and name once speakers are known, else as before.
struct QuestionAsker: View {
    let meeting: SavedMeeting
    let question: SavedQuestion
    var people: [Person] = []

    var body: some View {
        let time = SavedMeeting.format("a h:mm", question.at, MainView.calendar)
        if let key = question.voice {
            AskerLine(name: meeting.displayName(key: key), key: key, color: meeting.speakerColor(key, people: people), time: time)
        } else {
            Text(verbatim: "\(name) · \(time)").uiFont(12).foregroundStyle(Theme.ink3)
        }
    }

    private var name: String {
        switch question.asker {
        case "me": String(localized: "asker.me", defaultValue: "나")
        case "them": String(localized: "asker.them", defaultValue: "상대")
        case "typed": String(localized: "직접 물음")
        default: String(localized: "회의 중")
        }
    }
}

/// A to-do row that is written over in place gets the ledger's row look; one that is not stays as it was.
private struct EditableRow: ViewModifier {
    let on: Bool
    @Binding var hover: Bool
    func body(content: Content) -> some View {
        if on { content.modifier(LedgerRowStyle(hover: $hover)) } else { content.onHover { hover = $0 } }
    }
}

/// The widths of a list's to-do columns, the widest row's: every `TodoLine` in it lines its who and by when up.
struct TodoColumnWidths: Equatable {
    var owner: CGFloat = 0
    var due: CGFloat = 0
}

private struct TodoColumnsKey: PreferenceKey {
    static let defaultValue = TodoColumnWidths()
    static func reduce(value: inout TodoColumnWidths, nextValue: () -> TodoColumnWidths) {
        let n = nextValue()
        value.owner = max(value.owner, n.owner)
        value.due = max(value.due, n.due)
    }
}

private struct TodoColumnsEnvKey: EnvironmentKey {
    static let defaultValue: TodoColumnWidths? = nil
}

extension EnvironmentValues {
    var todoColumns: TodoColumnWidths? {
        get { self[TodoColumnsEnvKey.self] }
        set { self[TodoColumnsEnvKey.self] = newValue }
    }
}

private extension View {
    /// Puts this part in one of the list's columns, `width` wide (its own width until the list is measured; a longer
    /// part is cut short). `natural` is the same part drawn at its full width, unseen, to measure the column by.
    func column<N: View>(_ column: WritableKeyPath<TodoColumnWidths, CGFloat>, width: CGFloat?,
                         @ViewBuilder natural: () -> N) -> some View {
        frame(width: width, alignment: .leading)
            .background(alignment: .leading) {
                natural().fixedSize().hidden().allowsHitTesting(false)
                    .background(GeometryReader { g in
                        Color.clear.preference(key: TodoColumnsKey.self,
                                               value: { var w = TodoColumnWidths(); w[keyPath: column] = g.size.width; return w }())
                    })
            }
    }
}

/// Lines up the to-dos inside: their who and by when share columns as wide as the widest row's.
struct TodoColumns: ViewModifier {
    @State private var widths: TodoColumnWidths?
    func body(content: Content) -> some View {
        content
            .environment(\.todoColumns, widths)
            .onPreferenceChange(TodoColumnsKey.self) { w in if widths != w { widths = w } }
    }
}

extension View {
    func todoColumns() -> some View { modifier(TodoColumns()) }
}
