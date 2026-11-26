//! Slack calls to loopback (the fakes in tests and contracts) skip the proxy. It changes process environment
//! variables, so this file has only one test.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use tacie_sources::slack::{SlackApi, SlackHttp};

/// 연결마다 요청 하나를 읽고 `body`로 답해요. 받은 요청 줄을 모아요.
fn serve(body: &'static str) -> (u16, Arc<Mutex<Vec<String>>>) {
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
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (port, seen)
}

#[test]
fn slack_calls_to_loopback_skip_the_proxy() {
    let (proxy_port, proxied) = serve(r#"{"ok": false, "error": "proxied"}"#);
    // SAFETY: 이 테스트 바이너리에는 테스트가 이것 하나이고, 서버 스레드는 환경을 보지 않아요.
    unsafe {
        std::env::set_var("HTTP_PROXY", format!("http://127.0.0.1:{proxy_port}"));
        for name in ["NO_PROXY", "no_proxy", "REQUEST_METHOD"] {
            std::env::remove_var(name);
        }
    }
    let (port, direct) = serve(r#"{"ok": true}"#);
    let http = SlackHttp::new(&format!("http://127.0.0.1:{port}/api/"), "xoxb-test");
    let got = http.call("auth.test", &[]);
    assert!(got.is_ok(), "{got:?}");
    assert_eq!(*direct.lock().unwrap(), ["GET /api/auth.test HTTP/1.1"]);
    assert!(proxied.lock().unwrap().is_empty());
}
