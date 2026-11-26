import Foundation

/// Pure strings of the settings pages (Korean or English, AppLocale), so tests can pin them without the app.
enum SettingsText {
    private static var locale: Locale { AppLocale.locale }

    private static func format(_ pattern: String, _ date: Date, _ calendar: Calendar) -> String {
        let f = DateFormatter()
        f.locale = locale
        f.calendar = calendar
        f.timeZone = calendar.timeZone
        f.dateFormat = pattern
        return f.string(from: date)
    }

    /// "마지막 동기화 · 오늘 오전 1:23", "… · 어제 오후 4:00", "… · 10월 21일 오후 4:00".
    static func synced(_ at: Date, now: Date, calendar: Calendar) -> String {
        String(localized: "마지막 동기화 · \(when(at, now: now, calendar: calendar))")
    }

    /// The menu bar's sync item: what it does, with the state after it ("지금 동기화 · 오늘 오후 1:55 동기화함",
    /// "동기화 중 · 40%", "지금 동기화 · 지난번에 실패했어요", "지금 동기화 · 노션 연결이 끊겼어요").
    /// `lost`: the connections to connect again (IndexKeeper.lostNames).
    static func menuSync(fraction: Double?, syncing: Bool, failed: Bool, lost: [String] = [], builtAt: Date?, now: Date,
                         calendar: Calendar) -> String {
        if syncing { return String(localized: "동기화 중") + (fraction.map { " · \(Int($0 * 100))%" } ?? "") }
        if !lost.isEmpty { return String(localized: "지금 동기화") + " · " + lostLine(lost) }
        if failed { return String(localized: "지금 동기화 · 지난번에 실패했어요") }
        if let builtAt { return String(localized: "지금 동기화 · \(when(builtAt, now: now, calendar: calendar)) 동기화함") }
        return String(localized: "지금 동기화")
    }

    /// "노션 연결이 끊겼어요", "노션, Linear 연결이 끊겼어요": what the menu bar and the main window's foot say until
    /// the user connects again in 설정 → 연결.
    static func lostLine(_ names: [String]) -> String {
        String(localized: "\(names.joined(separator: ", ")) 연결이 끊겼어요")
    }

    /// "오늘 오후 1:55", "어제 오전 9:02", "10월 25일 오후 3:10".
    static func when(_ at: Date, now: Date, calendar: Calendar) -> String {
        let time = format(AppLocale.pattern(ko: "a h:mm", template: "jmm"), at, calendar)
        let day: String
        if calendar.isDate(at, inSameDayAs: now) {
            day = String(localized: "오늘")
        } else if let y = calendar.date(byAdding: .day, value: -1, to: now), calendar.isDate(at, inSameDayAs: y) {
            day = String(localized: "어제")
        } else {
            day = format(AppLocale.pattern(ko: "M월 d일", template: "MMMd"), at, calendar)
        }
        return "\(day) \(time)"
    }

    /// MiB below a GiB ("125MB", at least 1), one decimal from a GiB ("2.2GB"). English puts a space before the unit.
    static func size(_ bytes: Int) -> String {
        let mib = Double(bytes) / 1_048_576
        let gap = AppLocale.isKorean ? "" : " "
        if mib >= 1024 { return String(format: "%.1f", locale: locale, mib / 1024) + gap + "GB" }
        return "\(max(1, Int(mib.rounded())))" + gap + "MB"
    }

