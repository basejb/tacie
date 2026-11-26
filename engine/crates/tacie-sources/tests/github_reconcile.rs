//! Repo checks, document refetches, weekly reconcile and change checks. GitHub is answered by `FakeGitHub` (a
//! loopback fake server): fixed replies per route go in with `route`/`status`, and functions that pick a reply per
//! request go in with `status_with`.

mod common {
    pub mod github;
}

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use common::github::*;
use regex::Regex;
use serde_json::{Map, Value, json};
use tacie_index::pyjson;
use tacie_sources::error::{Kind, SourceError};
use tacie_sources::github::items::{STREAMS, blank_state, load_items};
use tacie_sources::github::probe::{LOST_AT, PROBES, changed, probe_repo};
use tacie_sources::github::reconcile::{MISSING_FLOOR, MISSING_LIMIT, Reconciled, reconcile};
use tacie_sources::github::refetch::{Talk, refetch_documents};
use tacie_sources::github::{GitHub, ISO, Timeouts, now_iso};
use tacie_sources::testing::github::{FakeGitHub, PanicGhToken, Reply};

fn numbers_of(items: &std::collections::BTreeMap<u64, Value>) -> Vec<u64> {
    items.keys().copied().collect()
}

fn issue_paths(numbers: impl IntoIterator<Item = u64>) -> Vec<String> {
    numbers
        .into_iter()
        .map(|n| format!("repos/o/n/issues/{n}"))
        .collect()
}

#[test]
fn constants_match_python() {
    assert_eq!(LOST_AT, 2);
    assert_eq!(MISSING_LIMIT, 0.2);
    assert_eq!(MISSING_FLOOR, 5);
    assert_eq!(ISO, "%Y-%m-%dT%H:%M:%SZ");
    assert_eq!(
        PROBES,
        [
            (
                "issues",
                "issues?state=all&sort=updated&direction=desc&per_page=1"
            ),
            (
                "comments",
                "issues/comments?sort=updated&direction=desc&per_page=1"
            ),
            (
                "review_comments",
                "pulls/comments?sort=updated&direction=desc&per_page=1"
            ),
        ]
    );
    assert_eq!(PROBES.map(|(s, _)| s), STREAMS);
    // 지금 시각, UTC ISO 글자예요
    let now = now_iso();
    assert!(
        Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
            .unwrap()
            .is_match(&now),
        "{now}"
    );
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let parsed = chrono::NaiveDateTime::parse_from_str(&now, ISO)
        .unwrap()
        .and_utc()
        .timestamp();
    assert!((secs - parsed).abs() <= 2, "{now} is not UTC now");
}

#[test]
fn reconcile_removes_moved_deleted_and_hidden() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let issues = numbers(raw, 1..=20);
    let by = by_number(&issues);
    let listed = issues
        .iter()
        .filter(|it| ![2, 3, 4, 5].contains(&it["number"].as_u64().unwrap()))
        .cloned()
        .collect();
    lists(&fake, listed, vec![], vec![]);
    fake.status(
        "repos/o/n/issues/2",
        410,
        &[],
        Some(json!({"message": "This issue was deleted"})),
    );
    // 숨김(또는 이 로그인에게 처음부터 안 보임)
    fake.status(
        "repos/o/n/issues/3",
        404,
        &[],
        Some(json!({"message": "Not Found"})),
    );
    // 옮겨짐: GitHub이 따라가게 해 줘요
    fake.status(
        "repos/o/n/issues/4",
        200,
        &[],
        Some(with(
            &by[&4],
            &[("repository_url", json!(home("o/elsewhere")))],
        )),
    );
    // 이 저장소, 글자만 다름
    fake.status(
        "repos/o/n/issues/5",
        200,
        &[],
        Some(with(&by[&5], &[("repository_url", json!(home("O/N")))])),
    );
    let five = age(&raw.join("items/issue-5.json"));
    let mut state = known_state();
    let got = reconcile(&gh, "o/n", raw, &mut state, Some("2026-10-24T00:00:00Z")).unwrap();
    assert_eq!(
        got,
        Reconciled {
            removed: vec![2, 3, 4],
            refetched: vec![],
            skipped: None
        }
    );
    // 20개 중 4개가 빠짐: 믿고, 하나씩 따로 물어요
    assert_eq!(status_asked(&fake), issue_paths(2..=5));
    let (items, bad) = load_items(raw).unwrap();
    assert_eq!(
        numbers_of(&items),
        [1].into_iter().chain(5..=20).collect::<Vec<_>>()
    );
    assert!(bad.is_empty());
    assert!(!raw.join("items/pull-3.json").exists());
    assert_eq!(mtime(&raw.join("items/issue-5.json")), five);
    assert!(asked_alone(&fake).is_empty());
    assert_eq!(state["reconciled_at"], "2026-10-24T00:00:00Z");
}

