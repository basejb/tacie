//! GitHub test helpers. Shared by reconcile and sync tests. Users pull it in with `mod common { pub mod github; }`.
//!
//! GitHub is `FakeGitHub` (a loopback fake server): list routes use `route` (the server splits lists by `per_page`),
//! repo state uses `status`. The connection is token mode with the fake's token (the gh token panics when called), and
//! the wait before retrying is removed (`connect`).
//!
//! The call records (`gh_paths`, `listed`, `status_asked`) return HTTP requests in `(path, since)` form: next-page
//! requests (`repositories/…`) are dropped, and `per_page`/`since` are taken off the query to split into
//! `(path, since)`. Over HTTP, `status` and `json` requests look the same, so repo checks (`repos/o/n`) and change
//! checks (`sort=updated`) are treated as `status` and dropped from `gh_paths`. A single issue (`repos/o/n/issues/{n}`)
//! can be either, so it goes into both `gh_paths` and `status_asked` (reconcile's disappearance check and the document
//! refetch's issue query never appear together in one test).

#![allow(dead_code)] // 테스트 파일마다 쓰는 도우미가 달라요

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde_json::{Map, Value, json};
use tacie_sources::github::items::{blank_state, load_state, save_item, save_state};
use tacie_sources::github::{GitHub, now_iso};
use tacie_sources::testing::github::{Call, FakeGitHub, PanicGhToken, Reply};

/// 세 목록의 길이에요(STREAMS 순서: issues, comments, review_comments).
pub const LISTS: [&str; 3] = [
    "repos/o/n/issues?state=all",
    "repos/o/n/issues/comments",
    "repos/o/n/pulls/comments",
];

/// 가짜의 토큰을 가진 token 모드 연결이에요. gh는 묻지 않고(부르면 패닉), 다시 묻기 전에 쉬지 않아요.
pub fn connect(fake: &FakeGitHub) -> GitHub {
    let gh = GitHub::new("none", &fake.url(), Arc::new(PanicGhToken))
        .unwrap()
        .with_pause(|_| {});
    gh.set_credentials("token", Some(FakeGitHub::TOKEN));
    gh
}

/// 요청 하나를 `(path, since)`로 돌려요: GraphQL은 `("graphql", None)`, 다음 쪽 요청과
/// login(`user`)은 None이에요.
pub fn gh_call(c: &Call) -> Option<(String, Option<String>)> {
    if c.method == "POST" {
        return (c.path == "graphql").then(|| ("graphql".to_string(), None));
    }
    if c.path.starts_with("repositories/") || c.path == "user" {
        return None;
    }
    let mut kept = Vec::new();
    let mut since = None;
    for (k, v) in form_urlencoded::parse(c.query.as_bytes()) {
        match k.as_ref() {
            "per_page" => {}
            "since" => since = Some(v.into_owned()),
            _ => kept.push(format!("{k}={v}")),
        }
    }
    let path = if kept.is_empty() {
        c.path.clone()
    } else {
        format!("{}?{}", c.path, kept.join("&"))
    };
    Some((path, since))
}

static REPO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^repos/[^/?]+/[^/?]+$").unwrap());
static ISSUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^repos/[^/?]+/[^/?]+/issues/\d+$").unwrap());

/// `status`만 묻는 길이에요: 저장소 확인과 바뀜 확인.
fn status_only(path: &str) -> bool {
    REPO.is_match(path) || path.contains("sort=updated")
}

/// 만든 저장소 o/n의 REST 이슈예요(`pull`이면 PR). `comments`는 GitHub이 센 댓글 수예요.
pub fn issue(n: u64, updated: &str, comments: u64, pull: bool) -> Value {
    let mut it = json!({"id": 1000 + n, "number": n, "title": format!("제목 {n}"), "body": "설명",
                        "user": {"login": "sora-p"}, "state": "open", "labels": [],
                        "created_at": "2026-10-01T00:00:00Z", "updated_at": updated, "comments": comments,
                        "html_url": format!("https://github.com/o/n/{}/{n}", if pull { "pull" } else { "issues" })});
    if pull {
        it["pull_request"] = json!({"merged_at": null});
    }
    it
}

/// #n의 댓글, 또는 PR #n의 인라인 리뷰 댓글(`review`)이에요.
pub fn comment(i: u64, n: u64, updated: &str, review: bool) -> Value {
    let mut c = json!({"id": i, "user": {"login": "jun-h"}, "created_at": updated, "updated_at": updated,
                       "body": format!("댓글 {i}"), "html_url": format!("https://github.com/o/n/pull/{n}#comment-{i}")});
    if review {
        c["pull_request_url"] = json!(format!("https://api.github.com/repos/o/n/pulls/{n}"));
        c["path"] = json!("a.ts");
        c["line"] = json!(1);
    } else {
        c["issue_url"] = json!(format!("https://api.github.com/repos/o/n/issues/{n}"));
    }
    c
}

