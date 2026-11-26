//! AI keys passed by the app (in memory) and key files (dev engine only) (#7 step 5). Hooked up to fake Jev and
//! OpenRouter servers.

mod common {
    pub mod fake_jev;
    pub mod fake_openrouter;
}

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use common::fake_jev::FakeJev;
use common::fake_openrouter::{FakeOpenRouter, reply};
use serde_json::json;
use tacie_engine::errors::Kind;
use tacie_engine::jev::{Jev, JevKw};
use tacie_engine::keys::Keys;
use tacie_engine::openrouter::OpenRouter;
use tacie_engine::pipeline::Answerer;
use tacie_engine::settings;

const APP_KEY: &str = "sk-app-not-a-real-key";
const FILE_KEY: &str = "sk-file-not-a-real-key";

fn env(name: &str, url: &str) -> HashMap<String, String> {
    HashMap::from([(name.to_string(), url.to_string())])
}

fn rerank_ok() -> serde_json::Value {
    json!({"answers": {"p0": {"noul": 0.9}}, "usage": {"input_tokens": 10}})
}

fn kw() -> JevKw {
    JevKw {
        timeout: Duration::from_secs(5),
        attempts: 1,
    }
}

/// 제품 엔진(키 파일을 읽지 않아요)은 키 파일이 있어도 키가 없어요. 앱이 키를 넘기면 다음 요청부터 그 키를 실어요.
#[tokio::test]
async fn product_engine_ignores_key_files_and_uses_the_app_key() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("typesafe")).unwrap();
    std::fs::write(
        dir.path().join("typesafe/config.json"),
        json!({"apiKey": FILE_KEY, "model": "jev-file"}).to_string(),
    )
    .unwrap();
    let fake = FakeJev::start(vec![(200, rerank_ok())]).await;
    let env = env("TACIE_JEV_URL", &fake.url);
    let keys = Arc::new(Keys::new(None));
    let jev = Jev::new(
        keys.clone(),
        &settings::resolve(&env),
        &env,
        Default::default(),
    );

    assert_eq!((jev.provider_name(), jev.model()), (None, None));
    let e = jev
        .rerank(&[], "q", &["p".to_string()], kw())
        .await
        .unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (Kind::Key, "no Jev key"));
    assert!(fake.bodies().is_empty());

    keys.set("typesafe", Some(APP_KEY));
    // 파일의 model도 읽지 않아요(제품은 jev-latest).
    assert_eq!(
        (jev.provider_name(), jev.model().as_deref()),
        (Some("typesafe"), Some("jev-latest"))
    );
    jev.rerank(&[], "q", &["p".to_string()], kw())
        .await
        .unwrap();
    let h = &fake.headers()[0];
    assert_eq!(h["authorization"], format!("Bearer {APP_KEY}").as_str());
}

/// 개발 엔진은 키 파일도 읽지만 앱이 넘긴 키가 먼저예요. 앱 키를 지우면 파일 키로 돌아가요.
#[tokio::test]
async fn app_key_wins_over_the_key_file_and_falls_back_when_removed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("openrouter")).unwrap();
    std::fs::write(
        dir.path().join("openrouter/config.json"),
        json!({"apiKey": FILE_KEY}).to_string(),
    )
    .unwrap();
    let fake = FakeOpenRouter::start(vec![
        (200, reply("답 [1]", "stop")),
        (200, reply("답 [1]", "stop")),
    ])
    .await;
    let env = env("TACIE_OPENROUTER_URL", &fake.url_base);
    let keys = Arc::new(Keys::from_files(dir.path()));
    let or = OpenRouter::with_keys(
        keys.clone(),
        &settings::resolve(&env),
        &env,
        Default::default(),
    );
    let cards = [json!({"path": "a.rs", "line_start": 1, "snippet": "x"})];

    keys.set("openrouter", Some(APP_KEY));
    or.answer("q", &[], &cards).await.unwrap();
    keys.set("openrouter", None);
    or.answer("q", &[], &cards).await.unwrap();
    let auth: Vec<String> = fake
        .headers()
        .iter()
        .map(|h| h["authorization"].to_str().unwrap().to_string())
        .collect();
    assert_eq!(
        auth,
        [format!("Bearer {APP_KEY}"), format!("Bearer {FILE_KEY}")]
    );
}

/// OpenRouter 키만 있으면 Jev도 OpenRouter로 가요(키 파일만 쓰던 때와 같은 순서예요).
#[test]
fn jev_follows_the_keys_it_is_given() {
    let none = HashMap::new();
    let keys = Arc::new(Keys::new(None));
    let jev = Jev::new(
        keys.clone(),
        &settings::resolve(&none),
        &none,
        Default::default(),
    );
    keys.set("openrouter", Some(APP_KEY));
    assert_eq!(jev.provider_name(), Some("openrouter"));
    assert_eq!(jev.url(), "https://openrouter.ai/api/alpha/decisions");
    keys.set("typesafe", Some(APP_KEY));
    assert_eq!(jev.provider_name(), Some("typesafe"));
    assert_eq!(jev.url(), "https://api.typesafe.ai/v1/systemone");
}

/// 요청 하나는 한 번 고른 공급자로만 가요: 고른 뒤에 키가 바뀌어도 주소·모델·키가 한 공급자의 것이에요.
#[test]
fn one_request_keeps_the_provider_it_picked() {
    let none = HashMap::new();
    let keys = Arc::new(Keys::new(None));
    let jev = Jev::new(
        keys.clone(),
        &settings::resolve(&none),
        &none,
        Default::default(),
    );
    keys.set("typesafe", Some("ts-key"));
    let route = jev.route().unwrap();
    // 요청 도중에 키가 바뀌어요(앱이 TypeSafe 키를 지우고 OpenRouter 키를 넣었어요).
    keys.set("typesafe", None);
    keys.set("openrouter", Some("or-key"));
    assert_eq!(
        (
            route.provider,
            route.url.as_str(),
            route.model.as_str(),
            route.bearer.unwrap().to_str().unwrap()
        ),
        (
            "typesafe",
            "https://api.typesafe.ai/v1/systemone",
            "jev-latest",
            "Bearer ts-key"
        )
    );
    let next = jev.route().unwrap();
    assert_eq!(
        (next.provider, next.url.as_str(), next.model.as_str()),
        (
            "openrouter",
            "https://openrouter.ai/api/alpha/decisions",
            "typesafe/jev-1.13"
        )
    );
    assert_eq!(next.bearer.unwrap().to_str().unwrap(), "Bearer or-key");
}
