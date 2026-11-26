import Foundation

extension SavedMeeting {
    /// Adds an utterance in time order. `first` is the earlier piece the engine joined into this one: it goes.
    mutating func addLine(_ line: TranscriptLine, replacing first: String?) {
        var line = line
        if let first, let earlier = transcript.first(where: { $0.key == first }) {
            // the engine's words cover only the latest WAV, so the earlier piece's come first
            if let a = earlier.audioAt { line.audioAt = a }
            if let w = earlier.words { line.words = w + (line.words ?? []) }
            line.translation = MeetingTranslation.joined(earlier.translation, line.translation)
            line.translatedFrom = line.translatedFrom ?? earlier.translatedFrom
        }
        if let first { transcript.removeAll { $0.key == first } }
        let i = transcript.firstIndex { $0.at > line.at } ?? transcript.endIndex
        transcript.insert(line, at: i)
    }

    /// A question's latest state: an answer that came after a save replaces it; one caught late joins in time order.
    mutating func setQuestion(_ q: SavedQuestion) {
        if let i = questions.firstIndex(where: { $0.id == q.id }) {
            var q = q
            // a question made again (searched again, its block settled) keeps the read of the meeting it had (docs/24 4절)
            if q.review == nil { q.review = questions[i].review }
            questions[i] = q
        } else {
            questions.insert(q, at: questions.firstIndex { $0.at > q.at } ?? questions.endIndex)
        }
    }

    /// A meeting nobody said anything in (started and ended at once) is not kept, unless it was recorded.
    var keep: Bool { !transcript.isEmpty || (recording == .kept && (recordingSeconds ?? 0) > 0) }

