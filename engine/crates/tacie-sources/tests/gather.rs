//! Gathering sources: saved-copy rules, and GitHub closing after repeated 404s or a changed repo id.
//!
//! To empty the repo chunks pass `code: Some(vec![])`; GitHub is answered by `FakeGitHub` routes (repo check
//! `repos/o/r`, the three lists, GraphQL). Slack is `FakeSlack`.

mod common {
    pub mod github;
}

use std::fs;
use std::path::{Path, PathBuf};

use common::github::{connect, gh_call, graphql_answers};
use serde_json::{Map, Value, json};
use tacie_index::RawChunk;
use tacie_sources::error::{Kind, SourceError};
use tacie_sources::gather::{Gather, gather};
use tacie_sources::github::GitHub;
use tacie_sources::github::content::{Entry, Snapshot, Tree, blob_sha};
use tacie_sources::github::docs::chunks as github_chunks;
use tacie_sources::github::items::{blank_state, build_items, load_state, save_item, save_state};
use tacie_sources::github::now_iso;
use tacie_sources::repo;
use tacie_sources::slack::SlackFetch;
use tacie_sources::testing::FakeSlack;
use tacie_sources::testing::github::{FakeGitHub, Reply};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
/// o/r의 세 목록이에요.
const LISTS: [&str; 3] = [
    "repos/o/r/issues?state=all",
    "repos/o/r/issues/comments",
    "repos/o/r/pulls/comments",
];
const SAVED_AT: &str = "2026-10-24T00:00:00Z";

/// GitHub 원본을 fetch처럼 항목마다 파일 하나로 둬요.
fn save_github_raw(raw: &Path) {
    let raw_lists = json!({
        "issues": [
            {"number": 655, "title": "알림 묶음 마지막",
             "body": "## 무엇\n<!-- 숨김 -->\n알림 누계를 보여 줘요.\n\n🤖 Generated with [Claude Code](x)",
             "user": {"login": "basejb"}, "state": "closed", "created_at": "2026-10-15T01:00:00Z",
             "html_url": "https://github.com/o/r/pull/655", "pull_request": {"merged_at": "2026-10-15T02:00:00Z"},
             "labels": []},
            {"number": 12, "title": "봇 이슈", "body": "x", "user": {"login": "dependabot[bot]", "type": "Bot"},
             "state": "open", "created_at": "2026-10-01T00:00:00Z", "html_url": "https://github.com/o/r/issues/12",
             "labels": []},
        ],
        "comments": [
            {"id": 1, "issue_url": "https://api.github.com/repos/o/r/issues/655", "user": {"login": "mina-k"},
             "created_at": "2026-10-15T03:00:00Z", "body": "누계는 월별로 끊어요?",
             "html_url": "https://github.com/o/r/pull/655#issuecomment-1"},
            {"id": 2, "issue_url": "https://api.github.com/repos/o/r/issues/655",
             "user": {"login": "ci[bot]", "type": "Bot"}, "created_at": "2026-10-15T04:00:00Z",
             "body": "coverage 80%", "html_url": "https://github.com/o/r/pull/655#issuecomment-2"},
        ],
        "review_comments": [],
        "pulls": [{"number": 655, "headRefName": "feat/x", "baseRefName": "dev",
                   "mergedAt": "2026-10-15T02:00:00Z",
                   "files": {"totalCount": 2, "nodes": [{"path": "apps/admin/a.ts"},
                                                        {"path": "apps/admin/__tests__/a.test.ts"}]},
                   "reviews": {"nodes": []}}],
    });
    let list = |k: &str| raw_lists[k].as_array().unwrap().clone();
    let old = list("pulls")
        .into_iter()
        .map(|p| (p["number"].as_u64().unwrap(), json!({"extras": p})))
        .collect();
    for (_, it) in build_items(
        &list("issues"),
        &list("comments"),
        &list("review_comments"),
        &old,
    ) {
        save_item(raw, &it).unwrap();
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        fs::copy(e.path(), to.join(e.file_name())).unwrap();
    }
}

