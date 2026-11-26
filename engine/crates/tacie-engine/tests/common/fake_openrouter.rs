//! A fake OpenRouter server: runs on a free port on 127.0.0.1, collects the requests it gets (path, headers, body),
//! and returns the preset responses in order.
//! The `OpenRouter` client that connects to this server (`client`) and the response shape (`reply`) are here too
//! (shared by the answer and ledger tests).

#![allow(dead_code)] // 테스트 파일마다 쓰는 도움 함수가 달라요

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tacie_engine::errors::Kind;
use tacie_engine::openrouter::OpenRouter;
use tacie_engine::settings;
use tempfile::TempDir;

pub const KEY: &str = "sk-or-test-not-a-real-key";

pub type Last = Arc<Mutex<HashMap<String, Kind>>>;

/// 설정 폴더에 OpenRouter 키 파일을 두고(with_key가 false면 비워 두고) 클라이언트를 만들어요.
pub fn client(
    with_key: bool,
    url_base: &str,
    extra: &[(&str, &str)],
) -> (OpenRouter, Last, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    if with_key {
        std::fs::create_dir_all(dir.path().join("openrouter")).unwrap();
        std::fs::write(
            dir.path().join("openrouter/config.json"),
            json!({"apiKey": KEY}).to_string(),
        )
        .unwrap();
    }
    let mut env: HashMap<String, String> =
        HashMap::from([("TACIE_OPENROUTER_URL".to_string(), url_base.to_string())]);
    env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let last: Last = Arc::new(Mutex::new(HashMap::new()));
    let or = OpenRouter::new(dir.path(), &settings::resolve(&env), &env, last.clone());
    (or, last, dir)
}

/// `/chat/completions`의 200 응답이에요.
pub fn reply(content: &str, finish_reason: &str) -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": finish_reason}],
           "usage": {"cost": 0.00123}})
}

/// 키 순서까지 보려고 직렬화한 글로 비교해요(serde_json Map의 ==는 순서를 보지 않아요).
pub fn same_text(got: &Value, want: &Value) {
    assert_eq!(
        serde_json::to_string(got).unwrap(),
        serde_json::to_string(want).unwrap()
    );
}

#[derive(Default)]
struct Shared {
    /// (응답 상태, 본문). 본문이 JSON 문자열이면 그 글을 그대로 보내요(오류 응답 흉내).
    script: VecDeque<(u16, Value)>,
    received: Vec<(String, HeaderMap, Value)>,
}

pub struct FakeOpenRouter {
    /// 이 주소를 `TACIE_OPENROUTER_URL`로 넘겨요(`/chat/completions`가 뒤에 붙어요).
    pub url_base: String,
    shared: Arc<Mutex<Shared>>,
}

impl FakeOpenRouter {
    pub async fn start(script: Vec<(u16, Value)>) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            script: script.into(),
            received: Vec::new(),
        }));
        let app = Router::new().fallback(respond).with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url_base = format!("http://{}/api/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url_base, shared }
    }

    /// 지금까지 받은 요청의 경로예요(받은 순서).
    pub fn paths(&self) -> Vec<String> {
        let s = self.shared.lock().unwrap();
        s.received.iter().map(|(p, _, _)| p.clone()).collect()
    }

    pub fn headers(&self) -> Vec<HeaderMap> {
        let s = self.shared.lock().unwrap();
        s.received.iter().map(|(_, h, _)| h.clone()).collect()
    }

    pub fn bodies(&self) -> Vec<Value> {
        let s = self.shared.lock().unwrap();
        s.received.iter().map(|(_, _, b)| b.clone()).collect()
    }
}

async fn respond(
    State(shared): State<Arc<Mutex<Shared>>>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut s = shared.lock().unwrap();
    let parsed = serde_json::from_slice(&body).unwrap_or(Value::Null);
    s.received.push((uri.path().to_string(), headers, parsed));
    let (status, value) = s
        .script
        .pop_front()
        .unwrap_or((500, Value::String("no scripted response".into())));
    let text = match value {
        Value::String(t) => t,
        v => v.to_string(),
    };
    (
        StatusCode::from_u16(status).unwrap(),
        [(header::CONTENT_TYPE, "application/json")],
        text,
    )
        .into_response()
}
