//! Backchannel detection and the meeting ledger (`is_backchannel`, `parse_ledger`, `parse_overview`,
//! `Engine::notes_from`).
//! The ledger model is a fake.

mod common {
    pub mod fakes;
}

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use common::fakes::{FakeAnswerer, FakeEmbed, FakeNoteWriter, FakeReranker, services};
use serde_json::{Value, json};
use tacie_engine::errors::Kind;
use tacie_engine::jev::Jev;
use tacie_engine::notes::{
    BACKCHANNELS, LEDGER_CHARS, is_backchannel, is_sound_only, keep_attendee_refs, ledger_object,
    one_line, parse_ledger, parse_overview, parse_question_review, spoken_seconds,
};
use tacie_engine::pipeline::Engine;
use tacie_engine::services::Services;
use tacie_engine::settings;
use tacie_index::{Index, Mask};

const MINI: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tacie-index/tests/fixtures/mini"
);

fn ids(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

/// 장부 모델만 가짜로 바꿔 끼운 엔진이에요. 색인은 쓰지 않아요.
fn engine(writer: &Arc<FakeNoteWriter>) -> Engine {
    let env = HashMap::new();
    let settings = settings::resolve(&env);
    let last = Arc::new(Mutex::new(HashMap::new()));
    let jev = Arc::new(Jev::from_config(
        Path::new("/nonexistent-tacie-config"),
        &settings,
        &env,
        last.clone(),
    ));
    let index = Index::open(Path::new(MINI), Mask::Plans, &Default::default()).unwrap();
    let services = Services {
        note_writer: writer.clone(),
        ..services(
            Arc::new(FakeReranker::all(0.9)),
            Arc::new(FakeAnswerer::ok("")),
        )
    };
    Engine::new(
        index,
        Arc::new(FakeEmbed::default()),
        services,
        settings,
        jev,
        last,
    )
}

/// (발화 id, "[id] 화자: 글") 줄들이에요.
fn lines(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(id, text)| (id.to_string(), format!("[{id}] 화자: {text}")))
        .collect()
}

#[test]
fn backchannel_list_and_rules() {
    for t in ["네", "아 네", "네네 알겠습니다", "그렇죠", "ok", "Yeah"] {
        assert!(is_backchannel(t), "{t}");
    }
    // ?, 진짜 질문, 12자 넘음, 빈 글
    for t in [
        "그래요?",
        "네?",
        "네 그거 어디서 바꿔요",
        "알겠습니다 알겠습니다 감사합니다",
        "",
    ] {
        assert!(!is_backchannel(t), "{t}");
    }
}

#[test]
fn backchannel_with_marks() {
    for t in [
        "네~",
        "아 네네.",
        "Okay!",
        "음…",
        "네, 네.",
        "음.",
        "좋아요. 감사합니다.",
    ] {
        assert!(is_backchannel(t), "{t}");
    }
}

#[test]
fn backchannel_counts_characters_and_reads_words_as_python_does() {
    assert_eq!(BACKCHANNELS.len(), 36);
    // 이어 붙인 글자 수(바이트가 아니에요)가 12까지예요.
    assert!(is_backchannel("알겠습니다 감사합니다 네네")); // 12자
    assert!(!is_backchannel("알겠습니다 감사합니다 그쵸 네")); // 13자
    assert!(!is_backchannel("네 ？")); // 전각 물음표도 질문이에요
    assert!(!is_backchannel("   \n "));
    // 단어 글자는 글자·숫자·밑줄뿐이에요: 결합 부호나 다른 이음표는 공백이 돼요.
    assert!(is_backchannel("네\u{301}"));
    assert!(is_backchannel("네\u{203f}네"));
    assert!(!is_backchannel("네_네"));
}

#[test]
fn one_line_turns_line_breaks_into_spaces() {
    assert_eq!(
        one_line("배포는\r\n목요일\n로 해요\r"),
        "배포는  목요일 로 해요 "
    );
}

#[test]
fn parse_ledger_drops_what_it_cannot_trust() {
    let raw = json!({"items": [
        {"kind": "decision", "text": "배포는 목요일로 해요.", "utterance_ids": ["s1u1"]},
        {"kind": "rumor", "text": "x", "utterance_ids": ["s1u1"]},
        {"kind": "decision", "text": "없는 근거", "utterance_ids": ["s1u9"]},
        {"kind": "commitment", "text": "근거 없음", "utterance_ids": []},
        {"kind": "open_question", "text": "가".repeat(201), "utterance_ids": ["s1u1"]},
        {"kind": "open_question", "text": "", "utterance_ids": ["s1u1"]},
        "그냥 문장", {"kind": "decision", "text": "모양이 틀린 근거", "utterance_ids": [{"id": "s1u1"}]},
        {"kind": ["decision"], "text": "목록 종류", "utterance_ids": ["s1u1"]}]})
    .to_string();
    let (items, dropped) = parse_ledger(&raw, &ids(&["s1u1", "s1u2"]), &HashMap::new()).unwrap();
    assert_eq!(
        items,
        [json!({"kind": "decision", "text": "배포는 목요일로 해요.", "utterance_ids": ["s1u1"]})]
    );
    assert_eq!(dropped, 8);
    assert_eq!(keys(&items[0]), ["kind", "text", "utterance_ids"]);
}