#[test]
fn reconcile_never_deletes_on_a_flaky_list() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let mut issues = numbers(raw, 1..=20);
    issues[0] = with(&issues[0], &[("comments", json!(1))]);
    let c = comment(11, 1, "2026-10-10T00:00:00Z", false);
    stored(raw, &issues[0], &[c], &[], Value::Null);
    age_all(&raw.join("items"));
    let files = listing(&raw.join("items"));
    // 빈 이슈 목록(세 번 물음)과, #1에 댓글이 있는데 빈 댓글 목록: 아무것도 지우거나 다시 받지 않아요
    lists(&fake, vec![], vec![], vec![]);
    let mut state = known_state();
    let got = reconcile(&gh, "o/n", raw, &mut state, None).unwrap();
    assert!(got.removed.is_empty() && got.refetched.is_empty());
    let why = got.skipped.unwrap();
    assert!(why.contains("issue list came back empty"), "{why}");
    assert!(why.contains("comment list came back empty"), "{why}");
    assert_eq!(
        why,
        "the issue list came back empty; the comment list came back empty"
    );
    assert_eq!(state["reconciled_at"], Value::Null);
    assert!(status_asked(&fake).is_empty() && asked_alone(&fake).is_empty());
    assert_eq!(listing(&raw.join("items")), files);
    // 빈 전체 목록은 세 번 물어요
    let thrice: Vec<String> = LISTS
        .iter()
        .flat_map(|p| [p.to_string(), p.to_string(), p.to_string()])
        .collect();
    assert_eq!(gh_paths(&fake), thrice);
}

#[test]
fn reconcile_refetches_docs_with_other_comment_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let t = "2026-10-10T00:00:00Z";
    let (five, six) = (issue(5, t, 2, false), issue(6, t, 10, false));
    let (c1, c2) = (comment(1, 5, t, false), comment(2, 5, t, false));
    let theirs: Vec<Value> = (3..=12).map(|i| comment(i, 6, t, false)).collect();
    stored(raw, &five, &[c1.clone(), c2.clone()], &[], Value::Null);
    stored(raw, &six, &theirs, &[], Value::Null);
    let edited = with(&c1, &[("body", json!("고친 댓글"))]);
    // c2는 지워졌어요: 전체 목록에 없고(12개 중 1개), #5의 목록에는 GitHub이 지금 가진 c1이 있어요
    let mut all = vec![c1.clone()];
    all.extend(theirs.iter().cloned());
    lists(
        &fake,
        vec![with(&five, &[("comments", json!(1))]), six.clone()],
        all,
        vec![],
    );
    fake.route(
        "repos/o/n/issues/5/comments",
        Reply::Json(json!([edited.clone()])),
    );
    assert_eq!(
        reconcile(&gh, "o/n", raw, &mut known_state(), None).unwrap(),
        Reconciled {
            removed: vec![],
            refetched: vec![5],
            skipped: None
        }
    );
    let (items, _) = load_items(raw).unwrap();
    // 통째로 다시 받았어요
    assert_eq!(items[&5]["comments"], json!([edited]));
    assert_eq!(items[&5]["issue"]["comments"], 1);
    assert_eq!(items[&6]["comments"], json!(theirs));
    assert_eq!(asked_alone(&fake), ["repos/o/n/issues/5/comments"]);

    // 전체 목록에 c2가 없어도 #5의 목록에 아직 있으면 c2는 남아요
    stored(raw, &five, &[c1.clone(), c2.clone()], &[], Value::Null);
    lists(
        &fake,
        vec![five.clone(), six.clone()],
        [vec![c1.clone()], theirs.clone()].concat(),
        vec![],
    );
    fake.route(
        "repos/o/n/issues/5/comments",
        Reply::Json(json!([c1.clone(), c2.clone()])),
    );
    fake.clear_calls();
    assert!(
        reconcile(&gh, "o/n", raw, &mut known_state(), None)
            .unwrap()
            .refetched
            .is_empty()
    );
    let ids: Vec<u64> = load_items(raw).unwrap().0[&5]["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [1, 2]);
    assert_eq!(asked_alone(&fake), ["repos/o/n/issues/5/comments"]);

    // 저장된 id가 MISSING_LIMIT과 MISSING_FLOOR보다 많이 빠짐(12개 중 6개): 어떤 문서도 묻지 않아요
    fake.route(
        LISTS[1],
        Reply::Json(json!([c1, c2, theirs[6], theirs[7], theirs[8], theirs[9]])),
    );
    fake.clear_calls();
    let mut state = known_state();
    let got = reconcile(&gh, "o/n", raw, &mut state, None).unwrap();
    assert!(got.refetched.is_empty());
    assert_eq!(
        got.skipped.as_deref(),
        Some("6 of 12 stored comments missing from the list")
    );
    assert_eq!(state["reconciled_at"], Value::Null);
    assert!(asked_alone(&fake).is_empty());
    assert!(status_asked(&fake).is_empty());
    assert_eq!(load_items(raw).unwrap().0[&6]["comments"], json!(theirs));
}

