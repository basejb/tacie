import Foundation

/// What's new (2026-11-10): after an update, the main window shows once what is worth trying. Only versions with an
/// entry here show it; a version of fixes and small changes has none, so the sheet does not come up after every update.
/// Someone who skipped versions sees the newest entry among them, with the others' titles under it.
/// A fresh install shows nothing: the onboarding already introduces the app.
struct WhatsNewEntry: Equatable {
    enum Scene: Equatable { case liveTranscript, teamSources, moreSources, islandTranscript, mcp }
    let version: String
    let scene: Scene
    let title: String
    let detail: String
    /// The rest of that version, one plain sentence each.
    let more: [String]
}

enum WhatsNew {
    /// Newest last. Write the copy with the release notes; it is reviewed with them.
    static let entries: [WhatsNewEntry] = [
        WhatsNewEntry(version: "0.1.32", scene: .liveTranscript,
                      title: String(localized: "말하는 동안 대화 기록에 바로 적혀요"),
                      detail: String(localized: "회의 중 메인 창을 열면 지금 하는 말이 연한 글씨로 먼저 보이고,\n문장이 끝나면 진하게 바뀌어요."),
                      more: [String(localized: "받아쓰기가 더 빠르고 정확해졌어요"),
                             String(localized: "연결이 느려지면 화면 위 Tacie의 시간 옆에 표시돼요"),
                             String(localized: "이름 없는 화자에 기억한 참석자를 추천해요")]),
        WhatsNewEntry(version: "0.1.39", scene: .teamSources,
                      title: String(localized: "Jira와 Confluence에서도 답을 찾아요"),
                      detail: String(localized: "설정 → 연결에서 Jira · Confluence를 연결하면\n이슈 댓글과 Confluence 페이지도 답의 근거가 돼요."),
                      more: [String(localized: "Google Drive의 PDF·Word·PowerPoint·Excel 파일도 읽어요"),
                             String(localized: "연결이 끊기면 메인 창과 메뉴에서 알려 줘요"),
                             String(localized: "설정 맨 위에서 설정을 검색할 수 있어요")]),
        WhatsNewEntry(version: "0.1.40", scene: .moreSources,
                      title: String(localized: "Asana, Dropbox, Obsidian에서도 답을 찾아요"),
                      detail: String(localized: "설정 → 연결에서 Asana·Dropbox를 연결하거나 Obsidian vault를 고르면\n그 안의 task, 파일, 노트도 답의 근거가 돼요."),
                      more: [String(localized: "3분 동안 아무도 말하지 않으면 회의 끝내기 카운트다운이 떠요")]),
        WhatsNewEntry(version: "0.1.41", scene: .islandTranscript,
                      title: String(localized: "회의 중에 대화 기록을 펼쳐 볼 수 있어요"),
                      detail: String(localized: "화면 위 Tacie에서 \"대화 기록\"을 누르면 화자별 대화가 말풍선으로 보여요.\n찾은 답은 질문 바로 아래에 붙어요."),
                      more: []),
        WhatsNewEntry(version: "0.1.45", scene: .mcp,
                      title: String(localized: "Claude·Cursor에서도 회의 기록과 연결한 서비스로 답을 찾아요"),
                      detail: String(localized: "설정 → 연결 → MCP에서 Claude Desktop, Cursor, Claude Code, Codex를 연결하면 그 앱에서 물어볼 때 Tacie가 회의 기록과 연결한 서비스에서 찾아 답해요. Tacie 앱이 열려 있어야 해요."),
                      more: [String(localized: "메인 창이 홈에서 열려요"),
                             String(localized: "회의 설정이 설정의 회의 탭으로 모였어요")]),
    ]

    static let seenKey = "whatsNewSeen"
    static var installed: String? { Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String }
    /// Installs from before this sheet existed have no seen version: they had 0.1.31's features already.
    static let before = "0.1.31"
    /// Dev builds launched with `--whats-new`: the sheet comes up whatever was seen, so it can be looked at.
    static var demo: Bool { Build.isDev && ProcessInfo.processInfo.arguments.contains("--whats-new") }

    /// What to show going from `seen` to `installed`: the newest entry after `seen` and up to `installed`, its `more`
    /// followed by the titles of the older ones skipped. Nil when there is none.
    static func pick(seen: String?, installed: String?, entries: [WhatsNewEntry] = entries) -> WhatsNewEntry? {
        guard let installed, let top = key(installed) else { return nil }
        let from = key(seen ?? before) ?? []
        let due = entries.filter { e in key(e.version).map { $0.lexicographicallyPrecedes(top) || $0 == top } ?? false }
            .filter { e in key(e.version).map { from.lexicographicallyPrecedes($0) } ?? false }
            .sorted { key($0.version)!.lexicographicallyPrecedes(key($1.version)!) }
        guard let newest = due.last else { return nil }
        let older = due.dropLast().reversed().map(\.title)
        return WhatsNewEntry(version: newest.version, scene: newest.scene, title: newest.title, detail: newest.detail,
                             more: newest.more + older)
    }

    /// The entry due on this Mac now, if any.
    /// A source build shows no "새로 생긴 기능" sheet: it has no update feed (docs/41 0절 17번, #56).
    static let offered = Build.hasAccount

    static var due: WhatsNewEntry? {
        guard offered else { return nil }
        if demo { return pick(seen: "0", installed: installed) ?? entries.last }
        return pick(seen: UserDefaults.standard.string(forKey: seenKey), installed: installed)
    }

    /// Closed, or a fresh install with nothing to announce: nothing up to this version shows again.
    static func markSeen() {
        guard let v = installed else { return }
        UserDefaults.standard.set(v, forKey: seenKey)
    }

    /// The release notes page of the installed version, showing every version after the one last seen (UpdateNotes).
    static var notesURL: URL? {
        guard let v = installed else { return nil }
        return UpdateNotes.since(UpdateNotes.page(version: v),
                                 installed: UserDefaults.standard.string(forKey: seenKey) ?? before)
    }

    static func key(_ v: String) -> [Int]? {
        let parts = v.split(separator: ".").map { Int($0) }
        return parts.contains(nil) || parts.isEmpty ? nil : parts.compactMap { $0 }
    }
}
