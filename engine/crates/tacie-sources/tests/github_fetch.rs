//! GitHub incremental sync. GitHub is answered by `FakeGitHub` (a loopback fake server): lists by `route`, repo state
//! by `status`, extra PR info by `graphql` (so extra PR info also goes through the real `pr_extras`). An empty full
//! list is asked three times, like the real `rest`. "Written files" are judged by modified time (set back to the past
//! first). The number of stderr lines is not asserted (`came back short`, `reconcile failed`). Expected values and
//! fixture files are fixed values made by the old Python engine (2026-10-26).

mod common {
    pub mod github;
}

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::slice;
use std::sync::{Arc, OnceLock, Weak};

use common::github::*;
use serde_json::{Map, Value, json};
use tacie_sources::error::{Kind, SourceError};
use tacie_sources::github::docs::{chunks, documents};
use tacie_sources::github::fetch::{FetchOut, fetch};
use tacie_sources::github::items::{blank_state, load_items, load_state, save_state};
use tacie_sources::github::{GitHub, ISO, now_iso};
use tacie_sources::testing::github::{FakeGitHub, Reply};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn out(changed: usize, lost: u64) -> FetchOut {
    FetchOut { changed, lost }
}

fn fixture(name: &str) -> Value {
    let path = Path::new(FIXTURES).join(name);
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// 옛 모양의 네 목록이에요(가짜 저장소 o/n).
struct V1 {
    issues: Vec<Value>,
    comments: Vec<Value>,
    review_comments: Vec<Value>,
    pulls: Vec<Value>,
}

fn v1_lists() -> V1 {
    let list = |n: &str| {
        fixture(&format!("github_v1/{n}.json"))
            .as_array()
            .unwrap()
            .clone()
    };
    V1 {
        issues: list("issues"),
        comments: list("comments"),
        review_comments: list("review_comments"),
        pulls: list("pulls"),
    }
}

/// 저장소가 있고, 세 목록과 PR 추가 정보가 `v`에서 와요.
fn fake_github(fake: &FakeGitHub, v: &V1) {
    repo_ok(fake);
    lists(
        fake,
        v.issues.clone(),
        v.comments.clone(),
        v.review_comments.clone(),
    );
    graphql_nodes(fake, v.pulls.clone());
}

/// path 순서의 문서(JSON 값)예요.
fn docs_of(raw: &Path) -> Value {
    let mut values: Vec<Value> = documents(raw)
        .unwrap()
        .iter()
        .map(|d| d.to_value())
        .collect();
    values.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Value::Array(values)
}

fn doc_paths(raw: &Path) -> Vec<String> {
    documents(raw)
        .unwrap()
        .into_iter()
        .map(|d| d.path)
        .collect()
}

/// 폴더 바로 아래 파일의 이름 → 바이트예요(폴더는 빼요).
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                fs::read(&p).unwrap(),
            )
        })
        .collect()
}

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    out.sort();
    out
}

fn item_numbers(raw: &Path) -> Vec<u64> {
    load_items(raw).unwrap().0.keys().copied().collect()
}

fn item(raw: &Path, n: u64) -> Value {
    load_items(raw).unwrap().0.remove(&n).unwrap()
}

fn ids(v: &Value) -> Vec<u64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_u64().unwrap())
        .collect()
}

fn count(fake: &FakeGitHub, path: &str) -> usize {
    gh_paths(fake).iter().filter(|p| *p == path).count()
}

fn route_list(fake: &FakeGitHub, path: &str, items: Vec<Value>) {
    fake.route(path, Reply::Json(Value::Array(items)));
}

/// 저장된 state.json의 키 순서예요.
fn state_keys(raw: &Path) -> Vec<String> {
    let text = fs::read_to_string(raw.join("state.json")).unwrap();
    let state: Map<String, Value> = serde_json::from_str(&text).unwrap();
    state.keys().cloned().collect()
}

fn blank_keys() -> Vec<String> {
    blank_state().keys().cloned().collect()
}

fn three_hours_before(at: &str) -> String {
    (chrono::NaiveDateTime::parse_from_str(at, ISO).unwrap() - chrono::TimeDelta::hours(3))
        .format(ISO)
        .to_string()
}

#[test]
fn v1_raw_is_ignored_and_refetched() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path().join("github");
    fs::create_dir(&raw).unwrap();
    for (name, bytes) in files(&Path::new(FIXTURES).join("github_v1")) {
        fs::write(raw.join(name), bytes).unwrap();
    }
    let mut v1 = files(&raw);
    v1.remove("state.json");
    assert_eq!(load_state(&raw), blank_state());
    assert!(documents(&raw).unwrap().is_empty() && chunks(&raw).unwrap().is_empty());
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    fake_github(&fake, &v1_lists());
    assert_eq!(fetch(&gh, "o/n", &raw).unwrap(), out(6, 0));
    let mut asked = listed(&fake);
    asked.sort();
    assert_eq!(
        asked,
        [at(LISTS[1], None), at(LISTS[0], None), at(LISTS[2], None)]
    );
    let st = load_state(&raw);
    assert_eq!((&st["version"], &st["repo"]), (&json!(2), &json!("o/n")));
    assert!(st["fetched_at"].is_string() && st["reconciled_at"] == st["fetched_at"]);
    assert_eq!(
        st["cursors"],
        json!({"issues": "2026-10-13T09:00:00Z", "comments": "2026-10-11T03:30:00Z",
               "review_comments": "2026-10-10T07:00:00Z"})
    );
    assert_eq!(
        json!([st["etags"], st["repo_id"], st["full_name"], st["lost"]]),
        json!([{}, 1, "o/n", 0])
    );
    assert_eq!(state_keys(&raw), blank_keys()); // 키는 빈 상태의 순서예요
    assert_eq!(docs_of(&raw), fixture("github_v1_documents.json"));
    let mut now = files(&raw);
    now.remove("state.json");
    assert_eq!(now, v1); // 그대로 남아요
    let mut want: Vec<String> = v1.keys().cloned().collect();
    want.extend(["items".to_string(), "state.json".to_string()]);
    want.sort();
    assert_eq!(names(&raw), want);
}

