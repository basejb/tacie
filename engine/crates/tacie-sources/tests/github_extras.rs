//! Extra PR info. GitHub is answered by `FakeGitHub::graphql` (a loopback fake server). Query text is fixed down to
//! the byte (github_cases.json). They are fixed values made by the old Python engine (2026-10-26).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use regex::Regex;
use serde_json::{Map, Value, json};
use tacie_index::pyjson;
use tacie_sources::error::{Kind, SourceError};
use tacie_sources::github::GRAPHQL_TRIES;
use tacie_sources::github::GitHub;
use tacie_sources::github::extras::{NESTED_PAGE, PR_BATCH, incomplete, pr_extras, update_extras};
use tacie_sources::github::items::{load_items, save_item};
use tacie_sources::testing::github::{FakeGitHub, PanicGhToken};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// 가짜의 토큰을 가진 token 모드 연결이에요(gh는 묻지 않아요).
fn connect(fake: &FakeGitHub) -> GitHub {
    let gh = GitHub::new("none", &fake.url(), Arc::new(PanicGhToken))
        .unwrap()
        .with_pause(|_| {});
    gh.set_credentials("token", Some(FakeGitHub::TOKEN));
    gh
}

/// 보낸 GraphQL 질의예요(차례대로).
fn queries(fake: &FakeGitHub) -> Vec<String> {
    fake.calls()
        .iter()
        .filter(|c| c.method == "POST" && c.path == "graphql")
        .map(|c| {
            c.body.as_ref().unwrap()["query"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

/// 질의가 묻는 PR 번호예요.
fn asked(query: &str) -> Vec<u64> {
    let re = Regex::new(r"p(\d+): pullRequest").unwrap();
    re.captures_iter(query)
        .map(|m| m[1].parse().unwrap())
        .collect()
}

/// `{"data": {"repository": aliases}}`예요.
fn data(aliases: Value) -> Value {
    json!({"data": {"repository": aliases}})
}

fn page(
    count: usize,
    total: Option<u64>,
    more: Option<&str>,
    make: impl Fn(usize) -> Value,
) -> Value {
    json!({"totalCount": total.unwrap_or(count as u64),
           "pageInfo": {"hasNextPage": more.is_some(), "endCursor": more},
           "nodes": (0..count).map(make).collect::<Vec<_>>()})
}

/// GitHub이 주는 pullRequest 노드 하나예요(한 쪽, pageInfo 있음).
fn node_with(
    n: u64,
    files: usize,
    reviews: usize,
    total_files: Option<u64>,
    total_reviews: Option<u64>,
    more_files: Option<&str>,
    more_reviews: Option<&str>,
) -> Value {
    json!({"number": n, "headRefName": "feat", "baseRefName": "dev", "mergedAt": null,
           "files": page(files, total_files, more_files, |i| json!({"path": format!("f{i}.ts")})),
           "reviews": page(reviews, total_reviews, more_reviews, |i| json!({
               "author": {"login": "jun-h"}, "body": format!("r{i}"), "state": "APPROVED",
               "submittedAt": "2026-10-01T00:00:00Z", "url": format!("https://github.com/o/n/pull/{n}#r{i}")}))})
}

fn node(n: u64, files: usize, reviews: usize) -> Value {
    node_with(n, files, reviews, None, None, None, None)
}

fn files_page(total: u64, next: Option<&str>, paths: impl Iterator<Item = String>) -> Value {
    json!({"totalCount": total, "pageInfo": {"hasNextPage": next.is_some(), "endCursor": next},
           "nodes": paths.map(|p| json!({"path": p})).collect::<Vec<_>>()})
}

fn pull_issue(n: u64) -> Value {
    json!({"id": 1000 + n, "number": n, "title": format!("제목 {n}"), "body": "설명", "user": {"login": "sora-p"},
           "state": "open", "labels": [], "created_at": "2026-10-01T00:00:00Z",
           "updated_at": "2026-10-01T00:00:00Z", "comments": 0,
           "html_url": format!("https://github.com/o/n/pull/{n}"), "pull_request": {"merged_at": null}})
}

/// 저장된 항목 하나예요.
fn stored(raw: &Path, issue: Value, extras: Value) {
    let kind = if issue.get("pull_request").is_some() {
        "pull"
    } else {
        "issue"
    };
    save_item(
        raw,
        &json!({"number": issue["number"], "kind": kind, "updated_at": issue["updated_at"], "issue": issue,
                "comments": [], "review_comments": [], "extras": extras}),
    )
    .unwrap();
}

#[test]
fn graphql_sends_owner_and_name_as_variables() {
    assert_eq!((PR_BATCH, NESTED_PAGE, GRAPHQL_TRIES), (20, 100, 3));
    // gh `-f owner=… -f name=…`가 보내던 모양 그대로예요: 본문은 {"query", "variables": {"owner", "name"}}
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    fake.graphql(|_, vars| {
        assert_eq!(*vars, json!({"owner": "o", "name": "n"}));
        (200, data(json!({"p1": node(1, 1, 0)})))
    });
    assert!(pr_extras(&gh, "o/n", &[1]).unwrap()[&1].is_some());
    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    let c = &calls[0];
    assert_eq!(
        (c.method.as_str(), c.path.as_str(), c.query.as_str()),
        ("POST", "graphql", "")
    );
    assert!(c.has_token);
    let body = c.body.as_ref().unwrap().as_object().unwrap();
    assert_eq!(body.keys().collect::<Vec<_>>(), ["query", "variables"]);
    assert_eq!(body["variables"], json!({"owner": "o", "name": "n"}));
    assert_eq!(
        body["variables"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["owner", "name"]
    );
}

#[test]
fn pr_extras_batches_and_pages() {
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    fake.graphql(|query, _| {
        let asked = asked(query);
        if query.contains("after:") {
            assert!(asked == [655] && !query.contains("reviews(") && query.contains("after: \"c1\""));
            return (200, data(json!({"p655": {"files": files_page(130, None, (0..30).map(|i| format!("g{i}.ts")))}})),
            );
        }
        assert!(asked.len() <= 20);
        let aliases: Map<String, Value> = asked
            .iter()
            .map(|&n| {
                let v = if n == 655 {
                    node_with(n, 100, 0, Some(130), None, Some("c1"), None)
                } else {
                    node(n, 1, 1)
                };
                (format!("p{n}"), v)
            })
            .collect();
        (200, data(Value::Object(aliases)))
    });
    let mut numbers: Vec<u64> = (1..45).collect();
    numbers.push(655);
    let got = pr_extras(&gh, "o/n", &numbers).unwrap();
    let sent = queries(&fake);
    let batches: Vec<Vec<u64>> = sent
        .iter()
        .filter(|q| !q.contains("after:"))
        .map(|q| asked(q))
        .collect();
    assert_eq!(
        batches,
        [
            (1..21).collect::<Vec<_>>(),
            (21..41).collect(),
            vec![41, 42, 43, 44, 655]
        ]
    );
    assert_eq!(sent.len(), 4); // 배치 셋과 #655만의 다음 쪽 하나
    let files = got[&655].as_ref().unwrap()["files"].as_object().unwrap();
    assert_eq!(files["nodes"].as_array().unwrap().len(), 130);
    assert_eq!(files["totalCount"], 130);
    assert_eq!(files.keys().collect::<Vec<_>>(), ["totalCount", "nodes"]); // pageInfo는 빠져요
    let reviews = got[&3].as_ref().unwrap()["reviews"].as_object().unwrap();
    assert!(!reviews.contains_key("pageInfo"));
    assert_eq!(reviews["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(got.len(), 45);

    // 다음 쪽이 없다고 하면 totalCount가 뭐라든 멈춰요
    fake.clear_calls();
    fake.graphql(|_, _| {
        (
            200,
            data(json!({"p9": node_with(9, 60, 0, Some(488), None, None, None)})),
        )
    });
    let got = pr_extras(&gh, "o/n", &[9]).unwrap();
    assert_eq!(
        got[&9].as_ref().unwrap()["files"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        60
    );
    assert_eq!(queries(&fake).len(), 1);
}

#[test]
fn pr_extras_queries_match_python_golden() {
    // 질의 글자(배치는 " "로 잇고, 다음 쪽 커서는 Python json.dumps의 ASCII 글자)와 돌려준 값이 golden과 같아요
    const CURSOR: &str = "c1\"é\u{2028}😀\u{7f}\\";
    let cases: Value = serde_json::from_str(
        &fs::read_to_string(Path::new(FIXTURES).join("github_cases.json")).unwrap(),
    )
    .unwrap();
    let golden = &cases["pr_extras"];
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    fake.graphql(|query, _| {
        let aliases = if query.contains("after:") {
            if query.contains("files(") {
                json!({"p655": {"files": files_page(130, None, (0..30).map(|i| format!("g{i}.ts")))}})
            } else {
                json!({"p655": {"reviews": {"totalCount": 2, "pageInfo": {"hasNextPage": false, "endCursor": "r2"},
                                            "nodes": [{"author": null, "body": "late", "state": "COMMENTED",
                                                       "submittedAt": "2026-10-02T00:00:00Z", "url": "https://x/r"}]}}})
            }
        } else {
            Value::Object(
                asked(query)
                    .into_iter()
                    .map(|n| {
                        let v = match n {
                            655 => node_with(n, 100, 1, Some(130), Some(2), Some(CURSOR), Some("r1")),
                            13 => Value::Null,
                            _ => node(n, 1, 1),
                        };
                        (format!("p{n}"), v)
                    })
                    .collect(),
            )
        };
        (200, data(aliases))
    });
    let numbers: Vec<u64> = golden["numbers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap())
        .collect();
    let got = pr_extras(&gh, "o/n", &numbers).unwrap();
    let want: Vec<&str> = golden["queries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q.as_str().unwrap())
        .collect();
    assert_eq!(queries(&fake), want);
    for c in fake.calls() {
        assert!(c.has_token);
        assert_eq!(
            c.body.as_ref().unwrap()["variables"],
            json!({"owner": "o", "name": "n"})
        );
    }
    let extras = golden["extras"].as_object().unwrap();
    assert_eq!(
        got.keys().map(|n| n.to_string()).collect::<Vec<_>>(),
        extras.keys().cloned().collect::<Vec<_>>()
    );
    assert_eq!(got[&13], None);
    for (n, x) in &got {
        let x = x.clone().unwrap_or(Value::Null);
        let want = &extras[&n.to_string()];
        assert_eq!(&x, want, "#{n}");
        assert_eq!(
            pyjson::to_string(&x, None),
            pyjson::to_string(want, None),
            "#{n}: key order"
        );
    }
}

#[test]
fn graphql_partial_errors_keep_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let raw = tmp.path();
    for n in [1, 2] {
        stored(raw, pull_issue(n), node(n, 2, 0));
    }
    let mut not_pull = pull_issue(3);
    not_pull
        .as_object_mut()
        .unwrap()
        .shift_remove("pull_request");
    stored(raw, not_pull, Value::Null); // 이슈는 묻지 않아요
    let partial = json!({"data": {"repository": {"p1": node(1, 3, 0), "p2": null}},
                         "errors": [{"type": "NOT_FOUND", "path": ["repository", "p2"]}]});
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let answer = partial.clone();
    fake.graphql(move |_, _| (200, answer.clone())); // 부분 오류도 200이에요
    let written = update_extras(&gh, "o/n", raw, &BTreeSet::from([1, 2, 3, 4])).unwrap();
    assert_eq!(written, BTreeSet::from([1]));
    let sent = queries(&fake);
    assert_eq!(sent.len(), 1);
    assert_eq!(asked(&sent[0]), [1, 2]); // 저장되지 않은 #4와 이슈 #3은 묻지 않아요
    let (items, _) = load_items(raw).unwrap();
    assert_eq!(
        items[&1]["extras"]["files"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(items[&2]["extras"], node(2, 2, 0)); // null로 답한 PR은 저장된 그대로예요
    let keys: Vec<&str> = items[&1]
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

    // 같은 답이면 아무것도 쓰지 않아요. 물을 PR이 없으면 묻지 않아요
    fake.clear_calls();
    assert!(
        update_extras(&gh, "o/n", raw, &BTreeSet::from([1, 2]))
            .unwrap()
            .is_empty()
    );
    assert_eq!(queries(&fake).len(), 1);
    fake.clear_calls();
    assert!(
        update_extras(&gh, "o/n", raw, &BTreeSet::from([3, 4]))
            .unwrap()
            .is_empty()
    );
    assert!(fake.calls().is_empty());

    // data가 없으면 2×(n+1) s 쉬고 다시 물어요. 세 번 다 없으면 그 답의 종류예요(GitHub의 글은 넣지 않아요)
    fake.clear_calls();
    fake.graphql(|_, _| (502, json!({"message": "secret-org"})));
    match pr_extras(&gh, "o/n", &[1]) {
        Err(SourceError::Service {
            kind: Kind::Down,
            message,
        }) => {
            assert_eq!(message, "GitHub answered HTTP 502")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(queries(&fake).len(), 3);
    fake.graphql(|_, _| (200, json!({"errors": [{"message": "x"}]})));
    assert_eq!(pr_extras(&gh, "o/n", &[1]).unwrap_err().kind(), Kind::Down);

    // 잘린 답이나 data가 객체가 아닌 답 뒤에 온 답은 써요
    fake.clear_calls();
    let tries = Arc::new(AtomicUsize::new(0));
    let seen = tries.clone();
    fake.graphql(move |_, _| match seen.fetch_add(1, Ordering::SeqCst) {
        // JSON 글자 하나(잘린 답처럼 data가 없어요)
        0 => (200, json!("{\"data\": {\"repos")),
        1 => (200, json!({"data": null, "errors": [{"message": "x"}]})),
        _ => (200, data(json!({"p1": node(1, 1, 0)}))),
    });
    let got = pr_extras(&gh, "o/n", &[1]).unwrap();
    assert_eq!(
        got[&1],
        Some(
            json!({"number": 1, "headRefName": "feat", "baseRefName": "dev", "mergedAt": null,
                                     "files": {"totalCount": 1, "nodes": [{"path": "f0.ts"}]},
                                     "reviews": {"totalCount": 0, "nodes": []}})
        )
    );
    assert_eq!(tries.load(Ordering::SeqCst), 3);

    // repository가 없거나 null이면 모든 PR이 None이에요
    fake.graphql(|_, _| (200, json!({"data": {"repository": null}})));
    assert_eq!(
        pr_extras(&gh, "o/n", &[1, 2]).unwrap(),
        BTreeMap::from([(1, None), (2, None)])
    );
}

#[test]
fn nested_paging_stops_on_a_repeated_cursor() {
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    fake.graphql(move |query, _| {
        assert!(seen.fetch_add(1, Ordering::SeqCst) < 5, "the paging did not stop");
        if query.contains("after:") {
            assert!(query.contains("after: \"c1\""));
            return (200, data(json!({"p7": {"files": files_page(300, Some("c1"), (0..100).map(|i| format!("g{i}.ts")))}})),
            );
        }
        (200, data(json!({"p7": node_with(7, 100, 0, Some(300), None, Some("c1"), None)})))
    });
    let got = pr_extras(&gh, "o/n", &[7]).unwrap();
    let files = &got[&7].as_ref().unwrap()["files"];
    assert_eq!(queries(&fake).len(), 2);
    assert_eq!(files["nodes"].as_array().unwrap().len(), 200);
    assert_eq!(files["totalCount"], 300);
    assert!(incomplete(&json!({"extras": got[&7]}))); // 다음 갱신에서 다시 물어요
}

#[test]
fn a_pr_gone_on_its_next_page_is_none() {
    // 다음 쪽에서 GitHub이 그 PR을 null로 답하면 그 PR 전체가 None이에요(저장된 것을 남겨요)
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    fake.graphql(|query, _| {
        if query.contains("after:") {
            return (200, data(json!({"p8": null})));
        }
        (
            200,
            data(
                json!({"p8": node_with(8, 100, 0, Some(150), None, Some("c1"), None),
                         "p9": node(9, 1, 0)}),
            ),
        )
    });
    let got = pr_extras(&gh, "o/n", &[9, 8, 9]).unwrap();
    assert_eq!(got[&8], None);
    assert!(got[&9].is_some());
    assert_eq!(asked(&queries(&fake)[0]), [8, 9]); // 정렬하고 겹친 것은 한 번
}

#[test]
fn pr_extras_needs_owner_name_and_a_connection() {
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    for repo in ["o", "o/n/x", ""] {
        assert!(pr_extras(&gh, repo, &[1]).is_err(), "{repo}");
        assert!(pr_extras(&gh, repo, &[]).is_err(), "{repo}"); // 묻기 전에 나눠요
    }
    assert!(pr_extras(&gh, "o/n", &[]).unwrap().is_empty());
    assert!(fake.calls().is_empty());

    let gh = GitHub::new("none", &fake.url(), Arc::new(PanicGhToken))
        .unwrap()
        .with_pause(|_| {});
    match pr_extras(&gh, "o/n", &[1]) {
        Err(SourceError::Service {
            kind: Kind::Key,
            message,
        }) => {
            assert_eq!(message, "GitHub is not connected")
        }
        other => panic!("{other:?}"),
    }
    assert!(fake.calls().is_empty());
}

#[test]
fn incomplete_reads_missing_and_short_extras() {
    let full = node(1, 2, 1);
    assert!(!incomplete(&json!({"extras": full})));
    for item in [
        json!({}),
        json!({"extras": null}),
        json!({"extras": []}),
        json!({"extras": {"files": {"totalCount": 0, "nodes": []}}}),
        json!({"extras": {"files": [], "reviews": {"totalCount": 0, "nodes": []}}}),
        json!({"extras": {"files": {"totalCount": 0, "nodes": []}, "reviews": {"nodes": []}}}), // reviews를 세기 전
        json!({"extras": {"files": {"totalCount": 3, "nodes": [{}, {}]}, "reviews": {"totalCount": 0}}}),
        json!({"extras": {"files": {"totalCount": 0}, "reviews": {"totalCount": 2, "nodes": [{}]}}}),
        json!({"extras": {"files": {"totalCount": 1, "nodes": null}, "reviews": {"totalCount": 0}}}),
    ] {
        assert!(incomplete(&item), "{item}");
    }
    for item in [
        json!({"extras": {"files": {}, "reviews": {"totalCount": 0}}}),
        json!({"extras": {"files": {"nodes": [{}]}, "reviews": {"totalCount": 1, "nodes": [{}, {}]}}}),
        json!({"extras": {"files": {"totalCount": 2.0, "nodes": [{}, {}]}, "reviews": {"totalCount": 0}}}),
    ] {
        assert!(!incomplete(&item), "{item}");
    }
}

/// R11: GraphQL의 경계예요. PR은 `PR_BATCH`개씩 한 질의이고(20 → 하나, 21 → 둘), 파일이 꼭 한 쪽(`NESTED_PAGE`)이면
/// GitHub이 다음 쪽이 없다고 할 때 더 묻지 않고, 다음 쪽이 있다고 하면 꽉 찬 쪽을 지나 끝까지 따라가요.
#[test]
fn graphql_page_boundaries() {
    let fake = FakeGitHub::start();
    let gh = connect(&fake);
    fake.graphql(|query, _| {
        if query.contains("after:") {
            assert!(asked(query) == [21] && query.contains("files("));
            let (paths, more) = if query.contains("after: \"f1\"") {
                (100..200, Some("f2")) // 꽉 찬 다음 쪽, 또 있어요
            } else {
                assert!(query.contains("after: \"f2\""), "{query}");
                (200..201, None)
            };
            let page = files_page(201, more, paths.map(|i| format!("f{i}.ts")));
            return (200, data(json!({"p21": {"files": page}})));
        }
        let aliases: Map<String, Value> = asked(query)
            .into_iter()
            .map(|n| {
                let v = match n {
                    // 꼭 한 쪽이고 다음 쪽이 없어요
                    20 => node_with(n, NESTED_PAGE, 0, Some(100), None, None, None),
                    // 꽉 찬 쪽 뒤에 더 있어요
                    21 => node_with(n, NESTED_PAGE, 0, Some(201), None, Some("f1"), None),
                    _ => node(n, 1, 0),
                };
                (format!("p{n}"), v)
            })
            .collect();
        (200, data(Value::Object(aliases)))
    });
    let twenty: Vec<u64> = (1..=20).collect();
    let got = pr_extras(&gh, "o/n", &twenty).unwrap();
    assert_eq!(queries(&fake).len(), 1);
    let files = &got[&20].as_ref().unwrap()["files"];
    assert_eq!(files["nodes"].as_array().unwrap().len(), 100);
    assert!(!incomplete(&json!({"extras": got[&20]})));

    fake.clear_calls();
    let twenty_one: Vec<u64> = (1..=21).collect();
    let got = pr_extras(&gh, "o/n", &twenty_one).unwrap();
    let sent = queries(&fake);
    let batches: Vec<Vec<u64>> = sent
        .iter()
        .filter(|q| !q.contains("after:"))
        .map(|q| asked(q))
        .collect();
    assert_eq!(batches, [(1..=20).collect::<Vec<_>>(), vec![21]]);
    assert_eq!(sent.len(), 4); // 배치 둘과 #21의 다음 쪽 둘
    let files = &got[&21].as_ref().unwrap()["files"];
    let paths: Vec<&str> = files["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths.len(), 201);
    assert_eq!(
        (paths[0], paths[100], paths[200]),
        ("f0.ts", "f100.ts", "f200.ts")
    );
    assert!(!incomplete(&json!({"extras": got[&21]})));
}