/// 키를 바꾼 사본이에요: 있는 키는 제자리에서 바꾸고 없는 키는 뒤에 붙여요.
pub fn with(v: &Value, pairs: &[(&str, Value)]) -> Value {
    let mut v = v.clone();
    for (k, x) in pairs {
        v[*k] = x.clone();
    }
    v
}

/// 저장된 항목 하나예요.
pub fn stored(
    raw: &Path,
    issue: &Value,
    comments: &[Value],
    review_comments: &[Value],
    extras: Value,
) {
    let kind = if issue.get("pull_request").is_some() {
        "pull"
    } else {
        "issue"
    };
    let item = json!({"number": issue["number"], "kind": kind, "updated_at": issue["updated_at"], "issue": issue,
                      "comments": comments, "review_comments": review_comments, "extras": extras});
    save_item(raw, &item).unwrap();
}

/// 이 번호들의 저장된 o/n 이슈예요(#3은 PR). 전체 목록이 주는 모양 그대로 돌려줘요.
pub fn numbers(raw: &Path, numbers: impl IntoIterator<Item = u64>) -> Vec<Value> {
    let out: Vec<Value> = numbers
        .into_iter()
        .map(|n| issue(n, "2026-10-10T00:00:00Z", 0, n == 3))
        .collect();
    for it in &out {
        stored(raw, it, &[], &[], Value::Null);
    }
    out
}

/// 번호 → 이슈예요.
pub fn by_number(issues: &[Value]) -> BTreeMap<u64, Value> {
    issues
        .iter()
        .map(|it| (it["number"].as_u64().unwrap(), it.clone()))
        .collect()
}

/// 저장소 확인이 o/n(id 1)을 찾은 뒤의 상태예요.
pub fn known_state() -> Map<String, Value> {
    let mut s = blank_state();
    s.insert("repo".into(), json!("o/n"));
    s.insert("fetched_at".into(), json!("2026-10-23T00:00:00Z"));
    s.insert("repo_id".into(), json!(1));
    s.insert("full_name".into(), json!("o/n"));
    s
}

/// GitHub이 이슈의 `repository_url`로 주는 주소예요.
pub fn home(repo: &str) -> String {
    format!("https://api.github.com/repos/{repo}")
}

/// 세 목록의 답이에요.
pub fn lists(
    fake: &FakeGitHub,
    issues: Vec<Value>,
    comments: Vec<Value>,
    review_comments: Vec<Value>,
) {
    for (path, items) in LISTS.iter().zip([issues, comments, review_comments]) {
        fake.route(path, Reply::Json(Value::Array(items)));
    }
}

/// `json`·`rest`·`graphql`로 부른 길이에요(차례대로).
pub fn gh_paths(fake: &FakeGitHub) -> Vec<String> {
    fake.calls()
        .iter()
        .filter_map(gh_call)
        .map(|(path, _)| path)
        .filter(|p| !status_only(p))
        .collect()
}

static ALONE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^repos/o/n/(issues|pulls)/\d+").unwrap());

/// 문서 하나를 따로 물은 REST 호출이에요: 그 이슈, 댓글, 리뷰 댓글(저장소 전체 목록도, PR 추가 정보도 아님).
/// 이슈 하나만 묻는 것(`repos/o/n/issues/{n}`)은 재조정의 사라짐 확인(`status_asked`)으로 보고 빼요.
pub fn asked_alone(fake: &FakeGitHub) -> Vec<String> {
    gh_paths(fake)
        .into_iter()
        .filter(|p| ALONE.is_match(p) && !ISSUE.is_match(p))
        .collect()
}

/// `status`로 물은 길이에요: 저장소 확인과 이슈 하나의 사라짐 확인이에요.
pub fn status_asked(fake: &FakeGitHub) -> Vec<String> {
    fake.calls()
        .iter()
        .filter_map(gh_call)
        .map(|(path, _)| path)
        .filter(|p| REPO.is_match(p) || ISSUE.is_match(p))
        .collect()
}

pub fn mtime(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

/// 수정 시각을 옛날로 돌려요: 다시 쓰면 바뀌어요.
pub fn age(path: &Path) -> SystemTime {
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(old)
        .unwrap();
    old
}

/// 폴더의 이름 → 수정 시각이에요. 폴더가 없으면 None이에요. 모든 파일의 시각을 먼저 옛날로
/// 돌려 두면(`age_all`) 다시 쓴 파일이 보여요.
pub fn listing(dir: &Path) -> Option<BTreeMap<String, SystemTime>> {
    let entries = fs::read_dir(dir).ok()?;
    Some(
        entries
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().into_string().unwrap(),
                    e.metadata().unwrap().modified().unwrap(),
                )
            })
            .collect(),
    )
}

/// 폴더의 모든 파일 시각을 옛날로 돌려요.
pub fn age_all(dir: &Path) {
    for e in fs::read_dir(dir).unwrap() {
        age(&e.unwrap().path());
    }
}

