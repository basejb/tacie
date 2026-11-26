import XCTest
@testable import Tacie

final class SettingsTextTests: XCTestCase {
    private var cal: Calendar {
        var c = Calendar(identifier: .gregorian)
        c.timeZone = TimeZone(identifier: "Asia/Seoul")!
        return c
    }

    private func date(_ d: Int, _ h: Int, _ m: Int, month: Int = 10) -> Date {
        cal.date(from: DateComponents(year: 2026, month: month, day: d, hour: h, minute: m))!
    }

    func testIndexStateDecodesConnectionFields() throws {
        let json = """
        {"chunks":9,"commit":"abc","built_at":"2026-10-23T01:23:26+0900","refresh":{"state":"idle"},
         "files":2106,"github_items":893,"bytes":131072000,"repo":"acme/widgets","branch":"dev","symbols_missing":478,
         "github_login":"basejb","github_lost":true}
        """
        let s = try EngineClient.engineDecoder.decode(IndexState.self, from: Data(json.utf8))
        XCTAssertEqual(s.githubLost, true)
        XCTAssertEqual(s.files, 2106)
        XCTAssertEqual(s.githubItems, 893)
        XCTAssertEqual(s.bytes, 131_072_000)
        XCTAssertEqual(s.repo, "acme/widgets")
        XCTAssertEqual(s.branch, "dev")
        XCTAssertEqual(s.symbolsMissing, 478)
        XCTAssertEqual(s.githubLogin, "basejb")
    }

    func testIndexStateFromOlderEngine() throws {
        let json = #"{"chunks":9,"commit":"abc","built_at":"2026-10-23T01:23:26+0900","refresh":{"state":"idle"}}"#
        let s = try EngineClient.engineDecoder.decode(IndexState.self, from: Data(json.utf8))
        XCTAssertNil(s.files); XCTAssertNil(s.githubItems); XCTAssertNil(s.bytes); XCTAssertNil(s.repo)
        XCTAssertNil(s.branch); XCTAssertNil(s.symbolsMissing); XCTAssertNil(s.githubLogin); XCTAssertNil(s.githubLost)
    }

    func testSynced() {
        let now = date(23, 16, 0)
        XCTAssertEqual(SettingsText.synced(date(23, 1, 23), now: now, calendar: cal), "마지막 동기화 · 오늘 오전 1:23")
        XCTAssertEqual(SettingsText.synced(date(22, 16, 0), now: now, calendar: cal), "마지막 동기화 · 어제 오후 4:00")
        XCTAssertEqual(SettingsText.synced(date(21, 16, 0), now: now, calendar: cal), "마지막 동기화 · 10월 21일 오후 4:00")
    }

    func testSize() {
        XCTAssertEqual(SettingsText.size(131_072_000), "125MB")
        XCTAssertEqual(SettingsText.size(2_400_000_000), "2.2GB")
        XCTAssertEqual(SettingsText.size(1_000), "1MB")
    }

    func testHoldings() {
        XCTAssertEqual(SettingsText.holdings(files: 2106, items: 893), "파일 2,106개 · PR과 이슈 893개")
        XCTAssertEqual(SettingsText.holdings(files: 2106, items: nil), "파일 2,106개")
        XCTAssertEqual(SettingsText.holdings(files: nil, items: 893), "PR과 이슈 893개")
        XCTAssertNil(SettingsText.holdings(files: nil, items: nil))
    }

    func testRetentionTitles() {
        XCTAssertEqual(Retention.allCases.map(SettingsText.retention), ["30일 보관", "90일 보관", "1년 보관", "계속 보관"])
    }

    func testDuration() {
        XCTAssertEqual(SettingsText.duration(11_400), "3시간 10분")
        XCTAssertEqual(SettingsText.duration(7_200), "2시간")
        XCTAssertEqual(SettingsText.duration(2_700), "45분")
        XCTAssertEqual(SettingsText.duration(59), "0분")
    }

    func testEngineConfigDecodesMissingJev() throws {
        let json = """
        {"models":{"stt":"a","answer":"b","jev":null},"model_defaults":{"stt":"a","answer":"b","jev":null},
         "thresholds":{"gate":0.7,"show":0.8},"threshold_defaults":{"gate":0.7,"show":0.8},
         "keys":[{"name":"openrouter","path":"~/.config/openrouter/config.json","state":"missing"}],"log_text":false}
        """
        let c = try EngineClient.engineDecoder.decode(EngineConfig.self, from: Data(json.utf8))
        XCTAssertNil(c.models.jev)
        XCTAssertEqual(c.keys[0].state, "missing")
        XCTAssertEqual(c.thresholds.show, 0.8)
        XCTAssertFalse(c.logText)
    }

