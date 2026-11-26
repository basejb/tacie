//! The open-source edition's transcription provider (docs/41 8절, 2-B): Soniox (with region) or ElevenLabs and the
//! user's key, as the app passes them.
#![cfg_attr(not(feature = "local-prompts"), allow(unused))]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tacie_engine::errors::Kind;
use tacie_engine::keys::Keys;
use tacie_engine::openrouter::OpenRouter;
use tacie_engine::services::Transcriber;
use tacie_engine::settings;
use tacie_engine::stt_provider::SttProvider;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SECRET: &str = "sk-abcdefghijklmnop";
const WAV: &[u8] = b"RIFF0000WAVEfmt fake-audio";

/// 요청 하나를 받아 `status`와 `body`를 돌려주는 서버예요: (주소, 받은 요청 원문).
async fn fake(status: &'static str, body: String) -> (String, Arc<Mutex<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let keep = seen.clone();
    tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else { return };
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = sock.read(&mut chunk).await.unwrap_or(0);
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).to_string();
            if let Some(h) = text.find("\r\n\r\n") {
                let len = text[..h].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                if buf.len() >= h + 4 + len { break; }
            }
            if n == 0 { break; }
        }
        *keep.lock().unwrap() = buf;
        let reply = format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
        sock.write_all(reply.as_bytes()).await.ok();
        sock.shutdown().await.ok();
    });
    (base, seen)
}

/// OpenRouter 키 없이(오픈소스판처럼) 만든 엔진이에요. ElevenLabs 주소는 가짜예요.
fn engine(eleven: &str) -> (OpenRouter, Arc<Keys>, Arc<Mutex<HashMap<String, Kind>>>) {
    let env = HashMap::from([
        ("TACIE_OPENROUTER_URL".to_string(), "http://127.0.0.1:9/api/v1".to_string()),
        ("TACIE_ELEVENLABS_URL".to_string(), format!("{eleven}/v1/speech-to-text")),
    ]);
    let keys = Arc::new(Keys::new(None));
    let last = Arc::new(Mutex::new(HashMap::new()));
    (OpenRouter::with_keys(keys.clone(), &settings::resolve(&env), &env, last.clone()), keys, last)
}

fn words_reply() -> String {
    json!({"language_code": "kor", "language_probability": 0.98, "text": "배포 목요일",
           "words": [{"text": "배포", "type": "word", "start": 0.1, "end": 0.5, "logprob": -0.1, "speaker_id": "speaker_0"},
                     {"text": " ", "type": "spacing", "start": 0.5, "end": 0.55},
                     {"text": "목요일", "type": "word", "start": 0.55, "end": 1.0, "logprob": -0.2, "speaker_id": "speaker_0"}]}).to_string()
}

#[test]
fn parse_variants() {
    let p = |v| SttProvider::parse(&v);
    assert_eq!(p(json!({"provider": "soniox", "key": "k"})).unwrap(), Some(SttProvider::Soniox { key: "k".into(), region: "us" }));
    assert_eq!(p(json!({"provider": "soniox", "key": "k", "region": "jp"})).unwrap(), Some(SttProvider::Soniox { key: "k".into(), region: "jp" }));
    assert!(p(json!({"provider": "soniox", "key": "k", "region": "kr"})).is_err());
    assert_eq!(p(json!({"provider": "elevenlabs", "key": "k"})).unwrap(), Some(SttProvider::ElevenLabs { key: "k".into() }));
    assert!(p(json!({"provider": "elevenlabs"})).is_err());
    assert!(p(json!({"provider": "deepgram", "key": "k"})).is_err());
    assert!(p(json!({"provider": "soniox", "key": "a b"})).is_err());
    assert_eq!(p(serde_json::Value::Null).unwrap(), None);
}

#[test]
fn debug_hides_key() {
    let s = SttProvider::ElevenLabs { key: "sk-abcdefghijklmnop".into() };
    assert!(!format!("{s:?}").contains("efghijkl"));
    assert!(!s.mask("x sk-abcdefghijklmnop y").contains("efghijkl"));
}

#[tokio::test]
async fn elevenlabs_request_shape() {
    let (base, seen) = fake("200 OK", words_reply()).await;
    let (or, keys, _) = engine(&base);
    keys.set_stt(Some(SttProvider::ElevenLabs { key: "k".into() }));
    or.transcribe(WAV).await.unwrap();
    let req = String::from_utf8_lossy(&seen.lock().unwrap()).to_string();
    assert!(req.starts_with("POST /v1/speech-to-text "), "{req}");
    assert!(req.to_ascii_lowercase().contains("xi-api-key: k"), "{req}");
    for (name, value) in [("model_id", "scribe_v2"), ("timestamps_granularity", "word"), ("tag_audio_events", "false"), ("language_code", "ko")] {
        assert!(req.contains(&format!("name=\"{name}\"\r\n\r\n{value}\r\n")), "{name}: {req}");
    }
    assert!(req.contains("name=\"file\"") && req.contains("fake-audio"), "{req}");
}

#[tokio::test]
async fn elevenlabs_words() {
    let (base, _) = fake("200 OK", words_reply()).await;
    let (or, keys, _) = engine(&base);
    keys.set_stt(Some(SttProvider::ElevenLabs { key: "k".into() }));
    let t = or.transcribe(WAV).await.unwrap();
    assert_eq!(t.text, "배포 목요일");
    assert_eq!(t.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["배포", "목요일"]);
    assert_eq!((t.words[1].start, t.words[1].end), (0.55, 1.0));
    assert_eq!(t.cost, 0.0);
}

#[tokio::test]
async fn elevenlabs_odd_words() {
    let body = json!({"text": "(웃음)", "words": [{"text": "(웃음)", "type": "audio_event", "start": 0.0, "end": 1.0}]}).to_string();
    let (base, _) = fake("200 OK", body).await;
    let (or, keys, _) = engine(&base);
    keys.set_stt(Some(SttProvider::ElevenLabs { key: "k".into() }));
    let t = or.transcribe(WAV).await.unwrap();
    assert_eq!(t.text, "(웃음)");
    assert!(t.words.is_empty());
}

#[tokio::test]
async fn elevenlabs_key_not_in_error() {
    let body = json!({"detail": {"status": "invalid_api_key", "message": format!("Invalid API key: {SECRET}")}}).to_string();
    let (base, _) = fake("401 Unauthorized", body).await;
    let (or, keys, last) = engine(&base);
    keys.set_stt(Some(SttProvider::ElevenLabs { key: SECRET.into() }));
    let e = or.transcribe(WAV).await.err().unwrap();
    assert!(!e.message.contains("efghijkl"), "{}", e.message);
    assert_eq!(e.kind, Kind::Key);
    assert_eq!(last.lock().unwrap().get("stt_provider"), Some(&Kind::Key));
}

/// 업체가 Soniox인데 발화별 받아쓰기가 오면(실시간이 끊긴 발화) OpenRouter 키가 없어 소리를 보내지 않고 끝나요.
#[tokio::test]
async fn soniox_provider_fallback_needs_openrouter() {
    let (base, seen) = fake("200 OK", words_reply()).await;
    let (or, keys, _) = engine(&base);
    keys.set_stt(Some(SttProvider::Soniox { key: "k".into(), region: "us" }));
    let e = or.transcribe(WAV).await.err().unwrap();
    assert_eq!(e.kind, Kind::Key);
    assert!(seen.lock().unwrap().is_empty());
}
