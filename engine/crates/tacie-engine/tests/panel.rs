//! Next meeting prep (`panel::prep`): stage order, keeping ledger items, no items, and model failure.

mod common {
    pub mod fakes;
    pub mod fixture;
}

use std::sync::{Arc, Mutex};

use common::fakes::{FakeAnswerer, FakeReranker, services};
use common::fixture::{engine_with, index_of};
use serde_json::{Value, json};
use tacie_engine::errors::{Kind, ServiceError};
use tacie_engine::panel::{Step, followup, prep, why};
use tacie_engine::pipeline::Engine;
use tacie_engine::services::{
    BoxFuture, FollowOut, PanelWriter, PrepOut, WhyOut,
};
use tacie_sources::meeting::{self, Entry, Meeting};

struct Panel {
    out: Result<PrepOut, Kind>,
    calls: Mutex<Vec<(Vec<Value>, Vec<Value>)>>,
}

impl Panel {
    fn ok(groups: Vec<Vec<usize>>, cited: Vec<usize>) -> Arc<Self> {
        Arc::new(Self {
            out: Ok(PrepOut {
                advice: "먼저 세션 저장소를 정해요".into(),
                groups,
                cited,
                ms: 1,
                cost: 0.01,
            }),
            calls: Mutex::new(vec![]),
        })
    }
    fn fail(kind: Kind) -> Arc<Self> {
        Arc::new(Self {
            out: Err(kind),
            calls: Mutex::new(vec![]),
        })
    }
}

impl PanelWriter for Panel {
    fn prep<'a>(
        &'a self,
        items: &'a [Value],
        passages: &'a [Value],
    ) -> BoxFuture<'a, Result<PrepOut, ServiceError>> {
        self.calls
            .lock()
            .unwrap()
            .push((items.to_vec(), passages.to_vec()));
        let r = match &self.out {
            Ok(o) => Ok(o.clone()),
            Err(k) => Err(ServiceError {
                kind: *k,
                message: "boom".into(),
            }),
        };
        Box::pin(async move { r })
    }
    fn why<'a>(&'a self, _: &'a str, _: &'a [Value]) -> BoxFuture<'a, Result<WhyOut, ServiceError>> {
        unreachable!()
    }
    fn followup<'a>(&'a self, _: &'a [Value], _: &'a [Value]) -> BoxFuture<'a, Result<FollowOut, ServiceError>> {
        unreachable!()
    }
}

fn entry(kind: &str, text: &str) -> Entry {
    Entry {
        kind: kind.into(),
        text: text.into(),
        owner: None,
        due: None,
        done: false,
        replaces: None,
    }
}

fn meeting_of(id: &str, start: &str, ledger: Vec<Entry>) -> Meeting {
    Meeting {
        id: id.into(),
        title: format!("회의 {id}"),
        start: format!("{start}T14:00:00+09:00"),
        end: format!("{start}T15:00:00+09:00"),
        summary: vec!["세션 저장소 이야기".into()],
        topics: vec![],
        ledger,
    }
}

/// 회의 a(질문 하나, 할 일 하나), b(질문 하나), 자료 문서 하나가 색인에 있는 엔진이에요.
fn setup(panel: Arc<Panel>, meetings: &[Meeting]) -> (Engine, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    meeting::save(dir.path(), meetings).unwrap();
    let chunks = json!([
        {"src": "meeting", "path": "meeting/a", "line": 1, "text": "meeting: a\nsummary: 세션 저장소 이야기"},
        {"src": "meeting", "path": "meeting/b", "line": 1, "text": "meeting: b\nsummary: 세션 저장소 이야기"},
        {"src": "repo", "path": "docs/session.md", "line": 1, "text": "# 세션\n세션은 쿠키에 둬요"},
    ]);
    let ix = index_of(&chunks, &json!({"repo": "acme/widgets"}));
    let reranker = Arc::new(FakeReranker::ok(&[0.9, 0.8, 0.7]));
    let mut s = services(reranker, Arc::new(FakeAnswerer::ok("")));
    s.panel = panel;
    let (engine, _) = engine_with(ix, s);
    (engine, dir)
}

