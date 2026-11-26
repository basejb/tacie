//! The iPhone engine's panel harness (docs/40 4-3, 4-5절): it picks the iPhone prompt and tool set pair, and when a
//! speaker is named with a name that already exists, it tells the model they cannot be merged instead of proposing
//! a merge.

mod common {
    pub mod fakes;
    pub mod fixture;
}

use std::collections::HashMap;

use common::fakes::{ScriptedCall, ScriptedPanel};
use common::fixture::harness_engine;
use serde_json::{Value, json};
use tacie_engine::harness::{self, Request};
use tacie_engine::services::{ChatTurn, ToolCall};

fn run_with(iphone: bool, req: &Request) -> (Value, Vec<ScriptedCall>) {
    let call = ToolCall {
        id: "c1".into(),
        name: "name_speaker".into(),
        arguments: json!({"speaker": "화자 1", "name": "지윤"}).to_string(),
    };
    let panel = ScriptedPanel::new(vec![
        ChatTurn { calls: vec![call], ..Default::default() },
        ChatTurn { content: "화자를 합칠 수 없어요.".into(), ..Default::default() },
    ]);
    let (engine, dir) = harness_engine(&[], panel.clone(), iphone);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let done = rt.block_on(harness::run(&engine, dir.path(), req, |_| {}));
    (done, panel.seen())
}

fn meeting_page() -> Request {
    let line = |key: &str, voice: &str, speaker: &str| json!({"key": key, "voice": voice, "speaker": speaker, "at": "", "text": "네"});
    Request {
        text: "화자 1을 지윤으로 바꿔 줘".into(),
        meeting_ids: vec!["m1".into()],
        today: "2026-11-17".into(),
        transcripts: HashMap::from([(
            "m1".to_string(),
            vec![line("k1", "화자 1", "화자 1"), line("k2", "화자 2", "지윤")],
        )]),
        ..Default::default()
    }
}

#[test]
fn iphone_name_taken_is_not_merged() {
    let (done, seen) = run_with(true, &meeting_page());
    assert_eq!(seen[0].name, "panel_chat_iphone");
    assert_eq!(seen[0].tools, "panel_iphone_meeting");
    let result = seen[1].turns.iter().find(|t| t["role"] == "tool").unwrap()["content"].as_str().unwrap().to_string();
    assert!(result.contains("iPhone에서는 아직 화자를 합칠 수 없어요"), "{result}");
    assert_eq!(done["proposals"], json!([]));
}

#[test]
fn mac_name_taken_still_merges() {
    let (done, seen) = run_with(false, &meeting_page());
    assert_eq!(seen[0].name, "panel_chat");
    assert_eq!(seen[0].tools, "panel_meeting_edit");
    assert_eq!(done["proposals"][0]["kind"], "merge_speakers");
}

#[test]
fn iphone_all_meetings_use_the_iphone_scope() {
    let req = Request { text: "이번 달 정한 거?".into(), today: "2026-11-17".into(), ..Default::default() };
    let (_, seen) = run_with(true, &req);
    assert_eq!(seen[0].name, "panel_chat_iphone");
    assert_eq!(seen[0].tools, "panel_iphone");
    assert_eq!(seen[0].scope, "이 iPhone의 모든 회의");
}

#[test]
fn iphone_english_question_uses_the_english_twin() {
    let req = Request { text: "What did we decide this month?".into(), today: "2026-11-17".into(), ..Default::default() };
    let (_, seen) = run_with(true, &req);
    assert_eq!(seen[0].name, "panel_chat_iphone_en");
    assert_eq!(seen[0].scope, "All meetings on this iPhone");
}

/// 도구 결과의 자리 말(`here`)이 iPhone에서는 "이 iPhone에"예요.
#[test]
fn iphone_tool_results_say_this_iphone() {
    let rewrite = |iphone: bool| {
        let call = ToolCall { id: "c1".into(), name: "rewrite_notes".into(), arguments: json!({"request": "짧게"}).to_string() };
        let panel = ScriptedPanel::new(vec![
            ChatTurn { calls: vec![call], ..Default::default() },
            ChatTurn { content: "다시 쓸 수 없어요.".into(), ..Default::default() },
        ]);
        let (engine, dir) = harness_engine(&[], panel.clone(), iphone);
        let req = Request { text: "정리 짧게 다시 써 줘".into(), meeting_ids: vec!["m1".into()], today: "2026-11-17".into(), ..Default::default() };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(harness::run(&engine, dir.path(), &req, |_| {}));
        let seen = panel.seen();
        seen[1].turns.iter().find(|t| t["role"] == "tool").unwrap()["content"].as_str().unwrap().to_string()
    };
    let on_iphone = rewrite(true);
    assert!(on_iphone.contains("이 iPhone에"), "{on_iphone}");
    assert!(!on_iphone.contains("Mac"), "{on_iphone}");
    assert!(rewrite(false).contains("이 Mac에"));
}

fn meeting(id: &str, title: &str) -> Value {
    json!({"id": id, "title": title, "start": "2026-11-18T10:00:00+09:00", "end": "2026-11-18T11:00:00+09:00",
           "summary": ["음성 인식을 시험했어요."], "topics": [], "ledger": []})
}