#[test]
fn corrupt_files_are_refetched() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    fake_github(&fake, &v1_lists());
    fetch(&gh, "o/n", raw).unwrap();
    fs::write(raw.join("state.json"), "{").unwrap();
    assert_eq!(load_state(raw), blank_state());
    fs::write(raw.join("items/pull-2.json"), "{").unwrap();
    let (items, bad) = load_items(raw).unwrap();
    assert_eq!(bad, BTreeSet::from([2]));
    assert_eq!(items.keys().copied().collect::<Vec<_>>(), [1, 3, 4, 5, 6]);
    assert!(!doc_paths(raw).contains(&"github/pull/2".to_string()));
    fake.clear_calls();
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(1, 0));
    // 모두 다시 나열해요
    let since: Vec<Option<String>> = listed(&fake).into_iter().map(|(_, s)| s).collect();
    assert_eq!(since, [None, None, None]);
    assert!(load_items(raw).unwrap().1.is_empty());
    assert_eq!(load_state(raw)["repo"], "o/n");
    assert_eq!(docs_of(raw), fixture("github_v1_documents.json"));
}

#[test]
fn fetch_writes_only_changed_items() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let mut v = v1_lists();
    v.issues
        .retain(|i| [1, 2, 4].contains(&i["number"].as_u64().unwrap()));
    fake_github(&fake, &v);
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(3, 0));
    let state = raw.join("state.json");
    full_next(raw);
    age_all(&raw.join("items"));
    let old_state = age(&state);
    let items = listing(&raw.join("items"));
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(0, 0));
    assert_eq!(listing(&raw.join("items")), items); // state.json만 써요
    assert_ne!(mtime(&state), old_state);
    let i = v.issues.iter().position(|i| i["number"] == 2).unwrap();
    v.issues[i]["body"] = json!("메일 탭을 새로고침해요. 바뀐 설명이에요.");
    lists(&fake, v.issues.clone(), v.comments.clone(), vec![]);
    route_list(&fake, LISTS[2], v.review_comments.clone());
    full_next(raw);
    let old_state = age(&state);
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(1, 0));
    let mut written: Vec<String> = listing(&raw.join("items"))
        .unwrap()
        .into_iter()
        .filter(|(name, at)| items.as_ref().unwrap()[name] != *at)
        .map(|(name, _)| format!("items/{name}"))
        .collect();
    if mtime(&state) != old_state {
        written.push("state.json".to_string());
    }
    assert_eq!(written, ["items/pull-2.json", "state.json"]);
    assert!(
        item(raw, 2)["issue"]["body"]
            .as_str()
            .unwrap()
            .contains("바뀐 설명")
    );
}

#[test]
fn short_full_list_keeps_stored_comments() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let comment = |i: u64, body: &str| {
        json!({"id": i, "issue_url": "https://api.github.com/repos/o/n/issues/5", "user": {"login": "jun-h"},
               "created_at": format!("2026-10-0{i}T00:00:00Z"), "updated_at": format!("2026-10-0{i}T00:00:00Z"),
               "body": body, "html_url": format!("https://github.com/o/n/issues/5#issuecomment-{i}")})
    };
    let issue = json!({"id": 105, "number": 5, "title": "재시도", "body": "설명", "user": {"login": "sora-p"},
                       "state": "open", "labels": [], "created_at": "2026-10-01T00:00:00Z",
                       "updated_at": "2026-10-03T00:00:00Z", "html_url": "https://github.com/o/n/issues/5"});
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    lists(
        &fake,
        vec![issue],
        vec![comment(1, "첫 댓글"), comment(2, "둘째 댓글")],
        vec![],
    );
    fetch(&gh, "o/n", raw).unwrap();
    route_list(&fake, LISTS[1], vec![comment(1, "고친 첫 댓글")]); // 전체 목록이 짧게 왔어요(2026-10-23)
    full_next(raw);
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(1, 0));
    let bodies: BTreeMap<u64, Value> = item(raw, 5)["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["id"].as_u64().unwrap(), c["body"].clone()))
        .collect();
    assert_eq!(
        bodies,
        BTreeMap::from([(1, json!("고친 첫 댓글")), (2, json!("둘째 댓글"))])
    );
    route_list(&fake, LISTS[1], vec![]);
    full_next(raw);
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(0, 0));
    let st = load_state(raw);
    // 빈 전체 목록: 시작 시각이라 다음 받기는 증분이에요
    assert_eq!(st["cursors"]["comments"], st["fetched_at"]);
}

