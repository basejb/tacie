import Foundation

/// The panel's words that the Mac panel and the iPhone ask screen both say (docs/36 1절, docs/40 6-2): the step lines
/// while Tacie works and the done line. Keys with a Korean default; each app's Localizable.xcstrings has the English.
/// The Mac adds its own words in an extension (PanelView.swift).
enum PanelText {
    /// What the answer read, leaving out a part that is zero; nil when it read nothing.
    static func read(meetings: Int, sources: Int) -> String? {
        let m = String(localized: "panel.read.meetings", defaultValue: "\(meetings)개")
        let s = String(localized: "panel.read.sources", defaultValue: "\(sources)개")
        switch (meetings > 0, sources > 0) {
        case (true, true): return String(localized: "panel.read", defaultValue: "회의 \(m)와 연결한 서비스 \(s)를 읽고 답했어요")
        case (true, false): return String(localized: "panel.read.onlyMeetings", defaultValue: "회의 \(m)를 읽고 답했어요")
        case (false, true): return String(localized: "panel.read.onlySources", defaultValue: "연결한 서비스 \(s)를 읽고 답했어요")
        case (false, false): return nil
        }
    }

    // the step line while Tacie works (docs/36 0-2절 4): what the engine is doing now
    static var stepFinding: String { String(localized: "panel.step.finding", defaultValue: "회의 기록 찾는 중…") }
    static var stepWriting: String { String(localized: "panel.step.writingNow", defaultValue: "정리하는 중…") }
    static func stepSearch(_ q: String) -> String { String(localized: "panel.step.search", defaultValue: "'\(q)' 찾는 중…") }
    static var stepMeetingList: String { String(localized: "panel.step.meetingList", defaultValue: "회의 목록 보는 중…") }
    /// `what`: "4월 결정", "민호 할 일", "결정·할 일·남은 논의"
    static func stepLedger(_ what: String) -> String { String(localized: "panel.step.ledger", defaultValue: "\(what) 모으는 중…") }
    static var stepTranscript: String { String(localized: "panel.step.transcript", defaultValue: "대화 기록 읽는 중…") }
    static func stepTranscriptFind(_ f: String) -> String {
        String(localized: "panel.step.transcriptFind", defaultValue: "대화 기록에서 '\(f)' 찾는 중…")
    }
    /// The iPhone ask screen handing a fix to one meeting (docs/40 12절): its title, as the meeting list shows it.
    static func stepOpenMeeting(_ title: String) -> String {
        String(localized: "panel.step.openMeeting", defaultValue: "「\(title)」의 대화 기록 읽는 중…")
    }
    static var stepPrep: String { String(localized: "panel.step.prep", defaultValue: "다음 회의 준비하는 중…") }
    static var ledgerAll: String { String(localized: "panel.step.ledgerAll", defaultValue: "결정·할 일·남은 논의") }
    static func kindName(_ kind: String) -> String? {
        switch kind {
        case "decision": return String(localized: "panel.step.kind.decision", defaultValue: "결정")
        case "commitment": return String(localized: "panel.step.kind.commitment", defaultValue: "할 일")
        case "open_question": return String(localized: "panel.step.kind.open", defaultValue: "남은 논의")
        default: return nil
        }
    }
    /// A month, a month of another year, or a year in the app language's date form: "11월" / "November",
    /// "2024년 11월" / "November 2024", "2024년" / "2024".
    static func month(_ d: Date, calendar: Calendar, locale: Locale) -> String { dated(d, "MMMM", calendar, locale) }
    static func yearMonth(_ d: Date, calendar: Calendar, locale: Locale) -> String { dated(d, "yMMMM", calendar, locale) }
    static func year(_ d: Date, calendar: Calendar, locale: Locale) -> String { dated(d, "y", calendar, locale) }
    /// The app's language (its strings), not the Mac's region.
    static var appLocale: Locale { Locale(identifier: Bundle.main.preferredLocalizations.first ?? "ko") }
    private static func dated(_ d: Date, _ template: String, _ calendar: Calendar, _ locale: Locale) -> String {
        let f = DateFormatter()
        f.locale = locale
        f.calendar = calendar
        f.timeZone = calendar.timeZone
        f.setLocalizedDateFormatFromTemplate(template)
        return f.string(from: d)
    }
    static func since(_ d: String) -> String { String(localized: "panel.step.since", defaultValue: "\(d)부터") }
    static func until(_ d: String) -> String { String(localized: "panel.step.until", defaultValue: "\(d)까지") }
    /// What it is reading, leaving out a part that is zero; nil when it has nothing yet.
    static func stepReading(meetings: Int, sources: Int) -> String? {
        let m = String(localized: "panel.read.meetings", defaultValue: "\(meetings)개")
        let s = String(localized: "panel.read.sources", defaultValue: "\(sources)개")
        switch (meetings > 0, sources > 0) {
        case (true, true): return String(localized: "panel.step.reading", defaultValue: "회의 \(m)와 연결한 서비스 \(s) 읽는 중…")
        case (true, false): return String(localized: "panel.step.readingMeetings", defaultValue: "회의 \(m) 읽는 중…")
        case (false, true): return String(localized: "panel.step.readingSources", defaultValue: "연결한 서비스 \(s) 읽는 중…")
        case (false, false): return nil
        }
    }

