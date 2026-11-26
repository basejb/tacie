import AppKit
import SwiftUI

/// Changes to the meeting's ledger made on the page. Each marks the line as the person's (`LedgerEntry.edited`), so
/// Rewrite keeps it.
@MainActor
enum LedgerEdit {
    static func text(_ library: MeetingLibrary, _ id: String, _ index: Int, _ words: String) {
        let t = words.trimmingCharacters(in: .whitespacesAndNewlines)
        library.update(id) { m in
            guard m.ledger.indices.contains(index) else { return }
            if t.isEmpty {
                m.ledger.remove(at: index)  // an emptied line goes, as in any list of lines
            } else if m.shownRefs(m.ledger[index].text) != t {
                // shown with today's names (a quote's [[speaker]]): unchanged words stay as they were kept
                m.ledger[index].text = m.keyedRefs(t)
                m.ledger[index].edited = true
                m.ledger[index].refs = nil  // new words: the lines it came from no longer say them
            }
        }
    }

    static func owner(_ library: MeetingLibrary, _ id: String, _ index: Int, _ owner: String?) {
        library.update(id) { m in
            guard m.ledger.indices.contains(index) else { return }
            m.ledger[index].owner = owner
            m.ledger[index].edited = true
        }
    }

    static func due(_ library: MeetingLibrary, _ id: String, _ index: Int, _ said: String?) {
        let t = (said ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        library.update(id) { m in
            guard m.ledger.indices.contains(index), (m.ledger[index].due ?? "") != t else { return }
            m.ledger[index].due = t.isEmpty ? nil : t
            m.ledger[index].edited = true
        }
    }

    static func remove(_ library: MeetingLibrary, _ id: String, _ index: Int) {
        library.update(id) { m in if m.ledger.indices.contains(index) { m.ledger.remove(at: index) } }
    }

    /// A new, empty to-do at the end; returns its index.
    static func addTodo(_ library: MeetingLibrary, _ meeting: SavedMeeting) -> Int {
        library.update(meeting.id) { m in m.ledger.append(LedgerEntry(kind: "commitment", text: "", edited: true)) }
        return meeting.ledger.count
    }
}

/// A ledger line's words. They read as plain text; a click puts the caret in the same place (same font, no box), and
/// leaving (Return, a click elsewhere) keeps the change. Esc puts the words back. The text is drawn by a `Text` when not
/// being written: a vertical `TextField` that has lost focus draws nothing on macOS.
struct LedgerText: View {
    let text: String
    var done = false
    var focusOnAppear = false
    var placeholder = String(localized: "ledger.placeholder.todo", defaultValue: "할 일")
    let commit: (String) -> Void

    @State private var editing = false
    @State private var draft = ""
    @FocusState private var focused: Bool

