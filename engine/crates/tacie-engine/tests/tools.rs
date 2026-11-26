//! The local MCP tools' meeting and ledger lists (docs/38): pure functions and the `/tools/*` routes.

mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::fakes::{FakeAnswerer, FakeReranker, services};
use common::fixture::{engine_with, index_of, no_github};
use http_body_util::BodyExt;
use tacie_engine::errors::Kind;
use tacie_engine::server::{AppState, router};
use tower::ServiceExt;
use common::sandbox::Sandbox;
use serde_json::{Value, json};
use tacie_engine::refresh::seed_meta;
use tacie_engine::tools::{LedgerQuery, MeetingQuery, list_ledger, list_meetings};
use tacie_sources::meeting::{Entry, Meeting};

fn entry(kind: &str, text: &str, owner: Option<&str>, done: bool) -> Entry {
    Entry {
        kind: kind.into(),
        text: text.into(),
        owner: owner.map(str::to_string),
        due: None,
        done,
        replaces: None,
    }
}

fn meeting_of(id: &str, day: u32, title: &str, summary: &[&str], ledger: Vec<Entry>) -> Meeting {
    Meeting {
        id: id.into(),
        title: title.into(),
        start: format!("2026-11-{day:02}T14:02:10+09:00"),
        end: format!("2026-11-{day:02}T14:41:55+09:00"),
        summary: summary.iter().map(|s| s.to_string()).collect(),
        topics: vec![],
        ledger,
    }
}

fn mq() -> MeetingQuery {
    MeetingQuery {
        from: None,
        to: None,
        text: None,
        ids: vec![],
        limit: 0,
    }
}

fn lq() -> LedgerQuery {
    LedgerQuery {
        kind: None,
        owner: None,
        done: None,
        from: None,
        to: None,
        meeting_ids: vec![],
        limit: 0,
    }
}

#[test]
fn meetings_newest_first_and_capped() {
    let all: Vec<Meeting> = (1..=35)
        .map(|d| {
            meeting_of(
                &format!("m{d}"),
                d.min(28),
                &format!("회의 {d}"),
                &[],
                vec![],
            )
        })
        .collect();
    let out = list_meetings(&all, &mq());
    let items = out["meetings"].as_array().unwrap();
    assert_eq!(items.len(), 30);
    assert_eq!(out["more"], true);
    assert!(
        items[0]["start"]
            .as_str()
            .unwrap()
            .starts_with("2026-11-28")
    );
}