#[test]
fn cursor_moves_only_with_data() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    cursors_state(raw, all_cursors("2026-10-19T00:00:00Z"));
    let reconciled = load_state(raw)["reconciled_at"].clone();
    let one = issue(1, "2026-10-20T01:00:00Z", 1, false);
    let two = issue(2, "2026-10-22T05:00:00Z", 0, true);
    let c = comment(11, 1, "2026-10-21T00:00:00Z", false);
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    lists(&fake, vec![one, two.clone()], vec![c.clone()], vec![]);
    route_list(&fake, "repos/o/n/issues/1/comments", vec![c]);
    route_list(&fake, "repos/o/n/issues/2/comments", vec![]);
    route_list(&fake, "repos/o/n/pulls/2/comments", vec![]);
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(2, 0));
    // since = 커서 − 3 h, 한 번씩 물어요
    let since = Some("2026-10-18T21:00:00Z");
    assert_eq!(listed(&fake), LISTS.map(|p| at(p, since)));
    assert_eq!(
        load_state(raw)["cursors"],
        json!({"issues": "2026-10-22T05:00:00Z", "comments": "2026-10-21T00:00:00Z",
               "review_comments": "2026-10-19T00:00:00Z"})
    ); // 리뷰 댓글은 오지 않았어요
    let r = comment(21, 2, "2026-10-23T00:00:00Z", true);
    lists(&fake, vec![], vec![], vec![r.clone()]);
    fake.route("repos/o/n/issues/2", Reply::Json(two));
    route_list(&fake, "repos/o/n/pulls/2/comments", vec![r]);
    fake.clear_calls();
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(1, 0));
    // 빈 답은 다시 묻지 않아요
    assert_eq!(
        listed(&fake),
        [
            at(LISTS[0], Some("2026-10-22T02:00:00Z")),
            at(LISTS[1], Some("2026-10-20T21:00:00Z")),
            at(LISTS[2], Some("2026-10-18T21:00:00Z"))
        ]
    );
    let st = load_state(raw);
    assert_eq!(
        st["cursors"],
        json!({"issues": "2026-10-22T05:00:00Z", "comments": "2026-10-21T00:00:00Z",
               "review_comments": "2026-10-23T00:00:00Z"})
    );
    assert_eq!(st["reconciled_at"], reconciled); // `since` 나열은 재조정이 아니에요
    assert_eq!(ids(&item(raw, 2)["review_comments"]), [21]);
}

#[test]
fn empty_full_list_sets_cursor_to_start() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let one = issue(1, "2026-10-20T01:00:00Z", 1, false);
    let c = comment(11, 1, "2026-10-21T00:00:00Z", false);
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    lists(&fake, vec![one], vec![c], vec![]);
    let before = now_iso();
    fetch(&gh, "o/n", raw).unwrap();
    let after = now_iso();
    assert_eq!(
        listed(&fake),
        [
            at(LISTS[0], None),
            at(LISTS[1], None),
            at(LISTS[2], None),
            at(LISTS[2], None),
            at(LISTS[2], None)
        ]
    );
    let st = load_state(raw);
    let review = st["cursors"]["review_comments"].as_str().unwrap();
    assert!(before.as_str() <= review && review <= after.as_str());
    assert_eq!(st["cursors"]["review_comments"], st["fetched_at"]);
    assert_eq!(st["cursors"]["issues"], "2026-10-20T01:00:00Z");
    assert_eq!(st["cursors"]["comments"], "2026-10-21T00:00:00Z");
    fake.clear_calls();
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(0, 0));
    let back = three_hours_before(st["fetched_at"].as_str().unwrap());
    assert_eq!(
        listed(&fake),
        [
            at(LISTS[0], Some("2026-10-19T22:00:00Z")),
            at(LISTS[1], Some("2026-10-20T21:00:00Z")),
            at(LISTS[2], Some(&back))
        ]
    );
}

#[test]
fn changed_document_is_replaced_whole() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let pr = issue(655, "2026-10-20T00:00:00Z", 2, true);
    let [c1, c2, c3] =
        [1, 2, 3].map(|i| comment(i, 655, &format!("2026-10-2{i}T00:00:00Z"), false));
    let extras = json!({"number": 655, "headRefName": "feat/x", "baseRefName": "dev", "mergedAt": null,
                        "files": {"totalCount": 1, "nodes": [{"path": "a.ts"}]}, "reviews": {"nodes": []}});
    stored(raw, &pr, &[c1.clone(), c2], &[], extras.clone());
    stored(
        raw,
        &issue(12, "2026-10-01T00:00:00Z", 0, false),
        &[],
        &[],
        Value::Null,
    );
    cursors_state(raw, all_cursors("2026-10-22T00:00:00Z"));
    let other = raw.join("items/issue-12.json");
    let twelve = age(&other);
    let new = with(&pr, &[("updated_at", json!("2026-10-23T00:00:00Z"))]);
    let r = comment(31, 655, "2026-10-23T00:00:00Z", true);
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    lists(&fake, vec![new.clone()], vec![c3.clone()], vec![]);
    route_list(
        &fake,
        "repos/o/n/issues/655/comments",
        vec![c1.clone(), c3.clone()],
    );
    route_list(&fake, "repos/o/n/pulls/655/comments", vec![r]);
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(1, 0));
    let it = item(raw, 655);
    assert_eq!(ids(&it["comments"]), [1, 3]); // c2는 지워졌어요
    assert_eq!(ids(&it["review_comments"]), [31]);
    assert_eq!(it["issue"], new);
    assert_eq!(it["updated_at"], new["updated_at"]);
    assert_eq!(it["extras"], extras);
    assert_eq!(mtime(&other), twelve);
    // GitHub이 센 수보다 짧은 목록(또는 리뷰 댓글이 저장돼 있는데 빈 목록)은 한 번 더 묻고, 그래도 그러면 합쳐요
    let c4 = comment(4, 655, "2026-10-24T00:00:00Z", false);
    let newer = with(
        &pr,
        &[
            ("updated_at", json!("2026-10-24T00:00:00Z")),
            ("comments", json!(3)),
        ],
    );
    route_list(&fake, LISTS[0], vec![newer]);
    route_list(&fake, LISTS[1], vec![c4.clone()]);
    route_list(&fake, "repos/o/n/issues/655/comments", vec![c4]);
    route_list(&fake, "repos/o/n/pulls/655/comments", vec![]);
    fake.clear_calls();
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(1, 0));
    let it = item(raw, 655);
    assert_eq!(ids(&it["comments"]), [1, 3, 4]);
    assert_eq!(ids(&it["review_comments"]), [31]);
    assert_eq!(count(&fake, "repos/o/n/issues/655/comments"), 2);
    assert_eq!(count(&fake, "repos/o/n/pulls/655/comments"), 2);
}

