import SwiftUI

/// Search settings (2026-11-12): the field at the top of the settings list. Typing swaps the panes for the rows that
/// match; picking one opens its pane, scrolls to the row and marks it for a moment.
///
/// The rows are listed here by hand, with the same localized titles the pages use (so English finds them too).
/// A row added to a page belongs here as well.
struct SettingsSearchEntry: Identifiable, Equatable {
    let pane: SettingsPane
    /// The section the row sits in, shown after the page name in the results ("회의 › 회의 중").
    var section: String? = nil
    let title: String
    /// Where the page scrolls: a row's title (`SettingsRow` carries it as its id), the page's footer links, or the top.
    var anchor: SettingsAnchor = .row
    /// Other words that find it, in both languages: "다크" finds 테마.
    var words: [String] = []
    /// The connection a 연결 card stands for: the row is listed only where that connection is offered.
    var source: ConnectionsScene.Source? = nil
    var id: String { "\(pane.rawValue)/\(title)" }

    static func == (a: Self, b: Self) -> Bool { a.id == b.id }
}

enum SettingsAnchor: Equatable { case row, footer, top }

enum SettingsSearch {
    /// The id of each page's footer links (`FooterLinks`).
    static let footerID = "settings-footer"

    @MainActor static var entries: [SettingsSearchEntry] {
        let start = String(localized: "시작"), screen = String(localized: "화면"), island = String(localized: "화면 위 Tacie")
        let update = String(localized: "업데이트"), links = String(localized: "settings.search.links", defaultValue: "아래 링크")
        let begin = String(localized: "회의를 시작할 때"), during = String(localized: "회의 중")
        let after = String(localized: "회의가 끝나면"), record = String(localized: "기록")
        let connected = String(localized: "연결됨"), sync = String(localized: "동기화")
        let share = String(localized: "화면 공유"), delete = String(localized: "데이터 삭제")
        var all: [SettingsSearchEntry] = [
            .init(pane: .general, section: start, title: String(localized: "로그인 시 자동 실행"), words: ["login", "시작", "startup"]),
            .init(pane: .general, section: screen, title: String(localized: "테마"), words: ["다크", "라이트", "dark", "light", "모드"]),
            .init(pane: .general, section: screen, title: String(localized: "언어"), words: ["한국어", "English", "language", "영어"]),
            .init(pane: .general, section: island, title: String(localized: "항상 표시"), words: ["화면 위 Tacie", "island"]),
            .init(pane: .general, section: links, title: String(localized: "문제 신고"), anchor: .footer, words: ["버그", "bug", "로그", "log"]),
            .init(pane: .general, section: links, title: String(localized: "처음 안내 다시 보기"), anchor: .footer, words: ["온보딩", "onboarding"]),
            .init(pane: .general, section: links, title: String(localized: "오픈소스 라이선스"), anchor: .footer, words: ["license"]),

            .init(pane: .meeting, section: begin, title: String(localized: "회의 모드"), words: ["답 카드", "기록만", "card"]),
            .init(pane: .meeting, section: begin, title: String(localized: "원격 회의 소리"), words: ["마이크", "mic", "Zoom", "줌", "화자"]),
            .init(pane: .meeting, section: begin, title: String(localized: "회의 시작 알림"), words: ["알림", "notification"]),
            .init(pane: .meeting, section: during, title: String(localized: "녹음 알림"), words: ["참석자", "동의", "consent", "notice"]),
            .init(pane: .meeting, section: during, title: String(localized: "참석자 안내 문구"), words: ["동의", "consent", "복사"]),
            .init(pane: .meeting, section: during, title: String(localized: "음악 멈추기"), words: ["Spotify", "Music", "음악", "music"]),
            .init(pane: .meeting, section: after, title: String(localized: "회의 정리"), words: ["요약", "summary", "AI", "결정", "할 일"]),
            .init(pane: .meeting, section: after, title: String(localized: "정리 언어"), words: ["언어", "language", "영어", "English", "한국어", "요약", "summary"]),
            .init(pane: .meeting, section: after, title: String(localized: "회의 정리 자동 열기"), words: ["요약", "summary", "메인 창"]),
            .init(pane: .meeting, section: record, title: String(localized: "회의 녹음"), words: ["녹음", "record", "audio"]),
            .init(pane: .meeting, section: record, title: String(localized: "참석자 기억"), words: ["화자", "speaker", "이름"]),
            .init(pane: .meeting, section: record, title: String(localized: "회의 기록 보관"), words: ["보관", "저장", "기간"]),

            .init(pane: .connections, section: connected, title: "GitHub", anchor: .top, words: ["저장소", "repo", "깃허브"], source: .github),
            .init(pane: .connections, section: connected, title: String(localized: "노션"), anchor: .top, words: ["Notion"], source: .notion),
            .init(pane: .connections, section: connected, title: "Linear", anchor: .top, words: ["리니어"], source: .linear),
            .init(pane: .connections, section: connected, title: "Google Drive", anchor: .top, words: ["구글 드라이브", "드라이브"], source: .gdrive),
            .init(pane: .connections, section: connected, title: "Jira · Confluence", anchor: .top, words: ["지라", "컨플루언스", "Atlassian"], source: .atlassian),
            .init(pane: .connections, section: connected, title: "Asana", anchor: .top, words: ["아사나"], source: .asana),
            .init(pane: .connections, section: connected, title: "Dropbox", anchor: .top, words: ["드롭박스"], source: .dropbox),
            .init(pane: .connections, section: connected, title: "Obsidian", anchor: .top, words: ["옵시디언", "노트", "마크다운"], source: .notes),
            .init(pane: .connections, section: sync, title: String(localized: "지금 동기화"), anchor: .top, words: ["sync"]),
            .init(pane: .connections, section: sync, title: String(localized: "자동 동기화"), words: ["sync"]),
            .init(pane: .connections, section: links, title: String(localized: "검색 데이터 다시 만들기"), anchor: .footer, words: ["색인", "index"]),
            .init(pane: .connections, section: links, title: String(localized: "검색 데이터 지우기"), anchor: .footer, words: ["색인", "index", "삭제"]),

            .init(pane: .privacy, title: String(localized: "보내는 정보 보기"), words: ["보내는", "AI", "음성 인식", "통계", "analytics", "사용 통계"]),
            .init(pane: .privacy, section: share, title: String(localized: "화면 공유·녹화 표시"), words: ["화면 공유", "screen", "녹화"]),
            .init(pane: .privacy, section: delete, title: String(localized: "회의 기록 삭제"), words: ["삭제", "delete"]),
            .init(pane: .privacy, section: delete, title: String(localized: "기억한 참석자 삭제"), words: ["삭제", "delete", "화자"]),

            .init(pane: .account, title: String(localized: "이번 달 사용량"), anchor: .top, words: ["사용량", "usage", "요금"]),
        ]
        if BuildValues.updateFeed != nil {  // AppUpdate.offered (the updater itself is app-only)
            all.append(.init(pane: .general, section: update, title: String(localized: "자동 업데이트"), words: ["업데이트", "update", "버전", "version"]))
            all.append(.init(pane: .general, section: links, title: String(localized: "업데이트 확인"), anchor: .footer, words: ["update", "버전", "version"]))
        }
        if MeetingTranslation.offered || !Build.isOpen {
            all.append(.init(pane: .meeting, section: during, title: String(localized: "번역"), words: ["translation", "영어"]))
        }
        if MCPAccess.offered {
            all.append(.init(pane: .connections, title: "MCP", words: ["Claude", "Cursor", "Claude Code", "AI 앱", "AI app"]))
        }
        if CalendarStore.offered {
            all.append(.init(pane: .meeting, section: begin, title: String(localized: "캘린더"), words: ["calendar", "일정", "구글 캘린더", "Outlook"]))
        }
        if TacieAccount.shared.signedIn {
            all.append(.init(pane: .account, title: String(localized: "새 기능 소식 메일"), words: ["메일", "이메일", "email", "newsletter", "수신", "광고"]))
            all.append(.init(pane: .account, title: String(localized: "로그아웃"), words: ["sign out", "logout"]))
            all.append(.init(pane: .account, title: String(localized: "계정 삭제"), words: ["delete", "탈퇴"]))
        }
        if Build.isOpen { all += openAI }
        return shown(all, panes: SettingsPane.main, sources: ConnectionsScene.Source.available)
    }