    /// The engine's sentence for "the sources do not say" (pipeline.rs `NO_ANSWER`; English answers come back in Korean).
    static let noAnswer = "찾은 자료에는 답이 없어요"
    static func isNoAnswer(_ s: String?) -> Bool {
        guard let s = s?.trimmingCharacters(in: .whitespacesAndNewlines), !s.isEmpty else { return true }
        return s.hasPrefix(noAnswer)
    }
}

/// The step line of a chat from the tools the engine reported (docs/38 7절).
enum PanelSteps {
    /// A chat's step line: the newest tool still running, said from its arguments; with every tool done, writing.
    static func toolText(_ steps: [PanelStep]) -> String {
        var running: [(String, [String: String])] = []
        var started = false
        for s in steps {
            guard case .tool(let name, let detail, let count) = s, name != "start" else { continue }
            started = true
            if count == nil {
                // a hand-off has no done step: the meeting's own tools that follow take its place
                running.removeAll { $0.0 == "open_meeting" }
                running.append((name, detail))
            } else if let i = running.lastIndex(where: { $0.0 == name }) {
                running.remove(at: i)
            }
        }
        guard let (name, d) = running.last else { return started ? PanelText.stepWriting : PanelText.stepFinding }
        switch name {
        case "search":
            guard let q = d["query"], !q.isEmpty else { return PanelText.stepFinding }
            return PanelText.stepSearch(cut(q))
        case "list_meetings": return PanelText.stepMeetingList
        case "filter_ledger":
            let kinds = (d["kinds"] ?? "").split(separator: ",").compactMap { PanelText.kindName(String($0)) }
            let what = kinds.isEmpty ? PanelText.ledgerAll : kinds.joined(separator: "·")
            let head = [period(from: d["from"], to: d["to"]), d["owner"].map { cut($0) }].compactMap { $0 }.filter { !$0.isEmpty }
            return PanelText.stepLedger((head + [what]).joined(separator: " "))
        case "transcript_window":
            guard let f = d["find"], !f.isEmpty else { return PanelText.stepTranscript }
            return PanelText.stepTranscriptFind(cut(f))
        case "prepare_meeting": return PanelText.stepPrep
        case "open_meeting":
            guard let t = d["title"], !t.isEmpty else { return PanelText.stepTranscript }
            return PanelText.stepOpenMeeting(cut(t, 24))
        default: return PanelText.stepFinding
        }
    }

    private static func cut(_ s: String, _ n: Int = 20) -> String { s.count > n ? String(s.prefix(n)) + "…" : s }

    /// "4월", "2024년 11월", "4/3–4/17", "4/3부터", "4/17까지" for a tool's `from`/`to` days; nil without either.
    static func period(from: String?, to: String?, calendar: Calendar = .current, today: Date = Date(),
                       locale: Locale = PanelText.appLocale) -> String? {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.calendar = calendar
        f.timeZone = calendar.timeZone
        f.dateFormat = "yyyy-MM-dd"
        let a = from.flatMap(f.date(from:)), b = to.flatMap(f.date(from:))
        func md(_ d: Date) -> String { "\(calendar.component(.month, from: d))/\(calendar.component(.day, from: d))" }
        switch (a, b) {
        case let (a?, b?):
            let (ya, ma, yb, mb) = (calendar.component(.year, from: a), calendar.component(.month, from: a),
                                    calendar.component(.year, from: b), calendar.component(.month, from: b))
            let lastDay = calendar.range(of: .day, in: .month, for: b)?.count ?? 31
            if ya == yb, ma == mb, calendar.component(.day, from: a) == 1, calendar.component(.day, from: b) == lastDay {
                return ya == calendar.component(.year, from: today) ? PanelText.month(a, calendar: calendar, locale: locale)
                    : PanelText.yearMonth(a, calendar: calendar, locale: locale)
            }
            if ya == yb, ma == 1, mb == 12, calendar.component(.day, from: a) == 1, calendar.component(.day, from: b) == 31 {
                return PanelText.year(a, calendar: calendar, locale: locale)
            }
            return "\(md(a))–\(md(b))"
        case let (a?, nil): return PanelText.since(md(a))
        case let (nil, b?): return PanelText.until(md(b))
        default: return nil
        }
    }

    /// Every step line a turn has shown so far, oldest first: the line before any tool, then each line the steps
    /// changed it to (the iPhone keeps them stacked; the Mac shows only the last). "정리하는 중…" (no tool running) is
    /// only ever the last line: a tool starting after it takes its place.
    static func lines(_ steps: [PanelStep]) -> [String] {
        var out = [PanelText.stepFinding]
        for k in steps.indices {
            let prefix = Array(steps[...k])
            guard prefix.contains(where: { if case .tool(let n, _, _) = $0 { n != "start" } else { false } }) else { continue }
            let line = toolText(prefix)
            if out.count > 1, out.last == PanelText.stepWriting { out.removeLast() }
            if out.last != line { out.append(line) }
        }
        return out
    }

    /// The meetings an answer read, as the Mac's done line counts them: its meeting sources.
    static func meetingsRead(_ a: ChatAnswer) -> Int { a.cards.filter { $0.path.hasPrefix("meeting/") }.count }
}