#[test]
fn parse_ledger_reads_ids_without_their_restart_prefix() {
    // 다시 쓰기는 줄을 "e0:s1u3" 꼴로 보내요. 모델은 앞부분을 떼고 "s1u3"로 돌려주곤 해요(2026-10-31, 항목이 다 버려졌어요).
    // 하나로만 맞으면 그 id로 돌려 놓고, 엔진이 다시 떠서 둘로 맞으면(e0:s1u1, e1:s1u1) 믿지 않아요.
    let raw = json!({"items": [
        {"kind": "decision", "text": "목요일에 배포해요.", "utterance_ids": ["s1u3"]},
        {"kind": "decision", "text": "그대로 쓴 id", "utterance_ids": ["e1:s1u2"]},
        {"kind": "decision", "text": "둘로 맞는 id", "utterance_ids": ["s1u1"]},
        {"kind": "decision", "text": "뒷부분만 같은 id", "utterance_ids": ["u3"]}],
        "summary": [{"text": "목요일 배포를 정했어요.", "utterance_ids": ["s1u3"]}]})
    .to_string();
    let sent = ids(&["e0:s1u1", "e0:s1u3", "e1:s1u1", "e1:s1u2"]);
    let (items, dropped) = parse_ledger(&raw, &sent, &HashMap::new()).unwrap();
    let refs: Vec<&Value> = items.iter().map(|it| &it["utterance_ids"]).collect();
    assert_eq!(refs, [&json!(["e0:s1u3"]), &json!(["e1:s1u2"])]);
    assert_eq!(dropped, 2);
    assert_eq!(
        parse_overview(&raw, &sent)["summary"],
        json!(["목요일 배포를 정했어요."])
    );
}

#[test]
fn parse_ledger_keeps_twelve_per_kind() {
    let got: Vec<Value> = (0..15)
        .map(
            |i| json!({"kind": "decision", "text": format!("결정 {i}"), "utterance_ids": ["s1u1"]}),
        )
        .collect();
    let raw = json!({ "items": got }).to_string();
    let (items, dropped) = parse_ledger(&raw, &ids(&["s1u1"]), &HashMap::new()).unwrap();
    assert_eq!((items.len(), dropped), (12, 3));
}

#[test]
fn ledger_reads_a_colon_slipped_in_after_the_kind() {
    // English meetings (2026-11-14): the model wrote `"kind": "open_question": "text": …` in about half the runs,
    // and the whole notes failed as unreadable
    let raw = r#"{"items": [
{"kind": "decision", "text": "Dropped the checklist.", "utterance_ids": ["s1u1"], "owner": null, "due": null},
{"kind": "open_question": "text": "Undecided: whether the email helps.", "utterance_ids": ["s1u2"], "owner": null, "due": null}
]}"#;
    let (items, dropped) = parse_ledger(raw, &ids(&["s1u1", "s1u2"]), &HashMap::new()).unwrap();
    assert_eq!(dropped, 0);
    let kinds: Vec<&str> = items.iter().map(|i| i["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["decision", "open_question"]);
    assert_eq!(items[1]["text"], "Undecided: whether the email helps.");
}

#[test]
fn ledger_reads_fenced_json() {
    let raw = "정리했어요.\n```json\n{\"items\": [{\"kind\": \"decision\", \"text\": \"목요일 배포\", \"utterance_ids\": [\"s1u1\"]}]}\n```";
    assert_eq!(
        parse_ledger(raw, &ids(&["s1u1"]), &HashMap::new())
            .unwrap()
            .0[0]["text"],
        "목요일 배포"
    );
    assert!(parse_ledger("JSON이 아니에요", &ids(&["s1u1"]), &HashMap::new()).is_err());
}

#[test]
fn ledger_reads_json_after_braces_in_a_preamble() {
    let raw = "형식 {kind, text}에 맞춰 정리했어요.\n{\"items\": [{\"kind\": \"decision\", \"text\": \"목요일 배포\", \"utterance_ids\": [\"s1u1\"]}]}\n끝 }";
    assert_eq!(
        parse_ledger(raw, &ids(&["s1u1"]), &HashMap::new())
            .unwrap()
            .0[0]["text"],
        "목요일 배포"
    );
    assert!(
        parse_ledger(
            "{\"kind\": \"decision\"} 뒤에 목록이 없어요",
            &ids(&["s1u1"]),
            &HashMap::new()
        )
        .is_err()
    );
}

#[test]
fn ledger_object_errors_say_why() {
    assert_eq!(
        ledger_object("{\"kind\": \"decision\"}").unwrap_err(),
        "no JSON object with items"
    );
    assert_eq!(
        parse_ledger(
            "{\"items\": {\"kind\": \"decision\"}}",
            &ids(&[]),
            &HashMap::new()
        )
        .unwrap_err(),
        "items is not a list"
    );
    // 안쪽 객체에 items가 있으면 그것도 찾아요({ 자리마다 읽어 봐요).
    let obj = ledger_object("{\"a\": {\"items\": [1]}").unwrap();
    assert_eq!(Value::Object(obj), json!({"items": [1]}));
}

fn owners(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(name, key)| (name.to_string(), key.to_string()))
        .collect()
}