    /// The open edition's 설정 → AI: its two cards and 직접 고르기 (the page has no rows, so they open the page's top).
    static var openAI: [SettingsSearchEntry] {
        [
            .init(pane: .ai, title: String(localized: "aiCard.key", defaultValue: "API 키"), anchor: .top,
                  words: ["OpenRouter", "키", "key", "API"]),
            .init(pane: .ai, title: String(localized: "온디바이스 처리"), anchor: .top,
                  words: ["로컬", "local", "Ollama", "LM Studio", "온디바이스", "on-device", "오프라인", "offline"]),
            .init(pane: .ai, title: String(localized: "업체 직접 선택"), anchor: .top,
                  words: ["음성 인식", "speech", "회의 정리", "답 카드", "Soniox", "ElevenLabs", "OpenAI", "TypeSafe", "업체", "provider"]),
        ]
    }

    /// The entries on a pane this build shows, without the connection cards it does not offer.
    static func shown(_ all: [SettingsSearchEntry], panes: [SettingsPane], sources: [ConnectionsScene.Source]) -> [SettingsSearchEntry] {
        all.filter { e in panes.contains(e.pane) && (e.source.map(sources.contains) ?? true) }
    }

    /// The entries whose title, pane or other words hold every word typed, title matches first.
    static func matches(_ query: String, in entries: [SettingsSearchEntry]) -> [SettingsSearchEntry] {
        let terms = query.lowercased().split(whereSeparator: \.isWhitespace).map(String.init)
        guard !terms.isEmpty else { return [] }
        func text(_ e: SettingsSearchEntry) -> String { ([e.title, e.pane.title, e.section ?? ""] + e.words).joined(separator: " ").lowercased() }
        let found = entries.filter { e in terms.allSatisfy { text(e).contains($0) } }
        let titled = found.filter { e in terms.allSatisfy { e.title.lowercased().contains($0) } }
        return titled + found.filter { !titled.contains($0) }
    }
}