    var body: some View {
        Group {
            if editing {
                TextField("", text: $draft, prompt: Text(placeholder).foregroundStyle(Theme.ink4), axis: .vertical)
                    .textFieldStyle(.plain)
                    .uiFont(15)
                    .foregroundStyle(Theme.ink)
                    .focused($focused)
                    .onSubmit { focused = false }
                    .onExitCommand { draft = text; focused = false }
                    .endsOnClickOutside($focused)
                    .onChange(of: focused) { _, now in
                        if !now {
                            editing = false
                            commit(draft)
                        }
                    }
                    .onAppear {
                        focused = true
                        // macOS selects the whole field on focus; the caret goes to the end instead, as in a note
                        DispatchQueue.main.async { NSApp.sendAction(#selector(NSResponder.moveToEndOfDocument(_:)), to: nil, from: nil) }
                    }
            } else {
                Text(text.isEmpty ? placeholder : text)
                    .uiFont(15)
                    .foregroundStyle(text.isEmpty ? AnyShapeStyle(Theme.ink4) : done ? AnyShapeStyle(Theme.ink3) : AnyShapeStyle(Theme.ink))
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                    .onTapGesture { start() }
                    .help("눌러서 고쳐요")
            }
        }
        .onAppear { if focusOnAppear { start() } }
    }

    private func start() {
        draft = text
        editing = true
    }
}

/// The look of a ledger line on a meeting page: a soft fill under the pointer reaching a little past the line, so the
/// words stay in the page's column.
struct LedgerRowStyle: ViewModifier {
    @Binding var hover: Bool
    @Environment(\.reducesMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content
            .padding(.horizontal, 10).padding(.vertical, 6)
            .background(Theme.ink.opacity(hover ? 0.035 : 0), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .padding(.horizontal, -10).padding(.vertical, -6)
            .onHover { inside in
                withAnimation(Theme.anim(.easeOut(duration: inside ? 0.1 : 0.16), reduce: reduceMotion)) { hover = inside }
            }
    }
}

/// A ledger row a source in the main window's panel pointed at: lit for a moment in the tint 대화 기록에서 보기 lights
/// transcript lines with (TranscriptText `marked`).
struct LedgerMarkStyle: ViewModifier {
    let on: Bool

    static let tint = Theme.speakers[0].opacity(0.16)

    func body(content: Content) -> some View {
        content
            .background(alignment: .center) {
                RoundedRectangle(cornerRadius: 8, style: .continuous).fill(on ? Self.tint : .clear)
                    .padding(.horizontal, -10).padding(.vertical, -6)
            }
    }
}

/// A small control in a ledger line's end: an icon and words, a tint under the pointer.
struct LedgerChipBody<Lead: View>: View {
    let words: Text?
    var faint = false
    @ViewBuilder let lead: Lead
    @State private var hover = false

    var body: some View {
        HStack(spacing: 5) {
            lead
            if let words { words.uiFont(12).lineLimit(1) }
        }
        .foregroundStyle(faint ? AnyShapeStyle(Theme.ink3) : AnyShapeStyle(Theme.ink2))
        .padding(.horizontal, words == nil ? 0 : 7)
        .frame(minWidth: 24)
        .frame(height: 24)
        .background(Theme.ink.opacity(hover ? 0.06 : 0), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        .contentShape(Rectangle())
        .onHover { hover = $0 }
    }
}

/// Who takes a to-do: the chip opens a popover in the namer's look, with the meeting's people to pick and a field to
/// write someone else's name.
struct OwnerChip<Label: View>: View {
    let meeting: SavedMeeting
    let owner: String?
    var people: [Person] = []
    let pick: (String?) -> Void
    @ViewBuilder let label: Label
    @State private var open = false

    var body: some View {
        Button { open = true } label: { label }
            .buttonStyle(.plain)
            .help("담당자 바꾸기")
            .popover(isPresented: $open, arrowEdge: .bottom) {
                OwnerPicker(meeting: meeting, owner: owner, people: people) { o in
                    pick(o)
                    open = false
                }
            }
    }
}

struct OwnerPicker: View {
    let meeting: SavedMeeting
    let owner: String?
    let people: [Person]
    let pick: (String?) -> Void

    @State private var query = ""
    @State private var hover: String?
    @State private var chosen: String?
    @FocusState private var focused: Bool

    private var current: String? { owner.map(meeting.ownerKey) }
    private var all: [(key: String, name: String)] { meeting.attendees.map { (key: $0.key, name: $0.name) } }
    private var shown: [(key: String, name: String)] {
        let q = query.trimmingCharacters(in: .whitespaces)
        return q.isEmpty ? all : all.filter { $0.name.localizedCaseInsensitiveContains(q) }
    }
    private var highlighted: String? { chosen ?? (query.isEmpty ? nil : shown.first?.key) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "person").uiFont(12, weight: .medium).foregroundStyle(Theme.ink3).frame(width: 22)
                TextField("", text: $query, prompt: Text("이름을 쓰거나 아래에서 골라요").foregroundStyle(Theme.ink4))
                    .textFieldStyle(.plain).uiFont(13.5).foregroundStyle(Theme.ink)
                    .focused($focused)
                    .onSubmit(submit)
                    .onKeyPress(.downArrow) { move(1) }
                    .onKeyPress(.upArrow) { move(-1) }
            }
            .padding(.horizontal, 8).frame(height: 38)
            .background(Theme.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).stroke(Theme.ink.opacity(0.3), lineWidth: 1.5))
            if !shown.isEmpty {
                NamerLabel(text: "이 회의 참석자")
                ForEach(shown, id: \.key) { p in
                    NamerRow(on: highlighted == p.key || hover == p.key, note: "") {
                        SpeakerDot(name: p.name, key: p.key, color: meeting.speakerColor(p.key, people: people), size: 22)
                        Text(p.name).uiFont(13.5, weight: .medium).foregroundStyle(p.name == p.key ? Theme.ink2 : Theme.ink)
                            .lineLimit(1)
                    } more: {
                        if current == p.key { Image(systemName: "checkmark").uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink2) }
                    }
                    .onHover { hover = $0 ? p.key : (hover == p.key ? nil : hover) }
                    .onTapGesture { pick(p.key) }
                }
            } else if !query.trimmingCharacters(in: .whitespaces).isEmpty {
                NamerLabel(text: "회의에 없는 사람")
                NamerRow(on: true, note: "Return") {
                    Image(systemName: "plus").uiFont(11, weight: .semibold).foregroundStyle(Theme.ink3).frame(width: 22)
                    Text(query.trimmingCharacters(in: .whitespaces)).uiFont(13.5, weight: .medium).lineLimit(1)
                }
                .onTapGesture(perform: submit)
            }
            if owner != nil {
                Rectangle().fill(Theme.hairline).frame(height: 1).padding(.vertical, 6)
                NamerRow(on: hover == "-", note: "") {
                    Image(systemName: "person.slash").uiFont(11.5, weight: .medium).foregroundStyle(Theme.ink3).frame(width: 22)
                    Text("담당자 없음").uiFont(13.5).foregroundStyle(Theme.ink2)
                }
                .onHover { hover = $0 ? "-" : (hover == "-" ? nil : hover) }
                .onTapGesture { pick(nil) }
            }
        }
        .padding(10)
        .frame(width: 260)
        .appearanceScheme()
        .foregroundStyle(Theme.ink)
        .presentationBackground(Theme.raised)
        .onAppear { focused = true }
        .onChange(of: query) { chosen = nil }
    }

