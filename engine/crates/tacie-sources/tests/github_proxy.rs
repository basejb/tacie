//! GitHub calls to loopback (the fakes in tests and contracts) skip the proxy, and other addresses follow the proxy
//! (spec 4-1). It changes process environment variables, so this file has only one test (same approach as
//! `slack_proxy.rs`).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tacie_sources::github::http::{Accept, Http, Target};
use tacie_sources::testing::github::{FakeGitHub, Reply};

/// 가짜 프록시예요: 연결마다 요청 줄 하나를 모으고 200 `[]`로 답해요.
fn proxy() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            let _ = reader.read_line(&mut first);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            log.lock().unwrap().push(first.trim_end().to_string());
            let mut stream = stream;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]"
            );
        }
    });
    (port, seen)
}

#[test]
fn loopback_base_skips_the_proxy() {
    let (proxy_port, proxied) = proxy();
    // SAFETY: 이 테스트 바이너리에는 테스트가 이것 하나이고, 서버 스레드는 환경을 보지 않아요.
    unsafe {
        for name in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"] {
            std::env::set_var(name, format!("http://127.0.0.1:{proxy_port}"));
        }
        for name in ["NO_PROXY", "no_proxy", "REQUEST_METHOD"] {
            std::env::remove_var(name);
        }
    }
    let t = Duration::from_secs(5);
    let fake = FakeGitHub::start();
    fake.login(Reply::Json(json!({"login": "octo"})));
    let a = Http::new(&fake.url())
        .unwrap()
        .get(
            FakeGitHub::TOKEN,
            Target::Path("user"),
            Accept::Json,
            &[],
            t,
        )
        .unwrap();
    assert_eq!(a.status, 200);
    assert_eq!(fake.calls().len(), 1);
    assert!(proxied.lock().unwrap().is_empty());

    // 대조: loopback이 아닌 주소는 프록시로 가요(프록시가 정말 설정됐다는 증거예요)
    let far = Http::new("http://github.invalid/").unwrap();
    let a = far
        .get("t", Target::Path("user"), Accept::Json, &[], t)
        .unwrap();
    assert_eq!(a.status, 200);
    assert_eq!(
        *proxied.lock().unwrap(),
        ["GET http://github.invalid/user HTTP/1.1"]
    );
}
