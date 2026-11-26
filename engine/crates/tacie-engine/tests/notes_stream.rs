//! Hooks meeting notes streaming (docs/14 "회의 정리 스트리밍") up to a fake server (`common/fake_openrouter.rs`). The
//! relay server's streaming address points at the fake server via `TACIE_NOTES_STREAM_URL`. The real API is not
//! called.
#![cfg(feature = "local-prompts")]

mod common {
    pub mod fake_openrouter;
}

use common::fake_openrouter::{FakeOpenRouter, client, reply};
use serde_json::{Value, json};
use tacie_engine::openrouter::{SseText, accept_stream_url, config_url};
use tacie_engine::services::NoteWriter;

const RAW: &str = r#"{"title": "배포 일정", "summary": [], "topics": [], "items": []}"#;

/// OpenRouter의 이벤트 스트림 모양이에요: 처리 중 주석, 글 조각 둘, 마지막에 비용.
fn sse(content: &str, finish: &str) -> Value {
    let (a, b) = content.split_at(content.len() / 2);
    let chunk = |c: &str| json!({"choices": [{"delta": {"content": c}}]}).to_string();
    Value::String(format!(
        ": OPENROUTER PROCESSING\n\ndata: {}\n\ndata: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        chunk(a),
        chunk(b),
        json!({"choices": [{"delta": {}, "finish_reason": finish}]}),
        json!({"choices": [], "usage": {"cost": 0.004}}),
    ))
}

#[test]
fn sse_text_joins_the_pieces_and_takes_the_cost_and_the_cut() {
    let mut s = SseText::default();
    let Value::String(body) = sse("가나다라", "length") else {
        unreachable!()
    };
    // 줄 중간에서 끊긴 조각도 이어 붙여요
    let (a, b) = body.split_at(37);
    s.feed(a);
    s.feed(b);
    s.finish();
    assert_eq!(s.text, "가나다라");
    assert_eq!(s.cost, 0.004);
    assert!(s.cut);
    assert!(s.done);
    assert!(s.error.is_none());
}

#[test]
fn sse_text_keeps_an_error_sent_inside_the_stream() {
    let mut s = SseText::default();
    s.feed("data: {\"choices\":[{\"delta\":{\"content\":\"{\\\"a\\\"\"}}]}\n\n");
    s.feed("data: {\"error\":{\"message\":\"upstream died\"}}\n\n");
    s.finish();
    assert_eq!(s.text, "{\"a\"");
    assert_eq!(s.error.as_deref(), Some("upstream died"));
}

#[test]
fn config_url_and_the_stream_urls_the_engine_takes() {
    assert_eq!(
        config_url("https://ai.tacie.app/ai/openrouter").as_deref(),
        Some("https://ai.tacie.app/ai/config")
    );
    assert_eq!(config_url("https://openrouter.ai/api/v1"), None);
    assert!(accept_stream_url(
        "https://abc.lambda-url.ap-northeast-2.on.aws/"
    ));
    assert!(!accept_stream_url(
        "http://abc.lambda-url.ap-northeast-2.on.aws/"
    ));
    assert!(!accept_stream_url("https://evil.example.com/"));
    assert!(!accept_stream_url("https://on.aws.evil.example.com/"));
}

#[tokio::test]
async fn ledger_streams_both_calls_when_a_stream_url_is_set() {
    let stream = FakeOpenRouter::start(vec![(200, sse(RAW, "stop")); 2]).await;
    let plain = FakeOpenRouter::start(vec![]).await;
    let url = format!("{}/", stream.url_base.trim_end_matches("/api/v1"));
    let (or, _last, _dir) = client(
        true,
        &plain.url_base,
        &[("TACIE_NOTES_STREAM_URL", url.as_str()), ("TACIE_NOTES_STAGED", "0")],
    );
    let out = or
        .ledger(
            "[s1u1] 나: 배포는 금요일",
            &[],
            tacie_engine::notes_kind::NotesKind::Meeting,
        )
        .await
        .unwrap();
    let joined: Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(joined["title"], "배포 일정");
    assert_eq!(out.cost, 0.004 * 2.0);
    assert_eq!(stream.paths(), ["/ai/openrouter/chat/completions"; 2]);
    assert!(stream.bodies().iter().all(|b| b["stream"] == true));
    assert!(plain.paths().is_empty());
}

#[tokio::test]
async fn a_failed_stream_falls_back_to_the_plain_call() {
    let stream = FakeOpenRouter::start(vec![(502, Value::String("bad gateway".into())); 2]).await;
    let plain = FakeOpenRouter::start(vec![(200, reply(RAW, "stop")); 2]).await;
    let url = format!("{}/", stream.url_base.trim_end_matches("/api/v1"));
    let (or, _last, _dir) = client(
        true,
        &plain.url_base,
        &[("TACIE_NOTES_STREAM_URL", url.as_str()), ("TACIE_NOTES_STAGED", "0")],
    );
    let out = or
        .ledger(
            "[s1u1] 나: 배포는 금요일",
            &[],
            tacie_engine::notes_kind::NotesKind::Meeting,
        )
        .await
        .unwrap();
    let joined: Value = serde_json::from_str(&out.text).unwrap();
    assert_eq!(joined["title"], "배포 일정");
    assert_eq!(stream.paths().len(), 2);
    assert_eq!(plain.paths(), ["/api/v1/chat/completions"; 2]);
    assert!(plain.bodies().iter().all(|b| b.get("stream").is_none()));
}