#[test]
fn unchanged_candidates_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let seven = issue(7, "2026-10-22T05:00:00Z", 1, false);
    let c = comment(70, 7, "2026-10-22T04:00:00Z", false);
    let eight = issue(8, "2026-10-22T03:00:00Z", 1, false);
    let d = comment(80, 8, "2026-10-22T03:00:00Z", false);
    stored(raw, &seven, slice::from_ref(&c), &[], Value::Null);
    stored(raw, &eight, slice::from_ref(&d), &[], Value::Null);
    let cursors = json!({"issues": "2026-10-22T05:00:00Z", "comments": "2026-10-22T04:00:00Z",
                         "review_comments": "2026-10-22T00:00:00Z"});
    cursors_state(raw, cursors.clone());
    age_all(&raw.join("items"));
    let mtimes = listing(&raw.join("items"));
    // 겹친 구간이 #7과, #7·#8의 댓글을 저장된 그대로 다시 가져와요
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    lists(&fake, vec![seven], vec![c, d], vec![]);
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(0, 0));
    // issues/7, issues/7/comments, issues/8 호출이 없고, 물을 PR도 없어요
    assert_eq!(gh_paths(&fake), LISTS);
    assert_eq!(listing(&raw.join("items")), mtimes);
    assert_eq!(load_state(raw)["cursors"], cursors);
}

#[test]
fn interrupted_run_does_not_advance_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    cursors_state(raw, all_cursors("2026-10-22T00:00:00Z"));
    let state = fs::read(raw.join("state.json")).unwrap();
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    lists(
        &fake,
        vec![
            issue(8, "2026-10-23T00:00:00Z", 0, false),
            issue(9, "2026-10-23T01:00:00Z", 0, false),
        ],
        vec![],
        vec![],
    );
    route_list(&fake, "repos/o/n/issues/8/comments", vec![]);
    fake.route("repos/o/n/issues/9/comments", Reply::Close);
    let err = fetch(&gh, "o/n", raw).unwrap_err();
    assert_eq!(err.kind(), Kind::Network, "{err:?}");
    assert_eq!(fs::read(raw.join("state.json")).unwrap(), state);
    let asked = listed(&fake);
    route_list(&fake, "repos/o/n/issues/9/comments", vec![]);
    fake.clear_calls();
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(1, 0));
    // 같은 범위를 다시 물어요
    assert_eq!(listed(&fake), asked);
    assert_eq!(asked, LISTS.map(|p| at(p, Some("2026-10-21T21:00:00Z"))));
    // 끊기기 전에 썼고 같아요
    assert!(!gh_paths(&fake).contains(&"repos/o/n/issues/8/comments".to_string()));
    assert_eq!(load_state(raw)["cursors"]["issues"], "2026-10-23T01:00:00Z");
}

