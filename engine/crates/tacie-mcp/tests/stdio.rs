use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const NOT_RUNNING: &str = "Tacie is not running, or its MCP is turned off. Open Tacie and turn it on in 설정 → 연결 → MCP (Settings → Connections → MCP).";

struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    out: BufReader<ChildStdout>,
    next: u64,
}

impl Mcp {
    fn start(sock: &Path) -> Mcp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tacie-mcp"))
            .arg("--socket")
            .arg(sock)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let out = BufReader::new(child.stdout.take().unwrap());
        Mcp {
            child,
            stdin,
            out,
            next: 1,
        }
    }
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin.as_mut().unwrap(), "{line}").unwrap();
        let mut got = String::new();
        self.out.read_line(&mut got).unwrap();
        let v: Value = serde_json::from_str(&got).unwrap();
        assert_eq!(v["id"], id);
        v
    }
    fn init(&mut self, name: &str) -> Value {
        let r = self.rpc(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": name, "version": "1"}}),
        );
        writeln!(
            self.stdin.as_mut().unwrap(),
            "{}",
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .unwrap();
        r
    }
    fn call(&mut self, tool: &str, args: Value) -> Value {
        self.rpc("tools/call", json!({"name": tool, "arguments": args}))
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn tmp() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir_in("/tmp").unwrap();
    let s = d.path().join("m.sock");
    (d, s)
}

/// Serves one connection: checks the request, replies with `reply`.
fn serve_once(
    l: UnixListener,
    reply: Value,
    check: impl FnOnce(Value) + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        check(serde_json::from_str(&line).unwrap());
        let mut s = s;
        writeln!(s, "{reply}").unwrap();
    })
}

fn text(v: &Value) -> &str {
    v["result"]["content"][0]["text"].as_str().unwrap()
}

#[test]
fn initialize_and_list_without_app() {
    let (_d, sock) = tmp();
    let mut m = Mcp::start(&sock);
    let r = m.init("claude-code");
    assert_eq!(r["result"]["serverInfo"]["name"], "tacie");
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    let l = m.rpc("tools/list", json!({}));
    let names: Vec<&str> = l["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["search", "list_meetings", "list_ledger", "read_transcript"]
    );
    let rt = &l["result"]["tools"][3];
    let d = rt["description"].as_str().unwrap();
    assert!(d.contains("line key") && d.contains("found: false") && d.contains("truncated: true"));
    let a = rt["inputSchema"]["properties"]["around"]["description"]
        .as_str()
        .unwrap();
    assert!(a.contains("words from the transcript") && a.contains("found: false"));
    let p = m.rpc("ping", json!({}));
    assert!(p["result"].is_object());
    let u = m.rpc("nope", json!({}));
    assert_eq!(u["error"]["code"], -32601);
}

#[test]
fn call_without_app_is_tool_error() {
    let (_d, sock) = tmp();
    let mut m = Mcp::start(&sock);
    m.init("cursor");
    let r = m.call("search", json!({"query": "x"}));
    assert_eq!(r["result"]["isError"], true);
    assert_eq!(text(&r), NOT_RUNNING);
}

#[test]
fn call_roundtrip() {
    let (_d, sock) = tmp();
    let l = UnixListener::bind(&sock).unwrap();
    let h = serve_once(
        l,
        json!({"v": 1, "ok": true, "result": {"cards": []}}),
        |req| {
            assert_eq!(req["v"], 1);
            assert_eq!(req["tool"], "search");
            assert_eq!(req["client"], "claude_code");
            assert_eq!(req["args"]["query"], "q");
        },
    );
    let mut m = Mcp::start(&sock);
    m.init("claude-code");
    let r = m.call("search", json!({"query": "q"}));
    assert!(r["result"]["isError"].is_null() || r["result"]["isError"] == false);
    let got: Value = serde_json::from_str(text(&r)).unwrap();
    assert_eq!(got, json!({"cards": []}));
    h.join().unwrap();
}

#[test]
fn app_error_maps_to_text() {
    let (_d, sock) = tmp();
    let l = UnixListener::bind(&sock).unwrap();
    let h = serve_once(l, json!({"v": 1, "ok": false, "error": "limit"}), |_| {});
    let mut m = Mcp::start(&sock);
    m.init("other");
    let r = m.call("list_meetings", json!({}));
    assert_eq!(r["result"]["isError"], true);
    assert_eq!(
        text(&r),
        "This month's Tacie usage limit is reached. It resets next month."
    );
    h.join().unwrap();
}

#[test]
fn exits_on_stdin_eof() {
    let (_d, sock) = tmp();
    let mut m = Mcp::start(&sock);
    m.init("x");
    drop(m.stdin.take());
    let t = Instant::now();
    loop {
        if let Some(s) = m.child.try_wait().unwrap() {
            assert!(s.success());
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(1), "did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn later_call_succeeds_after_app_starts() {
    let (_d, sock) = tmp();
    let mut m = Mcp::start(&sock);
    m.init("claude");
    let r = m.call("list_ledger", json!({}));
    assert_eq!(r["result"]["isError"], true);
    let l = UnixListener::bind(&sock).unwrap();
    let h = serve_once(
        l,
        json!({"v": 1, "ok": true, "result": {"items": []}}),
        |req| assert_eq!(req["client"], "claude_desktop"),
    );
    let r = m.call("list_ledger", json!({}));
    assert_eq!(
        serde_json::from_str::<Value>(text(&r)).unwrap(),
        json!({"items": []})
    );
    h.join().unwrap();
}