fn two_meetings() -> Vec<Meeting> {
    vec![
        meeting_of(
            "a",
            "2026-11-03",
            vec![entry("open_question", "세션을 어디에 둘까요"), entry("commitment", "비용 표 만들기")],
        ),
        meeting_of("b", "2026-11-05", vec![entry("open_question", "세션 저장소는요")]),
    ]
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn prep_reports_steps_in_order() {
    let (engine, dir) = setup(Panel::ok(vec![vec![0, 2]], vec![0]), &two_meetings());
    let mut steps = vec![];
    let out = prep(&engine, dir.path(), &ids(&["a"]), |s| steps.push(s)).await;
    assert_eq!(
        steps,
        vec![
            Step::Meetings { ids: ids(&["a"]) },
            Step::Related { ids: ids(&["b"]) },
            Step::Sources { count: 1 },
            Step::Writing,
        ],
        "{out}"
    );
    assert_eq!(out["meetings_read"], 2);
    assert_eq!(out["sources_read"], 1);
    assert_eq!(out["advice"], "먼저 세션 저장소를 정해요");
    assert_eq!(out["sources"].as_array().unwrap().len(), 1);
    // 묶음 [0, 2]: a의 질문과 b의 질문이 같은 이야기예요.
    // 목록에는 요청한 회의 a의 질문만 있고, b는 `also`로만 나와요.
    let open = out["open"].as_array().unwrap();
    assert_eq!(open.len(), 1, "{out}");
    assert_eq!(open[0]["meeting_id"], "a");
    assert_eq!(open[0]["also"], json!(["b"]));
}

#[tokio::test]
async fn prep_lists_every_requested_item_even_if_model_groups_none() {
    let panel = Panel::ok(vec![], vec![]);
    let (engine, dir) = setup(panel.clone(), &two_meetings());
    let out = prep(&engine, dir.path(), &ids(&["a"]), |_| {}).await;
    assert_eq!(out["open"].as_array().unwrap().len(), 1);
    assert_eq!(out["todos"].as_array().unwrap().len(), 1);
    assert_eq!(out["todos"][0]["text"], "비용 표 만들기");
    assert_eq!(out["open"][0]["also"], json!([]));
    // 모델에는 항목 셋과 회의 날짜가 가요.
    let calls = panel.calls.lock().unwrap();
    assert_eq!(calls[0].0.len(), 3);
    assert_eq!(calls[0].0[0]["date"], "2026-11-03");
    assert_eq!(calls[0].0[2]["date"], "2026-11-05");
}

#[tokio::test]
async fn prep_with_no_open_items_skips_the_model() {
    let panel = Panel::fail(Kind::Down);
    let done = vec![meeting_of("a", "2026-11-03", vec![entry("decision", "쿠키에 둬요")])];
    let (engine, dir) = setup(panel.clone(), &done);
    let mut steps = vec![];
    let out = prep(&engine, dir.path(), &ids(&["a"]), |s| steps.push(s)).await;
    assert!(panel.calls.lock().unwrap().is_empty());
    assert!(!steps.contains(&Step::Writing));
    assert_eq!(out["advice"], "");
    assert_eq!(out["open"], json!([]));
    assert!(out["error_kind"].is_null(), "{out}");
    // 모델을 부르지 않아도 찾은 자료는 그대로 보여요.
    assert_eq!(out["sources"].as_array().unwrap().len(), 1, "{out}");
}

#[tokio::test]
async fn prep_keeps_items_when_model_fails() {
    let (engine, dir) = setup(Panel::fail(Kind::Credit), &two_meetings());
    let out = prep(&engine, dir.path(), &ids(&["a"]), |_| {}).await;
    assert_eq!(out["error_kind"], "credit", "{out}");
    assert_eq!(out["open"].as_array().unwrap().len(), 1);
    assert_eq!(out["todos"].as_array().unwrap().len(), 1);
    assert_eq!(out["sources"].as_array().unwrap().len(), 1, "{out}");
}

#[tokio::test]
async fn prep_falls_back_to_found_sources_when_model_cites_none() {
    let (engine, dir) = setup(Panel::ok(vec![], vec![]), &two_meetings());
    let out = prep(&engine, dir.path(), &ids(&["a"]), |_| {}).await;
    assert_eq!(out["sources"].as_array().unwrap().len(), 1, "{out}");
}

#[tokio::test]
async fn prep_finds_related_meetings_and_sources_even_if_requested_ones_rank_first() {
    let dir = tempfile::tempdir().unwrap();
    let all: Vec<Meeting> = ["a", "b", "c", "d"]
        .iter()
        .map(|id| meeting_of(id, "2026-11-03", vec![entry("open_question", &format!("질문 {id}"))]))
        .collect();
    meeting::save(dir.path(), &all).unwrap();
    let mut chunks: Vec<Value> = ["a", "b", "c", "d"]
        .iter()
        .map(|id| json!({"src": "meeting", "path": format!("meeting/{id}"), "line": 1,
                         "text": format!("meeting: {id}\nsummary: 세션 저장소 이야기")}))
        .collect();
    for i in 0..4 {
        chunks.push(json!({"src": "repo", "path": format!("docs/s{i}.md"), "line": 1,
                           "text": format!("# 세션 {i}\n세션은 쿠키에 둬요")}));
    }
    let ix = index_of(&Value::Array(chunks), &json!({"repo": "acme/widgets"}));
    let reranker = Arc::new(FakeReranker::ok(&[0.99, 0.98, 0.97, 0.96, 0.9, 0.89, 0.88, 0.87]));
    let mut s = services(reranker, Arc::new(FakeAnswerer::ok("")));
    s.panel = Panel::ok(vec![], vec![]);
    let (engine, _) = engine_with(ix, s);
    let mut steps = vec![];
    let out = prep(&engine, dir.path(), &ids(&["a", "b"]), |s| steps.push(s)).await;
    assert_eq!(steps[1], Step::Related { ids: ids(&["c", "d"]) }, "{out}");
    assert_eq!(steps[2], Step::Sources { count: 3 }, "{out}");
    assert_eq!(out["meetings_read"], 4);
}

#[tokio::test]
async fn prep_ignores_search_hits_below_the_show_threshold() {
    let dir = tempfile::tempdir().unwrap();
    meeting::save(dir.path(), &two_meetings()).unwrap();
    let chunks = json!([
        {"src": "meeting", "path": "meeting/a", "line": 1, "text": "meeting: a\nsummary: 세션 저장소 이야기"},
        {"src": "meeting", "path": "meeting/b", "line": 1, "text": "meeting: b\nsummary: 세션 저장소 이야기"},
    ]);
    let ix = index_of(&chunks, &json!({"repo": "acme/widgets"}));
    // 어느 순서로 랭크돼도 b가 기준 아래가 되게, 둘 다 낮은 점수예요.
    let reranker = Arc::new(FakeReranker::ok(&[0.3, 0.2]));
    let panel = Panel::ok(vec![vec![0, 2]], vec![]);
    let mut s = services(reranker, Arc::new(FakeAnswerer::ok("")));
    s.panel = panel.clone();
    let (engine, _) = engine_with(ix, s);
    let mut steps = vec![];
    let out = prep(&engine, dir.path(), &ids(&["a"]), |s| steps.push(s)).await;
    assert_eq!(steps[1], Step::Related { ids: vec![] }, "{out}");
    assert_eq!(out["meetings_read"], 1);
    // 관련이 아니라서 모델에도 a의 항목만 가요.
    assert_eq!(panel.calls.lock().unwrap()[0].0.len(), 2);
    assert_eq!(out["open"][0]["also"], json!([]));
}

#[tokio::test]
async fn prep_never_lists_related_meetings_items() {
    let (engine, dir) = setup(Panel::ok(vec![], vec![]), &two_meetings());
    let out = prep(&engine, dir.path(), &ids(&["a"]), |_| {}).await;
    assert_eq!(out["meetings_read"], 2);
    for k in ["open", "todos"] {
        for i in out[k].as_array().unwrap() {
            assert_eq!(i["meeting_id"], "a", "{out}");
        }
    }
}

struct Lp {
    why: Result<WhyOut, Kind>,
    follow: Result<FollowOut, Kind>,
    calls: Mutex<usize>,
}

impl PanelWriter for Lp {
    fn prep<'a>(&'a self, _: &'a [Value], _: &'a [Value]) -> BoxFuture<'a, Result<PrepOut, ServiceError>> {
        unreachable!()
    }
    fn why<'a>(&'a self, _: &'a str, _: &'a [Value]) -> BoxFuture<'a, Result<WhyOut, ServiceError>> {
        *self.calls.lock().unwrap() += 1;
        let r = self.why.clone().map_err(|k| ServiceError { kind: k, message: "boom".into() });
        Box::pin(async move { r })
    }
    fn followup<'a>(&'a self, _: &'a [Value], _: &'a [Value]) -> BoxFuture<'a, Result<FollowOut, ServiceError>> {
        *self.calls.lock().unwrap() += 1;
        let r = self.follow.clone().map_err(|k| ServiceError { kind: k, message: "boom".into() });
        Box::pin(async move { r })
    }
}

