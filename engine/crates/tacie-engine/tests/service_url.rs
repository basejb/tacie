//! Overrides for outside service addresses (`TACIE_JEV_URL`, `TACIE_OPENROUTER_URL`). Debug builds accept any
//! value; release builds accept only http and https on loopback (127.0.0.1, localhost, [::1]) (docs/04 2-1).
//! The release-side decision is tested directly on the pure function with `allow_any = false`, and the contract
//! tests that run as release check it once more.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tacie_engine::jev::Jev;
use tacie_engine::settings::{self, is_loopback, service_url};

const NAME: &str = "TACIE_JEV_URL";

fn env(value: Option<&str>) -> HashMap<String, String> {
    value
        .map(|v| HashMap::from([(NAME.to_string(), v.to_string())]))
        .unwrap_or_default()
}

#[test]
fn override_urls_are_loopback_only_in_release() {
    let loopback = [
        "http://127.0.0.1:9/x",
        "https://LOCALHOST/",
        "http://[::1]:8080",
    ];
    // loopback처럼 보이지만 아닌 주소예요.
    let remote = [
        "http://127.0.0.1.evil.example",
        "http://localhost@evil.example",
        "http://user:pw@127.0.0.1/",
        "ftp://127.0.0.1",
        "http://127.0.0.2",
        "https://api.typesafe.ai/v1/systemone",
        "not a url",
    ];
    for url in loopback {
        assert!(is_loopback(url), "{url}");
        assert_eq!(
            service_url(&env(Some(url)), NAME, false).as_deref(),
            Some(url),
            "{url}"
        );
        assert_eq!(
            service_url(&env(Some(url)), NAME, true).as_deref(),
            Some(url)
        );
    }
    for url in remote {
        assert!(!is_loopback(url), "{url}");
        assert_eq!(service_url(&env(Some(url)), NAME, false), None, "{url}");
        assert_eq!(
            service_url(&env(Some(url)), NAME, true).as_deref(),
            Some(url),
            "debug builds take any value: {url}"
        );
    }
    // Tacie AI 중계는 release 빌드도 받아요(docs/14). 서울(ai.tacie.app), 옛 Worker(auth.tacie.app), 미국
    // (ai-us.tacie.app)이에요. 이름만 비슷한 주소는 아니에요
    for url in [
        "https://ai.tacie.app/ai/openrouter",
        "https://ai.tacie.app/ai/jev",
        "https://ai-us.tacie.app/ai/openrouter",
        "https://ai-us.tacie.app/ai/jev",
        "https://auth.tacie.app/ai/openrouter",
        "https://auth.tacie.app/ai/jev",
    ] {
        assert_eq!(
            service_url(&env(Some(url)), NAME, false).as_deref(),
            Some(url)
        );
    }
    for url in [
        "https://auth.tacie.app.evil.example/ai/jev",
        "https://ai.tacie.app.evil.example/ai/jev",
        "https://ai-us.tacie.app.evil.example/ai/jev",
        "https://ai-us.tacie.app/auth/exchange",
        "https://ai.tacie.app/other",
        "https://auth.tacie.app/notion/token",
        "http://auth.tacie.app/ai/jev",
    ] {
        assert_eq!(service_url(&env(Some(url)), NAME, false), None, "{url}");
    }
    for allow_any in [false, true] {
        assert_eq!(service_url(&env(Some("")), NAME, allow_any), None);
        assert_eq!(service_url(&env(None), NAME, allow_any), None);
    }
}

#[test]
fn jev_url_comes_from_service_url() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("typesafe")).unwrap();
    std::fs::write(
        dir.path().join("typesafe/config.json"),
        json!({"apiKey": "sk-test-not-a-real-key"}).to_string(),
    )
    .unwrap();
    let jev = |env: HashMap<String, String>| {
        let last = Arc::new(Mutex::new(HashMap::new()));
        Jev::from_config(dir.path(), &settings::resolve(&env), &env, last)
    };
    assert_eq!(jev(env(None)).url(), "https://api.typesafe.ai/v1/systemone");
    assert_eq!(
        jev(env(Some("http://127.0.0.1:9/v1"))).url(),
        "http://127.0.0.1:9/v1"
    );
}