fn paths(chunks: &[RawChunk]) -> Vec<&str> {
    chunks.iter().map(|c| c["path"].as_str().unwrap()).collect()
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

/// 무엇을 물으면 오프라인이에요(답 없이 끊겨요).
fn offline() -> Reply {
    Reply::Close
}

fn gone() -> Reply {
    Reply::Http {
        code: 404,
        headers: vec![(
            "Content-Type".into(),
            "application/json; charset=utf-8".into(),
        )],
        body: json!({"message": "Not Found", "status": "404"})
            .to_string()
            .into_bytes(),
    }
}

fn here(id: u64) -> Reply {
    Reply::Http {
        code: 200,
        headers: vec![
            ("Etag".into(), "W/\"a1\"".into()),
            ("Content-Type".into(), "application/json".into()),
        ],
        body: json!({"id": id, "full_name": "o/r"})
            .to_string()
            .into_bytes(),
    }
}

/// o/r의 GitHub 저장본(PR #655), 방금 동기화·재조정했고 목록은 새것이 없어요.
struct Closable {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    raw: PathBuf,
    fake: std::sync::Arc<FakeGitHub>,
    gh: GitHub,
    slack: std::sync::Arc<FakeSlack>,
}

impl Closable {
    fn new() -> Closable {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let raw = root.join("github");
        save_github_raw(&raw);
        let mut s = blank_state();
        s.insert("repo".into(), json!("o/r"));
        s.insert("fetched_at".into(), json!(SAVED_AT));
        s.insert("repo_id".into(), json!(1));
        s.insert("full_name".into(), json!("o/r"));
        s.insert(
            "cursors".into(),
            json!({"issues": SAVED_AT, "comments": SAVED_AT, "review_comments": SAVED_AT}),
        );
        s.insert("reconciled_at".into(), json!(now_iso()));
        save_state(&raw, &s).unwrap();
        let fake = FakeGitHub::start();
        for path in LISTS {
            fake.route(path, Reply::Json(json!([])));
        }
        graphql_answers(&fake, |_| None); // PR 추가 정보는 비어 와요: 저장된 것 그대로예요
        let gh = connect(&fake);
        Closable {
            _tmp: tmp,
            root,
            raw,
            fake,
            gh,
            slack: FakeSlack::fails("slack is not asked here"),
        }
    }

    /// 목록을 물은 길이에요.
    fn listed(&self) -> Vec<String> {
        self.fake
            .calls()
            .iter()
            .filter_map(gh_call)
            .map(|(p, _)| p)
            .filter(|p| LISTS.contains(&p.as_str()))
            .collect()
    }

    /// 모으기를 한 번 해요: (조각 path, 경고, info["github"])예요.
    fn sync(&self, answer: Reply, fetch: bool) -> (Vec<String>, Vec<String>, Value) {
        self.fake.status_seq("repos/o/r", vec![answer]);
        let mut warnings = Vec::new();
        let (chunks, info) = gather(
            &self.gh,
            &*self.slack,
            Gather {
                snapshot: None,
                graph: None,
                skip: &[],
                github_repo: Some("o/r"),
                github_raw: &self.raw,
                slack_raw: None,
                fetch,
                code: Some(vec![]),
                cached: None,
            },
            &mut warnings,
        )
        .unwrap();
        let paths = paths(&chunks).into_iter().map(str::to_string).collect();
        (paths, warnings, info["github"].clone())
    }

    fn lost(&self) -> Value {
        load_state(&self.raw)["lost"].clone()
    }
}

fn saved_info(chunks: usize) -> Value {
    json!({"repo": "o/r", "fetched_at": SAVED_AT, "chunks": chunks})
}

#[test]
fn gather_saved_copy_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    fake.status_seq("repos/o/n", vec![offline()]);
    fake.status_seq("repos/o/r", vec![offline()]);
    let slack = FakeSlack::fails("slack is not asked here");
    let v1 = tmp.path().join("v1");
    copy_dir(&Path::new(FIXTURES).join("github_v1"), &v1);
    let run = |repo: &str, raw: &Path, warnings: &mut Vec<String>| {
        gather(
            &gh,
            &*slack,
            Gather {
                snapshot: None,
                graph: None,
                skip: &[],
                github_repo: Some(repo),
                github_raw: raw,
                slack_raw: None,
                fetch: true,
                code: Some(vec![]),
                cached: None,
            },
            warnings,
        )
    };
    // 옛 모양은 저장본이 아니에요: 받기가 실패하면 그대로 실패해요
    let err = run("o/n", &v1, &mut Vec::new()).unwrap_err();
    assert_eq!(err.kind(), Kind::Network);
    let v2 = tmp.path().join("v2");
    save_github_raw(&v2);
    let mut s = blank_state();
    s.insert("repo".into(), json!("o/r"));
    s.insert("fetched_at".into(), json!(SAVED_AT));
    save_state(&v2, &s).unwrap();
    let mut warnings = Vec::new();
    let (chunks, info) = run("o/r", &v2, &mut warnings).unwrap();
    assert_eq!(warnings, ["github"]);
    assert_eq!(paths(&chunks), ["github/pull/655"]);
    assert_eq!(chunks, github_chunks(&v2).unwrap());
    assert_eq!(info["github"], saved_info(1));
    assert_eq!(keys(&info["github"]), ["repo", "fetched_at", "chunks"]);
    assert_eq!(keys(&Value::Object(info)), ["github"]);
    assert_eq!(slack.calls(), 0);
}

