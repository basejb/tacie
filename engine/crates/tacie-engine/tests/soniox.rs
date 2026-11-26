//! Tests creating a Soniox temporary key (`soniox::mint_key`) with a fake HTTP server (opens one port and gives a
//! set answer to one request).

use std::sync::{Arc, Mutex};

use tacie_engine::errors::Kind;
use tacie_engine::soniox::{StreamKey, mint_key, provider_key};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 요청 하나를 받아 `reply`를 돌려주는 서버예요. 받은 요청 글 전체를 돌려줘요.
async fn fake(reply: &'static str) -> (String, Arc<Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(String::new()));
    let keep = seen.clone();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = sock.read(&mut chunk).await.unwrap();
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).to_string();
            if let Some(head_end) = text.find("\r\n\r\n") {
                let len = text[..head_end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if buf.len() >= head_end + 4 + len {
                    *keep.lock().unwrap() = text;
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        sock.write_all(reply.as_bytes()).await.unwrap();
        sock.shutdown().await.ok();
    });
    (base, seen)
}

fn http(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn mint_key_sends_the_request_soniox_documents() {
    let reply: &'static str = Box::leak(
        http(
            "201 Created",
            r#"{"api_key":"tmp","expires_at":"2026-11-10T10:00:00Z"}"#,
        )
        .into_boxed_str(),
    );
    let (base, seen) = fake(reply).await;
    let got = mint_key(&reqwest::Client::new(), &base, "test-key")
        .await
        .unwrap();
    assert_eq!(
        got,
        StreamKey {
            api_key: "tmp".into(),
            expires_at: "2026-11-10T10:00:00Z".into()
        }
    );
    let req = seen.lock().unwrap().clone();
    assert!(req.starts_with("POST /v1/auth/temporary-api-key "), "{req}");
    assert!(
        req.to_ascii_lowercase()
            .contains("authorization: bearer test-key"),
        "{req}"
    );
    let body: serde_json::Value =
        serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"usage_type": "transcribe_websocket", "expires_in_seconds": 3600, "max_session_duration_seconds": 18000})
    );
}

#[tokio::test]
async fn mint_key_maps_401_to_key() {
    let reply: &'static str =
        Box::leak(http("401 Unauthorized", r#"{"message":"no"}"#).into_boxed_str());
    let (base, _) = fake(reply).await;
    let e = mint_key(&reqwest::Client::new(), &base, "k")
        .await
        .unwrap_err();
    assert_eq!(e.kind, Kind::Key);
}

#[tokio::test]
async fn mint_key_maps_402_to_credit() {
    let reply: &'static str = Box::leak(http("402 Payment Required", r#"{}"#).into_boxed_str());
    let (base, _) = fake(reply).await;
    let e = mint_key(&reqwest::Client::new(), &base, "k")
        .await
        .unwrap_err();
    assert_eq!(e.kind, Kind::Credit);
}

#[tokio::test]
async fn mint_key_maps_connection_refused_to_network() {
    // 열었다가 바로 닫은 포트예요: 연결이 거절돼요.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let e = mint_key(&reqwest::Client::new(), &base, "k")
        .await
        .unwrap_err();
    assert_eq!(e.kind, Kind::Network);
}

#[test]
fn region_jp_uses_the_japan_key_and_hosts() {
    use tacie_engine::soniox::Region;
    let jp = Region::from_env(Some("jp"));
    assert_eq!(jp.key_name, "soniox-jp");
    assert_eq!(jp.api, "https://api.jp.soniox.com");
    assert_eq!(jp.ws, "wss://stt-rt.jp.soniox.com/transcribe-websocket");
}

#[test]
fn region_defaults_to_us() {
    use tacie_engine::soniox::Region;
    for v in [None, Some(""), Some("us"), Some("eu?")] {
        let r = Region::from_env(v);
        assert_eq!(r.key_name, "soniox");
        assert_eq!(r.api, "https://api.soniox.com");
        assert_eq!(r.ws, "wss://stt-rt.soniox.com/transcribe-websocket");
    }
}

#[test]
fn relay_key_url_comes_from_the_openrouter_relay_base() {
    use tacie_engine::soniox::relay_key_url;
    assert_eq!(
        relay_key_url("https://ai.tacie.app/ai/openrouter").as_deref(),
        Some("https://ai.tacie.app/ai/soniox/stream-key")
    );
    assert_eq!(
        relay_key_url("http://127.0.0.1:9/ai/openrouter/").as_deref(),
        Some("http://127.0.0.1:9/ai/soniox/stream-key")
    );
    assert_eq!(relay_key_url("https://openrouter.ai/api/v1"), None);
    // only the Tacie relay (or a local test server) gets the session token
    assert_eq!(relay_key_url("https://evil.example/ai/openrouter"), None);
}

#[tokio::test]
async fn relay_key_passes_the_relays_answer() {
    let reply: &'static str = Box::leak(
        http(
            "200 OK",
            r#"{"api_key":"tmp","expires_at":"2026-11-10T10:00:00Z","ws_url":"wss://stt-rt.jp.soniox.com/transcribe-websocket"}"#,
        )
        .into_boxed_str(),
    );
    let (base, seen) = fake(reply).await;
    let got = tacie_engine::soniox::relay_key(
        &reqwest::Client::new(),
        &format!("{base}/ai/soniox/stream-key"),
        "session",
        "us",
    )
    .await
    .unwrap();
    assert_eq!(got.api_key, "tmp");
    assert_eq!(
        got.ws_url,
        "wss://stt-rt.jp.soniox.com/transcribe-websocket"
    );
    let req = seen.lock().unwrap().clone();
    assert!(req.starts_with("POST /ai/soniox/stream-key "), "{req}");
    assert!(
        req.to_ascii_lowercase()
            .contains("authorization: bearer session"),
        "{req}"
    );
    // the app's region goes to the relay, which picks the project
    let body: serde_json::Value =
        serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body, serde_json::json!({"region": "us"}));
}