    func matches(_ query: String) -> Bool {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return true }
        let topicTexts: [String] = (shownTopics ?? []).flatMap { [$0.title] + $0.points }
        let questionTexts: [String] = questions.flatMap { [$0.question, $0.answer ?? ""] }
        var texts: [String] = [title ?? ""]
        texts += shownSummary
        texts += topicTexts
        texts += ledger.map { shownRefs($0.text) }
        texts += questionTexts
        texts += transcript.map(\.text)
        return texts.contains { $0.localizedCaseInsensitiveContains(q) }
    }

    var modeTitle: String { mode == "inperson" ? String(localized: "대면 회의") : String(localized: "원격 회의") }
    private var modeShort: String { mode == "inperson" ? String(localized: "대면") : String(localized: "원격") }

    /// The model's or the person's title; before the notes come, the kind of meeting; without notes, its time too.
    func displayTitle(calendar: Calendar) -> String {
        if let title { return title }
        return notes == .waiting ? modeTitle : "\(Self.format("a h:mm", start, calendar)) \(modeTitle)"
    }

    /// The list row's second line: "오후 2:00 · 47분 · 원격", older ones by date, a live one by how long it has run.
    func meta(now: Date, calendar: Calendar) -> String {
        guard let end else {
            return String(localized: "\(Self.format("a h:mm", start, calendar))부터 · \(Self.minutes(start, now))분째")
        }
        let when = calendar.isDate(start, inSameDayAs: now) || calendar.isDate(start, inSameDayAs: now.addingTimeInterval(-86_400))
            ? Self.format("a h:mm", start, calendar) : Self.format("M월 d일 EEEE", start, calendar)
        return String(localized: "\(when) · \(listenedMinutes(now: end))분 · \(modeShort)")
    }

    /// The page's first line: "10월 23일 목요일 오후 2:00 · 원격 회의 · 47분" (also the copied notes' first line).
    func header(calendar: Calendar, now: Date = Date()) -> String {
        String(localized: "\(Self.format("M월 d일 EEEE a h:mm", start, calendar)) · \(modeTitle) · \(listenedMinutes(now: now))분")
    }

    /// The page's line, and while the meeting can still be taken up again "· 3분 전에 끝났어요" (이어서 듣기, docs/35 5-4).
    /// Never in copies: it is true only for a while.
    func pageHeader(calendar: Calendar, now: Date) -> String {
        let line = header(calendar: calendar, now: now)
        guard resumable(now: now), let end else { return line }
        let ago = max(1, Int(now.timeIntervalSince(end) / 60))
        return line + " · " + String(localized: "\(ago)분 전에 끝났어요")
    }

    /// Whole minutes listened (at least one), gaps left out.
    func listenedMinutes(now: Date) -> Int { max(1, Int((listenedSeconds(now: now) / 60).rounded())) }

    /// The notes as Markdown. The saved file (`transcript`) carries the transcript after them; a copy or a share does not.
    func markdown(calendar: Calendar, transcript withTranscript: Bool = false) -> String {
        var out = ["# \(displayTitle(calendar: calendar))", header(calendar: calendar)]
        if hasSpeakers { out.append(String(localized: "참석:") + " " + attendees.map(\.name).joined(separator: ", ")) }
        func section(_ title: String, _ lines: [String]) {
            guard !lines.isEmpty else { return }
            out += ["", title] + lines
        }
        // notes with topics have a summary of a few sentences that reads as one paragraph; older ones, short lines
        let summary = shownSummary
        section("## " + String(localized: "요약"), topics == nil ? summary.map { "- \($0)" } : summary.isEmpty ? [] : [summary.joined(separator: " ")])
        // a point left open says so: "- 유예 시간은 미정. (미정)" would repeat it, so only when the text does not
        func flow() {
            section("## \(shownKind.flowTitle)", (shownTopics ?? []).enumerated().flatMap { i, t in
                ["### \(i + 1). \(t.title)"] + t.points.enumerated().map { j, p in
                    t.role(j) == "open" && !p.contains(Self.openMark) ? "- \(p) (\(Self.openMark))" : "- \(p)"
                }
            })
        }
        // a meeting puts its results (decisions, to-dos, open questions) before the talk topic by topic (docs/24 7-3)
        if shownKind != .meeting { flow() }
        func lines(_ kind: String) -> [String] {
            ledger.filter { $0.kind == kind }.map { e in
                guard kind == "commitment" else {
                    guard let r = e.replaces else { return "- \(shownRefs(e.text))" }
                    return "- \(shownRefs(e.text)) (\(String(localized: "이전 결정")): \(r.text) · \(Self.shortDay(r.at, calendar)) \(r.title))"
                }
                let who = [e.owner.map(ownerName), e.due].compactMap { $0 }.joined(separator: " · ")
                return "- [\(e.done ? "x" : " ")] \(e.text)\(who.isEmpty ? "" : " (\(who))")"
            }
        }
        if shownKind == .meeting {
            // a meeting's decisions and to-dos go under one heading, as they always have
            section(Self.decisionsHeading, shownKind.sections.flatMap { kind, title -> [String] in
                let items = lines(kind)
                return items.isEmpty ? [] : ["### \(title)"] + items
            })
            flow()
        } else {
            for (kind, title) in shownKind.sections { section("## \(title)", lines(kind)) }
        }
        section("## " + String(localized: "Tacie의 답"), questions.filter { !$0.unanswered }.map { q in
            let asker = q.voice.map { " (\(displayName(key: $0)))" } ?? ""
            return (q.answer.map { "- **\(q.question)** \(Self.uncited($0))" } ?? "- **\(q.question)**") + asker
        })
        section("## " + String(localized: "연결하면 답할 질문"), questions.filter { $0.noSource == true }.map { "- \($0.question)" })
        section("## " + String(localized: "문서로 남길 질문"), toDocumentLines)
        // a blank line between turns, or Markdown runs them into one paragraph
        if withTranscript { section("## " + String(localized: "대화 기록"), Array(transcriptLines.map { ["", $0] }.joined().dropFirst())) }
        return out.joined(separator: "\n") + "\n"
    }

    /// The mark on a point the meeting left open (`NoteTopic.roles`).
    static var openMark: String { String(localized: "미정") }

    /// A day as the app's language says it without the year: "10월 30일", "Oct 30".
    static func shortDay(_ date: Date, _ calendar: Calendar) -> String {
        let f = DateFormatter()
        f.calendar = calendar
        f.timeZone = calendar.timeZone
        f.locale = AppLocale.locale
        f.dateFormat = AppLocale.pattern(ko: "M월 d일", template: "MMMd")
        return f.string(from: date)
    }

    /// The Markdown heading over a meeting's decisions and to-dos; the page cuts its 목록 복사 at it too.
    static var decisionsHeading: String { "## " + String(localized: "결정과 할 일") }

    /// The transcript to paste: the title and the date, then one line per turn, "화자 2: …".
    func transcriptText(calendar: Calendar) -> String {
        ([displayTitle(calendar: calendar), header(calendar: calendar), ""] + transcriptLines).joined(separator: "\n") + "\n"
    }

    /// Who spoke and what, a line per turn as the page groups them; no times, 질문 tags, answers or unheard stretches.
    /// Before speakers every line is its own, words only.
    private var transcriptLines: [String] {
        guard hasSpeakers else { return transcript.map(\.text) }
        return turns.map { t in
            let words = t.lines.map(\.text).joined(separator: " ")
            return t.name.map { "\($0): \(words)" } ?? words
        }
    }

    /// The saved file's name: "2026-10-23 결제 실패 재시도 정책.md". Characters Finder or other systems refuse become spaces.
    func fileName(calendar: Calendar) -> String {
        let title = displayTitle(calendar: calendar)
            .components(separatedBy: CharacterSet(charactersIn: "/:\\").union(.whitespacesAndNewlines))
            .filter { !$0.isEmpty }
            .joined(separator: " ")
        return "\(Self.format("yyyy-MM-dd", start, calendar)) \(title).md"
    }

    /// The person's own title wins over the model's from then on. A blank one changes nothing.
    func renamed(_ title: String) -> SavedMeeting {
        let t = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return self }
        var m = self
        m.title = t
        m.titleEdited = true
        return m
    }

    /// Notes from the engine. A title the person set stays, and so does a check on a to-do with the same words.
    /// 문서로 남길 질문 as Markdown lines (the page's section and 목록 복사): each question as it reads on its own, and
    /// under it the answer said in the meeting when there was one (docs/24 4절).
    var toDocumentLines: [String] {
        questions.filter(\.toDocument).flatMap { q -> [String] in
            ["- \(q.shownQuestion)"] + (heardAnswer(q).map { ["  - " + String(localized: "회의에서 나온 답: \($0)")] } ?? [])
        }
    }

    /// The answer said in the meeting to a question no source answered, with who said it by the name shown now.
    func heardAnswer(_ q: SavedQuestion) -> String? {
        q.review?.answer.map(shownRefs)
    }

    mutating func apply(title: String?, summary: [String], topics: [NoteTopic]? = nil, entries: [LedgerEntry],
                        kind: NotesKind? = nil, kindSource: String? = nil) {
        // nothing came back but notes were written before (다시 쓰기): keep those rather than wipe them
        if summary.isEmpty, topics?.isEmpty ?? true, entries.isEmpty, !self.summary.isEmpty || !ledger.isEmpty {
            notes = .done
            notesProblem = nil
            return
        }
        if let title { notesTitle = title }
        if !titleEdited, !calendarTitled, let title { self.title = title }
        self.summary = summary.map(keyedRefs)
        self.topics = topics?.map { NoteTopic(title: $0.title, points: $0.points.map(keyedRefs), roles: $0.roles) }
        let checked = Set(ledger.filter(\.done).map(\.text))
        // what the person changed or added stays; the rest is written anew
        // another kind has other sections: what the person wrote does not fit them (they were told before)
        let sameKind = (kind ?? .meeting) == shownKind
        let kept = sameKind ? ledger.filter { $0.edited == true } : []
        if let kind {
            self.kind = kind
            self.kindSource = kindSource
        }
        // an entry written again with the same words keeps its id (and check), so other devices see it as the same one
        var ids: [String: [LedgerEntry]] = Dictionary(grouping: ledger.filter { $0.id != nil }) { "\($0.kind)\n\($0.text)" }
        ledger = entries.map { e in
            var e = e
            // a quote names who said it as [[name]]: kept by speaker key, like the summary
            e.text = keyedRefs(e.text)
            if checked.contains(e.text) { e.done = true }
            let key = "\(e.kind)\n\(e.text)"
            if let old = ids[key]?.first {
                ids[key]?.removeFirst()
                e.id = old.id
                if old.done { e.done = true }
            }
            return e
        }.filter { e in !kept.contains { $0.kind == e.kind && $0.text == e.text } } + kept
        ensureLedgerIds()
        notes = .done
        notesProblem = nil
    }

    static func who(_ owner: String) -> String {
        owner == "me" ? String(localized: "owner.me", defaultValue: "나")
            : owner == "them" ? String(localized: "owner.them", defaultValue: "상대") : owner
    }

    private static func minutes(_ from: Date, _ to: Date) -> Int { max(1, Int((to.timeIntervalSince(from) / 60).rounded())) }

    private static func uncited(_ s: String) -> String {
        s.replacingOccurrences(of: #"\s?\[\d+\]"#, with: "", options: .regularExpression)
    }

    static func format(_ pattern: String, _ date: Date, _ calendar: Calendar) -> String {
        formatter(pattern, calendar).string(from: date)
    }

    /// One formatter per pattern and calendar, kept: making a new one on every call was a tenth of the main window's
    /// work each time a to-do was checked (every list row and date label asks again).
    private static func formatter(_ pattern: String, _ calendar: Calendar) -> DateFormatter {
        let key = "\(pattern)|\(calendar.identifier)|\(calendar.timeZone.identifier)|\(calendar.firstWeekday)"
        formattersLock.lock()
        defer { formattersLock.unlock() }
        if let f = formatters[key] { return f }
        let f = DateFormatter()
        f.locale = AppLocale.locale
        f.calendar = calendar
        f.timeZone = calendar.timeZone
        f.dateFormat = localized(pattern)
        formatters[key] = f
        return f
    }

    /// The Korean patterns the app uses, as they always were in Korean; in English the system's pattern for the same
    /// fields ("a h:mm" → "h:mm a", "M월 d일 EEEE" → "EEE, MMM d"). Other patterns ("yyyy-MM-dd") stay as written.
    static func localized(_ pattern: String) -> String {
        guard let template = templates[pattern] else { return pattern }
        return AppLocale.pattern(ko: pattern, template: template)
    }

    private static let templates: [String: String] = [
        "a h:mm": "jmm",
        "M월": "MMMM",
        "yyyy년": "yyyy",
        "M월 d일": "MMMd",
        "yyyy년 M월": "yyyyMMMM",
        "yyyy년 M월 d일": "yyyyMMMd",
        "M월 d일 (E)": "MMMdEEE",
        "yyyy년 M월 d일 (E)": "yyyyMMMdEEE",
        "M월 d일 EEEE": "MMMdEEE",
        "M월 d일 EEEE a h:mm": "MMMMdEEEEjmm",
    ]

    private static var formatters: [String: DateFormatter] = [:]
    private static let formattersLock = NSLock()
}

