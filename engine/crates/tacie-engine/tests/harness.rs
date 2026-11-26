//! Panel harness cards (docs/40): meetings in `meetings.json` become cards even with no meeting chunks in the
//! index.

mod common {
    pub mod fakes;
    pub mod fixture;
}

use std::sync::Arc;

use common::fakes::{FakeMeetingSearch, ScriptedCall, ScriptedPanel};
use common::fixture::{chunks, harness_engine, harness_engine_on, index_of, meta};
use serde_json::{Value, json};
use tacie_engine::errors::Kind;
use tacie_engine::harness::{self, Request};
use tacie_engine::services::{ChatTurn, ToolCall};

fn meeting(id: &str, title: &str, start: &str) -> serde_json::Value {
    json!({"id": id, "title": title, "start": start, "end": start,
           "summary": ["출시일을 정했어요."], "topics": [], "ledger": []})
}

#[test]
fn cards_come_from_meetings_json_when_index_is_empty() {
    let panel = ScriptedPanel::new(vec![
        ChatTurn {
            calls: vec![ToolCall { id: "c1".into(), name: "list_meetings".into(), arguments: "{}".into() }],
            ..Default::default()
        },
        ChatTurn { content: "4월 회의에서 정했어요 [1].".into(), ..Default::default() },
    ]);
    let meetings = [meeting("m-apr", "4월 회의", "2026-04-03T10:00:00+09:00"), meeting("m-may", "5월 회의", "2026-05-02T10:00:00+09:00")];
    let (engine, dir) = harness_engine(&meetings, panel.clone(), true);
    let req = Request { text: "출시일 언제 정했죠?".into(), today: "2026-11-17".into(), ..Default::default() };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let done = rt.block_on(harness::run(&engine, dir.path(), &req, |_| {}));

    assert_eq!(done["cards"][0]["path"], "meeting/m-apr", "{done}");
    assert_eq!(done["cards"][0]["source"], "meeting");
    assert_eq!(done["cards"][0]["title"], "4월 회의");
    assert!(done["answer"].as_str().unwrap().contains("[1]"), "{done}");
    assert_eq!(panel.calls()[0], ("panel_chat_iphone", "panel_iphone"));
}

fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall { id: id.into(), name: name.into(), arguments: args.to_string() }
}

fn tools(calls: Vec<ToolCall>) -> ChatTurn {
    ChatTurn { calls, ..Default::default() }
}

fn says(text: &str) -> ChatTurn {
    ChatTurn { content: text.into(), ..Default::default() }
}

fn priced(id: &str, title: &str) -> Value {
    json!({"id": id, "title": title, "start": "2026-04-03T10:00:00+09:00", "end": "2026-04-03T11:00:00+09:00",
           "summary": [format!("{title}을 이야기했어요.")], "topics": [], "ledger": []})
}

/// iPhone 엔진에 회의 검색 가짜를 끼워 질문 하나를 돌려요. 돌려주는 값은 `done`과 모델이 받은 호출들이에요.
fn iphone_run(meetings: &[Value], turns: Vec<ChatTurn>, search: Arc<FakeMeetingSearch>, req: &Request) -> (Value, Vec<ScriptedCall>) {
    let panel = ScriptedPanel::new(turns);
    let (mut engine, dir) = harness_engine(meetings, panel.clone(), true);
    engine.services.meeting_search = Some(search);
    engine.embedder = Arc::new(tacie_engine::sample::MissingModel);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let done = rt.block_on(harness::run(&engine, dir.path(), req, |_| {}));
    (done, panel.seen())
}

fn ask(text: &str) -> Request {
    Request { text: text.into(), today: "2026-11-17".into(), ..Default::default() }
}