/// 저장소 o/n(id 1)이 있어요(저장소 확인의 기본 답).
pub fn repo_ok(fake: &FakeGitHub) {
    fake.status(
        "repos/o/n",
        200,
        &[],
        Some(json!({"id": 1, "full_name": "o/n"})),
    );
}

/// 세 목록의 커서가 모두 `at`이에요.
pub fn all_cursors(at: &str) -> Value {
    json!({"issues": at, "comments": at, "review_comments": at})
}

/// 이 커서와 방금 한 재조정의 상태예요: 재조정할 때가 아니에요.
pub fn cursors_state(raw: &Path, cursors: Value) {
    let mut s = blank_state();
    s.insert("repo".into(), json!("o/n"));
    s.insert("fetched_at".into(), json!("2026-10-19T00:00:00Z"));
    s.insert("cursors".into(), cursors);
    s.insert("reconciled_at".into(), json!(now_iso()));
    save_state(raw, &s).unwrap();
}

/// 커서를 잊어서 다음 받기가 모두 다시 나열해요.
pub fn full_next(raw: &Path) {
    let mut s = load_state(raw);
    s.insert(
        "cursors".into(),
        json!({"issues": null, "comments": null, "review_comments": null}),
    );
    save_state(raw, &s).unwrap();
}

/// 세 목록을 부른 (path, since)예요. 차례대로예요.
pub fn listed(fake: &FakeGitHub) -> Vec<(String, Option<String>)> {
    fake.calls()
        .iter()
        .filter_map(gh_call)
        .filter(|(p, _)| LISTS.contains(&p.as_str()))
        .collect()
}

/// (path, since) 하나예요.
pub fn at(path: &str, since: Option<&str>) -> (String, Option<String>) {
    (path.to_string(), since.map(str::to_string))
}

fn page(count: usize, total: Option<u64>, make: impl Fn(usize) -> Value) -> Value {
    json!({"totalCount": total.unwrap_or(count as u64),
           "pageInfo": {"hasNextPage": false, "endCursor": null},
           "nodes": (0..count).map(make).collect::<Vec<_>>()})
}

/// GitHub이 주는 pullRequest 노드 하나예요: 한 쪽, pageInfo 있음(다음 쪽은 없어요).
pub fn pr_node(n: u64, files: usize, reviews: usize, total_files: Option<u64>) -> Value {
    json!({"number": n, "headRefName": "feat", "baseRefName": "dev", "mergedAt": null,
           "files": page(files, total_files, |i| json!({"path": format!("f{i}.ts")})),
           "reviews": page(reviews, None, |i| json!({
               "author": {"login": "jun-h"}, "body": format!("r{i}"), "state": "APPROVED",
               "submittedAt": "2026-10-01T00:00:00Z", "url": format!("https://github.com/o/n/pull/{n}#r{i}")}))})
}

static ALIAS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"p(\d+): pullRequest").unwrap());

/// GraphQL이 `answer(n)`으로 답해요: 물은 PR마다 그 노드, None이면 null이에요. 노드에 pageInfo가 없으면 다음 쪽이
/// 없다는 pageInfo를, 리뷰에 totalCount가 없으면 노드 수를 붙여요: 진짜 `pr_extras`가 이 답을 읽어요.
/// 물은 번호를 차례대로 모아 돌려줘요.
pub fn graphql_answers(
    fake: &FakeGitHub,
    answer: impl Fn(u64) -> Option<Value> + Send + Sync + 'static,
) -> Arc<Mutex<Vec<u64>>> {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    fake.graphql(move |query, _| {
        assert!(!query.contains("after:"), "a follow-up page was asked");
        let mut aliases = Map::new();
        for m in ALIAS.captures_iter(query) {
            let n: u64 = m[1].parse().unwrap();
            seen.lock().unwrap_or_else(PoisonError::into_inner).push(n);
            aliases.insert(format!("p{n}"), answer(n).map_or(Value::Null, one_page));
        }
        (200, json!({"data": {"repository": aliases}}))
    });
    asked
}

/// `graphql_answers`의 `pulls`판이에요: 번호로 찾고, 없으면 null이에요.
pub fn graphql_nodes(fake: &FakeGitHub, pulls: Vec<Value>) -> Arc<Mutex<Vec<u64>>> {
    let by = by_number(&pulls);
    graphql_answers(fake, move |n| by.get(&n).cloned())
}

fn one_page(mut node: Value) -> Value {
    for connection in ["files", "reviews"] {
        let Some(page) = node.get_mut(connection).and_then(Value::as_object_mut) else {
            continue;
        };
        if !page.contains_key("totalCount") {
            let count = page
                .get("nodes")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            page.insert("totalCount".into(), json!(count));
        }
        if !page.contains_key("pageInfo") {
            page.insert(
                "pageInfo".into(),
                json!({"hasNextPage": false, "endCursor": null}),
            );
        }
    }
    node
}