fn lp_engine(lp: Arc<Lp>) -> Engine {
    let ix = index_of(&json!([]), &json!({"repo": "acme/widgets"}));
    let mut s = services(Arc::new(FakeReranker::ok(&[])), Arc::new(FakeAnswerer::ok("")));
    s.panel = lp;
    engine_with(ix, s).0
}

fn lp(why: Result<WhyOut, Kind>, follow: Result<FollowOut, Kind>) -> Arc<Lp> {
    Arc::new(Lp { why, follow, calls: Mutex::new(0) })
}

fn why_ok() -> Result<WhyOut, Kind> {
    Ok(WhyOut { reason: "비용 때문이에요".into(), keys: vec!["e0:s1u3".into()], ms: 1, cost: 0.02 })
}

fn follow_ok() -> Result<FollowOut, Kind> {
    Ok(FollowOut {
        results: vec![(0, true, Some("쿠키로 정했어요".into())), (1, false, None)],
        ms: 1,
        cost: 0.03,
    })
}

#[tokio::test]
async fn why_returns_only_keys_from_window() {
    let p = lp(why_ok(), follow_ok());
    let engine = lp_engine(p.clone());
    let window = vec![json!({"key": "e0:s1u3", "speaker": "지윤", "at": "14:07", "text": "비용이 문제예요"})];
    let out = why(&engine, "쿠키에 둬요", &window).await;
    assert_eq!(out["reason"], "비용 때문이에요");
    assert_eq!(out["keys"], json!(["e0:s1u3"]));
    assert_eq!(out["cost"], 0.02);
    assert_eq!(*p.calls.lock().unwrap(), 1);
}