#[test]
fn reconcile_keeps_listed_and_refetches_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let by = by_number(&numbers(raw, (1..=6).chain(8..=12))); // #1–#12에서 #7만 없어요
    // 읽을 수 없어요: 목록에 예전 그대로 있어도 다시 받아요
    std::fs::write(raw.join("items/issue-8.json"), "{").unwrap();
    let newer = "2026-10-20T00:00:00Z";
    let mut listed = vec![
        with(&by[&6], &[("updated_at", json!(newer))]),
        issue(7, newer, 0, false),
    ];
    listed.extend([1, 2, 3, 8, 9, 10, 11, 12].map(|n| by[&n].clone()));
    lists(&fake, listed, vec![], vec![]);
    for n in [4, 6, 7, 8] {
        fake.route(
            &format!("repos/o/n/issues/{n}/comments"),
            Reply::Json(json!([])),
        );
    }
    // #4와 #5는 목록에 없지만(11개 중 2개) GitHub은 여기 있다고 해요
    fake.status(
        "repos/o/n/issues/4",
        200,
        &[],
        Some(with(
            &by[&4],
            &[
                ("updated_at", json!(newer)),
                ("repository_url", json!(home("o/n"))),
            ],
        )),
    );
    fake.status(
        "repos/o/n/issues/5",
        200,
        &[],
        Some(with(&by[&5], &[("repository_url", json!(home("o/n")))])),
    );
    let five = age(&raw.join("items/issue-5.json"));
    assert_eq!(
        reconcile(&gh, "o/n", raw, &mut known_state(), None).unwrap(),
        Reconciled {
            removed: vec![],
            refetched: vec![4, 6, 7, 8],
            skipped: None
        }
    );
    assert_eq!(status_asked(&fake), issue_paths([4, 5]));
    let (items, bad) = load_items(raw).unwrap();
    assert_eq!(numbers_of(&items), (1..=12).collect::<Vec<_>>());
    assert!(bad.is_empty());
    for n in [4, 6, 7] {
        assert_eq!(items[&n]["updated_at"], newer);
    }
    assert_eq!(mtime(&raw.join("items/issue-5.json")), five);
    // 이슈마다 목록에 온 것이나 따로 답한 것 그대로예요
    assert_eq!(
        asked_alone(&fake),
        [4, 6, 7, 8]
            .map(|n| format!("repos/o/n/issues/{n}/comments"))
            .to_vec()
    );
    assert_eq!(items[&4]["issue"]["repository_url"], home("o/n"));
}

#[test]
fn reconcile_checks_a_few_missing_in_a_small_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let issues = numbers(raw, [1, 2, 3]);
    lists(&fake, issues[..2].to_vec(), vec![], vec![]);
    fake.status(
        "repos/o/n/issues/3",
        404,
        &[],
        Some(json!({"message": "Not Found"})),
    );
    let mut state = known_state();
    // 3개 중 1개는 MISSING_LIMIT을 넘지만, MISSING_FLOOR개까지는 늘 하나씩 따로 물어요
    assert_eq!(
        reconcile(&gh, "o/n", raw, &mut state, None).unwrap(),
        Reconciled {
            removed: vec![3],
            refetched: vec![],
            skipped: None
        }
    );
    assert_eq!(status_asked(&fake), issue_paths([3]));
    assert_eq!(numbers_of(&load_items(raw).unwrap().0), [1, 2]);
    // 시작 시각이 없으면 지금(UTC)이에요
    let at = state["reconciled_at"].as_str().unwrap();
    assert!(
        chrono::NaiveDateTime::parse_from_str(at, ISO).is_ok(),
        "{at}"
    );
}

#[test]
fn reconcile_trusts_a_list_up_to_its_limit() {
    // 빠진 수가 max(MISSING_FLOOR, MISSING_LIMIT × 저장된 수)를 넘을 때만 믿지 않아요(같으면 믿어요)
    for (total, lacking, trusted) in [(25, 5, true), (25, 6, false), (40, 8, true), (40, 9, false)]
    {
        let tmp = tempfile::tempdir().unwrap();
        let raw = tmp.path();
        let fake = FakeGitHub::start();
        let gh = connect(&fake);
        let issues = numbers(raw, 1..=total);
        lists(&fake, issues[lacking as usize..].to_vec(), vec![], vec![]);
        for it in &issues[..lacking as usize] {
            let path = format!("repos/o/n/issues/{}", it["number"]);
            let here = with(it, &[("repository_url", json!(home("o/n")))]);
            fake.status(&path, 200, &[], Some(here));
        }
        let mut state = known_state();
        let got = reconcile(&gh, "o/n", raw, &mut state, Some("2026-10-24T00:00:00Z")).unwrap();
        assert!(got.removed.is_empty() && got.refetched.is_empty());
        if trusted {
            assert_eq!(got.skipped, None);
            assert_eq!(status_asked(&fake), issue_paths(1..=lacking));
            assert_eq!(state["reconciled_at"], "2026-10-24T00:00:00Z");
        } else {
            let why = format!("{lacking} of {total} stored issues missing from the list");
            assert_eq!(got.skipped, Some(why));
            assert!(status_asked(&fake).is_empty());
            assert_eq!(state["reconciled_at"], Value::Null);
        }
        assert!(asked_alone(&fake).is_empty());
    }
}

#[test]
fn reconcile_leaves_numbers_github_answered_otherwise() {
    // GitHub이 500으로 답한 #3은 그대로 둬요: 저장된 댓글 id가 목록과 달라도 다시 받지 않아요(이 저장소에 있는지
    // 모르니까요)
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let t = "2026-10-10T00:00:00Z";
    let issues = numbers(raw, [1, 2, 3]);
    let c11 = comment(11, 1, t, false);
    stored(
        raw,
        &issues[0],
        std::slice::from_ref(&c11),
        &[],
        Value::Null,
    );
    stored(
        raw,
        &issues[2],
        &[comment(31, 3, t, false)],
        &[],
        Value::Null,
    );
    let three = age(&raw.join("items/pull-3.json"));
    lists(&fake, issues[..2].to_vec(), vec![c11], vec![]);
    fake.status(
        "repos/o/n/issues/3",
        500,
        &[],
        Some(json!({"message": "x"})),
    );
    let mut state = known_state();
    assert_eq!(
        reconcile(&gh, "o/n", raw, &mut state, Some("2026-10-24T00:00:00Z")).unwrap(),
        Reconciled::default()
    );
    assert_eq!(status_asked(&fake), issue_paths([3]));
    assert!(asked_alone(&fake).is_empty());
    assert_eq!(mtime(&raw.join("items/pull-3.json")), three);
    assert_eq!(state["reconciled_at"], "2026-10-24T00:00:00Z");
}