/// The main window's list: today, yesterday, this week, last week, then months.
enum MeetingGroups {
    /// Which group a title is, so the list folds and shapes rows by kind, not by the (localized) title.
    enum Kind: Hashable { case today, yesterday, thisWeek, lastWeek, older }

    static func group(_ meetings: [SavedMeeting], now: Date, calendar: Calendar)
        -> [(title: String, kind: Kind, meetings: [SavedMeeting])] {
        let thisWeek = calendar.dateInterval(of: .weekOfYear, for: now)?.start ?? now
        let lastWeek = thisWeek.addingTimeInterval(-7 * 86_400)
        let sameYear = calendar.component(.year, from: now)
        func title(_ d: Date) -> (String, Kind) {
            if calendar.isDate(d, inSameDayAs: now) { return (String(localized: "오늘"), .today) }
            if calendar.isDate(d, inSameDayAs: now.addingTimeInterval(-86_400)) { return (String(localized: "어제"), .yesterday) }
            if d >= thisWeek { return (String(localized: "이번 주"), .thisWeek) }
            if d >= lastWeek { return (String(localized: "지난주"), .lastWeek) }
            return (SavedMeeting.format(calendar.component(.year, from: d) == sameYear ? "M월" : "yyyy년", d, calendar), .older)
        }
        var out: [(title: String, kind: Kind, meetings: [SavedMeeting])] = []
        for m in meetings.sorted(by: { $0.start > $1.start }) {
            let (t, k) = title(m.start)
            if out.last?.title == t { out[out.count - 1].meetings.append(m) } else { out.append((t, k, [m])) }
        }
        return out
    }

    /// Groups the list shows open: the recent days. Older months, and earlier years, fold shut until clicked.
    static let open: Set<Kind> = [.today, .yesterday, .thisWeek, .lastWeek]
    /// Groups whose rows show the time and the summary's first line; older rows are a title and a date.
    static let full: Set<Kind> = [.today, .yesterday, .thisWeek]

    /// After deleting the open meeting: the one below it, or above it when it was the last.
    static func selection(afterDeleting id: String, in ids: [String]) -> String? {
        guard let i = ids.firstIndex(of: id) else { return ids.first }
        if i + 1 < ids.count { return ids[i + 1] }
        return i > 0 ? ids[i - 1] : nil
    }
}