#[test]
fn owner_maps_attendee_name_to_key() {
    let raw = json!({"items": [
        {"kind": "commitment", "text": "PR 올리기", "utterance_ids": ["s1u1"], "owner": "지수", "due": "금요일까지"},
        {"kind": "commitment", "text": "문구 공유", "utterance_ids": ["s1u1"], "owner": " 지수 ", "due": "가".repeat(21)},
        {"kind": "decision", "text": "3번까지 해요.", "utterance_ids": ["s1u1"], "owner": "지수"},
        {"kind": "commitment", "text": "  리뷰  ", "utterance_ids": ["s1u1"], "owner": 1, "due": "  내일  "},
        {"kind": "commitment", "text": "문서", "utterance_ids": ["s1u1"], "owner": "화자 2", "due": "   "}]})
    .to_string();
    let (items, _) = parse_ledger(&raw, &ids(&["s1u1"]), &owners(&[("지수", "화자 2")])).unwrap();
    assert_eq!(
        (&items[0]["owner"], &items[0]["due"]),
        (&json!("화자 2"), &json!("금요일까지"))
    );
    assert_eq!(
        (&items[1]["owner"], &items[1]["due"]),
        (&json!("화자 2"), &Value::Null)
    );
    assert!(items[2].get("owner").is_none());
    assert_eq!(
        items[3],
        json!({"kind": "commitment", "text": "리뷰", "utterance_ids": ["s1u1"], "owner": null, "due": "내일"})
    );
    // 모델이 이름 대신 키를 써도 그 사람이에요.
    assert_eq!(
        (&items[4]["owner"], &items[4]["due"]),
        (&json!("화자 2"), &Value::Null)
    );
    assert_eq!(
        keys(&items[0]),
        ["kind", "text", "utterance_ids", "owner", "due"]
    );
}

#[test]
fn owner_outside_meeting_is_kept_as_written() {
    let long = "가나다라마바사아자차카타파하가나다라마바사아자차카";
    assert_eq!(long.chars().count(), 25);
    let raw = json!({"items": [
        {"kind": "commitment", "text": "문구 공유", "utterance_ids": ["s1u1"], "owner": " 팀장 ", "due": null},
        {"kind": "commitment", "text": "정리", "utterance_ids": ["s1u1"], "owner": long, "due": null}]})
    .to_string();
    let (items, _) = parse_ledger(&raw, &ids(&["s1u1"]), &owners(&[("지수", "화자 2")])).unwrap();
    assert_eq!(items[0]["owner"], json!("팀장"));
    let cut: String = long.chars().take(20).collect();
    assert_eq!(items[1]["owner"], json!(cut));
}

#[test]
fn owner_empty_is_null() {
    let raw = json!({"items": [
        {"kind": "commitment", "text": "문구 공유", "utterance_ids": ["s1u1"], "owner": "   ", "due": null},
        {"kind": "commitment", "text": "정리", "utterance_ids": ["s1u1"], "owner": "", "due": null},
        {"kind": "commitment", "text": "확인", "utterance_ids": ["s1u1"], "owner": null, "due": null},
        {"kind": "commitment", "text": "공지", "utterance_ids": ["s1u1"], "due": null}]})
    .to_string();
    let (items, _) = parse_ledger(&raw, &ids(&["s1u1"]), &owners(&[("지수", "화자 2")])).unwrap();
    assert_eq!(items.len(), 4);
    assert!(
        items.iter().all(|it| it["owner"] == Value::Null),
        "{items:?}"
    );
}

#[test]
fn parse_overview_keeps_grounded_title_and_summary() {
    let raw = json!({"title": "결제 실패 재시도 정책", "items": [], "summary": [
        {"text": "3번까지 다시 시도해요.", "utterance_ids": ["s1u1"]},
        {"text": "근거 없는 줄", "utterance_ids": ["s1u9"]},
        {"text": "가".repeat(251), "utterance_ids": ["s1u1"]},
        {"text": "간격을 늘려요.", "utterance_ids": ["s1u2"]},
        {"text": "환불은 다음 주에 봐요.", "utterance_ids": ["s1u2"]},
        {"text": "네 번째 줄", "utterance_ids": ["s1u1"]}]})
    .to_string();
    let out = parse_overview(&raw, &ids(&["s1u1", "s1u2"]));
    assert_eq!(
        out,
        json!({"title": "결제 실패 재시도 정책",
               "summary": ["3번까지 다시 시도해요.", "간격을 늘려요.", "환불은 다음 주에 봐요."], "topics": []})
    );
    assert_eq!(keys(&out), ["title", "summary", "topics"]);
}

#[test]
fn parse_overview_keeps_topics_with_grounded_points_in_order() {
    let raw = json!({"title": "결제", "items": [], "summary": [], "topics": [
        {"title": "재시도 횟수", "points": [
            {"text": "민지가 재시도가 많다고 꺼냈어요 [s1u1].", "utterance_ids": ["s1u1"]},
            {"text": "근거 없는 문장", "utterance_ids": ["s1u9"]},
            {"text": "세 번까지로 했어요.[s1u1][e0:s1u2]", "utterance_ids": ["s1u2"]}]},
        {"title": "근거 없는 주제", "points": [
            {"text": "아무 근거도 없어요.", "utterance_ids": []}]},
        {"title": "환불", "points": [
            {"text": "다음 주에 봐요.", "utterance_ids": ["s1u2"]}]},
        {"title": "", "points": [{"text": "제목 없음", "utterance_ids": ["s1u1"]}]}]})
    .to_string();
    let out = parse_overview(&raw, &ids(&["s1u1", "s1u2"]));
    assert_eq!(
        out["topics"],
        json!([{"title": "재시도 횟수",
                "points": ["민지가 재시도가 많다고 꺼냈어요.", "세 번까지로 했어요."]},
               {"title": "환불", "points": ["다음 주에 봐요."]}])
    );
    // 열두 개까지, 주제마다 세 문장까지예요(세 단계 정리, docs/24 7절).
    let point = json!({"text": "말했어요.", "utterance_ids": ["s1u1"]});
    let topic = json!({"title": "주제", "points": vec![point; 10]});
    let raw = json!({"items": [], "topics": vec![topic; 14]}).to_string();
    let out = parse_overview(&raw, &ids(&["s1u1"]));
    let topics = out["topics"].as_array().unwrap();
    assert_eq!(topics.len(), 12);
    assert_eq!(topics[0]["points"].as_array().unwrap().len(), 3);
    assert!(topics[0].get("roles").is_none());
}