#[test]
fn reconcile_fails_where_python_raises() {
    // 번호나 id를 읽을 수 없는 목록은 Err예요. 그 앞에서 GitHub이 지워졌다고 한 번호는 이미 지워졌고,
    // reconciled_at은 그대로예요.
    let comments_without_number = json!([{"id": 9}]);
    let comments_without_id = json!([{"issue_url": "https://api.github.com/repos/o/n/issues/1"}]);
    let review_with_bad_url =
        json!([{"id": 5, "pull_request_url": "https://api.github.com/repos/o/n/pulls/x"}]);
    for (comments, review, deleted) in [
        (comments_without_number, json!([]), true),
        (comments_without_id, json!([]), true),
        (json!([]), review_with_bad_url, true),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let raw = tmp.path();
        let fake = FakeGitHub::start();
        let gh = connect(&fake);
        let issues = numbers(raw, [1, 2, 3]);
        lists(&fake, issues[..2].to_vec(), vec![], vec![]);
        fake.route(LISTS[1], Reply::Json(comments));
        fake.route(LISTS[2], Reply::Json(review));
        fake.status("repos/o/n/issues/3", 404, &[], None);
        let mut state = known_state();
        let e = reconcile(&gh, "o/n", raw, &mut state, None).unwrap_err();
        assert!(matches!(e, SourceError::Json(_)), "{e:?}");
        assert_eq!(state["reconciled_at"], Value::Null);
        assert_eq!(raw.join("items/pull-3.json").exists(), !deleted);
    }
    // 번호가 없는 이슈: 아무것도 묻거나 지우기 전이에요
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let issues = numbers(raw, [1, 2, 3]);
    lists(
        &fake,
        vec![issues[0].clone(), issues[1].clone(), json!({"title": "x"})],
        vec![],
        vec![],
    );
    let mut state = known_state();
    assert!(matches!(
        reconcile(&gh, "o/n", raw, &mut state, None),
        Err(SourceError::Json(_))
    ));
    assert!(status_asked(&fake).is_empty());
    assert_eq!(numbers_of(&load_items(raw).unwrap().0), [1, 2, 3]);
    // 목록을 받지 못하면 gh의 오류 그대로예요
    fake.route(LISTS[0], Reply::Close);
    let e = reconcile(&gh, "o/n", raw, &mut state, None).unwrap_err();
    assert_eq!(e.kind(), Kind::Network);
    assert_eq!(state["reconciled_at"], Value::Null);
}