fn open_call(id: &str) -> ToolCall {
    ToolCall { id: "c1".into(), name: "open_meeting".into(), arguments: json!({"meeting_id": id}).to_string() }
}

/// 모든 회의 대화에서 고칠 회의를 넘기면(docs/40 12절) 질문이 끝나고 `done`에 `handoff`가 붙어요. 단계 줄은 그 회의
/// 제목을 붙인 시작 하나뿐이에요(끝 단계는 앱의 다음 요청이 이어 가요). 같은 라운드의 다른 도구는 돌리지 않아요.
#[test]
fn iphone_open_meeting_hands_off() {
    let search = ToolCall { id: "c2".into(), name: "list_meetings".into(), arguments: "{}".into() };
    let panel = ScriptedPanel::new(vec![ChatTurn { calls: vec![open_call("m1"), search], ..Default::default() }]);
    let (engine, dir) = harness_engine(&[meeting("m1", "대면 회의 음성 인식 테스트")], panel.clone(), true);
    let req = Request { text: "거기서 화자 1을 나로 바꿔".into(), today: "2026-11-18".into(), ..Default::default() };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut steps = Vec::new();
    let done = rt.block_on(harness::run(&engine, dir.path(), &req, |s| steps.push(s)));
    assert_eq!(done["handoff"], json!({"meeting_id": "m1"}));
    assert_eq!(done["answer"], "");
    assert_eq!(done["proposals"], json!([]));
    assert_eq!(done["tools"], json!(["open_meeting"]));
    assert_eq!(panel.seen().len(), 1, "no second model call");
    let opened: Vec<&Value> = steps.iter().filter(|s| s["step"]["tool"] == "open_meeting").collect();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0]["step"]["args"], json!({"meeting_id": "m1", "title": "대면 회의 음성 인식 테스트"}));
    assert!(steps.iter().all(|s| s["step"]["done"].is_null()), "{steps:?}");
}

/// 없는 회의 id면 넘기지 않고 모델에게 실패를 돌려줘요. 모델은 이어서 답해요.
#[test]
fn iphone_open_meeting_unknown_id_goes_back_to_the_model() {
    let panel = ScriptedPanel::new(vec![
        ChatTurn { calls: vec![open_call("nope")], ..Default::default() },
        ChatTurn { content: "어느 회의인지 알려 주세요.".into(), ..Default::default() },
    ]);
    let (engine, dir) = harness_engine(&[meeting("m1", "주간 회의")], panel.clone(), true);
    let req = Request { text: "화자 1을 나로 바꿔".into(), today: "2026-11-18".into(), ..Default::default() };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let done = rt.block_on(harness::run(&engine, dir.path(), &req, |_| {}));
    assert!(done.get("handoff").is_none(), "{done}");
    assert_eq!(done["answer"], "어느 회의인지 알려 주세요.");
    let seen = panel.seen();
    let result = seen[1].turns.iter().find(|t| t["role"] == "tool").unwrap()["content"].as_str().unwrap().to_string();
    assert!(result.contains("그런 회의가 없어요"), "{result}");
}

/// 회의 페이지 대화(넘긴 뒤 그 회의의 대화 기록을 붙여 다시 보낸 요청도)는 넘기지 않아요: 한 질문에 넘김은 한 번이에요.
#[test]
fn iphone_meeting_page_never_hands_off() {
    let panel = ScriptedPanel::new(vec![
        ChatTurn { calls: vec![open_call("m1")], ..Default::default() },
        ChatTurn { content: "이 회의예요.".into(), ..Default::default() },
    ]);
    let (engine, dir) = harness_engine(&[meeting("m1", "주간 회의")], panel.clone(), true);
    let mut req = meeting_page();
    req.history = vec![("오늘 회의 뭐했지".into(), "「주간 회의」 하나가 있어요.".into())];
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let done = rt.block_on(harness::run(&engine, dir.path(), &req, |_| {}));
    assert!(done.get("handoff").is_none(), "{done}");
    let seen = panel.seen();
    assert_eq!(seen[0].tools, "panel_iphone_meeting");
    let result = seen[1].turns.iter().find(|t| t["role"] == "tool").unwrap()["content"].as_str().unwrap().to_string();
    assert!(result.contains("없는 도구예요"), "{result}");
}

/// Mac 엔진은 넘기지 않아요(Mac 묶음에는 이 도구가 없어요).
#[test]
fn mac_never_hands_off() {
    let panel = ScriptedPanel::new(vec![
        ChatTurn { calls: vec![open_call("m1")], ..Default::default() },
        ChatTurn { content: "네.".into(), ..Default::default() },
    ]);
    let (engine, dir) = harness_engine(&[meeting("m1", "주간 회의")], panel.clone(), false);
    let req = Request { text: "화자 1을 나로 바꿔".into(), today: "2026-11-18".into(), ..Default::default() };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let done = rt.block_on(harness::run(&engine, dir.path(), &req, |_| {}));
    assert!(done.get("handoff").is_none(), "{done}");
}