    /// Return: the highlighted attendee, else the typed name as someone outside the meeting.
    private func submit() {
        if let h = highlighted { return pick(h) }
        let t = query.trimmingCharacters(in: .whitespacesAndNewlines)
        if !t.isEmpty { pick(t) }
    }

    private func move(_ by: Int) -> KeyPress.Result {
        let ids = shown.map(\.key)
        guard !ids.isEmpty else { return .ignored }
        let at = highlighted.flatMap { ids.firstIndex(of: $0) }
        chosen = ids[max(0, min(ids.count - 1, at.map { $0 + by } ?? (by > 0 ? 0 : ids.count - 1)))]
        return .handled
    }
}

/// A to-do's due: the chip opens a popover with the usual days and a calendar. A picked day is kept as "11월 10일까지",
/// which reads the same as a due said in the meeting.
struct DueChip<Label: View>: View {
    let due: Date?
    var said: String? = nil
    let commit: (String?) -> Void
    @ViewBuilder let label: Label
    @State private var open = false

    var body: some View {
        Button { open = true } label: { label }
            .buttonStyle(.plain)
            .help("기한 바꾸기")
            .popover(isPresented: $open, arrowEdge: .bottom) {
                DuePicker(due: due, said: said) { s in
                    commit(s)
                    open = false
                }
            }
    }
}

struct DuePicker: View {
    let due: Date?
    let said: String?
    let pick: (String?) -> Void

    @State private var month: Date
    @State private var hover: String?

    private var cal: Calendar { MainView.calendar }

    init(due: Date?, said: String?, pick: @escaping (String?) -> Void) {
        self.due = due
        self.said = said
        self.pick = pick
        _month = State(initialValue: due ?? Date())
    }

