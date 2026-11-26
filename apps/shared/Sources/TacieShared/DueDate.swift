import Foundation

/// A to-do's due as a day. The meeting notes keep the due as it was said ("by Friday" 금요일까지, "early next week" 다음 주 초);
/// this reads the common ways of saying it against the day of the meeting, so a to-do from two weeks ago still says
/// which Friday. What it cannot read stays as said and has no day.
enum DueDate {
    /// The day `said` points at, counted from the meeting's `start` (weeks start on Monday). Nil when nothing was said
    /// or it is not a way of naming a day this knows.
    static func day(_ said: String?, from start: Date, calendar cal: Calendar) -> Date? {
        guard let said else { return nil }
        let s = said.replacingOccurrences(of: " ", with: "")
        guard !s.isEmpty else { return nil }
        let base = cal.startOfDay(for: start)
        func plus(_ n: Int) -> Date? { cal.date(byAdding: .day, value: n, to: base) }

        // 11월 7일, 11/7
        if let m = match(s, #"(\d{1,2})월(\d{1,2})일"#) ?? match(s, #"(?<![\d/])(\d{1,2})/(\d{1,2})(?![\d/])"#),
           let month = Int(m[0]), let day = Int(m[1]) {
            return dated(month: month, day: day, near: base, calendar: cal)
        }
        // 3일 안에, 2주 뒤
        if let m = match(s, #"(\d+)(일|주)(안|이내|내|뒤|후)"#), let n = Int(m[0]) {
            return plus(m[1] == "주" ? n * 7 : n)
        }
        if s.contains("모레") { return plus(2) }
        if s.contains("내일") { return plus(1) }
        if ["오늘", "퇴근", "이따", "당장", "바로", "즉시", "빨리", "ASAP", "asap"].contains(where: s.contains) { return base }

        // weeks: 이번 주, 다음 주, 다다음 주 and a weekday
        let weeks: Int? = s.contains("다다음주") ? 2
            : ["다음주", "담주", "차주"].contains(where: s.contains) ? 1
            : ["이번주", "금주"].contains(where: s.contains) ? 0 : nil
        let monday = plus(-((cal.component(.weekday, from: base) + 5) % 7))!
        func weekday(_ i: Int, _ w: Int) -> Date { cal.date(byAdding: .day, value: w * 7 + i, to: monday)! }
        if let m = match(s, #"([월화수목금토일])요일"#), let i = "월화수목금토일".firstIndex(of: Character(m[0])) {
            let d = weekday("월화수목금토일".distance(from: "월화수목금토일".startIndex, to: i), weeks ?? 0)
            return weeks == nil && d < base ? cal.date(byAdding: .day, value: 7, to: d) : d
        }
        if s.contains("주말") {
            let d = weekday(5, weeks ?? 0)
            return weeks == nil && d < base ? cal.date(byAdding: .day, value: 7, to: d) : d
        }
        if let w = weeks {
            if s.contains("초") { return weekday(0, w) }
            if s.contains("중반") { return weekday(2, w) }
            let friday = weekday(4, w)
            return friday < base ? weekday(6, w) : friday
        }

        // months: 이번 달, 다음 달, 월말
        let months: Int? = ["다음달", "담달"].contains(where: s.contains) ? 1
            : ["이번달", "이달", "월말"].contains(where: s.contains) ? 0 : nil
        if let n = months, let first = cal.date(from: cal.dateComponents([.year, .month], from: base)),
           let month = cal.date(byAdding: .month, value: n, to: first) {
            if s.contains("초") { return n == 0 ? nil : month }
            if s.contains("중순") { return cal.date(byAdding: .day, value: 14, to: month) }
            return cal.date(byAdding: DateComponents(month: 1, day: -1), to: month)
        }
        // 15일까지: this month, or the next when that day has passed
        if let m = match(s, #"(?<!\d)(\d{1,2})일"#), let day = Int(m[0]), (1...31).contains(day) {
            var c = cal.dateComponents([.year, .month], from: base)
            c.day = day
            guard let d = cal.date(from: c), cal.component(.day, from: d) == day else { return nil }
            return d < base ? cal.date(byAdding: .month, value: 1, to: d) : d
        }
        return nil
    }

    /// A month and day said without a year: the one of the meeting's year, unless that is long before the meeting
    /// (said in December for January).
    private static func dated(month: Int, day: Int, near base: Date, calendar cal: Calendar) -> Date? {
        guard (1...12).contains(month), (1...31).contains(day) else { return nil }
        var c = DateComponents(year: cal.component(.year, from: base), month: month, day: day)
        guard let d = cal.date(from: c), cal.component(.day, from: d) == day else { return nil }
        if let limit = cal.date(byAdding: .day, value: -60, to: base), d < limit {
            c.year! += 1
            return cal.date(from: c)
        }
        return d
    }

    private static func match(_ s: String, _ pattern: String) -> [String]? {
        guard let re = try? NSRegularExpression(pattern: pattern),
              let m = re.firstMatch(in: s, range: NSRange(s.startIndex..., in: s)) else { return nil }
        return (1..<m.numberOfRanges).compactMap { Range(m.range(at: $0), in: s).map { String(s[$0]) } }
    }
}

/// Where a to-do sits on the 할 일 page by its due day: past it, today, the rest of this week, later, or no day.
enum DueBucket: Int, CaseIterable, Comparable {
    case overdue, today, week, later, none

    var title: String {
        switch self {
        case .overdue: "기한 지남"
        case .today: "오늘"
        case .week: "이번 주"
        case .later: "나중"
        case .none: "기한 없음"
        }
    }

    init(_ day: Date?, now: Date, calendar cal: Calendar) {
        guard let day else { self = .none; return }
        let n = DueBucket.days(to: day, now: now, calendar: cal)
        if n < 0 { self = .overdue; return }
        if n == 0 { self = .today; return }
        let today = cal.startOfDay(for: now)
        let sunday = cal.date(byAdding: .day, value: 6 - (cal.component(.weekday, from: today) + 5) % 7, to: today)!
        self = day <= sunday ? .week : .later
    }

    /// Whole days from today to `day`: negative once it has passed.
    static func days(to day: Date, now: Date, calendar cal: Calendar) -> Int {
        cal.dateComponents([.day], from: cal.startOfDay(for: now), to: cal.startOfDay(for: day)).day ?? 0
    }

    static func < (a: DueBucket, b: DueBucket) -> Bool { a.rawValue < b.rawValue }
}