#[tokio::test]
async fn relay_key_off_is_down() {
    let reply: &'static str =
        Box::leak(http("503 Service Unavailable", r#"{"error":{"code":"off"}}"#).into_boxed_str());
    let (base, _) = fake(reply).await;
    let e =
        tacie_engine::soniox::relay_key(&reqwest::Client::new(), &format!("{base}/x"), "s", "jp")
            .await
            .unwrap_err();
    assert_eq!(e.kind, Kind::Down);
}

#[test]
fn region_eu_uses_its_key_file_and_hosts() {
    use tacie_engine::soniox::Region;
    let eu = Region::from_env(Some("eu"));
    assert_eq!(eu.key_name, "soniox-eu");
    assert_eq!(eu.api, "https://api.eu.soniox.com");
    assert_eq!(eu.ws, "wss://stt-rt.eu.soniox.com/transcribe-websocket");
    assert_eq!(eu.name, "eu");
    assert_eq!(Region::from_env(None).name, "us");
}

/// 사용자 키(2-B): 그 키로 임시 키를 받고, 시험용 주소를 넘기면 그 주소의 ws를 돌려줘요.
#[tokio::test]
async fn provider_key_mints_with_the_users_key() {
    let reply: &'static str = Box::leak(http("201 Created", r#"{"api_key":"tmp","expires_at":"x"}"#).into_boxed_str());
    let (base, seen) = fake(reply).await;
    let (k, ws) = provider_key("jp", "sk-user", Some(&base), Some("ws://fake")).await.unwrap();
    assert_eq!((k.api_key.as_str(), ws.as_str()), ("tmp", "ws://fake"));
    assert!(seen.lock().unwrap().to_ascii_lowercase().contains("authorization: bearer sk-user"));
}

/// 덮어쓰기가 없으면 고른 리전의 Soniox 주소예요.
#[test]
fn provider_region_addresses() {
    assert_eq!(tacie_engine::soniox::provider_ws("jp", None), "wss://stt-rt.jp.soniox.com/transcribe-websocket");
    assert_eq!(tacie_engine::soniox::provider_ws("us", None), "wss://stt-rt.soniox.com/transcribe-websocket");
}

/// 다른 리전의 키처럼 거절되면 키 오류예요.
#[tokio::test]
async fn provider_key_refused_is_key() {
    let reply: &'static str = Box::leak(http("401 Unauthorized", r#"{"error":"bad key"}"#).into_boxed_str());
    let (base, _) = fake(reply).await;
    let e = provider_key("us", "sk-user", Some(&base), None).await.err().unwrap();
    assert_eq!(e.kind, Kind::Key);
}