    /// 오늘, 내일, 이번 주 금요일 (지났으면 빼요), 다음 주 월요일.
    private var quick: [(String, Date)] {
        let today = cal.startOfDay(for: Date())
        let wd = cal.component(.weekday, from: today)  // 1 = Sunday
        let toFriday = (6 - wd + 7) % 7
        let toMonday = (2 - wd + 7) % 7 == 0 ? 7 : (2 - wd + 7) % 7
        var out: [(String, Date)] = [(String(localized: "오늘"), today),
                                     (String(localized: "내일"), cal.date(byAdding: .day, value: 1, to: today)!)]
        if toFriday >= 2 { out.append((String(localized: "이번 주 금요일"), cal.date(byAdding: .day, value: toFriday, to: today)!)) }
        out.append((String(localized: "다음 주 월요일"), cal.date(byAdding: .day, value: toMonday, to: today)!))
        return out
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(quick, id: \.0) { title, d in
                NamerRow(on: hover == title, note: SavedMeeting.format("M월 d일 (E)", d, cal)) {
                    Text(title).uiFont(13.5, weight: .medium).lineLimit(1)
                } more: {
                    if let due, cal.isDate(due, inSameDayAs: d) {
                        Image(systemName: "checkmark").uiFont(11.5, weight: .semibold).foregroundStyle(Theme.ink2)
                    }
                }
                .onHover { hover = $0 ? title : (hover == title ? nil : hover) }
                .onTapGesture { pick(Self.said(d, cal)) }
            }
            Rectangle().fill(Theme.hairline).frame(height: 1).padding(.vertical, 6)
            MonthGrid(selected: due, month: $month) { d in pick(Self.said(d, cal)) }
            if due != nil || said != nil {
                Rectangle().fill(Theme.hairline).frame(height: 1).padding(.vertical, 6)
                if let said {
                    Text("회의에서 들은 말 · “\(said)”").uiFont(11.5).foregroundStyle(Theme.ink3)
                        .padding(.horizontal, 8).padding(.bottom, 4)
                }
                NamerRow(on: hover == "-", note: "") {
                    Text("기한 없애기").uiFont(13.5).foregroundStyle(Theme.warn)
                }
                .onHover { hover = $0 ? "-" : (hover == "-" ? nil : hover) }
                .onTapGesture { pick(nil) }
            }
        }
        .padding(10)
        .frame(width: 260)
        .appearanceScheme()
        .foregroundStyle(Theme.ink)
        .presentationBackground(Theme.raised)
    }

    static func said(_ d: Date, _ cal: Calendar) -> String {
        let c = cal.dateComponents([.month, .day], from: d)
        return "\(c.month ?? 1)월 \(c.day ?? 1)일까지"
    }
}

/// The bin at a ledger line's end, under the pointer only.
struct LedgerBin: View {
    let shown: Bool
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            Image(systemName: "trash").uiFont(11.5, weight: .medium)
                .foregroundStyle(hover ? AnyShapeStyle(Theme.warn) : AnyShapeStyle(Theme.ink3))
                .frame(width: 24, height: 24)
                .background(Theme.ink.opacity(hover ? 0.06 : 0), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .help("지우기")
        .opacity(shown ? 1 : 0)
        .allowsHitTesting(shown)
    }
}

/// 대화 기록에서 보기: when the line a note was written from was said; a click opens the transcript at it
/// (docs/24 2절 2번). Shown with the row's other controls, under the pointer.
struct LedgerSourceChip: View {
    let at: Date
    let shown: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            LedgerChipBody(words: nil, faint: true) {
                Image(systemName: "text.bubble").uiFont(10.5, weight: .medium)
            }
        }
        .buttonStyle(.plain)
        .help("대화 기록에서 보기 · \(SavedMeeting.format("a h:mm", at, MainView.calendar))")
        .opacity(shown ? 1 : 0)
        .allowsHitTesting(shown)
    }
}

/// "＋ 할 일 더하기" under the meeting's to-dos.
struct AddTodoButton: View {
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                Image(systemName: "plus").uiFont(11, weight: .semibold)
                Text("할 일 더하기").uiFont(13.5)
            }
            .foregroundStyle(hover ? AnyShapeStyle(Theme.ink2) : AnyShapeStyle(Theme.ink3))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }
}