#[test]
fn short_answer_keeps_listed_comments() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let pr = issue(5, "2026-10-20T00:00:00Z", 1, true);
    let c1 = comment(1, 5, "2026-10-20T00:00:00Z", false);
    let c2 = comment(2, 5, "2026-10-23T00:00:00Z", false);
    let r = comment(31, 5, "2026-10-23T00:00:00Z", true);
    stored(raw, &pr, slice::from_ref(&c1), &[], Value::Null);
    cursors_state(raw, all_cursors("2026-10-22T00:00:00Z"));
    // since 목록이 c2와 r을 가져오고, 문서의 목록은 두 번 비어 와요
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    lists(
        &fake,
        vec![with(
            &pr,
            &[
                ("updated_at", json!("2026-10-23T00:00:00Z")),
                ("comments", json!(2)),
            ],
        )],
        vec![c2.clone()],
        vec![r.clone()],
    );
    route_list(&fake, "repos/o/n/issues/5/comments", vec![]);
    route_list(&fake, "repos/o/n/pulls/5/comments", vec![]);
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(1, 0));
    let it = item(raw, 5);
    assert_eq!(ids(&it["comments"]), [1, 2]);
    assert_eq!(ids(&it["review_comments"]), [31]);
    assert_eq!(count(&fake, "repos/o/n/issues/5/comments"), 2);
    assert_eq!(count(&fake, "repos/o/n/pulls/5/comments"), 2);
    // GitHub이 센 수보다 긴 답은 믿어요. 비지 않고 목록의 댓글을 모두 가진 두 번째 답도요
    let c3 = comment(3, 5, "2026-10-24T00:00:00Z", false);
    lists(
        &fake,
        vec![with(
            &pr,
            &[
                ("updated_at", json!("2026-10-24T00:00:00Z")),
                ("comments", json!(2)),
            ],
        )],
        vec![c3.clone()],
        vec![],
    );
    route_list(
        &fake,
        "repos/o/n/issues/5/comments",
        vec![c1.clone(), c2, c3.clone()],
    );
    fake.route_seq(
        "repos/o/n/pulls/5/comments",
        vec![Reply::Json(json!([])), Reply::Json(json!([r.clone()]))],
    );
    fake.clear_calls();
    fetch(&gh, "o/n", raw).unwrap();
    let it = item(raw, 5);
    assert_eq!(ids(&it["comments"]), [1, 2, 3]);
    assert_eq!(ids(&it["review_comments"]), [31]);
    assert_eq!(count(&fake, "repos/o/n/issues/5/comments"), 1);
    assert_eq!(count(&fake, "repos/o/n/pulls/5/comments"), 2);
    // GitHub이 센 수가 없으면 이슈의 목록은 저장된 댓글이 있는데 비어 올 때만 의심해요
    let mut uncounted = pr.clone();
    uncounted.as_object_mut().unwrap().shift_remove("comments");
    uncounted["updated_at"] = json!("2026-10-25T00:00:00Z");
    route_list(&fake, LISTS[0], vec![uncounted]);
    route_list(&fake, LISTS[1], vec![]);
    route_list(&fake, "repos/o/n/issues/5/comments", vec![c1, c3]);
    route_list(&fake, "repos/o/n/pulls/5/comments", vec![r]);
    fake.clear_calls();
    fetch(&gh, "o/n", raw).unwrap();
    assert_eq!(ids(&item(raw, 5)["comments"]), [1, 3]);
    assert_eq!(count(&fake, "repos/o/n/issues/5/comments"), 1);
}

#[test]
fn new_or_edited_comment_is_not_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let seven = issue(7, "2026-10-22T05:00:00Z", 1, false);
    let eight = issue(8, "2026-10-22T05:00:00Z", 2, false);
    let c7 = comment(70, 7, "2026-10-22T04:00:00Z", false);
    let c8 = comment(80, 8, "2026-10-22T04:00:00Z", false);
    stored(raw, &seven, slice::from_ref(&c7), &[], Value::Null);
    stored(raw, &eight, slice::from_ref(&c8), &[], Value::Null);
    cursors_state(raw, all_cursors("2026-10-22T05:00:00Z"));
    let edited = with(
        &c7,
        &[
            ("updated_at", json!("2026-10-22T06:00:00Z")),
            ("body", json!("고친 댓글")),
        ],
    );
    let new = comment(81, 8, "2026-10-22T06:00:00Z", false);
    // 두 이슈는 저장된 그대로 나열되고, 댓글만 바뀌었어요
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    lists(
        &fake,
        vec![seven, eight],
        vec![edited.clone(), new.clone()],
        vec![],
    );
    route_list(&fake, "repos/o/n/issues/7/comments", vec![edited.clone()]);
    route_list(&fake, "repos/o/n/issues/8/comments", vec![c8, new]);
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(2, 0));
    assert_eq!(item(raw, 7)["comments"], json!([edited]));
    assert_eq!(ids(&item(raw, 8)["comments"]), [80, 81]);
}

#[test]
fn orphan_comment_does_not_move_full_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let one = issue(1, "2026-10-20T01:00:00Z", 1, false);
    let c = comment(11, 1, "2026-10-21T00:00:00Z", false);
    // 더 새것이고, 이 (짧은) 이슈 목록이 빠뜨린 이슈에 달렸어요
    let orphan = comment(99, 99, "2026-10-23T00:00:00Z", false);
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    lists(&fake, vec![one], vec![c, orphan], vec![]);
    fetch(&gh, "o/n", raw).unwrap();
    assert_eq!(
        load_state(raw)["cursors"]["comments"],
        "2026-10-21T00:00:00Z"
    );
    assert_eq!(item_numbers(raw), [1]);
}

/// 목록에 온 것이 모두 나열되지 않은 이슈의 것이면 커서는 그대로예요(시작 시각도 아니에요): 비어 온 목록과 달라요.
#[test]
fn a_list_of_orphans_keeps_the_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let mut s = blank_state();
    s.insert("repo".into(), json!("o/n"));
    s.insert(
        "cursors".into(),
        json!({"issues": null, "comments": "2026-10-19T00:00:00Z", "review_comments": null}),
    );
    save_state(raw, &s).unwrap();
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    lists(
        &fake,
        vec![issue(1, "2026-10-20T01:00:00Z", 0, false)],
        vec![comment(99, 99, "2026-10-23T00:00:00Z", false)],
        vec![comment(98, 98, "2026-10-23T00:00:00Z", true)],
    );
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(1, 0));
    let st = load_state(raw);
    assert_eq!(
        st["cursors"],
        json!({"issues": "2026-10-20T01:00:00Z", "comments": "2026-10-19T00:00:00Z", "review_comments": null})
    );
    assert_eq!(st["reconciled_at"], st["fetched_at"]);
}

