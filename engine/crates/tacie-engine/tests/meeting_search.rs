//! Hooks server meeting search (`/meetings/search`, docs/40 5-1절) up to a fake server. The real server is not
//! called.
//! The address points at the fake server via `TACIE_MEETINGS_URL` (directly) or `TACIE_CONFIG_URL` (the config
//! address, from which `meetings` is read).

mod common {
    pub mod fake_openrouter;
}

use common::fake_openrouter::{FakeOpenRouter, KEY, client};
use serde_json::{Value, json};
use tacie_engine::errors::Kind;
use tacie_engine::services::{Hit, MeetingSearch};

fn base(f: &FakeOpenRouter) -> String {
    f.url_base.trim_end_matches("/api/v1").to_string()
}

fn queries() -> Vec<String> {
    vec!["배포 일정".to_string(), "예산".to_string()]
}

fn ok_body() -> Value {
    json!({"results": [[{"id": "m1", "score": 0.9}, {"id": "m2", "score": 0.5}], []], "searched": 2})
}

const DEAD: &str = "http://127.0.0.1:1/api/v1";

#[tokio::test]
async fn search_posts_queries_with_bearer() {
    let server = FakeOpenRouter::start(vec![(200, ok_body())]).await;
    let url = base(&server);
    let (or, _last, _dir) = client(true, DEAD, &[("TACIE_MEETINGS_URL", url.as_str())]);
    let got = or.search(&queries(), 5).await.unwrap();
    assert_eq!(
        got,
        vec![
            vec![
                Hit {
                    id: "m1".into(),
                    score: 0.9
                },
                Hit {
                    id: "m2".into(),
                    score: 0.5
                }
            ],
            vec![]
        ]
    );
    assert_eq!(server.paths(), ["/meetings/search"]);
    assert_eq!(
        server.bodies()[0],
        json!({"queries": ["배포 일정", "예산"], "top": 5})
    );
    assert_eq!(
        server.headers()[0]
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        format!("Bearer {KEY}")
    );
}

#[tokio::test]
async fn search_maps_statuses() {
    for (status, kind) in [
        (401, Kind::Key),
        (402, Kind::Credit),
        (429, Kind::Busy),
        (503, Kind::Down),
    ] {
        let server = FakeOpenRouter::start(vec![(status, json!({"error": "x"}))]).await;
        let url = base(&server);
        let (or, _last, _dir) = client(true, DEAD, &[("TACIE_MEETINGS_URL", url.as_str())]);
        let e = or.search(&queries(), 5).await.unwrap_err();
        assert_eq!(e.kind, kind, "status {status}");
    }
}

#[tokio::test]
async fn search_transport_error_is_network() {
    let (or, _last, _dir) = client(true, DEAD, &[("TACIE_MEETINGS_URL", "http://127.0.0.1:1")]);
    let e = or.search(&queries(), 5).await.unwrap_err();
    assert_eq!(e.kind, Kind::Network);
}

#[tokio::test]
async fn search_url_failure_is_not_cached() {
    let search = FakeOpenRouter::start(vec![(200, ok_body()), (200, ok_body())]).await;
    let config = FakeOpenRouter::start(vec![
        (500, Value::String("boom".into())),
        (200, json!({"meetings": base(&search)})),
    ])
    .await;
    let cfg = format!("{}/ai/config", base(&config));
    let (or, _last, _dir) = client(true, DEAD, &[("TACIE_CONFIG_URL", cfg.as_str())]);
    assert!(or.search(&queries(), 5).await.is_err());
    assert!(search.paths().is_empty());
    assert_eq!(or.search(&queries(), 5).await.unwrap().len(), 2);
    // 성공한 주소는 기억해요: 설정 서버를 다시 부르지 않아요.
    assert_eq!(or.search(&queries(), 5).await.unwrap().len(), 2);
    assert_eq!(config.paths(), ["/ai/config"; 2]);
    assert_eq!(search.paths(), ["/meetings/search"; 2]);
}

#[tokio::test]
async fn search_without_a_key_is_key_error() {
    let (or, _last, _dir) = client(false, DEAD, &[("TACIE_MEETINGS_URL", "http://127.0.0.1:1")]);
    let e = or.search(&queries(), 5).await.unwrap_err();
    assert_eq!(e.kind, Kind::Key);
}

#[tokio::test]
async fn search_rejects_a_config_url_that_is_not_ours() {
    let config = FakeOpenRouter::start(vec![
        (200, json!({"meetings": "https://evil.example.com"})),
        (200, json!({"meetings": "http://abc.lambda-url.ap-northeast-2.on.aws"})),
    ])
    .await;
    let cfg = format!("{}/ai/config", base(&config));
    let (or, _last, _dir) = client(true, DEAD, &[("TACIE_CONFIG_URL", cfg.as_str())]);
    for _ in 0..2 {
        let e = or.search(&queries(), 5).await.unwrap_err();
        assert_eq!(e.kind, Kind::Down);
    }
    assert_eq!(config.paths().len(), 2); // 거절한 주소는 기억하지 않아요
}