/// A decision or an open question on a meeting page: a bullet whose words are written over in place, and a bin
/// under the pointer.
struct LedgerBullet: View {
    let meeting: SavedMeeting
    let index: Int
    let library: MeetingLibrary
    /// Opens the transcript at these lines (대화 기록에서 보기); nil leaves the chip out.
    var source: (([String]) -> Void)? = nil
    /// Opens the panel on why it was decided (decision rows only, nil leaves it out).
    var why: (() -> Void)? = nil
    /// Opens another meeting (이전 결정's meeting name); nil leaves the name plain.
    var openMeeting: ((String) -> Void)? = nil
    @State private var hover = false

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Circle().fill(Theme.ink.opacity(0.55)).frame(width: 4.5, height: 4.5).offset(y: -3.5)
            VStack(alignment: .leading, spacing: 2) {
                LedgerText(text: meeting.shownRefs(meeting.ledger[index].text), placeholder: String(localized: "내용")) { LedgerEdit.text(library, meeting.id, index, $0) }
                if let was = meeting.ledger[index].replaces {
                    ReplacedLine(was: was, open: library.meetings.contains { $0.id == was.meeting } ? openMeeting : nil)
                }
            }
            if let why, PanelEntries.showsWhy(meeting.ledger[index]) { WhyEntryLink(action: why) }
            HStack(spacing: 2) {
                if let source, let line = meeting.sourceLine(meeting.ledger[index]), let refs = meeting.ledger[index].refs {
                    LedgerSourceChip(at: line.at, shown: hover) { source(refs) }
                }
                LedgerBin(shown: hover) { LedgerEdit.remove(library, meeting.id, index) }
            }
            .alignmentGuide(.firstTextBaseline) { $0[VerticalAlignment.center] + 5 }
        }
        .modifier(LedgerRowStyle(hover: $hover))
    }
}

/// Under a decision that changed an earlier one (docs/30 8절): "이전 결정 · 그 글 · 10월 30일 회의 이름", the name opening
/// that meeting. A meeting since deleted leaves the name as plain words.
struct ReplacedLine: View {
    let was: ReplacedDecision
    let open: ((String) -> Void)?