#[test]
fn parse_overview_keeps_roles_beside_points() {
    let raw = json!({"items": [], "topics": [{"title": "배포", "points": [
        {"text": "QA가 늦어졌음.", "role": "background", "utterance_ids": ["s1u1"]},
        {"text": "근거 없음.", "role": "discussion", "utterance_ids": ["s1u9"]},
        {"text": "목요일로 정함.", "role": "conclusion", "utterance_ids": ["s1u1"]},
        {"text": "역할 모름.", "role": "weird", "utterance_ids": ["s1u1"]}]}]})
    .to_string();
    let out = parse_overview(&raw, &ids(&["s1u1"]));
    assert_eq!(
        out["topics"],
        json!([{"title": "배포", "points": ["QA가 늦어졌음.", "목요일로 정함.", "역할 모름."],
                "roles": ["background", "conclusion", ""]}])
    );
}

#[test]
fn ledger_text_drops_inline_refs_and_takes_a_reason() {
    let reason = "재시도는 세 번까지로 했어요. 다섯 번이면 결제사 차단에 걸리고, 고객 문의가 지난달 두 배로 늘었던 일이 있어서 그보다 줄이기로 했어요. 간격은 처음 1분에서 시작해 두 배씩 늘리고, 환불 문의는 다음 회의에서 따로 봐요.";
    assert!(reason.chars().count() > 100 && reason.chars().count() <= 200);
    let raw = json!({"items": [{"kind": "decision", "text": format!("{reason} [s1u1]"), "utterance_ids": ["s1u1"]}]})
        .to_string();
    let (items, _) = parse_ledger(&raw, &ids(&["s1u1"]), &HashMap::new()).unwrap();
    assert_eq!(items[0]["text"], reason);
}

#[test]
fn parse_overview_without_fields_or_with_a_long_title() {
    assert_eq!(
        parse_overview("{\"items\": []}", &ids(&["s1u1"])),
        json!({"title": null, "summary": [], "topics": []})
    );
    let raw = json!({"title": "가".repeat(31), "items": []}).to_string();
    assert_eq!(parse_overview(&raw, &ids(&[]))["title"], Value::Null);
    assert_eq!(
        parse_overview("JSON이 아니에요", &ids(&[])),
        json!({"title": null, "summary": [], "topics": []})
    );
    // 앞뒤 공백은 빼고 30자까지예요.
    let raw = json!({"title": format!("  {}  ", "가".repeat(30)), "items": []}).to_string();
    assert_eq!(parse_overview(&raw, &ids(&[]))["title"], "가".repeat(30));
}