/// 모델이 받은 도구 결과 글들이에요(마지막 호출이 앞의 결과를 모두 담아요).
fn tool_results(seen: &[ScriptedCall]) -> Vec<String> {
    seen.last().map(|c| c.turns.iter().filter(|t| t["role"] == "tool")
        .map(|t| t["content"].as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
}

#[test]
fn iphone_scoped_search_stays_local() {
    let search = FakeMeetingSearch::ok(&["m2"]);
    let req = Request { meeting_ids: vec!["m1".into()], ..ask("가격 얘기 뭐였죠?") };
    let (done, seen) = iphone_run(
        &[priced("m1", "가격 정책"), priced("m2", "채용 회의")],
        // 지어낸 wide도 범위를 넓히지 않아요.
        vec![tools(vec![call("c1", "search", json!({"query": "가격", "wide": true}))]), says("가격을 정했어요 [1].")],
        search.clone(),
        &req,
    );
    assert!(search.calls().is_empty(), "{:?}", search.calls());
    assert_eq!(done["cards"].as_array().unwrap().len(), 1, "{done}");
    assert_eq!(done["cards"][0]["path"], "meeting/m1", "{done}");
    assert!(tool_results(&seen)[0].contains("가격 정책"));
}

#[test]
fn iphone_round_searches_go_in_one_request() {
    let search = FakeMeetingSearch::ok(&["m1"]);
    let (_, _) = iphone_run(
        &[priced("m1", "가격 정책")],
        vec![
            tools(vec![call("c1", "search", json!({"query": "가격"})), call("c2", "search", json!({"query": "출시일"}))]),
            says("정했어요 [1]."),
        ],
        search.clone(),
        &ask("가격이랑 출시일 정했어요?"),
    );
    assert_eq!(search.calls(), vec![vec!["가격".to_string(), "출시일".to_string()]]);
}

#[test]
fn iphone_server_searches_stop_at_four() {
    let search = FakeMeetingSearch::ok(&["m1"]);
    let round = |r: usize| tools((0..3).map(|i| call(&format!("c{r}{i}"), "search", json!({"query": format!("질문 {r}-{i}")}))).collect());
    let (_, seen) = iphone_run(
        &[priced("m1", "가격 정책")],
        vec![round(1), round(2), round(3), says("찾은 것으로 답해요 [1].")],
        search.clone(),
        &ask("다 찾아 줘"),
    );
    let sent: usize = search.calls().iter().map(Vec::len).sum();
    assert_eq!(sent, harness::SERVER_SEARCHES_MAX, "{:?}", search.calls());
    assert!(tool_results(&seen).iter().any(|r| r.contains("검색을 더 할 수 없어요. 찾은 것으로 답해요.")));
}

#[test]
fn iphone_search_credit_stops_run() {
    let (done, seen) = iphone_run(
        &[priced("m1", "가격 정책")],
        vec![tools(vec![call("c1", "search", json!({"query": "가격"}))]), says("가격 [1].")],
        FakeMeetingSearch::fail(Kind::Credit),
        &ask("가격 정했어요?"),
    );
    assert_eq!(done["error_kind"], "credit", "{done}");
    assert!(done["answer"].is_null(), "{done}");
    assert_eq!(done["cards"], json!([]));
    assert_eq!(seen.len(), 1, "the run stops before asking the model again");
}

#[test]
fn iphone_search_key_stops_run() {
    let (done, _) = iphone_run(
        &[priced("m1", "가격 정책")],
        vec![tools(vec![call("c1", "search", json!({"query": "가격"}))]), says("가격 [1].")],
        FakeMeetingSearch::fail(Kind::Key),
        &ask("가격 정했어요?"),
    );
    assert_eq!(done["error_kind"], "key", "{done}");
    assert!(done["answer"].is_null(), "{done}");
}

#[test]
fn iphone_search_falls_back_to_keywords() {
    let (done, _) = iphone_run(
        &[priced("m2", "채용 회의"), priced("m1", "가격 정책")],
        vec![tools(vec![call("c1", "search", json!({"query": "가격"}))]), says("가격은 정했어요 [1].")],
        FakeMeetingSearch::fail(Kind::Network),
        &ask("가격 정했어요?"),
    );
    assert_eq!(done["cards"][0]["path"], "meeting/m1", "{done}");
    assert_eq!(done["cards"].as_array().unwrap().len(), 1, "{done}");
    assert!(done["answer"].as_str().unwrap().contains("[1]"), "{done}");
}

/// iPhone `search`가 돌려주는 회의 정리의 장부 항목은 하나마다 번호가 있어서, 그 번호를 인용하면 `items`에 남아요.
#[test]
fn iphone_search_numbers_ledger_items() {
    let m = json!({"id": "m1", "title": "반도체산업 특별법안 심사", "start": "2024-11-21T10:00:00+09:00",
                   "end": "2024-11-21T12:00:00+09:00", "summary": ["특례를 논의했어요."], "topics": [],
                   "ledger": [{"kind": "decision", "text": "계속 심사해요."},
                              {"kind": "open_question", "text": "근로시간 특례는 다음에 정해요."}]});
    let (done, seen) = iphone_run(
        &[m],
        vec![tools(vec![call("c1", "search", json!({"query": "반도체법 남은 논의"}))]), says("근로시간 특례가 남았어요 [3].")],
        FakeMeetingSearch::ok(&["m1"]),
        &ask("반도체법 회의에서 넘긴 거?"),
    );
    let result = &tool_results(&seen)[0];
    assert!(result.contains("[1] 2024-11-21 「반도체산업 특별법안 심사」"), "{result}");
    assert!(result.contains("[2] 결정: 계속 심사해요."), "{result}");
    // filter_ledger와 같은 종류 이름이에요("남은 논의")
    assert!(result.contains("[3] 남은 논의: 근로시간 특례는 다음에 정해요."), "{result}");
    assert_eq!(done["items"], json!([{"meeting_id": "m1", "index": 1}]), "{done}");
    assert_eq!(done["cards"][0]["path"], "meeting/m1", "{done}");
}

/// 영어 회의의 장부 줄은 영어 꼬리표이고, 바꾼 이전 결정은 날짜와 회의 제목까지 붙어요.
#[test]
fn iphone_search_items_use_the_meeting_language() {
    let m = json!({"id": "m1", "title": "Pricing review", "start": "2026-04-03T10:00:00+09:00",
                   "end": "2026-04-03T11:00:00+09:00", "summary": ["We went over the launch price and the discount for teams."],
                   "topics": [{"title": "Price", "points": ["Teams get a discount."]}],
                   "ledger": [{"kind": "decision", "text": "The price is 12 dollars a month.",
                               "replaces": {"text": "10 dollars", "meeting": "m0", "title": "March pricing", "at": "2026-03-02"}},
                              {"kind": "commitment", "text": "Update the page", "owner": "Mina", "due": "Friday"}]});
    let (_, seen) = iphone_run(
        &[m],
        vec![tools(vec![call("c1", "search", json!({"query": "price"}))]), says("It is 12 dollars [2].")],
        FakeMeetingSearch::ok(&["m1"]),
        &ask("What price did we pick?"),
    );
    let result = &tool_results(&seen)[0];
    assert!(result.contains("[2] Decision: The price is 12 dollars a month. (earlier decision 2026-03-02 March pricing: 10 dollars)"), "{result}");
    assert!(result.contains("[3] To-do: Update the page (owner Mina, due Friday)"), "{result}");
    assert!(result.contains("Topic: Price"), "{result}");
}

/// 한국어 회의의 검색 결과 항목은 `filter_ledger`와 같은 글이에요: 종류 이름, 담당·기한, 끝남이에요.
#[test]
fn iphone_search_items_match_filter_ledger() {
    let m = json!({"id": "m1", "title": "결제 회의", "start": "2026-04-03T10:00:00+09:00", "end": "2026-04-03T11:00:00+09:00",
                   "summary": ["결제 재시도를 이야기했어요."], "topics": [],
                   "ledger": [{"kind": "commitment", "text": "문서 고치기", "owner": "민호", "due": "금요일", "done": true},
                              {"kind": "decision", "text": "세 번까지 해요.",
                               "replaces": {"text": "두 번까지", "meeting": "m0", "title": "3월 결제 회의", "at": "2026-03-02"}}]});
    let (_, seen) = iphone_run(
        &[m.clone()],
        vec![tools(vec![call("c1", "search", json!({"query": "결제"})), call("c2", "filter_ledger", json!({}))]), says("세 번이에요 [2].")],
        FakeMeetingSearch::ok(&["m1"]),
        &ask("결제 재시도 몇 번이죠?"),
    );
    let results = tool_results(&seen);
    let (search, filter) = (&results[0], &results[1]);
    assert!(search.contains("결정: 세 번까지 해요. (이전 결정 2026-03-02 3월 결제 회의: 두 번까지)"), "{search}");
    assert!(search.contains("할 일: 문서 고치기 (담당 민호, 기한 금요일, 끝남)"), "{search}");
    assert!(filter.contains("할 일: 문서 고치기 (담당 민호, 기한 금요일, 끝남)"), "{filter}");
    assert!(filter.contains("결정: 세 번까지 해요. (이전 결정: 두 번까지)"), "{filter}");
}

#[test]
fn iphone_search_skips_unknown_ids() {
    let (done, _) = iphone_run(
        &[priced("m1", "가격 정책")],
        vec![tools(vec![call("c1", "search", json!({"query": "가격"}))]), says("가격 [1].")],
        FakeMeetingSearch::ok(&["not-here"]),
        &ask("가격 정했어요?"),
    );
    assert_eq!(done["cards"], json!([]), "{done}");
}

#[test]
fn iphone_prep_uses_meeting_search() {
    let search = FakeMeetingSearch::ok(&["m2", "m1"]);
    let req = Request { meeting_ids: vec!["m1".into()], ..ask("다음 회의 준비해 줘") };
    let (done, _) = iphone_run(
        &[priced("m1", "가격 정책"), priced("m2", "가격 후속")],
        vec![tools(vec![call("c1", "prepare_meeting", json!({"meeting_id": "m1"}))]), says("준비했어요.")],
        search.clone(),
        &req,
    );
    let prep = &done["prep"];
    assert!(prep["error_kind"].is_null(), "{done}");
    assert_eq!(prep["meetings_read"], 2, "{done}");
    assert_eq!(search.calls().len(), 1);
}

fn meeting_chunk(acl: Option<Value>) -> tacie_index::Index {
    let mut cs = chunks();
    let mut c = json!({"src": "meeting", "path": "meeting/m-apr", "line": 1, "title": "색인의 4월 회의",
                       "text": "meeting: 4월 회의\n출시일을 정했어요."});
    if let Some(a) = acl {
        c["acl"] = a;
    }
    cs.as_array_mut().unwrap().push(c);
    index_of(&cs, &meta())
}

fn list_and_cite(ix: tacie_index::Index) -> Value {
    let panel = ScriptedPanel::new(vec![
        tools(vec![call("c1", "list_meetings", json!({}))]),
        says("4월 회의에서 정했어요 [1]."),
    ]);
    let (engine, dir) = harness_engine_on(ix, &[meeting("m-apr", "4월 회의", "2026-04-03T10:00:00+09:00")], panel, true);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(harness::run(&engine, dir.path(), &ask("출시일 언제 정했죠?"), |_| {}))
}

#[test]
fn index_chunk_card_wins() {
    let done = list_and_cite(meeting_chunk(None));
    assert_eq!(done["cards"][0]["path"], "meeting/m-apr", "{done}");
    assert_eq!(done["cards"][0]["title"], "색인의 4월 회의", "{done}");
}

#[test]
fn disallowed_chunk_stays_hidden() {
    let done = list_and_cite(meeting_chunk(Some(Value::Null)));
    assert_eq!(done["cards"], json!([]), "{done}");
}

#[test]
fn iphone_prep_credit_stops_run() {
    let req = Request { meeting_ids: vec!["m1".into()], ..ask("다음 회의 준비해 줘") };
    let (done, seen) = iphone_run(
        &[priced("m1", "가격 정책")],
        vec![tools(vec![call("c1", "prepare_meeting", json!({"meeting_id": "m1"}))]), says("준비했어요.")],
        FakeMeetingSearch::fail(Kind::Credit),
        &req,
    );
    assert_eq!(done["error_kind"], "credit", "{done}");
    assert!(done["answer"].is_null(), "{done}");
    assert_eq!(seen.len(), 1, "the run stops before asking the model again");
}

#[test]
fn iphone_prep_skips_meetings_not_here() {
    let req = Request { meeting_ids: vec!["m1".into()], ..ask("다음 회의 준비해 줘") };
    let (done, _) = iphone_run(
        &[priced("m1", "가격 정책"), priced("m2", "가격 후속")],
        vec![tools(vec![call("c1", "prepare_meeting", json!({"meeting_id": "m1"}))]), says("준비했어요.")],
        FakeMeetingSearch::ok(&["ghost1", "ghost2", "m2"]),
        &req,
    );
    assert_eq!(done["prep"]["meetings_read"], 2, "{done}");
}

/// 결정 하나, 남은 논의 하나가 있는 회의예요.
fn decided_and_open() -> Value {
    json!({"id": "m1", "title": "결제 회의", "start": "2026-04-03T10:00:00+09:00", "end": "2026-04-03T11:00:00+09:00",
           "summary": ["결제 재시도를 이야기했어요."], "topics": [],
           "ledger": [{"kind": "decision", "text": "세 번까지 해요."},
                      {"kind": "open_question", "text": "알림 문구는 다음에 정해요."}]})
}

/// 장부 번호를 받고, filter_ledger를 부르고, 두 번호를 다 인용해 답하는 대본이에요.
fn kinds_script(first: ToolCall, filter: Option<Value>) -> Vec<ChatTurn> {
    let mut turns = vec![tools(vec![first])];
    if let Some(args) = filter {
        turns.push(tools(vec![call("c2", "filter_ledger", args)]));
    }
    turns.push(says("세 번까지로 정했어요 [2]. 알림 문구는 남았어요 [3]."));
    turns
}

/// iPhone: filter_ledger(kinds=["decision"])를 불렀으면, 검색 결과에서 인용한 남은 논의는 items와 답 표시에서 빠져요.
#[test]
fn iphone_drops_cited_items_of_kinds_not_asked() {
    let (done, _) = iphone_run(
        &[decided_and_open()],
        kinds_script(call("c1", "search", json!({"query": "결제"})), Some(json!({"kinds": ["decision"]}))),
        FakeMeetingSearch::ok(&["m1"]),
        &ask("결제 회의에서 정한 거?"),
    );
    assert_eq!(done["items"], json!([{"meeting_id": "m1", "index": 0}]), "{done}");
    assert_eq!(done["answer"], "세 번까지로 정했어요 [1]. 알림 문구는 남았어요 .", "{done}");
}

/// 같은 대본을 Mac에서 돌리면 아무것도 빼지 않아요(Mac은 그대로예요). Mac `search`는 장부에 번호를 붙이지 않아서
/// 회의 정리(meeting_notes)로 같은 번호를 받아요.
#[test]
fn mac_keeps_cited_items_of_other_kinds() {
    let panel = common::fakes::ScriptedPanel::new(kinds_script(
        call("c1", "meeting_notes", json!({"meeting_id": "m1"})),
        Some(json!({"kinds": ["decision"]})),
    ));
    let (engine, dir) = harness_engine(&[decided_and_open()], panel, false);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let done = rt.block_on(harness::run(&engine, dir.path(), &ask("결제 회의에서 정한 거?"), |_| {}));
    // 회의 정리는 장부 번호 차례가 검색 결과와 달라도, 두 항목이 다 남아요.
    let mut items: Vec<u64> = done["items"].as_array().unwrap().iter().map(|i| i["index"].as_u64().unwrap()).collect();
    items.sort();
    assert_eq!(items, vec![0, 1], "{done}");
    assert_eq!(done["answer"], "세 번까지로 정했어요 [1]. 알림 문구는 남았어요 [1].", "{done}");
}

/// iPhone이라도 filter_ledger를 kinds로 부르지 않았으면 아무것도 빼지 않아요.
#[test]
fn iphone_keeps_items_without_filter_kinds() {
    let (done, _) = iphone_run(
        &[decided_and_open()],
        kinds_script(call("c1", "search", json!({"query": "결제"})), None),
        FakeMeetingSearch::ok(&["m1"]),
        &ask("결제 회의에서 정한 거?"),
    );
    assert_eq!(done["items"], json!([{"meeting_id": "m1", "index": 0}, {"meeting_id": "m1", "index": 1}]), "{done}");
}

/// 이 Mac의 작은 모델(이슈 #54): 도구를 고르게 하지 않고, 엔진이 회의 정리와 관련 대화 줄을 먼저 넣어 한 번에 물어요.
fn small_run(meetings: &[Value], turns: Vec<ChatTurn>, req: &Request) -> (Value, Vec<ScriptedCall>) {
    let panel = ScriptedPanel::small(turns);
    let (mut engine, dir) = harness_engine(meetings, panel.clone(), false);
    engine.embedder = Arc::new(tacie_engine::sample::MissingModel);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let done = rt.block_on(harness::run(&engine, dir.path(), req, |_| {}));
    (done, panel.seen())
}

#[test]
fn small_model_on_a_meeting_page_gets_notes_and_lines_in_one_call() {
    let mut req = Request { meeting_ids: vec!["m1".into()], ..ask("Redis를 계속 쓰는 이유가 뭐였죠?") };
    req.transcripts.insert("m1".into(), vec![
        json!({"key": "a1", "speaker": "1", "text": "이벤트 큐를 SQS로 옮기자는 얘기가 있었죠."}),
        json!({"key": "a2", "speaker": "2", "text": "하루 10만 건이라 Redis 한 대로 충분했어요."}),
    ]);
    let (done, seen) = small_run(&[priced("m1", "큐 회의")], vec![says("하루 10만 건이라 Redis 한 대로 충분했어요 [1].")], &req);
    assert_eq!(seen.len(), 1, "one call only");
    assert_eq!(seen[0].choice, "none");
    let results = tool_results(&seen);
    assert!(results.iter().any(|r| r.contains("큐 회의")), "{results:?}");
    assert!(results.iter().any(|r| r.contains("10만 건")), "{results:?}");
    assert!(done["answer"].as_str().unwrap().contains("Redis"), "{done}");
    assert_eq!(done["tools"], json!(["meeting_notes", "transcript_window"]));
}

#[test]
fn small_model_across_meetings_searches_first() {
    let (done, seen) = small_run(&[priced("m1", "가격 정책"), priced("m2", "채용 회의")],
                                 vec![says("가격을 정했어요.")], &ask("가격 정책 얘기 뭐였죠?"));
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].choice, "none");
    assert_eq!(done["tools"], json!(["search", "filter_ledger"]));
}

#[test]
fn small_model_is_not_nudged_back_into_tools() {
    let req = Request { meeting_ids: vec!["m1".into()], ..ask("이거 바꿀까요?") };
    let (_, seen) = small_run(&[priced("m1", "큐 회의")], vec![says("바꿔도 될지 알려 주세요?"), says("두 번째")], &req);
    assert_eq!(seen.len(), 1, "a question back stays the answer");
}

#[test]
fn a_hosted_model_still_picks_its_tools() {
    let panel = ScriptedPanel::new(vec![says("답이에요.")]);
    let (engine, dir) = harness_engine(&[priced("m1", "큐 회의")], panel.clone(), false);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let done = rt.block_on(harness::run(&engine, dir.path(), &ask("뭐였죠?"), |_| {}));
    assert_eq!(panel.seen()[0].choice, "auto");
    assert_eq!(done["tools"], json!([]));
}