    static func holdings(files: Int?, items: Int?) -> String? {
        let parts = [files.map { String(localized: "파일 \($0)개") }, items.map { String(localized: "PR과 이슈 \($0)개") }]
            .compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    static func retention(_ r: Retention) -> String {
        switch r {
        case .days30: String(localized: "30일 보관")
        case .days90: String(localized: "90일 보관")
        case .year: String(localized: "1년 보관")
        case .forever: String(localized: "계속 보관")
        }
    }

    /// The Linear card's line (docs/18 7절, 7-1절, 0절) from the engine's `linear` block. `lost`: Linear refused the
    /// refresh token, so the user has to connect again; after seven days the found issues are out of search.
    static func linearLine(_ info: IndexState.LinearInfo?, lost: Bool, now: Date) -> String {
        // the engine's lost_since counts too: it knows the connection is lost even before the app does
        let engineSince = info?.lostSince.flatMap(Self.isoDate)
        if lost || engineSince != nil {
            // no day from the engine and nothing found (left): a day counted from now would slide with the clock
            if engineSince == nil, (info?.issues ?? 0) == 0 { return String(localized: "Linear 연결이 끊겼어요") }
            let since = engineSince ?? now
            let until = since.addingTimeInterval(7 * 86_400)
            if now >= until { return String(localized: "Linear 연결이 끊겨서 Linear 자료는 찾지 않아요") }
            let day = lostUntil(until)
            return String(localized: "Linear 연결이 끊겼어요 · 다시 연결하지 않으면 \(day)부터 Linear 자료는 찾지 않아요")
        }
        guard let info else { return String(localized: "연결됨") }
        // nothing found yet and the fetch failed: say why before "no issues"
        if (info.issues ?? 0) == 0, let kind = info.errorKind {
            return kind == "key" ? String(localized: "워크스페이스 관리자가 연결을 허용해야 해요")
                : String(localized: "Linear에 닿지 못했어요. 다음 동기화에 다시 받아요")
        }
        if info.issues == 0 { return String(localized: "받을 이슈가 없어요") }
        let parts = [info.workspace, info.issues.map { String(localized: "이슈 \($0)개") }]
        let line = parts.compactMap { $0 }.joined(separator: " · ")
        return line.isEmpty ? String(localized: "연결됨") : line
    }

    /// The Google Drive card's line (docs/19 8-1절) from the engine's `gdrive` block. `lost`: Google refused the refresh
    /// token, so the user has to connect again; after seven days the found files are out of search.
    static func gdriveLine(_ info: IndexState.GDriveInfo?, lost: Bool, now: Date) -> String {
        // the engine's lost_since counts too: it knows the connection is lost even before the app does
        let engineSince = info?.lostSince.flatMap(Self.isoDate)
        if lost || engineSince != nil {
            // no day from the engine and nothing found (left): a day counted from now would slide with the clock
            if engineSince == nil, (info?.files ?? 0) == 0 { return String(localized: "Google Drive 연결이 끊겼어요") }
            let since = engineSince ?? now
            let until = since.addingTimeInterval(7 * 86_400)
            if now >= until { return String(localized: "Google Drive 연결이 끊겨서 Drive 자료는 찾지 않아요") }
            let day = lostUntil(until)
            return String(localized: "Google Drive 연결이 끊겼어요 · 다시 연결하지 않으면 \(day)부터 Drive 자료는 찾지 않아요")
        }
        guard let info else { return String(localized: "연결됨") }
        if (info.files ?? 0) == 0, let kind = info.errorKind {
            return kind == "key" ? String(localized: "Google Drive가 연결을 받아 주지 않았어요. 다시 연결해 주세요")
                : String(localized: "Google Drive에 닿지 못했어요. 다음 동기화에 다시 받아요")
        }
        if info.files == 0, info.initialDone == true { return String(localized: "받을 문서가 없어요") }
        let count = info.files.map { String(localized: "문서 \($0)개") }
        let parts = [info.account, count, info.initialDone == false ? String(localized: "더 받는 중") : nil]
        let line = parts.compactMap { $0 }.joined(separator: " · ")
        return line.isEmpty ? String(localized: "연결됨") : line
    }

    /// The Jira · Confluence card's line (docs/31 5절, 5-1절) from the engine's `atlassian` block. `lost`: Atlassian
    /// refused the refresh token, so the user has to connect again; after seven days the found items are out of search.
    /// `signedOut`: the relay wants a Tacie session for the refresh (not a lost connection). A pause for Atlassian's
    /// hourly limit says when fetching carries on, in this Mac's time (`calendar`).
    static func atlassianLine(_ info: IndexState.AtlassianInfo?, lost: Bool, signedOut: Bool = false, now: Date,
                              calendar: Calendar = .current) -> String {
        // the engine's lost_since counts too: it knows the connection is lost even before the app does
        let engineSince = info?.lostSince.flatMap(Self.isoDate)
        if lost || engineSince != nil {
            // no day from the engine and nothing found (left): a day counted from now would slide with the clock
            if engineSince == nil, (info?.issues ?? 0) + (info?.pages ?? 0) == 0 {
                return String(localized: "Jira · Confluence 연결이 끊겼어요")
            }
            let since = engineSince ?? now
            let until = since.addingTimeInterval(7 * 86_400)
            if now >= until { return String(localized: "Jira · Confluence 연결이 끊겨서 찾지 않아요") }
            let day = lostUntil(until)
            return String(localized: "Jira · Confluence 연결이 끊겼어요 · 다시 연결하지 않으면 \(day)부터 찾지 않아요")
        }
        if signedOut { return atlassianSignedOut }
        guard let info else { return String(localized: "연결됨") }
        if let until = info.pausedUntil.flatMap(Self.isoDate), until > now {
            let minute = calendar.component(.minute, from: until)
            let time = format(minute == 0 ? AppLocale.pattern(ko: "a h시", template: "j")
                                          : AppLocale.pattern(ko: "a h시 m분", template: "jmm"), until, calendar)
            return String(localized: "Atlassian 한도에 걸려 잠시 멈췄어요 · \(time)에 이어서 받아요")
        }
        let found = (info.issues ?? 0) + (info.pages ?? 0)
        if found == 0, info.issues != nil || info.pages != nil, info.initialDone == true {
            return String(localized: "받을 이슈와 페이지가 없어요")
        }
        if found == 0, let kind = info.errorKind, kind != "key" {
            return String(localized: "Atlassian에 닿지 못했어요. 다음 동기화에 다시 받아요")
        }
        if info.initialDone == false { return String(localized: "최근 자료부터 받는 중 · 나머지는 이어서 받아요") }
        let parts = [String(localized: "연결됨"), info.site, info.issues.map { String(localized: "이슈 \($0)개") },
                     info.pages.map { String(localized: "페이지 \($0)개") }]
        return parts.compactMap { $0 }.joined(separator: " · ")
    }

    /// Atlassian ended the approval with an error other than the user's cancel, an admin's block among them (docs/31 5-1절).
    static var atlassianBlocked: String { String(localized: "Atlassian 조직 관리자가 바깥 앱 연결을 막아 두었어요") }
    /// The keychain could not be read at launch (locked, refused): the connection is unknown until it is read again.
    static var atlassianUnreadable: String { String(localized: "키체인에서 Jira · Confluence 연결을 읽지 못했어요") }
    /// The relay wants a Tacie session to connect or renew Jira · Confluence.
    static var atlassianSignedOut: String { String(localized: "Tacie에 로그인한 뒤에 연결할 수 있어요") }

    /// The Asana card's line (docs/32 5절, 5-1절) from the engine's `asana` block. `lost`: the relay said Asana refused
    /// the refresh token (AsanaLink.lost; the engine's `lost_since` alone may lag behind a new connection), so the user
    /// has to connect again; seven days after `lost_since` (or `lostAt`, when the app saw it) the tasks leave search.
    /// `signedOut`: the relay wants a Tacie session for the refresh (not a lost connection).
    static func asanaLine(_ info: IndexState.AsanaInfo?, lost: Bool, lostAt: Date? = nil, signedOut: Bool = false,
                          now: Date) -> String {
        // the engine's lost_since counts too: it knows the connection is lost even before the app does
        let engineSince = info?.lostSince.flatMap(Self.isoDate)
        if lost || engineSince != nil {
            // no day from the engine and nothing found (left), or no day at all: say no day
            guard let since = engineSince ?? lostAt, engineSince != nil || (info?.tasks ?? 0) > 0 else {
                return String(localized: "Asana 연결이 끊겼어요")
            }
            let until = since.addingTimeInterval(7 * 86_400)
            if now >= until { return String(localized: "Asana 연결이 끊겨서 찾지 않아요") }
            let day = lostUntil(until)
            return String(localized: "Asana 연결이 끊겼어요 · 다시 연결하지 않으면 \(day)부터 찾지 않아요")
        }
        if signedOut { return atlassianSignedOut }
        guard let info else { return String(localized: "연결됨") }
        let found = info.tasks ?? 0
        if found == 0, info.tasks != nil, info.initialDone == true { return String(localized: "받을 task가 없어요") }
        if found == 0, let kind = info.errorKind, kind != "key" {
            return String(localized: "Asana에 닿지 못했어요. 다음 동기화에 다시 받아요")
        }
        if info.initialDone == false { return String(localized: "받은 task부터 찾아요 · 나머지는 이어서 받아요") }
        let parts = [String(localized: "연결됨"), info.workspaces.map { String(localized: "워크스페이스 \($0)개") },
                     info.tasks.map { String(localized: "task \($0)개") }]
        return parts.compactMap { $0 }.joined(separator: " · ")
    }

    /// The workspaces an admin closed to outside apps (docs/32 5-1절): the rest are fetched. nil when none.
    static func asanaBlockedLine(_ info: IndexState.AsanaInfo?) -> String? {
        guard let names = info?.blockedWorkspaces, !names.isEmpty else { return nil }
        let list = names.joined(separator: ", ")
        return String(localized: "\(list) 워크스페이스는 관리자가 막아 두어 받지 않아요")
    }

    /// The keychain could not be read at launch (locked, refused): the connection is unknown until it is read again.
    static var asanaUnreadable: String { String(localized: "키체인에서 Asana 연결을 읽지 못했어요") }

    /// The Dropbox card's line (docs/33) from the engine's `dropbox` block. `lost`/`lostAt`/`lost_since` as on Asana's:
    /// seven days after the day the connection broke the files leave search.
    static func dropboxLine(_ info: IndexState.DropboxInfo?, lost: Bool, lostAt: Date? = nil, now: Date) -> String {
        let engineSince = info?.lostSince.flatMap(Self.isoDate)
        if lost || engineSince != nil {
            guard let since = engineSince ?? lostAt, engineSince != nil || (info?.files ?? 0) > 0 else {
                return String(localized: "Dropbox 연결이 끊겼어요")
            }
            let until = since.addingTimeInterval(7 * 86_400)
            if now >= until { return String(localized: "Dropbox 연결이 끊긴 지 7일이 지나 이제 찾지 않아요") }
            let day = lostUntil(until)
            return String(localized: "Dropbox 연결이 끊겼어요 · 다시 연결하지 않으면 \(day)부터 찾지 않아요")
        }
        guard let info else { return String(localized: "연결됨") }
        let found = info.files ?? 0
        if info.files == 0, info.initialDone == true { return String(localized: "받을 파일이 없어요") }
        // a refused token ("key") is the app's to renew (`refused`, renew), so the card stays connected
        if found == 0, let kind = info.errorKind, kind != "key" {
            return String(localized: "Dropbox에 닿지 못했어요. 다음 동기화에 다시 받아요")
        }
        let parts = [String(localized: "연결됨"), info.account, info.files.map { String(localized: "파일 \($0)개") },
                     info.initialDone == false ? String(localized: "더 받는 중") : nil]
        return parts.compactMap { $0 }.joined(separator: " · ")
    }

    /// The keychain could not be read at launch (locked, refused): the connection is unknown until it is read again.
    static var dropboxUnreadable: String { String(localized: "키체인에서 Dropbox 연결을 읽지 못했어요") }

    /// The day a lost connection's data leaves search, in Seoul time as before: "11월 18일", "Nov 18".
    private static func lostUntil(_ until: Date) -> String {
        var seoul = Calendar(identifier: .gregorian)
        seoul.timeZone = TimeZone(identifier: "Asia/Seoul")!
        return format(AppLocale.pattern(ko: "M월 d일", template: "MMMd"), until, seoul)
    }

    private static func isoDate(_ s: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.date(from: s) ?? ISO8601DateFormatter().date(from: s)
    }

    /// Whole minutes: "3시간 10분", "2시간", "45분", "0분".
    static func duration(_ t: TimeInterval) -> String {
        let minutes = max(0, Int(t) / 60)
        let h = minutes / 60, m = minutes % 60
        if h == 0 { return String(localized: "\(m)분") }
        return m == 0 ? String(localized: "\(h)시간") : String(localized: "\(h)시간 \(m)분")
    }

    /// How long until the monthly usage resets, the bold part of "12일 뒤에 자동으로 초기화돼요".
    static func untilReset(_ t: TimeInterval) -> String { UsageMonth.untilReset(t) }

    /// What the engine's refresh step is doing.
    static func step(_ step: String?) -> String {
        switch step {
        case "repo": return String(localized: "GitHub 코드와 문서 받는 중")
        case "sources": return String(localized: "GitHub PR과 이슈 받는 중")
        case "notion": return String(localized: "노션 페이지 받는 중")
        case "linear": return String(localized: "Linear 이슈 받는 중")
        case "gdrive": return String(localized: "Google Drive 문서 받는 중")
        case "atlassian": return String(localized: "Jira 이슈와 Confluence 페이지 받는 중")
        case "asana": return String(localized: "Asana task 받는 중")
        case "dropbox": return String(localized: "Dropbox 파일 받는 중")
        case "embed", "swap": return String(localized: "검색 준비 중")
        default: return String(localized: "준비 중")
        }
    }

    /// "동기화하는 중 · 검색 준비 중 42%": the percent while embedding reports its progress.
    static func syncing(step s: String?, progress: (done: Int, total: Int)?) -> String {
        var line = String(localized: "동기화하는 중 · \(step(s))")
        if let p = progress, p.total > 0 {
            line += " \(min(100, max(0, p.done * 100 / p.total)))%"
        }
        return line
    }

    /// Repos whose full name contains the query (case does not matter); an empty query keeps all, in order.
    static func filter(_ repos: [GitHubRepo], query: String) -> [GitHubRepo] {
        let q = query.trimmingCharacters(in: .whitespaces)
        if q.isEmpty { return repos }
        return repos.filter { $0.fullName.localizedCaseInsensitiveContains(q) }
    }

    /// Why loading GitHub or a sync failed, from the engine's `error_kind`. Any other kind ("down", "credit", none)
    /// need not be GitHub's fault, so it does not name GitHub.
    static func githubProblem(_ kind: String?) -> String {
        switch kind {
        case "key": String(localized: "GitHub 연결을 다시 해 주세요.")
        case "network": String(localized: "인터넷 연결을 확인해 주세요.")
        case "busy": String(localized: "GitHub가 잠시 바빠요. 조금 뒤에 다시 해 주세요.")
        default: String(localized: "동기화하지 못했어요. 잠시 뒤에 다시 해 주세요.")
        }
    }

    /// The engine's names of sources that kept a saved copy, in the order they are named, with the Korean particle
    /// after each (English has none) and the English name.
    private static let kept: [(source: String, name: String, particle: String, en: String)] = [
        ("repo", "코드", "는", "Code"), ("github", "GitHub", "는", "GitHub"), ("slack", "슬랙", "은", "Slack"),
        ("notion", "노션", "은", "Notion"), ("linear", "Linear", "는", "Linear"), ("gdrive", "Google Drive", "는", "Google Drive"),
        ("atlassian", "Jira · Confluence", "는", "Jira · Confluence"), ("asana", "Asana", "는", "Asana"),
        ("dropbox", "Dropbox", "는", "Dropbox"),
    ]

    /// What the menu line adds after a sync, from the refresh's `warnings`: sources that kept their saved copy
    /// ("GitHub·슬랙은 지난 데이터로 찾아요", 은/는 from the last name), and "github-lost" when GitHub cannot see the
    /// repo, the two joined with " · ". nil when there is nothing to say.
    static func syncWarning(_ warnings: [String]) -> String? {
        let lost = warnings.contains("github-lost")
        let names = kept.filter { warnings.contains($0.source) && !(lost && $0.source == "github") }
        var parts: [String] = []
        if lost { parts.append(String(localized: "GitHub 저장소를 볼 수 없어 PR·이슈 없이 찾아요")) }
        if let last = names.last {
            if AppLocale.isKorean {
                // the particle follows the last name: Korean only
                parts.append("\(names.map(\.name).joined(separator: "·"))\(last.particle) 지난 데이터로 찾아요")
            } else {
                let f = ListFormatter()
                f.locale = locale
                let list = f.string(from: names.map(\.en)) ?? names.map(\.en).joined(separator: ", ")
                parts.append(String(localized: "sync.keptNames", defaultValue: "\(list) 지난 데이터로 찾아요"))
            }
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// The engine did not take a sync the user started (no answer, refused).
    static var syncNotStarted: String { String(localized: "동기화를 시작하지 못했어요") }

    /// The repo picker's button for the chosen repo and branch; nil when they are the ones in use (nothing to do).
    static func repoAction(current: Bool, hasRepo: Bool, sameBranch: Bool) -> String? {
        if current { return sameBranch ? nil : String(localized: "이 브랜치로 바꾸기") }
        return hasRepo ? String(localized: "이 저장소로 바꾸기") : String(localized: "이 저장소 연결")
    }

    /// The GitHub card while the search data moves to another repo; until it ends the card would name the old one.
    static func switching(to repo: String) -> String { String(localized: "저장소를 바꾸는 중 · \(repo)") }

    /// Under the repo list: an organization that has not approved Tacie (OAuth App access restrictions) lists none of
    /// its repos and GitHub does not say so, so the list cannot point at it.
    static var orgHint: String { String(localized: "조직의 저장소가 보이지 않으면 그 조직이 아직 Tacie를 승인하지 않은 거예요. 조직 소유자가 GitHub 설정에서 승인할 수 있어요.") }

    /// The GitHub card's line while the engine leaves GitHub out (IndexState.githubLost): GitHub cannot see the repo, or
    /// the name is another repo now, which only "다시 만들기" (the connection page's rebuild button) takes on.
    static var githubLost: String { String(localized: "GitHub에서 이 저장소를 볼 수 없어서 PR과 이슈를 검색에서 뺐어요. 권한을 확인한 뒤 다시 동기화하고, 다른 저장소로 바뀌었다면 '다시 만들기'를 눌러 주세요.") }

    /// "10월 22일" from an ISO-8601 time (nil when there is none).
    static func pushed(_ iso: String?, calendar: Calendar) -> String? {
        guard let iso, let date = ISO8601DateFormatter().date(from: iso) else { return nil }
        return format(AppLocale.pattern(ko: "M월 d일", template: "MMMd"), date, calendar)
    }
}