#[tokio::test]
async fn ledger_leaves_out_items_that_stand_only_on_questions_tacie_took() {
    let raw = json!({"items": [
        {"kind": "open_question", "text": "서명 위치를 정하지 못했어요.", "utterance_ids": ["s1u1"]},
        {"kind": "open_question", "text": "누가 맡을지 정하지 못했어요.", "utterance_ids": ["s1u2"]},
        {"kind": "decision", "text": "서명은 위에 둬요.", "utterance_ids": ["s1u1", "s1u2"]},
        {"kind": "decision", "text": "서명 위치는 위예요.", "utterance_ids": ["s1u1"]}]}) // 질문을 결정으로 읽었어요
    .to_string();
    let writer = Arc::new(FakeNoteWriter::ok(&raw));
    let engine = engine(&writer);
    // s1u1은 판단을 넘은 질문이에요: 답이나 문서 빈틈에 이미 있어요.
    let out = engine
        .notes_from(
            &lines(&[
                ("s1u1", "서명 기본 위치가 위예요, 아래예요?"),
                ("s1u2", "그건 누가 맡을지 정해야겠네요"),
            ]),
            &ids(&["s1u1"]),
        )
        .await;
    let texts: Vec<&str> = out["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["누가 맡을지 정하지 못했어요.", "서명은 위에 둬요."]);
    assert_eq!(out["dropped"], 2);
}

#[tokio::test]
async fn notes_from_answers_in_python_shapes() {
    // 줄이 없으면 모델을 부르지 않아요. ms, cost는 정수 0이에요.
    let writer = Arc::new(FakeNoteWriter::ok("{\"items\": []}"));
    let out = engine(&writer).notes_from(&[], &ids(&[])).await;
    assert_eq!(
        serde_json::to_string(&out).unwrap(),
        r#"{"kind":"meeting","kind_source":"auto","title":null,"summary":[],"topics":[],"items":[],"dropped":0,"ms":0,"cost":0}"#
    );
    assert!(writer.calls().is_empty());

    let one = lines(&[("s1u1", "배포는 목요일로 해요")]);
    let writer = Arc::new(FakeNoteWriter::fail(Kind::Network, "ledger request failed"));
    let out = engine(&writer).notes_from(&one, &ids(&[])).await;
    assert_eq!(
        serde_json::to_string(&out).unwrap(),
        r#"{"items":[],"error":"ledger request failed","error_kind":"network"}"#
    );
    // 오류 글은 앞 200자만이에요.
    let writer = Arc::new(FakeNoteWriter::fail(Kind::Down, &"가".repeat(300)));
    let out = engine(&writer).notes_from(&one, &ids(&[])).await;
    assert_eq!(out["error"], "가".repeat(200));
    assert_eq!(out["error_kind"], "down");

    for raw in ["JSON 아님", "{\"items\": \"x\"}"] {
        let writer = Arc::new(FakeNoteWriter::ok(raw));
        let out = engine(&writer).notes_from(&one, &ids(&[])).await;
        assert_eq!(
            serde_json::to_string(&out).unwrap(),
            r#"{"items":[],"error":"unreadable"}"#,
            "{raw}"
        );
    }

    let raw = json!({"items": [{"kind": "decision", "text": "목요일에 배포해요.", "utterance_ids": ["s1u1"]}],
                     "summary": [{"text": "목요일에 배포해요.", "utterance_ids": ["s1u2"]}], "title": "배포 일정"})
    .to_string();
    let writer = Arc::new(FakeNoteWriter::ok(&raw));
    let two = lines(&[("s1u1", "배포는 목요일로 해요"), ("s1u2", "네 그렇게 해요")]);
    let out = engine(&writer).notes_from(&two, &ids(&[])).await;
    assert_eq!(
        keys(&out),
        [
            "kind",
            "kind_source",
            "title",
            "summary",
            "topics",
            "items",
            "dropped",
            "ms",
            "cost"
        ]
    );
    assert_eq!(
        out,
        json!({"kind": "meeting", "kind_source": "auto", "title": "배포 일정", "summary": ["목요일에 배포해요."], "topics": [],
               "items": [{"kind": "decision", "text": "목요일에 배포해요.", "utterance_ids": ["s1u1"]}],
               "dropped": 0, "ms": 1.0, "cost": 0.01})
    );
    assert_eq!(
        writer.calls(),
        ["[s1u1] 화자: 배포는 목요일로 해요\n[s1u2] 화자: 네 그렇게 해요"]
    );
}

#[tokio::test]
async fn notes_from_keeps_the_latest_part_of_a_long_meeting() {
    // 줄마다 글자 수 + 1(줄바꿈)을 세서 40000자까지예요. 바이트가 아니라 글자예요.
    let raw = json!({"items": [
        {"kind": "decision", "text": "앞부분 결정", "utterance_ids": ["s1u1"]},
        {"kind": "decision", "text": "뒷부분 결정", "utterance_ids": ["s1u3"]}]})
    .to_string();
    let a = ("s1u1".to_string(), "가".repeat(20000));
    let b = ("s1u2".to_string(), "나".repeat(15000));
    let c = ("s1u3".to_string(), "다".repeat(10000));
    let writer = Arc::new(FakeNoteWriter::ok(&raw));
    let out = engine(&writer)
        .notes_from(&[a, b.clone(), c.clone()], &ids(&[]))
        .await;
    assert_eq!(writer.calls(), [format!("{}\n{}", b.1, c.1)]);
    // 버린 줄의 id는 근거가 되지 못해요.
    assert_eq!(out["items"][0]["text"], "뒷부분 결정");
    assert_eq!(out["dropped"], 1);

    // 딱 40000자면 그대로예요.
    let half = LEDGER_CHARS / 2 - 1;
    let d = ("s1u1".to_string(), "가".repeat(half));
    let e = ("s1u2".to_string(), "나".repeat(half));
    let writer = Arc::new(FakeNoteWriter::ok("{\"items\": []}"));
    engine(&writer).notes_from(&[d, e], &ids(&[])).await;
    assert_eq!(writer.calls()[0].chars().count(), 2 * half + 1);
}

#[test]
fn sound_only_is_tags_without_words() {
    for t in ["[게임 소리]", "[효과음] [배경음악]", " [타자 소리]. "] {
        assert!(is_sound_only(t), "{t}");
    }
    for t in ["[박수 소리] 쉽지 않네.", "게임 소리", "", "네", "[]  배포"] {
        assert!(!is_sound_only(t), "{t}");
    }
}

#[test]
fn spoken_seconds_spans_first_to_last_word() {
    assert_eq!(spoken_seconds(&[]), None);
    assert_eq!(spoken_seconds(&[(0.5, 0.75), (1.0, 1.5)]), Some(1.0));
    assert_eq!(spoken_seconds(&[(2.0, 1.0)]), Some(0.0));
}

#[test]
fn close_cut_keeps_the_finished_objects() {
    use tacie_engine::notes::close_cut;
    let raw = r#"```json
{"title": "배포", "topics": [{"title": "일정", "points": [{"text": "목요일 \"확정\"이에요 }", "utterance_ids": ["s1u1"]}, {"text": "금요"#;
    let closed: Value = serde_json::from_str(&close_cut(raw).unwrap()).unwrap();
    assert_eq!(
        closed,
        json!({"title": "배포", "topics": [{"title": "일정", "points": [
            {"text": "목요일 \"확정\"이에요 }", "utterance_ids": ["s1u1"]}]}]})
    );
    assert_eq!(close_cut(r#"{"items": [{"kind": "decision""#), None);
    assert_eq!(close_cut("JSON이 아니에요"), None);
}

#[test]
fn merge_notes_puts_the_overview_on_the_items() {
    use tacie_engine::notes::merge_notes;
    let overview = r#"앞말 {"title": "배포", "summary": [], "topics": [], "items": ["무시"]}"#;
    let items = r#"```json
{"items": [{"kind": "decision", "text": "목요일", "utterance_ids": ["s1u1"]}]}
```"#;
    let joined: Value = serde_json::from_str(&merge_notes(overview, items)).unwrap();
    assert_eq!(
        joined,
        json!({"items": [{"kind": "decision", "text": "목요일", "utterance_ids": ["s1u1"]}],
               "title": "배포", "summary": [], "topics": []})
    );
    // 앞 호출을 못 읽으면 items만이에요. 뒤 호출만 못 읽으면(작은 모델이 같은 줄을 되풀이하다 잘린 할 일) 요약·주제는
    // 남기고 items는 비워요: 다 버리면 회의 정리가 빈 화면이 돼요(오픈소스판 e2e, 2026-11-25). 둘 다 못 읽으면 그 글 그대로예요.
    assert_eq!(
        serde_json::from_str::<Value>(&merge_notes("엉망", r#"{"items": []}"#)).unwrap(),
        json!({"items": []})
    );
    assert_eq!(
        serde_json::from_str::<Value>(&merge_notes(overview, r#"{"items": [{"kind": "commitment", "text": "되풀이"#)).unwrap(),
        json!({"items": [], "title": "배포", "summary": [], "topics": []})
    );
    assert_eq!(merge_notes("엉망", "엉망"), "엉망");
}

#[test]
fn attendee_refs_stay_and_other_names_lose_their_brackets() {
    let mut notes = json!({
        "title": "t",
        "summary": ["[[민재]]가 [[화자 2]]에게 넘김. [[지원]]은 없음.", "화자 2와 화자 9가 맡음."],
        "topics": [{"title": "a", "points": ["[[ 민재 ]]가 꺼냄.", "[[]] 그대로"]}],
    });
    let names: HashSet<String> = ["민재", "화자 2"].iter().map(|s| s.to_string()).collect();
    keep_attendee_refs(&mut notes, &names);
    assert_eq!(
        notes["summary"][0],
        "[[민재]]가 [[화자 2]]에게 넘김. 지원은 없음."
    );
    assert_eq!(notes["summary"][1], "[[화자 2]]와 화자 9가 맡음.");
    assert_eq!(notes["topics"][0]["points"][0], "[[민재]]가 꺼냄.");
    assert_eq!(notes["topics"][0]["points"][1], "[[]] 그대로");
}

// ---- 녹음 유형(docs/23) ----

use tacie_engine::notes::parse_ledger_for;
use tacie_engine::notes_kind::{CLASSIFY_CHARS, NotesKind, classify_sample, parse_kind};

#[test]
fn parse_kind_reads_kind_and_confidence() {
    assert_eq!(
        parse_kind(r#"{"kind":"interview","confidence":0.9}"#),
        NotesKind::Interview
    );
    assert_eq!(
        parse_kind(r#"{"kind":"talk","confidence":0.7}"#),
        NotesKind::Talk
    );
    assert_eq!(
        parse_kind(r#"{"kind":"interview","confidence":0.69}"#),
        NotesKind::Meeting
    );
    assert_eq!(
        parse_kind(r#"{"kind":"podcast","confidence":1}"#),
        NotesKind::Meeting
    );
    assert_eq!(parse_kind(r#"{"kind":"talk"}"#), NotesKind::Meeting);
    assert_eq!(parse_kind("not json"), NotesKind::Meeting);
    assert_eq!(
        parse_kind("```json\n{\"kind\": \"talk\", \"confidence\": 0.95}\n```"),
        NotesKind::Talk
    );
}

#[test]
fn notes_kind_names_round_trip() {
    for k in [NotesKind::Meeting, NotesKind::Interview, NotesKind::Talk] {
        assert_eq!(NotesKind::parse(k.as_str()), Some(k));
    }
    assert_eq!(NotesKind::parse("podcast"), None);
    assert_eq!(
        NotesKind::Meeting.prompt_names(),
        ("notes_overview_v2", "notes_items")
    );
    assert_eq!(
        NotesKind::Interview.prompt_names(),
        ("notes_interview_overview", "notes_interview_items")
    );
    assert_eq!(
        NotesKind::Talk.prompt_names(),
        ("notes_talk_overview", "notes_talk_items")
    );
    assert_eq!(
        NotesKind::Meeting.item_kinds(),
        &["decision", "commitment", "open_question"]
    );
    assert_eq!(NotesKind::Interview.item_kinds(), &["fact", "quote"]);
    assert_eq!(NotesKind::Talk.item_kinds(), &["concept", "takeaway"]);
}

#[test]
fn classify_sample_keeps_whole_lines_up_to_3000_chars() {
    assert_eq!(CLASSIFY_CHARS, 3000);
    let line = "가".repeat(1000);
    let four = vec![line.clone(); 4].join("\n");
    let s = classify_sample(&four);
    assert_eq!(s, format!("{line}\n{line}"));
    let long = "나".repeat(4000);
    assert_eq!(classify_sample(&long).chars().count(), 3000);
    assert_eq!(classify_sample("짧아요"), "짧아요");
}

#[test]
fn parse_ledger_drops_kinds_of_other_type() {
    let raw = json!({"items": [
        {"kind": "fact", "text": "거래액 30억 원", "utterance_ids": ["s1u1"]},
        {"kind": "quote", "text": "[[하늘]] 여러 번 던져 봐요", "utterance_ids": ["s1u1"]},
        {"kind": "decision", "text": "패션으로 정함", "utterance_ids": ["s1u1"]},
    ]})
    .to_string();
    let (items, dropped) = parse_ledger_for(
        &raw,
        &ids(&["s1u1"]),
        &HashMap::new(),
        NotesKind::Interview.item_kinds(),
    )
    .unwrap();
    let kinds: Vec<&str> = items.iter().map(|i| i["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["fact", "quote"]);
    assert_eq!(dropped, 1);
}

#[test]
fn parse_ledger_new_kinds_have_no_owner() {
    let raw = json!({"items": [
        {"kind": "concept", "text": "롱테일: 작은 수요가 모여 큰 시장", "utterance_ids": ["s1u1"], "owner": "화자 1", "due": "내일"},
    ]})
    .to_string();
    let (items, _) = parse_ledger_for(
        &raw,
        &ids(&["s1u1"]),
        &HashMap::new(),
        NotesKind::Talk.item_kinds(),
    )
    .unwrap();
    assert_eq!(keys(&items[0]), ["kind", "text", "utterance_ids"]);
}

// ---- 분류하고 유형대로 쓰기(docs/23 3-3절) ----

#[tokio::test]
async fn notes_with_classifies_then_writes_that_kind() {
    let writer = Arc::new(
        FakeNoteWriter::ok(r#"{"items": []}"#)
            .with_classify(r#"{"kind":"interview","confidence":0.9}"#),
    );
    let two = lines(&[("s1u1", "왜 창업했나요"), ("s1u2", "선배들을 보고요")]);
    let out = engine(&writer).notes_with(&two, &[], &ids(&[]), None, None).await;
    assert_eq!(*writer.kinds.lock().unwrap(), [NotesKind::Interview]);
    assert_eq!(out["kind"], "interview");
    assert_eq!(out["kind_source"], "auto");
    assert_eq!(
        keys(&out),
        [
            "kind",
            "kind_source",
            "title",
            "summary",
            "topics",
            "items",
            "dropped",
            "ms",
            "cost"
        ]
    );
}

#[tokio::test]
async fn notes_with_user_kind_skips_classify() {
    let writer = Arc::new(
        FakeNoteWriter::ok(r#"{"items": []}"#)
            .with_classify(r#"{"kind":"interview","confidence":0.9}"#),
    );
    let two = lines(&[("s1u1", "오늘은 임베딩을 설명할게요")]);
    let out = engine(&writer)
        .notes_with(&two, &[], &ids(&[]), Some(NotesKind::Talk), None)
        .await;
    assert!(writer.classify_calls.lock().unwrap().is_empty());
    assert_eq!(*writer.kinds.lock().unwrap(), [NotesKind::Talk]);
    assert_eq!(out["kind"], "talk");
    assert_eq!(out["kind_source"], "user");
}

#[tokio::test]
async fn classify_failure_falls_back_to_meeting() {
    let writer = Arc::new(FakeNoteWriter::ok(r#"{"items": []}"#).classify_fail());
    let two = lines(&[("s1u1", "배포는 목요일로 해요")]);
    let out = engine(&writer).notes_with(&two, &[], &ids(&[]), None, None).await;
    assert_eq!(*writer.kinds.lock().unwrap(), [NotesKind::Meeting]);
    assert_eq!(out["kind"], "meeting");
    assert_eq!(out["kind_source"], "auto");
    assert!(out.get("error").is_none());
}

#[tokio::test]
async fn classify_sees_only_the_first_3000_chars() {
    let writer = Arc::new(
        FakeNoteWriter::ok(r#"{"items": []}"#)
            .with_classify(r#"{"kind":"meeting","confidence":0.9}"#),
    );
    let long: Vec<(String, String)> = (0..40)
        .map(|i| {
            (
                format!("s1u{i}"),
                format!("[s1u{i}] 화자: {}", "가".repeat(200)),
            )
        })
        .collect();
    engine(&writer)
        .notes_with(&long, &[], &ids(&[]), None, None)
        .await;
    let sent = writer.classify_calls.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].chars().count() <= 3000);
    assert!(sent[0].starts_with("[s1u0] 화자:"));
}

#[tokio::test]
async fn notes_cost_includes_classify() {
    // 가짜 장부: ms 1.0, cost 0.01. 가짜 분류: ms 2.0, cost 0.005.
    let writer = Arc::new(
        FakeNoteWriter::ok(r#"{"items": []}"#).with_classify(r#"{"kind":"talk","confidence":0.9}"#),
    );
    let two = lines(&[("s1u1", "설명할게요")]);
    let out = engine(&writer).notes_with(&two, &[], &ids(&[]), None, None).await;
    assert_eq!(out["ms"], 3.0);
    assert_eq!(out["cost"], 0.015);
}

#[tokio::test]
async fn empty_notes_say_meeting_without_classifying() {
    let writer = Arc::new(
        FakeNoteWriter::ok(r#"{"items": []}"#).with_classify(r#"{"kind":"talk","confidence":0.9}"#),
    );
    let out = engine(&writer).notes_with(&[], &[], &ids(&[]), None, None).await;
    assert_eq!(out["kind"], "meeting");
    assert_eq!(out["kind_source"], "auto");
    assert!(writer.classify_calls.lock().unwrap().is_empty());
}

#[test]
fn question_review_keeps_known_questions_and_grounded_answers() {
    // docs/24 4절: 받은 질문 id만, 처음 나온 것만이에요. answered는 답 글과 보낸 발화의 근거가 있어야 하고,
    // 아니면 doc이에요. 다시 쓴 질문이 없거나 길면 null이에요.
    let raw = r#"```json
{"questions": [
 {"id": "v1", "verdict": "answered", "question": "결제 재시도는 몇 번인가요?", "answer": "[[민호]]: 3회라고 답함.", "utterance_ids": ["s1u5"]},
 {"id": "v2", "verdict": "answered", "question": "배포는 어디서 하나요?", "answer": "모름", "utterance_ids": ["s9u9"]},
 {"id": "v3", "verdict": "not_question", "question": "", "answer": null, "utterance_ids": []},
 {"id": "v4", "verdict": "maybe", "question": "x", "answer": null, "utterance_ids": []},
 {"id": "v9", "verdict": "doc", "question": "모르는 질문", "answer": null, "utterance_ids": []},
 {"id": "v1", "verdict": "doc", "question": "두 번째", "answer": null, "utterance_ids": []}
]}
```"#;
    let asked: HashMap<String, Option<String>> = ["v1", "v2", "v3", "v4"]
        .iter()
        .map(|q| (q.to_string(), Some("s1u4".to_string())))
        .collect();
    let out = parse_question_review(raw, &asked, &ids(&["s1u4", "s1u5"])).unwrap();
    assert_eq!(
        Value::Array(out),
        json!([
            {"id": "v1", "verdict": "answered", "question": "결제 재시도는 몇 번인가요?", "answer": "[[민호]]: 3회라고 답함.", "utterance_ids": ["s1u5"]},
            {"id": "v2", "verdict": "doc", "question": "배포는 어디서 하나요?", "answer": null, "utterance_ids": []},
            {"id": "v3", "verdict": "not_question", "question": null, "answer": null, "utterance_ids": []}
        ])
    );
    assert!(parse_question_review("못 읽어요", &asked, &ids(&["s1u1"])).is_err());

    // 근거 중 모르는 id는 빼고 남은 걸로 answered예요. 질문 줄만 근거면 답이 아니에요. 긴 답은 잘라요.
    let long = "가".repeat(260);
    let raw = format!(
        r#"{{"questions": [
 {{"id": "v1", "verdict": "answered", "question": "a?", "answer": "[[화자 1]]: 3회.", "utterance_ids": ["s9u9", "s1u5"]}},
 {{"id": "v2", "verdict": "answered", "question": "b?", "answer": "네", "utterance_ids": ["s1u4"]}},
 {{"id": "v3", "verdict": "answered", "question": "c?", "answer": "{long}", "utterance_ids": ["s1u5"]}},
 {{"id": "v4", "verdict": "answered", "question": "d?", "answer": "[[화자]]: 명확한 답은 없음.", "utterance_ids": ["s1u5"]}}
]}}"#
    );
    let out = parse_question_review(&raw, &asked, &ids(&["s1u4", "s1u5"])).unwrap();
    assert_eq!(
        (out[0]["verdict"].clone(), out[0]["utterance_ids"].clone()),
        (json!("answered"), json!(["s1u5"]))
    );
    assert_eq!(
        (out[1]["verdict"].clone(), out[1]["answer"].clone()),
        (json!("doc"), json!(null))
    );
    assert_eq!(out[3]["verdict"], json!("doc"));
    let cut = out[2]["answer"].as_str().unwrap();
    assert!(
        out[2]["verdict"] == "answered" && cut.chars().count() == 200 && cut.ends_with('…'),
        "{cut}"
    );
}

fn past(rows: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    rows.iter()
        .map(|(a, t, d)| (a.to_string(), t.to_string(), d.to_string()))
        .collect()
}

/// 바뀐 결정(docs/30 8절): 이번 결정과 지난 결정을 번호 줄로 보내고, 모델이 쓴 1부터 번호를 0부터로 돌려줘요.
/// 범위 밖 번호, 짝이 이미 있는 이번 결정, 두 결정에 같이 나오지 않는 대상(subject)의 짝은 버려요.
#[tokio::test]
async fn decision_changes_sends_numbered_lines_and_reads_pairs() {
    let writer = Arc::new(FakeNoteWriter::ok(
        "```json\n{\"changes\": [{\"new\": 2, \"past\": 1, \"subject\": \"재시도\"}, {\"new\": 2, \"past\": 2, \"subject\": \"재시도\"}, {\"new\": 9, \"past\": 1, \"subject\": \"재시도\"}, {\"new\": 1, \"past\": 2, \"subject\": \"화면\"}]}\n```",
    ));
    let out = engine(&writer)
        .decision_changes(
            &["화면은 다크 모드부터".into(), "재시도를 5번으로\n늘림".into()],
            &past(&[
                ("2026-10-30", "결제 정리", "재시도는 3번까지"),
                ("2026-10-21", "킥오프", "배포는 금요일"),
            ]),
        )
        .await;
    assert_eq!(out["changes"], json!([{"decision": 1, "past": 0}]));
    let calls = writer.changes.lock().unwrap().clone();
    assert_eq!(calls[0].0, "[1] 화면은 다크 모드부터\n[2] 재시도를 5번으로 늘림");
    assert_eq!(
        calls[0].1,
        "[1] 2026-10-30 · 결제 정리 · 재시도는 3번까지\n[2] 2026-10-21 · 킥오프 · 배포는 금요일"
    );
}

/// 모델이 지난 결정 하나에 이번 결정 여럿을 몰아 붙이면(관계없는 결정들) 그 짝은 버려요.
#[tokio::test]
async fn decision_changes_drop_one_past_decision_taken_by_many() {
    let writer = Arc::new(FakeNoteWriter::ok(
        "{\"changes\": [{\"new\": 1, \"past\": 1, \"subject\": \"주제\"}, {\"new\": 2, \"past\": 1, \"subject\": \"주제\"}, {\"new\": 3, \"past\": 1, \"subject\": \"주제\"}]}",
    ));
    let out = engine(&writer)
        .decision_changes(
            &["주제 A로 정함".into(), "주제 B로 정함".into(), "주제 C로 정함".into()],
            &past(&[("2026-10-25", "t", "오늘 주제를 인덱스로 정했어요")]),
        )
        .await;
    assert_eq!(out["changes"], json!([]));
}

#[tokio::test]
async fn decision_changes_skips_the_call_without_past_decisions() {
    let writer = Arc::new(FakeNoteWriter::ok("{\"changes\": []}"));
    let out = engine(&writer)
        .decision_changes(&["재시도를 5번으로 늘림".into()], &[])
        .await;
    assert_eq!(out["changes"], json!([]));
    assert!(writer.changes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn decision_changes_failure_is_an_empty_list_with_the_kind() {
    let writer = Arc::new(FakeNoteWriter::fail(Kind::Credit, "no credit"));
    let out = engine(&writer)
        .decision_changes(&["a".into()], &past(&[("2026-10-30", "t", "b")]))
        .await;
    assert_eq!(out["changes"], json!([]));
    assert_eq!(out["error_kind"], json!("credit"));
}
