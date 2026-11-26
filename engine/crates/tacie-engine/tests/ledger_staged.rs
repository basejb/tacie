//! Hooks three-stage notes for Korean meetings (`notes_v2`, docs/24 7절) up to a fake server. The per-agenda calls
//! and the ledger call go out together, so their arrival order is not fixed: the fake server gives every call the
//! same answer, and that answer holds all the keys each stage reads.
#![cfg(feature = "local-prompts")]

mod common {
    pub mod fake_openrouter;
}

use common::fake_openrouter::{FakeOpenRouter, client, reply};
use serde_json::{Value, json};
use tacie_engine::lang::Lang;
use tacie_engine::notes_kind::NotesKind;
use tacie_engine::services::NoteWriter;

const MEETING: &str = "[s1u1] 민재: 배포 일정 이야기해요\n[s1u2] 수아: 목요일에 배포해요\n[s1u3] 민재: 좋아요";

fn sent(fake: &FakeOpenRouter) -> Vec<String> {
    fake.bodies()
        .iter()
        .map(|b| b["messages"][0]["content"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn korean_meeting_is_written_in_three_stages() {
    let all = json!({
        "agenda": [{"title": "배포 일정", "ranges": [["s1u1", "s1u3"]]}],
        "points": [{"role": "conclusion", "text": "목요일 배포로 정함.", "utterance_ids": ["s1u2"]}],
        "items": [{"kind": "decision", "text": "목요일 배포로 정함.", "utterance_ids": ["s1u2"]}],
        "title": "배포 일정",
        "summary": [{"text": "목요일 배포로 정함.", "utterance_ids": ["s1u2"]}],
    })
    .to_string();
    let fake = FakeOpenRouter::start(vec![(200, reply(&all, "stop")); 4]).await;
    let (or, _, _dir) = client(true, &fake.url_base, &[]);
    let out = or
        .ledger(MEETING, &["민재".into(), "수아".into()], NotesKind::Meeting)
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(v["title"], "배포 일정");
    assert_eq!(v["summary"][0]["text"], "목요일 배포로 정함.");
    assert_eq!(v["topics"][0]["title"], "배포 일정");
    assert_eq!(v["topics"][0]["points"][0]["role"], "conclusion");
    assert_eq!(v["items"][0]["kind"], "decision");
    assert_eq!(out.cost, 0.00123 * 4.0);

    let prompts = sent(&fake);
    assert_eq!(prompts.len(), 4);
    // 첫 호출은 안건 나누기, 마지막은 요약이에요. 요약에는 대화 원문이 없고 회의록이 가요.
    assert!(prompts[0].contains("회의에서 다룬 안건을 나눠요"));
    assert!(prompts[3].contains("<회의록>\n안건 1. 배포 일정\n- (결론) 목요일 배포로 정함. [s1u2]"));
    assert!(!prompts[3].contains("[s1u1] 민재"));
    let middle = &prompts[1..3];
    assert!(middle.iter().any(|p| p.contains("안건: 배포 일정\n") && p.contains("[s1u2] 수아: 목요일에 배포해요")));
    assert!(middle.iter().any(|p| p.contains("<안건>\n1. 배포 일정\n</안건>")));
}

#[tokio::test]
async fn no_agenda_falls_back_to_two_calls() {
    let fake = FakeOpenRouter::start(vec![(200, reply(r#"{"agenda": []}"#, "stop")); 3]).await;
    let (or, _, _dir) = client(true, &fake.url_base, &[]);
    let out = or.ledger(MEETING, &[], NotesKind::Meeting).await;
    assert!(out.is_ok());
    let prompts = sent(&fake);
    assert_eq!(prompts.len(), 3);
    assert!(prompts[1..].iter().all(|p| !p.contains("안건을 나눠요")));
}

#[tokio::test]
async fn english_meeting_is_written_in_the_same_three_stages() {
    let all = json!({
        "agenda": [{"title": "Release date", "ranges": [["s1u1", "s1u3"]]}],
        "points": [{"role": "discussion", "text": "Weighed Thursday against Friday.", "utterance_ids": ["s1u2"]}],
        "items": [{"kind": "decision", "text": "Ship on Thursday.", "utterance_ids": ["s1u2"]}],
        "title": "Release date",
        "summary": [{"text": "Agreed to ship on Thursday.", "utterance_ids": ["s1u2"]}],
    })
    .to_string();
    let fake = FakeOpenRouter::start(vec![(200, reply(&all, "stop")); 4]).await;
    let (or, _, _dir) = client(true, &fake.url_base, &[]);
    let meeting = "[s1u1] Min: let's talk about the release date\n[s1u2] Sua: we ship on Thursday then\n[s1u3] Min: sounds good to me";
    let out = or.ledger(meeting, &["Min".into(), "Sua".into()], NotesKind::Meeting).await.unwrap();
    let v: Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(v["title"], "Release date");
    assert_eq!(v["topics"][0]["title"], "Release date");
    assert_eq!(v["items"][0]["kind"], "decision");

    let prompts = sent(&fake);
    assert_eq!(prompts.len(), 4);
    assert!(prompts[0].contains("split it into the agenda items"));
    assert!(prompts[3].contains("<회의록>\nTopic 1. Release date\n- (discussion) Weighed Thursday against Friday. [s1u2]"));
    assert!(prompts.iter().all(|p| !p.contains("안건을 나눠요") && !p.contains("(앞뒤)")));
}

/// 설정 → 회의 → "회의 정리 언어"가 "앱 언어로"이고 앱이 한국어면(2026-11-19), 영어 회의도 한국어 프롬프트로 써요.
#[tokio::test]
async fn english_meeting_in_korean_uses_the_korean_prompts() {
    let all = json!({
        "agenda": [{"title": "출시일", "ranges": [["s1u1", "s1u3"]]}],
        "points": [{"role": "conclusion", "text": "목요일 출시로 정함.", "utterance_ids": ["s1u2"]}],
        "items": [{"kind": "decision", "text": "목요일 출시로 정함.", "utterance_ids": ["s1u2"]}],
        "title": "출시일",
        "summary": [{"text": "목요일 출시로 정함.", "utterance_ids": ["s1u2"]}],
    })
    .to_string();
    let fake = FakeOpenRouter::start(vec![(200, reply(&all, "stop")); 4]).await;
    let (or, _, _dir) = client(true, &fake.url_base, &[]);
    let meeting = "[s1u1] Min: let's talk about the release date\n[s1u2] Sua: we ship on Thursday then\n[s1u3] Min: sounds good to me";
    let out = or
        .ledger_in(meeting, &["Min".into(), "Sua".into()], NotesKind::Meeting, None, Some(Lang::Ko))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(v["title"], "출시일");
    let prompts = sent(&fake);
    assert_eq!(prompts.len(), 4);
    assert!(prompts[0].contains("회의에서 다룬 안건을 나눠요"));
    assert!(prompts.iter().all(|p| !p.contains("or the part of it you see")));
}

/// 앱이 영어면 한국어 회의도 영어판에 "영어로" 단락을 붙인 프롬프트로 써요.
#[tokio::test]
async fn korean_meeting_in_english_uses_the_english_prompts_with_the_rule() {
    let all = json!({
        "agenda": [{"title": "Release date", "ranges": [["s1u1", "s1u3"]]}],
        "points": [{"role": "conclusion", "text": "Ship on Thursday.", "utterance_ids": ["s1u2"]}],
        "items": [{"kind": "decision", "text": "Ship on Thursday.", "utterance_ids": ["s1u2"]}],
        "title": "Release date",
        "summary": [{"text": "Agreed to ship on Thursday.", "utterance_ids": ["s1u2"]}],
    })
    .to_string();
    let fake = FakeOpenRouter::start(vec![(200, reply(&all, "stop")); 4]).await;
    let (or, _, _dir) = client(true, &fake.url_base, &[]);
    let out = or
        .ledger_in(MEETING, &["민재".into(), "수아".into()], NotesKind::Meeting, None, Some(Lang::En))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(v["title"], "Release date");
    let prompts = sent(&fake);
    assert_eq!(prompts.len(), 4);
    assert!(prompts[0].contains("split it into the agenda items"));
    assert!(prompts.iter().all(|p| p.contains("Write every text value in English")));
}

/// 한국어가 아닌 섞인 회의를 회의 언어대로 쓸 때(2026-11-19): 안건 나누기가 고른 회의 언어로 다음 단계를 모두 써요.
/// 단계마다 고르면 주제가 저마다 다른 언어(영어, 일본어)가 됐어요.
#[tokio::test]
async fn mixed_meeting_writes_every_stage_in_the_outline_language() {
    let all = json!({
        "language": "Japanese",
        "agenda": [{"title": "リリース日", "ranges": [["s1u1", "s1u3"]]}],
        "points": [{"role": "conclusion", "text": "金曜日に決定。", "utterance_ids": ["s1u2"]}],
        "items": [{"kind": "decision", "text": "金曜日に決定。", "utterance_ids": ["s1u2"]}],
        "title": "リリース日",
        "summary": [{"text": "金曜日に決定。", "utterance_ids": ["s1u2"]}],
    })
    .to_string();
    let fake = FakeOpenRouter::start(vec![(200, reply(&all, "stop")); 4]).await;
    let (or, _, _dir) = client(true, &fake.url_base, &[]);
    let meeting = "[s1u1] Min: let's talk about the release date\n[s1u2] Yuki: 金曜日の朝に出しましょう、決済テストは水曜日までです\n[s1u3] 민재: 좋아요 금요일로 해요";
    let out = or.ledger(meeting, &["Min".into(), "Yuki".into()], NotesKind::Meeting).await.unwrap();
    let v: Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(v["title"], "リリース日");
    let prompts = sent(&fake);
    assert_eq!(prompts.len(), 4);
    assert!(prompts[0].contains("split it into the agenda items"));
    assert!(!prompts[0].contains("or the part of it you see"));
    assert!(prompts[1..].iter().all(|p| p.contains("Write every text value in Japanese")));
}

/// 안건 나누기가 언어를 돌려주지 않으면 지금처럼 단계마다 회의 언어로 써요.
#[tokio::test]
async fn no_outline_language_keeps_the_stages_as_they_were() {
    let all = json!({
        "agenda": [{"title": "Release date", "ranges": [["s1u1", "s1u3"]]}],
        "points": [{"role": "conclusion", "text": "Ship Friday.", "utterance_ids": ["s1u2"]}],
        "items": [{"kind": "decision", "text": "Ship Friday.", "utterance_ids": ["s1u2"]}],
        "title": "Release date",
        "summary": [{"text": "Ship Friday.", "utterance_ids": ["s1u2"]}],
    })
    .to_string();
    let fake = FakeOpenRouter::start(vec![(200, reply(&all, "stop")); 4]).await;
    let (or, _, _dir) = client(true, &fake.url_base, &[]);
    let meeting = "[s1u1] Min: let's talk about the release date\n[s1u2] Sua: we ship on Friday then\n[s1u3] Min: sounds good to me";
    or.ledger(meeting, &[], NotesKind::Meeting).await.unwrap();
    assert!(sent(&fake).iter().all(|p| !p.contains("or the part of it you see")));
}