#[test]
fn repo_lost_twice_closes_github() {
    let c = Closable::new();
    // 404 한 번: 저장본이에요
    assert_eq!(
        c.sync(gone(), true),
        (
            vec!["github/pull/655".into()],
            vec!["github".into()],
            saved_info(1)
        )
    );
    assert_eq!(c.lost(), 1);
    assert!(c.listed().is_empty()); // 저장소가 없는 동안 아무것도 나열하지 않아요
    // GitHub의 답이 없는 것은 404가 아니에요
    assert_eq!(
        c.sync(offline(), true),
        (
            vec!["github/pull/655".into()],
            vec!["github".into()],
            saved_info(1)
        )
    );
    assert_eq!(c.lost(), 1);
    // 두 번째 404: 닫아요
    let (paths, warnings, info) = c.sync(gone(), true);
    assert!(paths.is_empty());
    assert_eq!(warnings, ["github-lost"]);
    assert_eq!(
        info,
        json!({"repo": "o/r", "fetched_at": SAVED_AT, "chunks": 0, "lost": true})
    );
    assert_eq!(keys(&info), ["repo", "fetched_at", "chunks", "lost"]);
    // 받지 않아도 닫혀 있어요
    let before = c.fake.calls().len();
    let (paths, warnings, _) = c.sync(here(1), false);
    assert!(paths.is_empty());
    assert_eq!(warnings, ["github-lost"]);
    assert_eq!(c.fake.calls().len(), before); // 받지 않았어요
    // 다시 200: 열어요
    let (paths, warnings, info) = c.sync(here(1), true);
    assert_eq!(paths, ["github/pull/655"]);
    assert!(warnings.is_empty());
    assert_eq!(keys(&info), ["repo", "fetched_at", "chunks"]); // lost가 없어요
    assert_eq!((&info["repo"], &info["chunks"]), (&json!("o/r"), &json!(1)));
    assert!(!c.listed().is_empty());
    assert_eq!(c.lost(), 0);
    // 404 한 번 뒤의 200은 닫지 않아요
    let (paths, warnings, _) = c.sync(gone(), true);
    assert_eq!(
        (paths, warnings),
        (
            vec!["github/pull/655".to_string()],
            vec!["github".to_string()]
        )
    );
    let (paths, warnings, _) = c.sync(here(1), true);
    assert_eq!(
        (paths, warnings),
        (vec!["github/pull/655".to_string()], Vec::<String>::new())
    );
    assert_eq!(c.slack.calls(), 0);
}

#[test]
fn repo_id_change_closes_github() {
    let c = Closable::new();
    // 지워졌다가 다시 만들어졌거나, 다른 저장소가 그 이름을 가져갔어요
    let (paths, warnings, info) = c.sync(here(2), true);
    assert!(paths.is_empty());
    assert_eq!(warnings, ["github-lost"]);
    assert_eq!(info["lost"], true);
    assert!(c.listed().is_empty());
    let st = load_state(&c.raw);
    assert_eq!((&st["lost"], &st["repo_id"]), (&json!(2), &json!(1))); // 지킨 id는 처음 것이에요
    let (paths, warnings, _) = c.sync(here(2), true);
    assert!(paths.is_empty());
    assert_eq!(warnings, ["github-lost"]);
    let (paths, warnings, _) = c.sync(here(1), true);
    assert_eq!(paths, ["github/pull/655"]);
    assert!(warnings.is_empty());
    assert_eq!(c.lost(), 0);
}

