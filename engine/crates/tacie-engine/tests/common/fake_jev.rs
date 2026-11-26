//! A fake Jev server: runs on a free port on 127.0.0.1, collects the requests it gets (headers, body), and returns
//! the preset responses in order.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

#[derive(Default)]
struct Shared {
    /// (응답 상태, 본문). 본문이 JSON 문자열이면 그 글을 그대로 보내요(오류 응답 흉내).
    script: VecDeque<(u16, Value)>,
    received: Vec<(HeaderMap, Value)>,
}

pub struct FakeJev {
    /// 이 주소를 `TACIE_JEV_URL`로 넘겨요.
    pub url: String,
    shared: Arc<Mutex<Shared>>,
}

impl FakeJev {
    pub async fn start(script: Vec<(u16, Value)>) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            script: script.into(),
            received: Vec::new(),
        }));
        let app = Router::new().fallback(respond).with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, shared }
    }

    /// 지금까지 받은 요청 본문이에요(받은 순서).
    pub fn bodies(&self) -> Vec<Value> {
        let s = self.shared.lock().unwrap();
        s.received.iter().map(|(_, b)| b.clone()).collect()
    }

    /// 지금까지 받은 요청 헤더예요(받은 순서).
    pub fn headers(&self) -> Vec<HeaderMap> {
        let s = self.shared.lock().unwrap();
        s.received.iter().map(|(h, _)| h.clone()).collect()
    }
}

async fn respond(
    State(shared): State<Arc<Mutex<Shared>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut s = shared.lock().unwrap();
    let parsed = serde_json::from_slice(&body).unwrap_or(Value::Null);
    s.received.push((headers, parsed));
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