#[tokio::test]
async fn why_with_empty_window_does_not_call_model() {
    let p = lp(why_ok(), follow_ok());
    let engine = lp_engine(p.clone());
    let out = why(&engine, "쿠키에 둬요", &[]).await;
    assert_eq!(out["reason"], tacie_engine::pipeline::NO_ANSWER);
    assert_eq!(out["keys"], json!([]));
    assert_eq!(*p.calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn why_reports_model_failure() {
    let engine = lp_engine(lp(Err(Kind::Credit), follow_ok()));
    let window = vec![json!({"key": "k", "speaker": "a", "at": "1", "text": "t"})];
    let out = why(&engine, "x", &window).await;
    assert_eq!(out["error_kind"], "credit", "{out}");
    assert_eq!(out["keys"], json!([]));
    // 앱의 WhyAnswer는 reason을 늘 읽어요: 실패에도 빈 글을 실어야 error_kind가 앱까지 가요.
    assert_eq!(out["reason"], "", "{out}");
}

#[tokio::test]
async fn followup_marks_each_prepared_item() {
    let p = lp(why_ok(), follow_ok());
    let engine = lp_engine(p);
    let prepared = vec![json!({"text": "세션 저장소"}), json!({"text": "비용 표"})];
    let ledger = vec![json!({"kind": "decision", "text": "쿠키에 둬요"})];
    let out = followup(&engine, &prepared, &ledger).await;
    assert_eq!(
        out["results"],
        json!([{"i": 0, "decided": true, "how": "쿠키로 정했어요"}, {"i": 1, "decided": false, "how": null}])
    );
    assert_eq!(out["cost"], 0.03);
}

#[tokio::test]
async fn followup_reports_model_failure() {
    let engine = lp_engine(lp(why_ok(), Err(Kind::Down)));
    let out = followup(&engine, &[json!({"text": "a"})], &[]).await;
    assert_eq!(out["error_kind"], "down", "{out}");
}
