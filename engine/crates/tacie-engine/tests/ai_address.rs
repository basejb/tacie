//! The open-source edition's AI address (docs/41 8절, 2-A): the OpenAI-compatible address, model and key the app
//! passes.
#![cfg_attr(not(feature = "local-prompts"), allow(unused))]

mod common {
    pub mod fake_openrouter;
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use common::fake_openrouter::{FakeOpenRouter, reply, same_text};
use serde_json::{Value, json};
use tacie_engine::ai_address::AiAddress;
use tacie_engine::keys::Keys;
use tacie_engine::notes_kind::NotesKind;
use tacie_engine::openrouter::{OpenRouter, chat_body};
use tacie_engine::prompts::Prompt;
use tacie_engine::services::NoteWriter;
use tacie_engine::settings;

const MEETING: &str = "[s1u1] 민재: 배포 일정 이야기해요\n[s1u2] 수아: 목요일에 배포해요\n[s1u3] 민재: 좋아요";
const SECRET: &str = "sk-abcdefghijklmnop";

type Last = Arc<Mutex<HashMap<String, tacie_engine::errors::Kind>>>;

/// OpenRouter 키 없이(제품 엔진처럼) 만든 클라이언트와 그 키·지난 거절이에요. 지금 길은 닿지 않는 주소예요.
fn engine() -> (OpenRouter, Arc<Keys>, Last) {
    let env = HashMap::from([("TACIE_OPENROUTER_URL".to_string(), "http://127.0.0.1:9/api/v1".to_string())]);
    let keys = Arc::new(Keys::new(None));
    let last: Last = Arc::new(Mutex::new(HashMap::new()));
    let or = OpenRouter::with_keys(keys.clone(), &settings::resolve(&env), &env, last.clone());
    (or, keys, last)
}

fn address(url: &str, key: Option<&str>) -> AiAddress {
    AiAddress { url: url.to_string(), model: "gemma4:e4b".to_string(), key: key.map(str::to_string) }
}

fn staged_reply() -> Value {
    let all = json!({
        "agenda": [{"title": "배포 일정", "ranges": [["s1u1", "s1u3"]]}],
        "points": [{"role": "conclusion", "text": "목요일 배포로 정함.", "utterance_ids": ["s1u2"]}],
        "items": [{"kind": "decision", "text": "목요일 배포로 정함.", "utterance_ids": ["s1u2"]}],
        "title": "배포 일정",
        "summary": [{"text": "목요일 배포로 정함.", "utterance_ids": ["s1u2"]}],
    })
    .to_string();
    reply(&all, "stop")
}

fn parse(v: serde_json::Value) -> Result<Option<AiAddress>, String> {
    AiAddress::parse(&v)
}

#[test]
fn url_is_normalised() {
    let a = parse(json!({"url": "https://api.openai.com/v1/", "model": "gpt-5-mini", "key": "sk-1"})).unwrap().unwrap();
    assert_eq!(a.chat_url(), "https://api.openai.com/v1/chat/completions");
    assert_eq!(a.host(), "api.openai.com");
    let b = parse(json!({"url": "http://127.0.0.1:11434/v1/chat/completions", "model": "gemma4:e4b"})).unwrap().unwrap();
    assert_eq!(b.chat_url(), "http://127.0.0.1:11434/v1/chat/completions");
    assert_eq!(b.key, None);
    assert!(parse(json!({"url": "ftp://x", "model": "m"})).is_err());
    assert!(parse(json!({"url": "https://x", "model": ""})).is_err());
    assert!(parse(json!({"url": "https://x", "model": "m".repeat(201)})).is_err());
    assert!(parse(json!({"url": "https://x", "model": "m", "key": "a b"})).is_err());
    assert!(parse(json!({"model": "m"})).is_err());
    assert_eq!(parse(serde_json::Value::Null), Ok(None));
}

#[test]
fn mask_hides_key() {
    let a = parse(json!({"url": "https://x/v1", "model": "m", "key": "sk-abcdefghijklmnop"})).unwrap().unwrap();
    let out = a.mask("bad key sk-abcdefghijklmnop here");
    assert!(!out.contains("efghijkl"), "{out}");
    assert!(out.contains("sk-a") && out.contains("mnop"), "{out}");
    let keyless = parse(json!({"url": "https://x/v1", "model": "m"})).unwrap().unwrap();
    assert_eq!(keyless.mask("plain"), "plain");
}

/// AI 주소가 없으면 요청 본문은 지금과 같아요(중계와 OpenRouter 키 둘 다).
#[cfg(feature = "local-prompts")]
#[test]
fn body_unchanged_without_address() {
    let p = || Prompt { name: "readings", fields: vec![("terms", "a, b".to_string())] };
    let relay = chat_body("anthropic/claude-haiku-5.5", 300, &p(), "https://ai.tacie.app/ai/openrouter");
    same_text(&relay, &json!({"model": "anthropic/claude-haiku-5.5", "max_tokens": 300, "temperature": 0,
        "reasoning": {"enabled": false}, "prompt": {"name": "readings", "fields": {"terms": "a, b"}}}));
    let direct = chat_body("anthropic/claude-haiku-5.5", 300, &p(), "https://openrouter.ai/api/v1");
    assert_eq!(direct["reasoning"], json!({"enabled": false}));
    assert_eq!(direct["messages"][0]["role"], "user");
    assert!(direct.get("prompt").is_none());
}

#[cfg(feature = "local-prompts")]
#[tokio::test]
async fn address_body_has_standard_fields_only() {
    let fake = FakeOpenRouter::start(vec![(200, staged_reply()); 4]).await;
    let (or, keys, _) = engine();
    keys.set_address(Some(address(&fake.url_base, Some("k-test"))));
    or.ledger(MEETING, &[], NotesKind::Meeting).await.unwrap();
    let bodies = fake.bodies();
    assert_eq!(bodies.len(), 4);
    for b in &bodies {
        let mut k: Vec<&String> = b.as_object().unwrap().keys().collect();
        k.sort();
        assert_eq!(k, ["max_tokens", "messages", "model", "temperature"]);
        assert_eq!(b["model"], "gemma4:e4b");
    }
    assert!(fake.paths().iter().all(|p| p == "/api/v1/chat/completions"), "{:?}", fake.paths());
    assert!(fake.headers().iter().all(|h| h["authorization"] == "Bearer k-test"));
}

#[cfg(feature = "local-prompts")]
#[tokio::test]
async fn keyless_loopback_address_works() {
    let fake = FakeOpenRouter::start(vec![(200, staged_reply()); 4]).await;
    let (or, keys, _) = engine();
    keys.set_address(Some(address(&fake.url_base, None)));
    let out = or.ledger(MEETING, &[], NotesKind::Meeting).await.unwrap();
    assert!(out.text.contains("배포 일정"));
    assert!(fake.headers().iter().all(|h| h.get("authorization").is_none()));
}

#[cfg(feature = "local-prompts")]
#[tokio::test]
async fn openai_style_retry() {
    let refuse = json!(r#"{"error":{"message":"Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead."}}"#);
    let mut script = vec![(400, refuse)];
    script.extend(vec![(200, staged_reply()); 4]);
    let fake = FakeOpenRouter::start(script).await;
    let (or, keys, _) = engine();
    keys.set_address(Some(address(&fake.url_base, Some("k"))));
    or.ledger(MEETING, &[], NotesKind::Meeting).await.unwrap();
    let bodies = fake.bodies();
    assert!(bodies[0].get("max_tokens").is_some());
    for b in &bodies[1..] {
        assert!(b.get("max_tokens").is_none() && b.get("temperature").is_none(), "{b}");
        assert!(b.get("max_completion_tokens").is_some());
    }
}

#[cfg(feature = "local-prompts")]
#[tokio::test]
async fn odd_reply_is_a_chat_failure() {
    let fake = FakeOpenRouter::start(vec![(200, json!({"error": "model not found"})); 8]).await;
    let (or, keys, last) = engine();
    keys.set_address(Some(address(&fake.url_base, None)));
    assert!(or.ledger(MEETING, &[], NotesKind::Meeting).await.is_err());
    assert!(last.lock().unwrap().contains_key("chat"), "{:?}", last.lock().unwrap());
}

#[cfg(feature = "local-prompts")]
#[tokio::test]
async fn key_not_in_error_text() {
    let body = json!(format!(r#"{{"error":{{"message":"Incorrect API key provided: {SECRET}"}}}}"#));
    let fake = FakeOpenRouter::start(vec![(401, body); 8]).await;
    let (or, keys, last) = engine();
    keys.set_address(Some(address(&fake.url_base, Some(SECRET))));
    let e = or.ledger(MEETING, &[], NotesKind::Meeting).await.err().unwrap();
    assert!(!e.message.contains(SECRET) && !e.message.contains("efghijkl"), "{}", e.message);
    assert_eq!(last.lock().unwrap().get("chat"), Some(&tacie_engine::errors::Kind::Key));
}

/// 회의 정리 한 번은 시작할 때의 대상으로 끝나요: 도중에 주소를 바꿔도 남은 호출이 처음 주소로 가요.
#[cfg(feature = "local-prompts")]
#[tokio::test]
async fn notes_run_keeps_one_target() {
    let first = FakeOpenRouter::start(vec![(200, staged_reply()); 4]).await;
    let second = FakeOpenRouter::start(vec![(200, staged_reply()); 4]).await;
    let (or, keys, _) = engine();
    keys.set_address(Some(address(&first.url_base, None)));
    let or = Arc::new(or);
    let run = {
        let or = or.clone();
        tokio::spawn(async move { or.ledger(MEETING, &[], NotesKind::Meeting).await.map(|o| o.text) })
    };
    for _ in 0..5000 {
        if !first.paths().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert!(!first.paths().is_empty(), "the run never reached the first address");
    keys.set_address(Some(address(&second.url_base, None)));
    run.await.unwrap().unwrap();
    assert_eq!(first.paths().len(), 4);
    assert!(second.paths().is_empty());
}

#[test]
fn debug_never_prints_the_key() {
    let a = parse(json!({"url": "https://x/v1", "model": "m", "key": "sk-abcdefghijklmnop"})).unwrap().unwrap();
    let out = format!("{a:?}");
    assert!(!out.contains("efghijkl"), "{out}");
}

/// 문맥 초과처럼 `max_tokens`가 나오기만 한 거절은 OpenAI 방식으로 다시 보내지도, 기억하지도 않아요.
#[cfg(feature = "local-prompts")]
#[tokio::test]
async fn context_refusal_is_not_openai_style() {
    let refuse = json!(r#"{"error":{"message":"This model's maximum context length is 8192 tokens. However, you requested 9000 tokens (5000 in the messages, 4000 in max_tokens)."}}"#);
    let mut script = vec![(400, refuse)];
    script.extend(vec![(200, staged_reply()); 8]);
    let fake = FakeOpenRouter::start(script).await;
    let (or, keys, _) = engine();
    keys.set_address(Some(address(&fake.url_base, None)));
    let _ = or.ledger(MEETING, &[], NotesKind::Meeting).await;
    assert!(fake.bodies().iter().all(|b| b.get("max_completion_tokens").is_none()), "{:?}", fake.bodies());
}

/// 다시 보낸 요청도 실패하면 그 방식을 기억하지 않아요.
#[cfg(feature = "local-prompts")]
#[tokio::test]
async fn openai_style_remembered_only_after_success() {
    let refuse = json!(r#"{"error":{"message":"Unsupported parameter: 'max_tokens' is not supported with this model."}}"#);
    let fake = FakeOpenRouter::start(vec![(400, refuse), (500, json!("down")), (200, staged_reply()), (200, staged_reply()), (200, staged_reply()), (200, staged_reply())]).await;
    let (or, keys, _) = engine();
    keys.set_address(Some(address(&fake.url_base, None)));
    let _ = or.ledger(MEETING, &[], NotesKind::Meeting).await;
    let bodies = fake.bodies();
    assert!(bodies[1].get("max_completion_tokens").is_some());
    assert!(bodies.len() > 2 && bodies[2].get("max_tokens").is_some(), "{bodies:?}");
}