#[test]
fn an_invisible_repo_with_nothing_saved_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    fake.status_seq("repos/o/r", vec![gone()]);
    let slack = FakeSlack::fails("slack is not asked here");
    let raw = tmp.path().join("github");
    let err = gather(
        &gh,
        &*slack,
        Gather {
            snapshot: None,
            graph: None,
            skip: &[],
            github_repo: Some("o/r"),
            github_raw: &raw,
            slack_raw: None,
            fetch: false, // 저장본이 없으면 fetch가 거짓이어도 받아요
            code: Some(vec![]),
            cached: None,
        },
        &mut Vec::new(),
    )
    .unwrap_err();
    match err {
        SourceError::Service { kind, message } => {
            assert_eq!(kind, Kind::Key);
            assert_eq!(message, "GitHub repo not visible");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(load_state(&raw)["lost"], 1);
}

#[test]
fn a_lost_count_python_cannot_compare_fails() {
    let c = Closable::new();
    let mut s = load_state(&c.raw);
    s.insert("lost".into(), Value::Null); // null은 `LOST_AT`과 비교할 수 없어요
    save_state(&c.raw, &s).unwrap();
    let mut warnings = Vec::new();
    let err = gather(
        &c.gh,
        &*c.slack,
        Gather {
            snapshot: None,
            graph: None,
            skip: &[],
            github_repo: Some("o/r"),
            github_raw: &c.raw,
            slack_raw: None,
            fetch: false,
            code: Some(vec![]),
            cached: None,
        },
        &mut warnings,
    )
    .unwrap_err();
    assert!(matches!(err, SourceError::Json(_)), "{err:?}");
}

/// 슬랙 원본에 `failed`와 `fetched_at`을 더한 것이에요(fetch가 쓰는 모양).
fn slack_raw() -> Value {
    json!({
        "team_url": "https://acme.slack.com/",
        "users": {"U1": "민수", "U2": "하은"},
        "channels": [
            {"id": "C1", "name": "dev-help", "is_private": false, "members": null, "replies": {},
             "messages": [{"ts": "1729503000.000400", "user": "U2", "text": "배포 끝났어요"}]},
            {"id": "G1", "name": "leads", "is_private": true, "members": ["U1"], "replies": {},
             "messages": [{"ts": "1729510000.000100", "user": "U1", "text": "채용 얘기"}]},
        ],
        "failed": [],
        "fetched_at": "2026-10-25T00:00:00Z",
    })
}

#[test]
fn gather_slack_saved_copy_rules() {
    let c = Closable::new();
    let slack_dir = c.root.join("slack");
    let run = |slack: &dyn SlackFetch, fetch: bool, warnings: &mut Vec<String>| {
        c.fake.status_seq("repos/o/r", vec![here(1)]);
        gather(
            &c.gh,
            slack,
            Gather {
                snapshot: None,
                graph: None,
                skip: &[],
                github_repo: Some("o/r"),
                github_raw: &c.raw,
                slack_raw: Some(&slack_dir),
                fetch,
                code: Some(vec![]),
                cached: None,
            },
            warnings,
        )
    };
    // 저장본이 없으면 슬랙 실패가 그대로 실패예요(fetch가 거짓이어도 받아요)
    let failing = FakeSlack::fails("slack: every channel failed (#dev-help)");
    let err = run(&*failing, false, &mut Vec::new()).unwrap_err();
    assert_eq!(err.to_string(), "slack: every channel failed (#dev-help)");
    assert_eq!(failing.calls(), 1);
    // 받으면 GitHub 뒤에 슬랙 조각이 와요
    let writing = FakeSlack::writes(slack_raw());
    let mut warnings = Vec::new();
    let (chunks, info) = run(&*writing, true, &mut warnings).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(
        paths(&chunks),
        [
            "github/pull/655",
            "slack/C1/1729503000.000400",
            "slack/G1/1729510000.000100"
        ]
    );
    assert_eq!(keys(&Value::Object(info.clone())), ["github", "slack"]);
    assert_eq!(
        info["slack"],
        json!({"team": "https://acme.slack.com/", "fetched_at": "2026-10-25T00:00:00Z", "chunks": 2,
               "channels": ["#dev-help", "🔒leads"]})
    );
    assert_eq!(
        keys(&info["slack"]),
        ["team", "fetched_at", "chunks", "channels"]
    );
    // 받지 않으면 저장본 그대로예요
    let (again, _) = run(&*writing, false, &mut Vec::new()).unwrap();
    assert_eq!(writing.calls(), 1);
    assert_eq!(again, chunks);
    // 저장본이 있으면 실패해도 경고와 함께 저장본이에요
    let mut warnings = Vec::new();
    let (kept, _) = run(&*failing, true, &mut warnings).unwrap();
    assert_eq!(warnings, ["slack"]);
    assert_eq!(kept, chunks);
    assert_eq!(failing.calls(), 2);
}

/// 파일(경로, 바이트)로 만든 커밋 하나의 스냅숏이에요(트리는 경로 바이트 순서).
fn snapshot_of(commit: &str, files: &[(&str, &[u8])]) -> Snapshot {
    let mut entries: Vec<Entry> = files
        .iter()
        .map(|(p, b)| Entry {
            path: p.to_string(),
            mode: "100644".to_string(),
            sha: blob_sha(b),
        })
        .collect();
    entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    Snapshot {
        tree: Tree {
            commit: commit.to_string(),
            entries,
        },
        contents: files
            .iter()
            .map(|(p, b)| (p.to_string(), b.to_vec()))
            .collect(),
        graph: None,
    }
}

#[test]
fn repo_chunks_come_first_and_use_the_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let files: [(&str, &[u8]); 2] = [
        ("app.py", b"def main():\n    return 1\n"),
        ("README.md", "# 앱\n\n설명이에요.\n".as_bytes()),
    ];
    let snap = snapshot_of(&"c".repeat(40), &files);
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let slack = FakeSlack::fails("slack is not asked here");
    let cache = tmp.path().join("cache/chunks/o__r.json");
    let mut warnings = Vec::new();
    let (chunks, info) = gather(
        &gh,
        &*slack,
        Gather {
            snapshot: Some(&snap),
            graph: None,
            skip: &[],
            github_repo: None,
            github_raw: &tmp.path().join("github"),
            slack_raw: None,
            fetch: true,
            code: None,
            cached: Some(&cache),
        },
        &mut warnings,
    )
    .unwrap();
    let want = repo::chunks(&snap, None, &[], None).unwrap();
    assert_eq!(chunks, want);
    assert_eq!(paths(&chunks), ["app.py", "README.md"]);
    assert!(info.is_empty() && warnings.is_empty());
    assert!(cache.exists()); // 조각 캐시를 썼어요
    let cached: Map<String, Value> =
        serde_json::from_str(&fs::read_to_string(&cache).unwrap()).unwrap();
    assert_eq!(cached.len(), 2);
    // 커밋이 비면 캐시 없이 잘라요
    let other = tmp.path().join("cache/chunks/other.json");
    let no_commit = snapshot_of("", &files);
    let (again, _) = gather(
        &gh,
        &*slack,
        Gather {
            snapshot: Some(&no_commit),
            graph: None,
            skip: &[],
            github_repo: Some(""),
            github_raw: &tmp.path().join("github"),
            slack_raw: None,
            fetch: true,
            code: None,
            cached: Some(&other),
        },
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(again, want);
    assert!(!other.exists());
    // 아는 조각(`code`)이 있으면 스냅숏보다 먼저예요
    let (known, _) = gather(
        &gh,
        &*slack,
        Gather {
            snapshot: Some(&snap),
            graph: None,
            skip: &[],
            github_repo: None,
            github_raw: &tmp.path().join("github"),
            slack_raw: None,
            fetch: true,
            code: Some(want[..1].to_vec()),
            cached: Some(&other),
        },
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(known, want[..1]);
    assert!(fake.calls().is_empty()); // 빈 저장소 이름은 GitHub을 묻지 않아요
}