#[test]
fn refetch_documents_doubts_short_answers() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let (t, u) = ("2026-10-10T00:00:00Z", "2026-10-20T00:00:00Z");
    let (c1, c2, c3, c4) = (
        comment(1, 5, t, false),
        comment(2, 5, t, false),
        comment(3, 5, u, false),
        comment(4, 7, u, false),
    );
    let c1e = with(
        &c1,
        &[("body", json!("고친 댓글")), ("updated_at", json!(u))],
    );
    let (r1, r2) = (comment(21, 6, t, true), comment(22, 6, u, true));
    let (five, six, seven) = (
        issue(5, u, 3, false),
        issue(6, u, 0, true),
        issue(7, u, 1, false),
    );
    let extras = json!({"number": 6, "headRefName": "feat", "baseRefName": "dev", "mergedAt": null,
                        "files": {"totalCount": 0, "nodes": []}, "reviews": {"totalCount": 0, "nodes": []}});
    let (c81, c82, r91) = (
        comment(81, 8, t, false),
        comment(82, 8, u, false),
        comment(91, 9, t, true),
    );
    let (eight, nine) = (issue(8, u, 1, false), issue(9, u, 0, true));
    stored(
        raw,
        &issue(5, t, 2, false),
        &[c1.clone(), c2.clone()],
        &[],
        Value::Null,
    );
    stored(
        raw,
        &issue(6, t, 0, true),
        &[],
        std::slice::from_ref(&r1),
        extras.clone(),
    );
    stored(
        raw,
        &issue(8, t, 1, false),
        std::slice::from_ref(&c81),
        &[],
        Value::Null,
    );
    stored(
        raw,
        &issue(9, t, 0, true),
        &[],
        std::slice::from_ref(&r91),
        Value::Null,
    );
    // #5: c3(since 목록이 가져옴)이 없는 답이 두 번 → 저장된 것, 목록의 것, 답을 id로 합쳐요. 답이 이기고(목록의 옛
    // c1이 아니라 고친 c1), 그다음 목록이에요
    fake.route_seq(
        "repos/o/n/issues/5/comments",
        vec![Reply::Json(json!([c1e, c2])), Reply::Json(json!([c1e, c2]))],
    );
    // #6: 댓글 0개는 보통의 답, 리뷰 댓글은 처음엔 빈 답(저장된 것이 있고 r2가 없음) → 다시 물어 온전한 답
    fake.route("repos/o/n/issues/6/comments", Reply::Json(json!([])));
    fake.route_seq(
        "repos/o/n/pulls/6/comments",
        vec![Reply::Json(json!([])), Reply::Json(json!([r1, r2]))],
    );
    // #7: 목록에 없어 이슈를 따로 묻고, GitHub이 센 1개보다 적은 답 → 다시 물어 온전한 답
    fake.route("repos/o/n/issues/7", Reply::Json(seven.clone()));
    fake.route_seq(
        "repos/o/n/issues/7/comments",
        vec![Reply::Json(json!([])), Reply::Json(json!([c4]))],
    );
    // #8: 수(1)는 맞지만 since 목록의 c82가 없는 답 → 다시 물어요
    fake.route_seq(
        "repos/o/n/issues/8/comments",
        vec![Reply::Json(json!([c81])), Reply::Json(json!([c81, c82]))],
    );
    // #9: 수가 없는 리뷰 댓글이 저장된 것이 있는데 비어 옴 → 다시 물어요
    fake.route("repos/o/n/issues/9/comments", Reply::Json(json!([])));
    fake.route_seq(
        "repos/o/n/pulls/9/comments",
        vec![Reply::Json(json!([])), Reply::Json(json!([r91]))],
    );
    let listed: HashMap<u64, Value> = [
        (5, five.clone()),
        (6, six.clone()),
        (8, eight.clone()),
        (9, nine.clone()),
    ]
    .into();
    let since: HashMap<u64, Talk> = [
        (
            5,
            Talk {
                comments: vec![c1.clone(), c3.clone()],
                review_comments: vec![],
            },
        ),
        (
            6,
            Talk {
                comments: vec![],
                review_comments: vec![r2.clone()],
            },
        ),
        (
            8,
            Talk {
                comments: vec![c82.clone()],
                review_comments: vec![],
            },
        ),
    ]
    .into();
    let asked: BTreeSet<u64> = [9, 8, 7, 6, 5].into();
    let (written, prs) = refetch_documents(&gh, "o/n", raw, &asked, &listed, Some(&since)).unwrap();
    assert_eq!(written, [5, 6, 7, 8, 9].into());
    assert_eq!(prs, [6, 9].into());
    assert_eq!(
        gh_paths(&fake),
        [
            "repos/o/n/issues/5/comments",
            "repos/o/n/issues/5/comments",
            "repos/o/n/issues/6/comments",
            "repos/o/n/pulls/6/comments",
            "repos/o/n/pulls/6/comments",
            "repos/o/n/issues/7",
            "repos/o/n/issues/7/comments",
            "repos/o/n/issues/7/comments",
            "repos/o/n/issues/8/comments",
            "repos/o/n/issues/8/comments",
            "repos/o/n/issues/9/comments",
            "repos/o/n/pulls/9/comments",
            "repos/o/n/pulls/9/comments",
        ]
    );
    let (items, _) = load_items(raw).unwrap();
    assert_eq!(items[&5]["comments"], json!([c1e, c2, c3]));
    assert_eq!(items[&5]["issue"], five);
    assert_eq!(items[&6]["kind"], "pull");
    assert_eq!(items[&6]["comments"], json!([]));
    assert_eq!(items[&6]["review_comments"], json!([r1, r2]));
    assert_eq!(items[&6]["extras"], extras); // 저장된 PR 추가 정보는 그대로
    assert_eq!(items[&7]["comments"], json!([c4]));
    assert_eq!(items[&7]["issue"], seven);
    assert_eq!(items[&7]["extras"], Value::Null);
    assert_eq!(items[&8]["comments"], json!([c81, c82]));
    assert_eq!(items[&9]["comments"], json!([]));
    assert_eq!(items[&9]["review_comments"], json!([r91]));
    let keys: Vec<&str> = items[&7]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "number",
            "kind",
            "updated_at",
            "issue",
            "comments",
            "review_comments",
            "extras"
        ]
    );

    // 같은 답이면 다시 쓰지 않아요(마지막 답을 되풀이해요)
    fake.clear_calls();
    let (written, prs) = refetch_documents(&gh, "o/n", raw, &asked, &listed, Some(&since)).unwrap();
    assert!(written.is_empty());
    assert_eq!(prs, [6, 9].into());
    assert_eq!(
        gh_paths(&fake),
        [
            "repos/o/n/issues/5/comments",
            "repos/o/n/issues/5/comments",
            "repos/o/n/issues/6/comments",
            "repos/o/n/pulls/6/comments",
            "repos/o/n/issues/7",
            "repos/o/n/issues/7/comments",
            "repos/o/n/issues/8/comments",
            "repos/o/n/issues/9/comments",
            "repos/o/n/pulls/9/comments",
        ]
    );
    // since가 없고 아무것도 묻지 않으면 gh를 띄우지 않아요
    fake.clear_calls();
    let none = refetch_documents(&gh, "o/n", raw, &BTreeSet::new(), &listed, None).unwrap();
    assert_eq!(none, (BTreeSet::new(), BTreeSet::new()));
    assert!(fake.calls().is_empty());
}

/// 답의 머리예요.
type Headers = &'static [(&'static str, &'static str)];

