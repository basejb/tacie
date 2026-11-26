//! The US relay (ai-us.tacie.app, apps/server README "US relay"): the address the engine accepts, and the rule for
//! moving to Seoul when it cannot connect.

use tacie_engine::openrouter::config_url;
use tacie_engine::settings::{retry_in_seoul, retry_in_seoul_after, seoul_fallback};
use tacie_engine::soniox::relay_key_url;

#[test]
fn us_paths_map_to_the_same_seoul_paths() {
    assert_eq!(
        seoul_fallback("https://ai-us.tacie.app/ai/jev").as_deref(),
        Some("https://ai.tacie.app/ai/jev")
    );
    assert_eq!(
        seoul_fallback("https://ai-us.tacie.app/ai/openrouter/chat/completions").as_deref(),
        Some("https://ai.tacie.app/ai/openrouter/chat/completions")
    );
    assert_eq!(
        seoul_fallback("https://ai-us.tacie.app/ai/soniox/stream-key").as_deref(),
        Some("https://ai.tacie.app/ai/soniox/stream-key")
    );
    // 서울, 옛 Worker, 그 밖의 주소는 옮기지 않아요(서울에서 다시 서울로 가지 않아요)
    for url in [
        "https://ai.tacie.app/ai/jev",
        "https://auth.tacie.app/ai/jev",
        "https://ai-us.tacie.app.evil.example/ai/jev",
        "https://ai-us.tacie.app/me",
        "http://127.0.0.1:9/ai/jev",
    ] {
        assert_eq!(seoul_fallback(url), None, "{url}");
    }
}

#[test]
fn the_us_relay_has_its_own_config_and_stream_key() {
    assert_eq!(
        config_url("https://ai-us.tacie.app/ai/openrouter").as_deref(),
        Some("https://ai-us.tacie.app/ai/config")
    );
    assert_eq!(
        relay_key_url("https://ai-us.tacie.app/ai/openrouter").as_deref(),
        Some("https://ai-us.tacie.app/ai/soniox/stream-key")
    );
}

/// 연결하지 못한 요청만 서울로 옮겨요. 미국 이름을 닫힌 loopback 포트로 풀어서 진짜 연결 실패를 만들어요.
#[tokio::test]
async fn only_a_failed_connection_moves_to_seoul() {
    let url = "https://ai-us.tacie.app/ai/jev";
    let client = reqwest::Client::builder()
        .no_proxy()
        .resolve("ai-us.tacie.app", "127.0.0.1:443".parse().unwrap())
        .build()
        .unwrap();
    let e = client.post(url).send().await.unwrap_err();
    assert!(e.is_connect(), "{e}");
    assert_eq!(
        retry_in_seoul(url, &e).as_deref(),
        Some("https://ai.tacie.app/ai/jev")
    );
    // 같은 실패라도 서울 주소면 그대로예요
    assert_eq!(retry_in_seoul("https://ai.tacie.app/ai/jev", &e), None);
}

/// 미국 중계가 세션을 모르면(401, 서울 로그인이 아직 복제되지 않았을 때) 서울로 한 번 옮겨요. 다른 거절은 그대로예요.
#[test]
fn only_an_unknown_session_moves_to_seoul() {
    let us = "https://ai-us.tacie.app/ai/openrouter/chat/completions";
    assert_eq!(
        retry_in_seoul_after(us, 401).as_deref(),
        Some("https://ai.tacie.app/ai/openrouter/chat/completions")
    );
    for status in [200, 400, 402, 403, 429, 500, 503] {
        assert_eq!(retry_in_seoul_after(us, status), None, "{status}");
    }
    // 서울에서 받은 401은 다시 옮기지 않아요(한 번만)
    assert_eq!(
        retry_in_seoul_after("https://ai.tacie.app/ai/jev", 401),
        None
    );
}