#[test]
fn meetings_filter_by_date_and_text() {
    let all = vec![
        meeting_of("old", 1, "결제 회의", &[], vec![]),
        meeting_of("a", 5, "주간", &["결제 흐름을 봤어요"], vec![]),
        meeting_of("b", 9, "Billing sync", &["PAYMENT retry"], vec![]),
        meeting_of("c", 9, "다른 회의", &["없어요"], vec![]),
        meeting_of("late", 12, "결제 후속", &[], vec![]),
    ];
    let mut q = mq();
    q.from = Some("2026-11-02".into());
    q.to = Some("2026-11-09".into());
    q.text = Some("결제".into());
    let out = list_meetings(&all, &q);
    let ids: Vec<&str> = out["meetings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["a"]);
    assert_eq!(out["more"], false);
    // 영어는 대소문자를 가리지 않고, `to`는 그날 하루 전체예요.
    q.text = Some("payment".into());
    let out = list_meetings(&all, &q);
    assert_eq!(out["meetings"][0]["id"], "b");
}

#[test]
fn meetings_summary_trimmed_and_ids_include_ledger() {
    let all = vec![
        meeting_of(
            "a",
            3,
            "하나",
            &["첫째", "둘째", "셋째"],
            vec![entry("decision", "정했어요", None, false)],
        ),
        meeting_of("b", 4, "둘", &[], vec![]),
    ];
    let out = list_meetings(&all, &mq());
    for m in out["meetings"].as_array().unwrap() {
        assert!(m.get("ledger").is_none() && m.get("topics").is_none());
    }
    let a = out["meetings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "a")
        .unwrap();
    assert_eq!(a["summary"], json!(["첫째", "둘째"]));
    let mut q = mq();
    q.ids = vec!["a".into()];
    let out = list_meetings(&all, &q);
    let items = out["meetings"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["ledger"][0]["text"], "정했어요");
    assert!(items[0]["topics"].is_array());
}

#[test]
fn ledger_filters() {
    let all = vec![
        meeting_of(
            "a",
            3,
            "하나",
            &[],
            vec![
                entry("commitment", "문서 쓰기", Some("김민호"), false),
                entry("commitment", "이미 끝", Some("김민호"), true),
                entry("commitment", "다른 사람", Some("지윤"), false),
                entry("decision", "결정", Some("김민호"), false),
            ],
        ),
        meeting_of(
            "b",
            4,
            "둘",
            &[],
            vec![entry("open_question", "질문", None, false)],
        ),
    ];
    let mut q = lq();
    q.kind = Some("commitment".into());
    q.done = Some(false);
    q.owner = Some("민호".into());
    let out = list_ledger(&all, &q);
    let items = out["entries"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "문서 쓰기");
    assert_eq!(items[0]["meeting"]["title"], "하나");
    assert_eq!(items[0]["meeting"]["id"], "a");
    assert_eq!(out["more"], false);
    let mut q = lq();
    q.meeting_ids = vec!["b".into()];
    assert_eq!(list_ledger(&all, &q)["entries"][0]["kind"], "open_question");
}

#[test]
fn limits_are_clamped() {
    let all: Vec<Meeting> = (1..=60)
        .map(|d| {
            meeting_of(
                &format!("m{d}"),
                (d % 28) + 1,
                "회의",
                &[],
                vec![entry("decision", "x", None, false)],
            )
        })
        .collect();
    for limit in [0, 500] {
        let mut q = mq();
        q.limit = limit;
        assert_eq!(
            list_meetings(&all, &q)["meetings"]
                .as_array()
                .unwrap()
                .len(),
            30
        );
        let mut q = lq();
        q.limit = limit;
        assert_eq!(
            list_ledger(&all, &q)["entries"].as_array().unwrap().len(),
            50
        );
    }
    let mut q = mq();
    q.limit = 5;
    assert_eq!(
        list_meetings(&all, &q)["meetings"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
}

#[test]
fn server_tools_meetings_roundtrip() {
    let s = Sandbox::new();
    s.set_meta(seed_meta());
    s.github.set_credentials("none", None);
    let all = vec![
        meeting_of(
            "a",
            3,
            "하나",
            &["요약"],
            vec![entry("decision", "정해요", None, false)],
        ),
        meeting_of(
            "b",
            4,
            "둘",
            &[],
            vec![entry("commitment", "할 일", Some("민호"), false)],
        ),
    ];
    let (code, _) = s.call("POST", "/meetings", &json!({"meetings": all}).to_string());
    assert_eq!(code, StatusCode::OK);
    let (code, out) = s.call("POST", "/tools/meetings", "{}");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out["meetings"].as_array().unwrap().len(), 2);
    assert_eq!(out["meetings"][0]["id"], "b");
    let (code, out) = s.call("POST", "/tools/ledger", r#"{"kind":"decision"}"#);
    assert_eq!(code, StatusCode::OK);
    let items: &Vec<Value> = out["entries"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "decision");
}

#[test]
fn server_tools_empty_without_meetings() {
    let s = Sandbox::new();
    s.set_meta(seed_meta());
    let (code, out) = s.call("POST", "/tools/meetings", "");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out["meetings"], json!([]));
    let (code, out) = s.call("POST", "/tools/ledger", "{}");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out["entries"], json!([]));
}

const TOKEN: &str = "tok";

/// 회의 `a`~`j` 열 개가 색인에 있고, 리랭커 점수가 `p`(앞에서부터)인 서버예요.
fn search_state(reranker: FakeReranker, answerer: &Arc<FakeAnswerer>) -> Arc<AppState> {
    let chunks: Vec<Value> = "abcdefghij"
        .chars()
        .map(|c| {
            json!({"src": "meeting", "path": format!("meeting/{c}"), "line": 1,
                   "title": format!("회의 {c}"), "text": format!("meeting: {c}\nsummary: 세션 저장소 이야기 {c}")})
        })
        .collect();
    let ix = index_of(&json!(chunks), &json!({"repo": "acme/widgets"}));
    let (engine, _) = engine_with(ix, services(Arc::new(reranker), answerer.clone()));
    Arc::new(AppState {
        engine: Arc::new(engine),
        token: TOKEN.to_string(),
        github: no_github(),
        refresher: None,
        meeting_dir: None,
    })
}

fn scores(top: f64) -> Vec<f64> {
    (0..10).map(|i| top - 0.01 * i as f64).collect()
}

async fn search(state: Arc<AppState>, body: &str) -> (StatusCode, Value) {
    let app: Router = router(state);
    let req = Request::post("/tools/search")
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn search_returns_up_to_8_cards_with_scores() {
    let answerer = Arc::new(FakeAnswerer::ok("답 [1]"));
    let st = search_state(FakeReranker::ok(&scores(0.95)), &answerer);
    let (code, out) = search(st, r#"{"text": "세션 저장소"}"#).await;
    assert_eq!(code, StatusCode::OK);
    let cards = out["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 8, "{out}");
    let s: Vec<f64> = cards.iter().map(|c| c["score"].as_f64().unwrap()).collect();
    assert!(s.windows(2).all(|w| w[0] >= w[1]), "{s:?}");
    assert_eq!(out["confident"], true);
    assert_eq!(out["ranked"], true);
    assert!(out.get("reason").is_none());
    assert_eq!(cards[0]["meeting_id"], "a");
    assert_eq!(cards[0]["source"], "meeting");
    assert_eq!(cards[0]["title"], "회의 a");
}

#[tokio::test]
async fn search_below_threshold_still_returns_cards() {
    let answerer = Arc::new(FakeAnswerer::ok("답 [1]"));
    let st = search_state(FakeReranker::ok(&scores(0.4)), &answerer);
    let (code, out) = search(st, r#"{"text": "세션 저장소"}"#).await;
    assert_eq!(code, StatusCode::OK);
    assert!(!out["cards"].as_array().unwrap().is_empty(), "{out}");
    assert_eq!(out["confident"], false);
    assert_eq!(out["ranked"], true);
}

#[tokio::test]
async fn search_rerank_failure_keeps_embedding_order() {
    let answerer = Arc::new(FakeAnswerer::ok("답 [1]"));
    let st = search_state(FakeReranker::fail(Kind::Credit, "no credit"), &answerer);
    let (code, out) = search(st, r#"{"text": "세션 저장소"}"#).await;
    assert_eq!(code, StatusCode::OK);
    let cards = out["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 8, "{out}");
    assert_eq!(out["ranked"], false);
    assert_eq!(out["confident"], false);
    assert_eq!(out["reason"], "credit");
    assert!(cards.iter().all(|c| c["score"].is_null()));
}

#[tokio::test]
async fn search_scope_meetings_only() {
    let answerer = Arc::new(FakeAnswerer::ok("답 [1]"));
    let st = search_state(FakeReranker::ok(&scores(0.95)), &answerer);
    let (code, out) = search(st, r#"{"text": "세션 저장소", "meeting_ids": ["c"]}"#).await;
    assert_eq!(code, StatusCode::OK);
    let cards = out["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 1, "{out}");
    assert_eq!(cards[0]["meeting_id"], "c");
}

#[tokio::test]
async fn search_never_composes() {
    let answerer = Arc::new(FakeAnswerer::ok("답 [1]"));
    let st = search_state(FakeReranker::ok(&scores(0.95)), &answerer);
    let (code, out) = search(st, r#"{"text": "세션 저장소"}"#).await;
    assert_eq!(code, StatusCode::OK);
    assert!(!out["cards"].as_array().unwrap().is_empty());
    assert_eq!(answerer.calls().len(), 0);
}

#[tokio::test]
async fn search_rejects_bad_body() {
    let answerer = Arc::new(FakeAnswerer::ok(""));
    for body in ["{}", r#"{"text": "  "}"#, r#"{"text": "x", "meeting_ids": "a"}"#, "{no"] {
        let st = search_state(FakeReranker::ok(&scores(0.9)), &answerer);
        let (code, _) = search(st, body).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
    }
}

#[test]
fn mcp_cards_cuts_snippet_and_maps_fields() {
    use tacie_engine::tools::mcp_cards;
    let long = "가".repeat(900);
    let found = json!({"cards": [
        {"title": "t", "kind": "repo", "url": "u", "snippet": long, "score": 0.9, "path": "docs/a.md"},
        {"title": "m", "kind": "meeting", "url": null, "snippet": "s", "score": 0.5, "path": "meeting/x1"},
    ], "best_p": 0.9, "shown": true, "low_confidence": false});
    let out = mcp_cards(&found);
    let c = out["cards"].as_array().unwrap();
    assert_eq!(c[0]["snippet"].as_str().unwrap().chars().count(), 600);
    assert!(c[0].get("meeting_id").is_none());
    assert_eq!(c[1]["meeting_id"], "x1");
    assert_eq!(out["confident"], true);
    assert_eq!(out["ranked"], true);
}