    var body: some View {
        let lead = "\(String(localized: "이전 결정")) · \(was.text) · "
        var name = AttributedString("\(SavedMeeting.shortDay(was.at, .current)) \(was.title)")
        if open != nil {
            name.link = URL(string: "tacie-meeting:\(was.meeting)")
            name.foregroundColor = Theme.ink2
            name.underlineStyle = .single
        }
        return Text(AttributedString(lead) + name)
            .uiFont(13)
            .foregroundStyle(Theme.ink3)
            .tint(Theme.ink2)
            .environment(\.openURL, OpenURLAction { _ in
                open?(was.meeting)
                return .handled
            })
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// A click on the page outside a field ends the writing there. SwiftUI leaves the focus in the field when the click
/// lands on something that takes no focus (the page's plain background), so it would never end on its own
/// (the same watch as `EditableTitle`).
private struct EndsOnClickOutside: ViewModifier {
    var focused: FocusState<Bool>.Binding
    @State private var frame = CGRect.zero
    @State private var watch: Any?

    func body(content: Content) -> some View {
        content
            .background(GeometryReader { g in
                Color.clear.onAppear { frame = g.frame(in: .global) }
                    .onChange(of: g.frame(in: .global)) { _, f in frame = f }
            })
            .onChange(of: focused.wrappedValue, initial: true) { _, now in now ? start() : stop() }
            .onDisappear(perform: stop)
    }

    private func start() {
        stop()
        watch = NSEvent.addLocalMonitorForEvents(matching: .leftMouseDown) { e in
            if let h = e.window?.contentView?.bounds.height {
                let p = CGPoint(x: e.locationInWindow.x, y: h - e.locationInWindow.y)
                if !frame.insetBy(dx: -6, dy: -6).contains(p) {
                    DispatchQueue.main.async { focused.wrappedValue = false }
                }
            }
            return e
        }
    }

    private func stop() {
        if let w = watch { NSEvent.removeMonitor(w) }
        watch = nil
    }
}

extension View {
    func endsOnClickOutside(_ focused: FocusState<Bool>.Binding) -> some View { modifier(EndsOnClickOutside(focused: focused)) }
}

/// A month to pick a day from, in the page's inks: "2026년 11월" with ‹ ›, 일–토, the days. Today has a ring, the due
/// a filled dot; days before today are faint but can still be picked.
struct MonthGrid: View {
    let selected: Date?
    @Binding var month: Date
    let pick: (Date) -> Void
    @State private var hover: Date?
    /// Which way the last ‹ › went: the new month's days slide in from that side.
    @State private var step = 1
    @Environment(\.reducesMotion) private var reduceMotion

    private var cal: Calendar { MainView.calendar }

    private var days: [Date?] {
        guard let first = cal.date(from: cal.dateComponents([.year, .month], from: month)),
              let count = cal.range(of: .day, in: .month, for: first)?.count else { return [] }
        let lead = cal.component(.weekday, from: first) - 1  // the grid starts on Sunday
        return Array(repeating: nil, count: lead) + (0..<count).map { cal.date(byAdding: .day, value: $0, to: first) }
    }

    /// The weekday letters over the grid, Sunday first: "일월화수목금토" in Korean, the system's ("S M T W T F S") otherwise.
    private static let weekdays: [String] = {
        if AppLocale.isKorean { return Array("일월화수목금토").map(String.init) }
        var c = Calendar(identifier: .gregorian)
        c.locale = AppLocale.locale
        return c.veryShortStandaloneWeekdaySymbols
    }()

    var body: some View {
        let today = cal.startOfDay(for: Date())
        VStack(spacing: 6) {
            HStack {
                Text(SavedMeeting.format("yyyy년 M월", month, cal)).uiFont(13, weight: .semibold)
                    .contentTransition(.numericText(countsDown: step < 0))
                Spacer()
                step("chevron.left", -1)
                step("chevron.right", 1)
            }
            .padding(.leading, 8).padding(.trailing, 2)
            LazyVGrid(columns: Array(repeating: GridItem(.fixed(32), spacing: 2), count: 7), spacing: 2) {
                ForEach(Array(Self.weekdays.enumerated()), id: \.offset) { _, w in
                    Text(w).uiFont(11, weight: .medium).foregroundStyle(Theme.ink3).frame(height: 22)
                }
            }
            // the days as one block per month: the new month slides in from the side the arrow points to
            ZStack(alignment: .top) {
                LazyVGrid(columns: Array(repeating: GridItem(.fixed(32), spacing: 2), count: 7), spacing: 2) {
                    ForEach(Array(days.enumerated()), id: \.offset) { _, d in
                        if let d { cell(d, today: today) } else { Color.clear.frame(height: 30) }
                    }
                }
                .id(month)
                .transition(reduceMotion ? .opacity
                            : .asymmetric(insertion: .opacity.combined(with: .offset(x: CGFloat(step) * 14)),
                                          removal: .opacity.combined(with: .offset(x: CGFloat(-step) * 14)).animation(Theme.exit)))
            }
            .clipped()
        }
        .padding(.top, 2)
    }

    private func step(_ symbol: String, _ by: Int) -> some View {
        Button {
            step = by
            withAnimation(Theme.anim(Theme.enter, reduce: reduceMotion)) { month = cal.date(byAdding: .month, value: by, to: month) ?? month }
        } label: {
            Image(systemName: symbol).uiFont(11, weight: .semibold).foregroundStyle(Theme.ink2)
                .frame(width: 26, height: 26).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func cell(_ d: Date, today: Date) -> some View {
        let on = selected.map { cal.isDate($0, inSameDayAs: d) } ?? false
        let isToday = cal.isDate(d, inSameDayAs: today)
        let past = d < today
        return Text(verbatim: "\(cal.component(.day, from: d))")
            .uiFont(12.5, weight: on || isToday ? .semibold : .regular, digits: true)
            .foregroundStyle(on ? AnyShapeStyle(Theme.onStrong) : past ? AnyShapeStyle(Theme.ink4) : AnyShapeStyle(Theme.ink))
            .frame(width: 30, height: 30)
            .background {
                if on {
                    Circle().fill(Theme.ink)
                } else if hover == d {
                    Circle().fill(Theme.fill)
                }
            }
            .overlay { if isToday && !on { Circle().strokeBorder(Theme.ink.opacity(0.35), lineWidth: 1) } }
            .contentShape(Circle())
            .onHover { hover = $0 ? d : (hover == d ? nil : hover) }
            .onTapGesture { pick(d) }
    }
}