#[test]
fn empty_full_lists_are_no_reconcile() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let week_ago = "2026-10-17T00:00:00Z";
    let mut s = blank_state();
    s.insert("repo".into(), json!("o/n"));
    s.insert("fetched_at".into(), json!(week_ago));
    s.insert("reconciled_at".into(), json!(week_ago));
    save_state(raw, &s).unwrap();
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    lists(&fake, vec![], vec![], vec![]);
    fetch(&gh, "o/n", raw).unwrap();
    let st = load_state(raw);
    assert_eq!(st["cursors"]["issues"], st["fetched_at"]);
    assert_ne!(st["fetched_at"], week_ago);
    assert_eq!(st["reconciled_at"], week_ago);
    // 빈 댓글 목록에 없는 댓글을 세요
    route_list(
        &fake,
        LISTS[0],
        vec![issue(1, "2026-10-20T01:00:00Z", 1, false)],
    );
    full_next(raw);
    fetch(&gh, "o/n", raw).unwrap();
    assert_eq!(load_state(raw)["reconciled_at"], week_ago);
    // 리뷰 댓글은 수가 없어요: 막지 않아요
    route_list(
        &fake,
        LISTS[1],
        vec![comment(11, 1, "2026-10-21T00:00:00Z", false)],
    );
    full_next(raw);
    fetch(&gh, "o/n", raw).unwrap();
    let st = load_state(raw);
    assert_eq!(st["reconciled_at"], st["fetched_at"]);
    assert_ne!(st["fetched_at"], week_ago);
}

#[test]
fn only_changed_open_and_incomplete_prs_are_asked() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let pull = |n: u64, state: &str, extras: Value| {
        let it = with(
            &issue(n, "2026-10-01T00:00:00Z", 0, true),
            &[("state", json!(state))],
        );
        stored(raw, &it, &[], &[], extras);
    };
    pull(1, "closed", pr_node(1, 1, 0, None));
    pull(2, "closed", pr_node(2, 1, 0, None));
    pull(3, "open", pr_node(3, 1, 0, None));
    pull(4, "closed", pr_node(4, 60, 0, Some(488)));
    let mut old = pr_node(5, 1, 0, None);
    old["reviews"]
        .as_object_mut()
        .unwrap()
        .shift_remove("totalCount");
    pull(5, "closed", old);
    // 이슈는 묻지 않아요
    stored(
        raw,
        &with(
            &issue(6, "2026-10-01T00:00:00Z", 0, false),
            &[("state", json!("closed"))],
        ),
        &[],
        &[],
        Value::Null,
    );
    cursors_state(raw, all_cursors("2026-10-19T00:00:00Z"));
    let two = with(
        &issue(2, "2026-10-20T00:00:00Z", 0, true),
        &[("state", json!("closed"))],
    );
    let asked = graphql_answers(&fake, |n| Some(pr_node(n, 1, 1, None)));
    repo_ok(&fake);
    lists(&fake, vec![two], vec![], vec![]);
    route_list(&fake, "repos/o/n/issues/2/comments", vec![]);
    route_list(&fake, "repos/o/n/pulls/2/comments", vec![]);
    fetch(&gh, "o/n", raw).unwrap();
    let got: BTreeSet<u64> = asked.lock().unwrap().iter().copied().collect();
    assert_eq!(got, BTreeSet::from([2, 3, 4, 5]));
    let items = load_items(raw).unwrap().0;
    assert_eq!(items[&1]["extras"], pr_node(1, 1, 0, None));
    assert_eq!(items[&4]["extras"]["reviews"]["totalCount"], 1);
    // 이제 모두 다 받았어요: 열린 것만 다시 물어요
    asked.lock().unwrap().clear();
    fetch(&gh, "o/n", raw).unwrap();
    let got: BTreeSet<u64> = asked.lock().unwrap().iter().copied().collect();
    assert_eq!(got, BTreeSet::from([3]));
}

/// 재조정 절반은 github_reconcile.rs의 같은 이름 테스트예요.
#[test]
fn reconcile_never_deletes_on_a_flaky_list() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let mut issues = numbers(raw, 1..=20);
    issues[0] = with(&issues[0], &[("comments", json!(1))]);
    let c = comment(11, 1, "2026-10-10T00:00:00Z", false);
    stored(raw, &issues[0], slice::from_ref(&c), &[], Value::Null);
    age_all(&raw.join("items"));
    let files = listing(&raw.join("items"));
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    // fetch를 거쳐서: 20개 중 6개가 빠진 목록(30 %, MISSING_FLOOR도 넘어요)으로는 지우지 않고, 따로 묻지도 않아요
    let long_ago = "2026-01-01T00:00:00Z";
    let mut s = known_state();
    s.insert("cursors".into(), all_cursors("2026-10-19T00:00:00Z"));
    s.insert("reconciled_at".into(), json!(long_ago));
    save_state(raw, &s).unwrap();
    lists(&fake, issues[..14].to_vec(), vec![c], vec![]);
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(0, 0));
    assert_eq!(status_asked(&fake), ["repos/o/n"]);
    assert!(asked_alone(&fake).is_empty());
    assert_eq!(listing(&raw.join("items")), files);
    assert_eq!(load_state(raw)["reconciled_at"], long_ago); // 그래서 다음 갱신이 다시 해요
    route_list(&fake, LISTS[0], issues); // 다시 온전해요
    fetch(&gh, "o/n", raw).unwrap();
    let st = load_state(raw);
    assert_eq!(st["reconciled_at"], st["fetched_at"]);
    assert_ne!(st["reconciled_at"], long_ago);
    assert_eq!(listing(&raw.join("items")), files);
}