    func testSyncingText() {
        XCTAssertEqual(SettingsText.syncing(step: "embed", progress: (420, 1000)), "동기화하는 중 · 검색 준비 중 42%")
        XCTAssertEqual(SettingsText.syncing(step: "repo", progress: nil), "동기화하는 중 · GitHub 코드와 문서 받는 중")
    }

    func testDecodesGitHubAnswers() throws {
        let d = EngineClient.engineDecoder
        let repos = try d.decode(GitHubRepos.self, from: Data(#"""
        {"repos":[{"full_name":"a/b","private":true,"default_branch":"dev","pushed_at":null}],"needs_approval":["example-corp"],"needs_sso":[]}
        """#.utf8))
        XCTAssertEqual(repos.repos.first?.fullName, "a/b")
        XCTAssertEqual(repos.repos.first?.defaultBranch, "dev")
        XCTAssertEqual(repos.needsApproval, ["example-corp"])
        XCTAssertEqual(repos.needsSso, [])
        let r = try d.decode(IndexState.Refresh.self, from: Data(#"{"state":"running","step":"embed","progress":{"done":1,"total":4}}"#.utf8))
        XCTAssertEqual(r.progress?.done, 1)
        XCTAssertEqual(r.progress?.total, 4)
        let s = try d.decode(IndexState.self, from: Data(#"{"chunks":0,"commit":null,"built_at":null,"refresh":{"state":"idle"},"connected":false,"cleared":true}"#.utf8))
        XCTAssertEqual(s.connected, false)
        XCTAssertEqual(s.cleared, true)
    }

    func testEngineErrorCarriesKind() {
        XCTAssertEqual(EngineError.http(502, #"{"error":"x","error_kind":"key"}"#).kind, "key")
        XCTAssertNil(EngineError.http(502, "Bad Gateway").kind)
        XCTAssertNil(EngineError.notRunning.kind)
    }

    func testFilterRepos() {
        func repo(_ n: String) -> GitHubRepo { GitHubRepo(fullName: n, private: false, defaultBranch: "main", pushedAt: nil) }
        let all = ["example-corp/Payments", "basejb/tacie", "basejb/dotfiles"].map(repo)
        XCTAssertEqual(SettingsText.filter(all, query: "pay").map(\.fullName), ["example-corp/Payments"])
        XCTAssertEqual(SettingsText.filter(all, query: "BASEJB/").map(\.fullName), ["basejb/tacie", "basejb/dotfiles"])
        XCTAssertEqual(SettingsText.filter(all, query: "").map(\.fullName), all.map(\.fullName))
    }

    func testGitHubProblem() {
        XCTAssertEqual(SettingsText.githubProblem("key"), "GitHub 연결을 다시 해 주세요.")
        XCTAssertEqual(SettingsText.githubProblem("network"), "인터넷 연결을 확인해 주세요.")
        XCTAssertEqual(SettingsText.githubProblem("busy"), "GitHub가 잠시 바빠요. 조금 뒤에 다시 해 주세요.")
        for kind in [nil, "down", "credit"] {  // not necessarily GitHub's fault
            XCTAssertEqual(SettingsText.githubProblem(kind), "동기화하지 못했어요. 잠시 뒤에 다시 해 주세요.")
        }
    }

    func testMenuSyncNamesLostConnections() {
        let cal = Calendar(identifier: .gregorian), now = Date()
        XCTAssertEqual(SettingsText.menuSync(fraction: nil, syncing: false, failed: true, lost: ["GitHub"], builtAt: now, now: now,
                                             calendar: cal), "지금 동기화 · GitHub 연결이 끊겼어요")
        // a lost Notion or Linear token does not fail the sync, but still needs 다시 연결
        XCTAssertEqual(SettingsText.menuSync(fraction: nil, syncing: false, failed: false, lost: ["노션", "Linear"], builtAt: now,
                                             now: now, calendar: cal), "지금 동기화 · 노션, Linear 연결이 끊겼어요")
        XCTAssertEqual(SettingsText.menuSync(fraction: nil, syncing: false, failed: true, lost: [], builtAt: now, now: now,
                                             calendar: cal), "지금 동기화 · 지난번에 실패했어요")
        XCTAssertEqual(SettingsText.menuSync(fraction: 0.4, syncing: true, failed: true, lost: ["GitHub"], builtAt: now, now: now,
                                             calendar: cal), "동기화 중 · 40%")
    }

    func testSyncWarning() {
        let lost = "GitHub 저장소를 볼 수 없어 PR·이슈 없이 찾아요"
        let cases: [([String], String?)] = [
            (["github"], "GitHub는 지난 데이터로 찾아요"),
            (["slack"], "슬랙은 지난 데이터로 찾아요"),
            (["repo"], "코드는 지난 데이터로 찾아요"),
            (["github", "slack"], "GitHub·슬랙은 지난 데이터로 찾아요"),
            (["slack", "repo", "github"], "코드·GitHub·슬랙은 지난 데이터로 찾아요"),  // a fixed order; 은/는 from the last
            (["repo", "github"], "코드·GitHub는 지난 데이터로 찾아요"),
            (["github-lost"], lost),
            (["github", "github-lost", "slack"], "\(lost) · 슬랙은 지난 데이터로 찾아요"),  // GitHub is not also "kept"
            ([], nil),
            (["other"], nil),
        ]
        for (warnings, want) in cases {
            XCTAssertEqual(SettingsText.syncWarning(warnings), want, "\(warnings)")
        }
        // the id-change case is fixed only by rebuilding: the button on the connection page says "다시 만들기"
        XCTAssertEqual(SettingsText.githubLost,
                       "GitHub에서 이 저장소를 볼 수 없어서 PR과 이슈를 검색에서 뺐어요. 권한을 확인한 뒤 다시 동기화하고, 다른 저장소로 바뀌었다면 '다시 만들기'를 눌러 주세요.")
        XCTAssertEqual(SettingsText.switching(to: "basejb/tacie"), "저장소를 바꾸는 중 · basejb/tacie")
        for text in cases.compactMap(\.1) + [SettingsText.githubLost, SettingsText.syncNotStarted, SettingsText.orgHint,
                                              SettingsText.switching(to: "o/n")] {
            for word in ["자료", "색인", "조각", "판단", "발화", "통합"] {
                XCTAssertFalse(text.contains(word), "\(word) in \(text)")
            }
        }
    }

    func testCredentialsResolve() {
        XCTAssertTrue(GitHubCredentials.resolve(tacie: false, token: "gho_t", disconnected: false) == ("gh", nil))
        XCTAssertTrue(GitHubCredentials.resolve(tacie: true, token: "gho_t", disconnected: false) == ("token", "gho_t"))
        XCTAssertTrue(GitHubCredentials.resolve(tacie: true, token: nil, disconnected: false) == ("none", nil))
        XCTAssertTrue(GitHubCredentials.resolve(tacie: true, token: "", disconnected: false) == ("none", nil))
        // 연결 해제 whose keychain delete failed: the token is still there, but never handed over
        XCTAssertTrue(GitHubCredentials.resolve(tacie: true, token: "gho_t", disconnected: true) == ("none", nil))
    }

    func testRepoPickerButtonSaysWhatItChanges() {
        // the repo and branch in use: nothing to press
        XCTAssertNil(SettingsText.repoAction(current: true, hasRepo: true, sameBranch: true))
        XCTAssertEqual(SettingsText.repoAction(current: true, hasRepo: true, sameBranch: false), "이 브랜치로 바꾸기")
        XCTAssertEqual(SettingsText.repoAction(current: false, hasRepo: true, sameBranch: false), "이 저장소로 바꾸기")
        XCTAssertEqual(SettingsText.repoAction(current: false, hasRepo: false, sameBranch: false), "이 저장소 연결")
    }

    func testUntilReset() {
        XCTAssertEqual(SettingsText.untilReset(12 * 86_400 - 60), "12일")
        XCTAssertEqual(SettingsText.untilReset(30 * 3_600), "2일")
        XCTAssertEqual(SettingsText.untilReset(5 * 3_600 + 60), "6시간")
        XCTAssertEqual(SettingsText.untilReset(10), "1시간")
    }
}
