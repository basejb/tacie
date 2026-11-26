import Foundation

/// The pages the main window's sidebar opens (2026-11-16): Home, Meetings, and the ones that gather one thing from
/// every meeting.
enum SideView: String, CaseIterable {
    case home, meetings, todos, decisions, questions

    /// The pages that gather one thing from every meeting (CollectedPage).
    static let collected: [SideView] = [.todos, .decisions, .questions]

    var title: String {
        switch self {
        case .home: String(localized: "홈")
        case .meetings: String(localized: "mainView.meetings", defaultValue: "회의")
        case .todos: String(localized: "할 일")
        case .decisions: String(localized: "결정 사항")
        case .questions: String(localized: "문서로 남길 질문")
        }
    }

    var symbol: String {
        switch self {
        case .home: "house"
        case .meetings: "mic"
        case .todos: "checkmark.circle"
        case .decisions: "flag"
        case .questions: "doc.text"
        }
    }
}

/// What the collected pages show, newest meeting first. Only ended meetings: a meeting going on has no ledger yet
/// and its questions may still be answered.
enum Collected {
    /// A meeting and the ledger rows (indices into `ledger`) of one kind it holds.
    struct Entries: Identifiable {
        let meeting: SavedMeeting
        let indices: [Int]
        var id: String { meeting.id }
    }

    struct Questions: Identifiable {
        let meeting: SavedMeeting
        let questions: [SavedQuestion]
        var id: String { meeting.id }
    }

    /// One to-do of a meeting, with the day its due points at.
    struct Todo: Identifiable {
        struct Key: Hashable {
            let meeting: String
            let index: Int
        }

        let meeting: SavedMeeting
        let index: Int
        let day: Date?
        var id: Key { Key(meeting: meeting.id, index: index) }
        var entry: LedgerEntry { meeting.ledger[index] }
    }

    /// Every to-do of the ended meetings, newest meeting first and in the order each meeting said them.
    static func allTodos(_ meetings: [SavedMeeting], calendar: Calendar) -> [Todo] {
        ended(meetings).flatMap { m in
            m.ledger.indices.filter { m.ledger[$0].kind == "commitment" }.map {
                Todo(meeting: m, index: $0, day: DueDate.day(m.ledger[$0].due, from: m.start, calendar: calendar))
            }
        }
    }

    /// Open to-dos by due: past it first, then today, this week, later, no day; earliest day first inside each.
    static func byDue(_ todos: [Todo], now: Date, calendar: Calendar) -> [(bucket: DueBucket, todos: [Todo])] {
        let groups = Dictionary(grouping: todos) { DueBucket($0.day, now: now, calendar: calendar) }
        return DueBucket.allCases.compactMap { b in
            guard let g = groups[b] else { return nil }
            // stable: the same day keeps the newest meeting first
            let sorted = g.enumerated().sorted { ($0.element.day ?? .distantFuture, $0.offset) < ($1.element.day ?? .distantFuture, $1.offset) }
            return (b, sorted.map(\.element))
        }
    }

    /// Finished to-dos, the latest checked first; those checked before the time was kept go last, newest meeting first.
    static func finished(_ todos: [Todo]) -> [Todo] {
        todos.enumerated().sorted { a, b in
            switch (a.element.entry.doneAt, b.element.entry.doneAt) {
            case let (x?, y?): x == y ? a.offset < b.offset : x > y
            case (_?, nil): true
            case (nil, _?): false
            case (nil, nil): a.offset < b.offset
            }
        }.map(\.element)
    }

    static func todos(_ meetings: [SavedMeeting], done: Bool) -> [Entries] {
        entries(meetings) { $0.kind == "commitment" && $0.done == done }
    }

    static func decisions(_ meetings: [SavedMeeting]) -> [Entries] {
        entries(meetings) { $0.kind == "decision" }
    }

    /// The same questions as a meeting page's 문서로 남길 질문.
    static func questions(_ meetings: [SavedMeeting]) -> [Questions] {
        ended(meetings).compactMap { m in
            let qs = m.questions.filter(\.toDocument)
            return qs.isEmpty ? nil : Questions(meeting: m, questions: qs)
        }
    }

    /// The list's numbers: open to-dos and questions to write down. 결정 사항 has none (nothing to do about them).
    static func count(_ view: SideView, in meetings: [SavedMeeting]) -> Int? {
        switch view {
        case .todos: todos(meetings, done: false).reduce(0) { $0 + $1.indices.count }
        case .decisions: nil
        case .questions: questions(meetings).reduce(0) { $0 + $1.questions.count }
        case .home, .meetings: nil
        }
    }

    private static func entries(_ meetings: [SavedMeeting], _ keep: (LedgerEntry) -> Bool) -> [Entries] {
        ended(meetings).compactMap { m in
            let i = m.ledger.indices.filter { keep(m.ledger[$0]) }
            return i.isEmpty ? nil : Entries(meeting: m, indices: i)
        }
    }

    private static func ended(_ meetings: [SavedMeeting]) -> [SavedMeeting] {
        meetings.filter { $0.end != nil }.sorted { $0.start > $1.start }
    }
}