#[test]
fn reconcile_error_keeps_the_incremental_cursors() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = Arc::new(connect(&fake));
    // 리뷰 댓글의 since 목록에 답한 뒤 연결을 끊어요: 갱신 중에 사용자가 GitHub 연결을 끊은 것과 같아요
    let cut: Arc<OnceLock<Weak<GitHub>>> = Arc::new(OnceLock::new());
    let hook = cut.clone();
    fake.status_with(move |path, _| {
        assert!(path.starts_with("repos/o/n/pulls/comments?"), "{path}");
        if path.contains("since=")
            && let Some(gh) = hook.get().and_then(Weak::upgrade)
        {
            gh.set_credentials("none", None);
        }
        Reply::Json(json!([]))
    });
    let one = issue(1, "2026-10-10T00:00:00Z", 0, false);
    stored(raw, &one, &[], &[], Value::Null);
    let long_ago = "2026-01-01T00:00:00Z";
    let mut s = known_state();
    s.insert("cursors".into(), all_cursors("2026-10-19T00:00:00Z"));
    s.insert("reconciled_at".into(), json!(long_ago));
    save_state(raw, &s).unwrap();
    let newer = with(&one, &[("updated_at", json!("2026-10-20T00:00:00Z"))]);
    repo_ok(&fake);
    // since 이슈 목록은 newer를 가져오고, 재조정의 전체 이슈 목록(since 없음)은 연결 오류예요
    fake.route(LISTS[0], Reply::Close);
    let since_list = |since: &str| {
        let at = since.replace(':', "%3A");
        route_list(
            &fake,
            &format!("{}&since={at}", LISTS[0]),
            vec![newer.clone()],
        );
    };
    since_list("2026-10-18T21:00:00Z");
    route_list(&fake, LISTS[1], vec![]);
    route_list(&fake, "repos/o/n/issues/1/comments", vec![]);
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(1, 0));
    let st = load_state(raw);
    assert_eq!(st["cursors"]["issues"], "2026-10-20T00:00:00Z");
    assert_eq!(st["reconciled_at"], long_ago); // 다음에 다시 해요
    assert!(listed(&fake).contains(&at(LISTS[0], None)));
    assert_eq!(item(raw, 1)["issue"], newer);
    // 거절된 키는 그래도 받기를 실패시키고, 아무것도 저장하지 않아요
    let saved = fs::read(raw.join("state.json")).unwrap();
    since_list("2026-10-19T21:00:00Z"); // 이슈 커서가 옮겨 갔어요
    cut.set(Arc::downgrade(&gh)).unwrap();
    fake.clear_calls();
    let err = fetch(&gh, "o/n", raw).unwrap_err();
    // 세 since 목록 뒤, 재조정이 처음 GitHub에 물으려 할 때예요(바뀐 문서는 없어요)
    assert_eq!(
        listed(&fake),
        [
            at(LISTS[0], Some("2026-10-19T21:00:00Z")),
            at(LISTS[1], Some("2026-10-18T21:00:00Z")),
            at(LISTS[2], Some("2026-10-18T21:00:00Z"))
        ]
    );
    assert_eq!(gh_paths(&fake).len(), 3);
    assert!(
        matches!(
            err,
            SourceError::Service {
                kind: Kind::Key,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(fs::read(raw.join("state.json")).unwrap(), saved);
}

#[test]
fn a_repo_gone_or_replaced_saves_only_the_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let mut s = known_state();
    s.insert("fetched_at".into(), json!("2026-10-19T00:00:00Z"));
    s.insert("cursors".into(), all_cursors("2026-10-19T00:00:00Z"));
    s.insert("reconciled_at".into(), json!("2026-10-18T00:00:00Z"));
    save_state(raw, &s).unwrap();
    fake.status("repos/o/n", 404, &[], Some(json!({"message": "Not Found"})));
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(0, 1));
    assert!(gh_paths(&fake).is_empty()); // 저장소가 없는 동안 아무것도 나열하지 않아요
    assert_eq!(
        fs::read_to_string(raw.join("state.json")).unwrap(),
        r#"{"version": 2, "repo": "o/n", "fetched_at": "2026-10-19T00:00:00Z", "cursors": {"issues": "2026-10-19T00:00:00Z", "comments": "2026-10-19T00:00:00Z", "review_comments": "2026-10-19T00:00:00Z"}, "etags": {}, "reconciled_at": "2026-10-18T00:00:00Z", "repo_id": 1, "full_name": "o/n", "lost": 1}"#
    );
    // 다른 id: 그 이름은 이제 다른 저장소라 바로 닫아요. 모르는 키는 상태에 남지 않아요(load_state)
    let mut s = blank_state();
    s.insert("repo".into(), json!("o/n"));
    s.insert("fetched_at".into(), json!("2026-10-19T00:00:00Z"));
    s.insert("repo_id".into(), json!(1));
    s.insert("full_name".into(), json!("o/n"));
    s.insert("lost".into(), json!(1));
    s.insert("extra".into(), json!("kept"));
    save_state(raw, &s).unwrap();
    fake.status(
        "repos/o/n",
        200,
        &[],
        Some(json!({"id": 2, "full_name": "o/n"})),
    );
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(0, 2));
    assert_eq!(
        fs::read_to_string(raw.join("state.json")).unwrap(),
        r#"{"version": 2, "repo": "o/n", "fetched_at": "2026-10-19T00:00:00Z", "cursors": {"issues": null, "comments": null, "review_comments": null}, "etags": {}, "reconciled_at": null, "repo_id": 1, "full_name": "o/n", "lost": 2}"#
    );
    // 그 밖의 답은 오류이고 저장된 상태는 그대로예요
    let before = fs::read(raw.join("state.json")).unwrap();
    fake.status("repos/o/n", 502, &[], Some(json!({"message": "x"})));
    let err = fetch(&gh, "o/n", raw).unwrap_err();
    assert_eq!(err.kind(), Kind::Down);
    assert_eq!(fs::read(raw.join("state.json")).unwrap(), before);
    assert!(gh_paths(&fake).is_empty());
}

