//! Hooks transcription (`transcribe`, `stt_confidence`) up to a fake server. Request bodies are not changed, down
//! to key order.

mod common {
    pub mod fake_openrouter;
}

use common::fake_openrouter::{FakeOpenRouter, KEY, client, same_text};
use serde_json::{Value, json};
use tacie_engine::errors::Kind;
use tacie_engine::openrouter::{stt_confidence, stt_words};
use tacie_engine::services::Transcriber;

fn ok() -> (u16, Value) {
    (
        200,
        json!({"text": "  링크는 7일이에요\n", "usage": {"cost": 0.001}}),
    )
}

fn bad() -> (u16, Value) {
    (400, Value::String("bad request".into()))
}

fn keys(v: &Value) -> Vec<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

#[test]
fn stt_confidence_from_words() {
    let words = json!([{"word": "a", "confidence": 1.0}, {"word": "b", "confidence": 0.8},
                       {"word": "(웃음)", "type": "audio_event", "confidence": 0.1}]);
    let mean = stt_confidence(&json!({"text": "a b", "words": words})).unwrap();
    assert!((mean - 0.9).abs() < 1e-9);
    // 제공자가 직접 준 점수가 먼저예요
    assert_eq!(
        stt_confidence(&json!({"text": "a", "confidence": 0.7, "words": words})),
        Some(0.7)
    );
    assert_eq!(stt_confidence(&json!({"text": "a"})), None);
}

#[tokio::test]
async fn transcribe_posts_verbose_body_in_order_and_reads_the_result() {
    let words = json!([{"word": "a", "confidence": 1.0}, {"word": "b", "confidence": 0.5}]);
    let fake = FakeOpenRouter::start(vec![(
        200,
        json!({"text": "  점심 뭐 먹어요\n", "words": words, "usage": {"cost": 0.002}}),
    )])
    .await;
    let (or, last, _dir) = client(true, &fake.url_base, &[("TACIE_STT_MODEL", "x/stt-test")]);
    last.lock().unwrap().insert("stt".into(), Kind::Credit);

    let out = or.transcribe(b"RIFF").await.unwrap();
    assert_eq!(out.text, "점심 뭐 먹어요");
    assert_eq!(out.cost, 0.002);
    assert_eq!(out.confidence, Some(0.75));
    assert!(out.ms >= 0.0);
    assert!(!last.lock().unwrap().contains_key("stt"));

    assert_eq!(fake.paths(), ["/api/v1/audio/transcriptions"; 1]);
    assert_eq!(fake.headers()[0]["authorization"], format!("Bearer {KEY}"));
    same_text(
        &fake.bodies()[0],
        &json!({"model": "x/stt-test", "language": "ko", "provider": {"zdr": true},
                "input_audio": {"data": "UklGRg==", "format": "wav"},
                "response_format": "verbose_json", "timestamp_granularities": ["word"]}),
    );
}

#[tokio::test]
async fn transcribe_without_cost_or_scores() {
    let fake = FakeOpenRouter::start(vec![(200, json!({"text": "네"}))]).await;
    let (or, _, _dir) = client(true, &fake.url_base, &[]);
    let out = or.transcribe(b"RIFF").await.unwrap();
    assert_eq!(
        (out.text.as_str(), out.cost, out.confidence),
        ("네", 0.0, None)
    );
}

#[tokio::test]
async fn stt_drops_word_scores_once_a_model_refuses_them() {
    let fake = FakeOpenRouter::start(vec![bad(), ok(), ok()]).await;
    let (or, _, _dir) = client(true, &fake.url_base, &[]);
    let first = or.transcribe(b"RIFF").await.unwrap();
    assert_eq!(first.text, "링크는 7일이에요");
    assert_eq!(first.confidence, None);
    or.transcribe(b"RIFF").await.unwrap();
    // 한 번 다시 보내고, 그 뒤로는 평범한 요청만이에요
    let verbose: Vec<bool> = fake
        .bodies()
        .iter()
        .map(|b| b.get("response_format").is_some())
        .collect();
    assert_eq!(verbose, [true, false, false]);
    let second = &fake.bodies()[1];
    assert_eq!(
        keys(second),
        ["model", "language", "provider", "input_audio"]
    );
    assert_eq!(second.get("timestamp_granularities"), None);
}

#[tokio::test]
async fn stt_goes_only_to_endpoints_that_keep_no_audio() {
    // 회의 소리가 받아쓰기 제공자에게 남으면 안 돼요: 말 점수를 뺀 두 번째 요청도 같은 규칙이에요
    let fake = FakeOpenRouter::start(vec![bad(), ok()]).await;
    let (or, _, _dir) = client(true, &fake.url_base, &[]);
    or.transcribe(b"RIFF").await.unwrap();
    let bodies = fake.bodies();
    assert_eq!(bodies.len(), 2);
    assert!(bodies.iter().all(|b| b["provider"]["zdr"] == json!(true)));
}

#[tokio::test]
async fn stt_keeps_word_scores_when_the_audio_was_the_problem() {
    let fake = FakeOpenRouter::start(vec![bad(), bad(), ok()]).await;
    let (or, _last, _dir) = client(true, &fake.url_base, &[]);
    assert!(or.transcribe(b"RIFF").await.is_err());
    // 둘 다 400이면 verbose는 켠 채예요: 다음 호출은 다시 verbose로 시작해요
    or.transcribe(b"RIFF").await.unwrap();
    let verbose: Vec<bool> = fake
        .bodies()
        .iter()
        .map(|b| b.get("response_format").is_some())
        .collect();
    assert_eq!(verbose, [true, false, true]);
    assert_eq!(keys(&fake.bodies()[0]).len(), 6);
}

#[tokio::test]
async fn stt_errors() {
    // 키 파일이 없으면 요청 없이 key예요
    let fake = FakeOpenRouter::start(vec![]).await;
    let (or, _, _dir) = client(false, &fake.url_base, &[]);
    let e = or.transcribe(b"RIFF").await.unwrap_err();
    assert_eq!(e.kind, Kind::Key);
    assert_eq!(e.message, "no OpenRouter key");
    assert!(fake.paths().is_empty());

    // 연결이 안 되면 network예요
    let (or, _, _dir) = client(true, "http://127.0.0.1:1", &[]);
    let e = or.transcribe(b"RIFF").await.unwrap_err();
    assert_eq!(e.kind, Kind::Network);
    assert!(e.message.starts_with("stt request failed: "));

    // 402는 credit이고 last에 남아요
    let fake = FakeOpenRouter::start(vec![(402, Value::String("no credit".into()))]).await;
    let (or, last, _dir) = client(true, &fake.url_base, &[]);
    let e = or.transcribe(b"RIFF").await.unwrap_err();
    assert_eq!(e.kind, Kind::Credit);
    assert_eq!(last.lock().unwrap().get("stt"), Some(&Kind::Credit));
}

#[test]
fn stt_words_reads_both_shapes_and_drops_events() {
    let d = json!({"text": "링크는 7일", "words": [
        {"word": "링크는", "start": 0.12, "end": 0.58},
        {"text": " ", "type": "spacing", "start": 0.58, "end": 0.6},
        {"text": "(웃음)", "type": "audio_event", "start": 0.6, "end": 0.9},
        {"text": "7일", "type": "word", "start": 0.9, "end": 1.3},
        {"word": "시각없음"}
    ]});
    let w = stt_words(&d);
    assert_eq!(
        w.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(),
        ["링크는", "7일"]
    );
    assert_eq!((w[0].start, w[0].end, w[1].start), (0.12, 0.58, 0.9));
    assert!(stt_words(&json!({"text": "a"})).is_empty());
}
