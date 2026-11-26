//! Outgoing calls follow the proxy (environment variables, macOS system settings), and calls to loopback (fakes in
//! contract tests, the eval cache proxy) skip the proxy. It changes process environment variables, so this file has
//! only one test, and the changes are done before any tokio threads start.

mod common {
    #[allow(dead_code)] // 헤더는 보지 않아요
    pub mod fake_jev;
}

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::fake_jev::FakeJev;
use serde_json::json;
use tacie_engine::jev::{Jev, JevKw};
use tacie_engine::settings;

const RERANK_OK: &str = r#"{"answers": {"p0": {"noul": 0.9}}, "usage": {"input_tokens": 10}}"#;

/// 받은 요청 줄을 쌓고 Jev 모양의 200을 답하는 HTTP 프록시예요(연결마다 요청 하나).
fn recording_proxy() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let seen = lines.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            let mut length = 0usize;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
                let lower = header.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            seen.lock().unwrap().push(first.trim_end().to_string());
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{RERANK_OK}",
                RERANK_OK.len()
            );
        }
    });
    (port, lines)
}

fn jev(config: &std::path::Path, url: &str) -> Jev {
    let env = HashMap::from([("TACIE_JEV_URL".to_string(), url.to_string())]);
    let last = Arc::new(Mutex::new(HashMap::new()));
    Jev::from_config(config, &settings::resolve(&env), &env, last)
}

#[test]
fn remote_calls_use_the_env_proxy_and_loopback_calls_skip_it() {
    if !cfg!(debug_assertions) {
        // release 빌드는 loopback이 아닌 덮어쓰기(아래 (a))를 받지 않아요.
        eprintln!("skip: release builds ignore non-loopback TACIE_JEV_URL");
        return;
    }
    let (proxy_port, proxied) = recording_proxy();
    // SAFETY: 이 테스트 바이너리에는 테스트가 이것 하나이고, 아직 다른 스레드가 환경을 읽지 않아요
    // (프록시 스레드는 환경을 보지 않아요).
    unsafe {
        std::env::set_var("HTTP_PROXY", format!("http://127.0.0.1:{proxy_port}"));
        for name in ["NO_PROXY", "no_proxy", "REQUEST_METHOD"] {
            std::env::remove_var(name);
        }
    }

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("typesafe")).unwrap();
    std::fs::write(
        dir.path().join("typesafe/config.json"),
        json!({"apiKey": "sk-test-not-a-real-key"}).to_string(),
    )
    .unwrap();
    let kw = JevKw {
        timeout: Duration::from_secs(5),
        attempts: 1,
    };
    let passages = ["조각".to_string()];

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // (a) 바깥 주소는 프록시로 가요(absolute-form 요청 줄).
        let remote = jev(dir.path(), "http://jev.invalid/v1/systemone");
        let out = remote
            .rerank(&[], "나: 포트는요?", &passages, kw)
            .await
            .unwrap();
        assert_eq!(out.p, [0.9]);
        {
            let lines = proxied.lock().unwrap();
            assert_eq!(lines.len(), 1, "{lines:?}");
            assert!(
                lines[0].starts_with("POST http://jev.invalid/v1/systemone "),
                "{lines:?}"
            );
        }

        // (b) loopback 주소는 프록시를 건너뛰고 바로 가요.
        let fake = FakeJev::start(vec![(200, serde_json::from_str(RERANK_OK).unwrap())]).await;
        let local = jev(dir.path(), &fake.url);
        let out = local
            .rerank(&[], "나: 포트는요?", &passages, kw)
            .await
            .unwrap();
        assert_eq!(out.p, [0.9]);
        assert_eq!(fake.bodies().len(), 1);
        assert_eq!(
            proxied.lock().unwrap().len(),
            1,
            "loopback call went through the proxy"
        );
    });
}