#[test]
fn cursors_are_read_like_python_strptime() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    repo_ok(&fake);
    lists(&fake, vec![], vec![], vec![]);
    // 한 자리 월·일·시·분·초도 받아요
    cursors_state(raw, all_cursors("2026-1-2T3:4:5Z"));
    assert_eq!(fetch(&gh, "o/n", raw).unwrap(), out(0, 0));
    assert_eq!(
        listed(&fake),
        LISTS.map(|p| at(p, Some("2026-01-02T00:04:05Z")))
    );
    // 있을 수 없는 날짜의 커서는 없는 것과 같아요: 모두 다시 나열해요(빈 목록은 세 번)
    cursors_state(
        raw,
        json!({"issues": "2026-10-19T00:00:00Z", "comments": "2026-02-30T00:00:00Z",
               "review_comments": "2026-10-19T00:00:00Z"}),
    );
    fake.clear_calls();
    fetch(&gh, "o/n", raw).unwrap();
    let want: Vec<_> = LISTS
        .iter()
        .flat_map(|p| [at(p, None), at(p, None), at(p, None)])
        .collect();
    assert_eq!(listed(&fake), want);
    let st = load_state(raw);
    assert_eq!(
        st["cursors"],
        all_cursors(st["fetched_at"].as_str().unwrap())
    );
    // 0001-01-01T03:00:00Z보다 이른 커서는 나열하기 전에 실패하고 상태는 그대로예요
    cursors_state(
        raw,
        json!({"issues": "2026-10-19T00:00:00Z", "comments": "0001-01-01T02:59:59Z",
               "review_comments": null}),
    );
    let before = fs::read(raw.join("state.json")).unwrap();
    fake.clear_calls();
    assert!(fetch(&gh, "o/n", raw).is_err());
    assert!(gh_paths(&fake).is_empty());
    assert_eq!(status_asked(&fake), ["repos/o/n"]);
    assert_eq!(fs::read(raw.join("state.json")).unwrap(), before);
}

#[test]
fn lists_python_cannot_read_fail_the_fetch() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    // 번호가 없는 이슈: 전체든 증분이든 오류이고 아무것도 쓰지 않아요
    let nameless = json!({"id": 7, "title": "x"});
    lists(
        &fake,
        vec![issue(1, "2026-10-20T00:00:00Z", 0, false), nameless],
        vec![],
        vec![],
    );
    let full = tmp.path().join("full");
    assert!(matches!(
        fetch(&gh, "o/n", &full),
        Err(SourceError::Json(_))
    ));
    assert_eq!(names(&full), Vec::<String>::new());
    let incremental = tmp.path().join("incremental");
    fs::create_dir(&incremental).unwrap();
    cursors_state(&incremental, all_cursors("2026-10-19T00:00:00Z"));
    assert!(matches!(
        fetch(&gh, "o/n", &incremental),
        Err(SourceError::Json(_))
    ));
    assert_eq!(names(&incremental), ["state.json"]);
    // 수가 아닌 댓글 수: 항목은 쓴 뒤에 오류이고 상태는 쓰지 않아요
    lists(
        &fake,
        vec![with(
            &issue(1, "2026-10-20T00:00:00Z", 0, false),
            &[("comments", json!("x"))],
        )],
        vec![],
        vec![],
    );
    let counted = tmp.path().join("counted");
    assert!(matches!(
        fetch(&gh, "o/n", &counted),
        Err(SourceError::Json(_))
    ));
    assert_eq!(names(&counted), ["items"]);
    assert_eq!(names(&counted.join("items")), ["issue-1.json"]);
}

#[test]
fn comment_counts_add_up_like_python() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    repo_ok(&fake);
    graphql_nodes(&fake, vec![]);
    // 댓글 목록이 비었을 때 이슈들의 수를 차례로 더해요: 0.5와 −0.5는 0이라 온전하고, true는 1이에요
    for (name, counts, whole) in [
        ("halves", vec![json!(0.5), json!(-0.5)], true),
        ("true", vec![json!(true)], false),
        ("none", vec![json!(null), json!(0), json!(false)], true),
    ] {
        let issues = counts
            .into_iter()
            .enumerate()
            .map(|(i, n)| {
                with(
                    &issue(i as u64 + 1, "2026-10-20T00:00:00Z", 0, false),
                    &[("comments", n)],
                )
            })
            .collect();
        lists(&fake, issues, vec![], vec![]);
        let raw = tmp.path().join(name);
        fetch(&gh, "o/n", &raw).unwrap();
        let st = load_state(&raw);
        assert_eq!(st["reconciled_at"] == st["fetched_at"], whole, "{name}");
    }
}