#[test]
fn rate_limited_probe_is_busy() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let limited = json!({"message": "API rate limit exceeded for user ID 1."});
    let cases: [(Headers, Value, Kind); 4] = [
        (
            &[("x-ratelimit-remaining", "0")],
            json!({"message": "x"}),
            Kind::Busy,
        ),
        (&[], limited.clone(), Kind::Busy),
        (
            &[],
            json!({"message": "You have exceeded a secondary rate limit."}),
            Kind::Busy,
        ),
        (
            &[("x-ratelimit-remaining", "4999")],
            json!({"message": "Resource not accessible by integration"}),
            Kind::Key,
        ),
    ];
    for (head, body, kind) in cases {
        fake.status("repos/o/n", 403, head, Some(body));
        let mut state = known_state();
        let e = probe_repo(&gh, "o/n", &mut state).unwrap_err();
        assert_eq!(e.kind(), kind);
        assert_eq!(e.to_string(), "GitHub answered 403 for the repo");
        assert_eq!(
            pyjson::to_string(&Value::Object(state), None),
            pyjson::to_string(&Value::Object(known_state()), None)
        );
    }
    // 재조정은 호출 제한에 걸린 답에서 멈춰요(fetch가 다음에 다시 해요). 그 밖의 403은 항목을 그대로 둬요
    let issues = numbers(raw, [1, 2, 3]);
    lists(&fake, issues[..2].to_vec(), vec![], vec![]);
    fake.status(
        "repos/o/n/issues/3",
        403,
        &[("x-ratelimit-remaining", "0")],
        Some(limited),
    );
    let mut state = known_state();
    match reconcile(&gh, "o/n", raw, &mut state, None) {
        Err(SourceError::Service { kind, message }) => {
            assert_eq!(kind, Kind::Busy);
            assert_eq!(message, "GitHub's rate limit stopped the reconcile");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(state["reconciled_at"], Value::Null);
    assert_eq!(numbers_of(&load_items(raw).unwrap().0), [1, 2, 3]);
    fake.status(
        "repos/o/n/issues/3",
        403,
        &[],
        Some(json!({"message": "Must have admin rights to Repository."})),
    );
    assert!(
        reconcile(&gh, "o/n", raw, &mut state, None)
            .unwrap()
            .removed
            .is_empty()
    );
    assert_eq!(numbers_of(&load_items(raw).unwrap().0), [1, 2, 3]);
}

/// 요청마다 (길과 쿼리, 보낸 If-None-Match)예요.
fn probe_calls(fake: &FakeGitHub) -> Vec<(String, Option<String>)> {
    fake.calls()
        .iter()
        .map(|c| {
            let path = if c.query.is_empty() {
                c.path.clone()
            } else {
                format!("{}?{}", c.path, c.query)
            };
            (path, c.if_none_match.clone())
        })
        .collect()
}

/// 이 상태, 머리, 몸통의 답이에요.
fn answer(code: u16, headers: &[(&str, &str)], body: &str) -> Reply {
    Reply::Http {
        code,
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: body.as_bytes().to_vec(),
    }
}

#[test]
fn changed_uses_etag_and_top() {
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let (cur, newer) = ("2026-10-24T10:00:00Z", "2026-10-24T11:00:00Z");
    let mut state = blank_state();
    state.insert(
        "cursors".into(),
        json!({"issues": cur, "comments": cur, "review_comments": cur}),
    );
    state.insert(
        "etags".into(),
        Value::Object(
            STREAMS
                .iter()
                .map(|s| {
                    (
                        s.to_string(),
                        json!({"etag": format!("W/\"{s}\""), "top": cur}),
                    )
                })
                .collect(),
        ),
    );
    let kept = state.clone();
    let probes: Vec<String> = PROBES
        .iter()
        .map(|(_, p)| format!("repos/o/n/{p}"))
        .collect();
    // 목록마다 저장된 etag로 물어요: 304는 저장된 top을 써요(여기서는 커서)
    fake.status_with(move |_, _| answer(304, &[("Etag", "x")], ""));
    assert_eq!(changed(&gh, "o/n", &state), (Some(false), Map::new()));
    let want: Vec<_> = probes
        .iter()
        .zip(STREAMS)
        .map(|(p, s)| (p.clone(), Some(format!("W/\"{s}\""))))
        .collect();
    assert_eq!(probe_calls(&fake), want);
    // 저장된 top이 커서보다 뒤면 304여도 바뀐 것이고, 저장할 새 etag는 없어요
    let mut ahead = state.clone();
    ahead["etags"]["comments"]["top"] = json!(newer);
    assert_eq!(changed(&gh, "o/n", &ahead), (Some(true), Map::new()));

    // 이슈 목록의 가장 새 항목이 커서보다 뒤: 바뀜, 그 목록의 새 etag와 top을 저장하라고 줘요
    fake.status_with(move |path, _| {
        if path.contains("/issues?") {
            answer(
                200,
                &[("ETag", "W/\"new\"")],
                &format!("[{{\"number\": 3, \"updated_at\": \"{newer}\"}}]"),
            )
        } else {
            answer(304, &[], "")
        }
    });
    let (found, etags) = changed(&gh, "o/n", &state);
    assert_eq!(found, Some(true));
    assert_eq!(
        Value::Object(etags),
        json!({"issues": {"etag": "W/\"new\"", "top": newer}})
    );

    // 저장된 etag가 없으면 모두 200: 커서와 같은 top은 새것이 아니고, 빈 목록은 top이 없어요
    let mut blank = state.clone();
    blank.insert("etags".into(), json!({}));
    fake.clear_calls();
    fake.status_with(move |path, _| {
        let body = if path.contains("/pulls/") {
            "[]".to_string()
        } else {
            format!("[{{\"updated_at\": \"{cur}\"}}]")
        };
        answer(200, &[("ETag", "\"e\"")], &body)
    });
    let (found, etags) = changed(&gh, "o/n", &blank);
    assert_eq!(found, Some(false));
    // 키 순서까지 고정이에요
    assert_eq!(
        pyjson::to_string(&Value::Object(etags), None),
        pyjson::to_string(
            &json!({"issues": {"etag": "\"e\"", "top": cur}, "comments": {"etag": "\"e\"", "top": cur},
                    "review_comments": {"etag": "\"e\"", "top": null}}),
            None
        )
    );
    assert!(probe_calls(&fake).iter().all(|(_, etag)| etag.is_none()));
    assert_eq!(probe_calls(&fake).len(), 3);

    // 다 묻지 못함: GitHub에 닿지 못했거나, 너무 느렸거나, GitHub이 다른 답을 했어요
    fake.status_with(|_, _| Reply::Close);
    assert_eq!(changed(&gh, "o/n", &state), (None, Map::new()));
    for status in [502, 403, 404] {
        fake.status_with(move |_, _| answer(status, &[], ""));
        assert_eq!(changed(&gh, "o/n", &state), (None, Map::new()));
    }
    // 느린 답도 다 묻지 못한 거예요: 확인의 시간 제한(`Timeouts::check`)에서 끊어요
    let quick = connect(&fake).with_timeouts(Timeouts {
        check: Duration::from_millis(200),
        ..Timeouts::default()
    });
    fake.status_with(|_, _| {
        Reply::Slow(
            Duration::from_secs(2),
            Box::new(answer(200, &[("ETag", "\"s\"")], "[]")),
        )
    });
    let start = std::time::Instant::now();
    assert_eq!(changed(&quick, "o/n", &state), (None, Map::new()));
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    // 앞 목록이 바뀌었어도 뒤 목록이 실패하면 아무것도 돌려주지 않아요
    fake.status_with(move |path, _| {
        if path.contains("/pulls/") {
            answer(502, &[], "")
        } else {
            answer(
                200,
                &[("ETag", "\"f\"")],
                &format!("[{{\"updated_at\": \"{newer}\"}}]"),
            )
        }
    });
    assert_eq!(changed(&gh, "o/n", &state), (None, Map::new()));
    // 연결이 없으면 아무것도 띄우지 않아요
    let none = GitHub::new("none", &fake.url(), Arc::new(PanicGhToken)).unwrap();
    fake.clear_calls();
    assert_eq!(changed(&none, "o/n", &state), (None, Map::new()));
    assert!(fake.calls().is_empty());
    assert_eq!(state, kept); // 받은 상태를 바꾸지 않아요
    assert_eq!(
        pyjson::to_string(&Value::Object(state), None),
        pyjson::to_string(&Value::Object(kept), None)
    );
}

#[test]
fn probe_repo_counts_and_kinds() {
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let order = |s: &Map<String, Value>| s.keys().cloned().collect::<Vec<_>>();
    let blank_order = order(&blank_state());
    // 처음 본 저장소: id와 지금 이름을 두고 lost는 0이에요
    let mut state = blank_state();
    fake.status(
        "repos/o/n",
        200,
        &[],
        Some(json!({"id": 1, "full_name": "o/n"})),
    );
    assert!(probe_repo(&gh, "o/n", &mut state).unwrap());
    assert_eq!(
        (&state["repo_id"], &state["full_name"], &state["lost"]),
        (&json!(1), &json!("o/n"), &json!(0))
    );
    assert_eq!(order(&state), blank_order); // 제자리에서 바꿔요
    assert_eq!(probe_calls(&fake), [("repos/o/n".to_string(), None)]);

    // 404와 410은 하나씩 세요
    fake.status("repos/o/n", 404, &[], Some(json!({"message": "Not Found"})));
    assert!(!probe_repo(&gh, "o/n", &mut state).unwrap());
    assert_eq!(state["lost"], 1);
    // GitHub의 답이 없는 것은 404가 아니에요: 상태는 그대로예요
    fake.status_seq("repos/o/n", vec![Reply::Close]);
    let before = state.clone();
    assert_eq!(
        probe_repo(&gh, "o/n", &mut state).unwrap_err().kind(),
        Kind::Network
    );
    assert_eq!(state, before);
    fake.status("repos/o/n", 410, &[], Some(json!({"message": "Gone"})));
    assert!(!probe_repo(&gh, "o/n", &mut state).unwrap());
    assert_eq!(state["lost"], 2);
    assert_eq!(state["repo_id"], 1);

    // 200은 lost를 0으로 돌려요
    fake.status(
        "repos/o/n",
        200,
        &[],
        Some(json!({"id": 1, "full_name": "o/n"})),
    );
    assert!(probe_repo(&gh, "o/n", &mut state).unwrap());
    assert_eq!(state["lost"], 0);

    // 다른 id: 그 이름이 이제 다른 저장소예요. 바로 닫고(lost 2) 처음 id를 지켜요
    fake.status(
        "repos/o/n",
        200,
        &[],
        Some(json!({"id": 2, "full_name": "o/n"})),
    );
    assert!(!probe_repo(&gh, "o/n", &mut state).unwrap());
    assert_eq!(
        (&state["lost"], &state["repo_id"]),
        (&json!(LOST_AT), &json!(1))
    );
    state["lost"] = json!(5); // 이미 더 많이 셌으면 그대로예요(max)
    assert!(!probe_repo(&gh, "o/n", &mut state).unwrap());
    assert_eq!(state["lost"], 5);
    state["lost"] = json!(2);

    // 그 밖의 답은 오류이고 상태는 그대로예요(저장된 것을 써요)
    for (status, kind) in [
        (401, Kind::Key),
        (403, Kind::Key),
        (429, Kind::Busy),
        (502, Kind::Down),
    ] {
        fake.status("repos/o/n", status, &[], Some(json!({"message": "x"})));
        let before = state.clone();
        let e = probe_repo(&gh, "o/n", &mut state).unwrap_err();
        assert_eq!(e.kind(), kind, "{status}");
        assert_eq!(
            e.to_string(),
            format!("GitHub answered {status} for the repo")
        );
        assert_eq!(state, before);
        assert_eq!(state["lost"], 2);
    }

    // 정수 id가 없는 200은 down이에요
    for body in [
        Some(json!({"full_name": "o/n"})),
        Some(json!({"id": "1"})),
        Some(json!({"id": 1.0})),
        Some(json!([{"id": 1}])),
        None,
    ] {
        fake.status("repos/o/n", 200, &[], body);
        let before = state.clone();
        match probe_repo(&gh, "o/n", &mut state) {
            Err(SourceError::Service { kind, message }) => {
                assert_eq!(kind, Kind::Down);
                assert_eq!(message, "GitHub's answer for the repo has no id");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(state, before);
    }

    // 이름이 비어 오면 물은 이름이에요
    let mut fresh = blank_state();
    fake.status(
        "repos/o/m",
        200,
        &[],
        Some(json!({"id": 7, "full_name": ""})),
    );
    assert!(probe_repo(&gh, "o/m", &mut fresh).unwrap());
    assert_eq!(
        (&fresh["repo_id"], &fresh["full_name"]),
        (&json!(7), &json!("o/m"))
    );
    // 저장된 lost가 null이면 0부터 세요
    fresh["lost"] = Value::Null;
    fake.status("repos/o/m", 404, &[], None);
    assert!(!probe_repo(&gh, "o/m", &mut fresh).unwrap());
    assert_eq!(fresh["lost"], 1);
    // 더 셀 수 없는 수는 그대로 둬요(넘쳐서 패닉하지 않아요)
    fresh["lost"] = json!(i64::MAX);
    assert!(!probe_repo(&gh, "o/m", &mut fresh).unwrap());
    assert_eq!(fresh["lost"], json!(i64::MAX));

    // 연결이 없으면 띄우지 않고 key예요
    let none = GitHub::new("none", &fake.url(), Arc::new(PanicGhToken)).unwrap();
    fake.clear_calls();
    let before = state.clone();
    assert_eq!(
        probe_repo(&none, "o/n", &mut state).unwrap_err().kind(),
        Kind::Key
    );
    assert!(fake.calls().is_empty());
    assert_eq!(state, before);
}

/// R11: 진짜 저장소 기준(12개)으로는 보지 못하는 큰 목록이에요. 전체 이슈 목록이 여러 쪽(`Link`)으로 와도 모두
/// 읽고, 목록에 없는 번호만 하나씩 물어 GitHub이 그렇다고 한 것(404, 410, 옮겨짐)만 지워요.
#[test]
fn reconcile_deletes_across_list_pages() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let issues = numbers(raw, 1..=250);
    let gone = [100, 201, 250];
    let listed: Vec<Value> = issues
        .iter()
        .filter(|it| !gone.contains(&it["number"].as_u64().unwrap()))
        .cloned()
        .collect();
    assert_eq!(listed.len(), 247); // 100 + 100 + 47, 세 쪽이에요
    lists(&fake, listed, vec![], vec![]);
    fake.status(
        "repos/o/n/issues/100",
        404,
        &[],
        Some(json!({"message": "Not Found"})),
    );
    fake.status(
        "repos/o/n/issues/201",
        410,
        &[],
        Some(json!({"message": "Gone"})),
    );
    fake.status(
        "repos/o/n/issues/250",
        200,
        &[],
        Some(with(
            &issues[249],
            &[("repository_url", json!(home("o/elsewhere")))],
        )),
    );
    let mut state = known_state();
    let got = reconcile(&gh, "o/n", raw, &mut state, Some("2026-10-26T00:00:00Z")).unwrap();
    assert_eq!(
        got,
        Reconciled {
            removed: gone.to_vec(),
            refetched: vec![],
            skipped: None
        }
    );
    assert_eq!(state["reconciled_at"], "2026-10-26T00:00:00Z");
    let (items, bad) = load_items(raw).unwrap();
    assert!(bad.is_empty());
    assert_eq!(items.len(), 247);
    assert!(gone.iter().all(|n| !items.contains_key(n)));
    assert_eq!(status_asked(&fake), issue_paths(gone));
    // 이슈 목록은 첫 쪽 하나와 GitHub이 준 다음 쪽 주소 둘이에요
    let pages: Vec<String> = fake
        .calls()
        .iter()
        .filter(|c| c.path.ends_with("/issues"))
        .map(|c| c.path.clone())
        .collect();
    assert_eq!(
        pages,
        [
            "repos/o/n/issues",
            "repositories/1/issues",
            "repositories/1/issues"
        ]
    );
}