/// The row a search picked: the page scrolls to it and `SettingsRow` marks it for a moment.
private struct SettingsTargetKey: EnvironmentKey { static let defaultValue: String? = nil }
extension EnvironmentValues {
    var settingsTarget: String? {
        get { self[SettingsTargetKey.self] }
        set { self[SettingsTargetKey.self] = newValue }
    }
}

/// The search field, the same look as the main window's 회의 검색 (⌘F focuses it).
struct SettingsSearchField: View {
    @Binding var query: String
    var focused: FocusState<Bool>.Binding
    let move: (Int) -> Void
    let submit: () -> Void

    var body: some View {
        HStack(spacing: 7) {
            Image(systemName: "magnifyingglass").uiFont(12, weight: .medium).foregroundStyle(Theme.ink3)
            TextField(String(localized: "설정 검색"), text: $query)
                .textFieldStyle(.plain)
                .uiFont(13)
                .focused(focused)
                .onSubmit(submit)
                .onKeyPress(.downArrow) { move(1); return .handled }
                .onKeyPress(.upArrow) { move(-1); return .handled }
            if query.isEmpty && !focused.wrappedValue {
                Text("⌘F").uiFont(11.5).foregroundStyle(Theme.ink4)
            } else if !query.isEmpty {
                Button { query = "" } label: {
                    Image(systemName: "xmark.circle.fill").uiFont(12).foregroundStyle(Theme.ink4)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(String(localized: "지우기"))
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 30)
        .background(Theme.ink.opacity(0.045), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .background {
            Button("") { focused.wrappedValue = true }.keyboardShortcut("f", modifiers: .command)
                .frame(width: 0, height: 0).opacity(0).accessibilityHidden(true)
        }
    }
}

/// What the search found, in place of the panes: the row's title and, smaller, the page it is on.
struct SettingsSearchResults: View {
    let results: [SettingsSearchEntry]
    let current: Int
    let pick: (SettingsSearchEntry) -> Void
    @State private var hover: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if results.isEmpty {
                Text("맞는 설정이 없어요.").uiFont(12.5).foregroundStyle(Theme.ink3)
                    .padding(.horizontal, 8).padding(.top, 6)
            }
            ForEach(Array(results.enumerated()), id: \.element.id) { i, e in
                Button { pick(e) } label: {
                    HStack(spacing: 8) {
                        Text(e.title).uiFont(13.5).foregroundStyle(Theme.ink).lineLimit(1)
                        Spacer(minLength: 4)
                        Text(e.section.map { "\(e.pane.title) › \($0)" } ?? e.pane.title).uiFont(11.5).foregroundStyle(Theme.ink3).lineLimit(1)
                    }
                    .padding(.horizontal, 8)
                    .frame(height: 30)
                    .background {
                        // the one Return opens; the pointer lightens the others
                        if i == current {
                            RoundedRectangle(cornerRadius: 7, style: .continuous).fill(SettingsStyle.selected)
                        } else if hover == e.id {
                            RoundedRectangle(cornerRadius: 7, style: .continuous).fill(Theme.ink.opacity(0.035))
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(PressStyle())
                .onHover { hover = $0 ? e.id : (hover == e.id ? nil : hover) }
            }
        }
    }
}

/// A light fill behind the row a search picked, gone after a moment (the row stays put).
struct SettingsTargetMark: ViewModifier {
    let title: String
    @Environment(\.settingsTarget) private var target
    @Environment(\.reducesMotion) private var reduceMotion
    @State private var on = false

    func body(content: Content) -> some View {
        content
            .background {
                RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.ink.opacity(0.055))
                    .padding(.horizontal, -12)
                    .opacity(on ? 1 : 0)
            }
            .onChange(of: target, initial: true) { _, t in
                guard t == title else { return }
                on = true
                Task {
                    try? await Task.sleep(for: .seconds(1.6))
                    withAnimation(Theme.anim(.easeOut(duration: 0.6), reduce: reduceMotion)) { on = false }
                }
            }
    }
}
