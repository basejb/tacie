//! Dropbox HTTP calls (docs/33). `DropboxHttp` is the real API; tests plug in a fake.
//!
//! - RPC (`api.dropboxapi.com`) uses a JSON body; downloads and exports (`content.dropboxapi.com`) carry arguments in
//!   the `Dropbox-API-Arg` header. The header must be ASCII JSON, so `ascii_json` turns characters into \uXXXX.
//! - Each request reads the current token through `TokenFn`. 401 is `Key`; 429 waits `Retry-After` (backoff if absent)
//!   and retries; 5xx and 403 retry with exponential backoff (up to 64 seconds). If that still fails it is `Busy`
//!   (limit) or `Down`.
//! - Other 4xx are a failure of one file and come back as `Got::Fail` (with `error_summary`).
//!
//! The token lives only in the `Authorization` header (marked sensitive) and never shows in errors, logs or `Debug`.
//! File names and content are not put in error text either.

use std::fmt;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue, RETRY_AFTER};
use serde_json::{Value, json};

use crate::error::{Kind, SourceError};
use crate::gdrive::api::TokenFn;
use crate::net::net_error;

pub const API: &str = "https://api.dropboxapi.com/2/";
pub const CONTENT: &str = "https://content.dropboxapi.com/2/";
/// 호출 하나를 몇 번까지 하는지예요(한도·5xx를 기다리며). 1, 2, 4, 8, 16, 32초를 쉬어요.
pub const TRIES: u32 = 7;
/// 요청 하나를 기다리는 시간이에요.
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// 백오프 한 번의 최대예요.
const MAX_WAIT: Duration = Duration::from_secs(64);
/// `Retry-After`를 따라 쉬는 최대예요.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);

/// 호출 하나의 답이에요.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Got {
    /// 2xx의 몸통이에요.
    Body(Vec<u8>),
    /// 한도·인증·서버 오류가 아닌 4xx예요: 상태와 Dropbox의 `error_summary`(없으면 빈 글).
    Fail { status: u16, summary: String },
}

impl Got {
    /// 몸통을 JSON으로 읽어요. 실패(`Fail`)나 JSON이 아니면 `Down`이에요.
    pub fn json(self, what: &str) -> Result<Value, SourceError> {
        match self {
            Got::Body(b) => serde_json::from_slice(&b).map_err(|_| {
                SourceError::service(Kind::Down, format!("dropbox: {what} is not JSON"))
            }),
            Got::Fail { status, summary } => Err(SourceError::service(
                Kind::Down,
                format!("dropbox: {what} failed {status} {summary}"),
            )),
        }
    }
}

/// `Got::Fail`의 뜻이에요.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// 커서가 무효예요(`reset`): 처음부터 다시 받아요.
    Reset,
    NotFound,
    Restricted,
    Unsupported,
    Retry,
    /// 422: 팀 루트 머리가 맞지 않아요.
    PathRoot,
    Other(String),
}

pub fn failure(status: u16, summary: &str) -> Failure {
    if status == 422 {
        return Failure::PathRoot;
    }
    let s = summary.trim_end_matches("/..").trim_end_matches('/');
    if s.starts_with("reset") {
        Failure::Reset
    } else if s.starts_with("retry_error") {
        Failure::Retry
    } else if s.contains("not_found") {
        Failure::NotFound
    } else if s.contains("restricted_content") {
        Failure::Restricted
    } else if s.contains("unsupported_file")
        || s.contains("non_exportable")
        || s.contains("invalid_export_format")
    {
        Failure::Unsupported
    } else {
        Failure::Other(s.splitn(3, '/').take(2).collect::<Vec<_>>().join("/"))
    }
}

pub trait DropboxApi: Send + Sync {
    /// POST {api}{path}, JSON 몸통 `arg`(Content-Type: application/json). `root`가 있으면 Path-Root 머리.
    fn rpc(&self, path: &str, arg: &Value, root: Option<&str>) -> Result<Got, SourceError>;
    /// POST {content}{path}, 몸통 없음·Content-Type 없음, `Dropbox-API-Arg: ascii_json(arg)`.
    fn content(&self, path: &str, arg: &Value, root: Option<&str>) -> Result<Got, SourceError>;
}

pub struct DropboxHttp {
    api: String,
    content: String,
    token: TokenFn,
    client: Result<Client, String>,
    tries: u32,
    /// 백오프의 첫 쉼이에요(테스트는 짧게 줘요).
    unit: Duration,
}

impl fmt::Debug for DropboxHttp {
    /// 주소만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DropboxHttp")
            .field("api", &self.api)
            .field("content", &self.content)
            .finish_non_exhaustive()
    }
}

impl DropboxHttp {
    pub fn new(api: &str, content: &str, token: TokenFn) -> Self {
        Self::with_tries(api, content, token, TRIES, TIMEOUT)
    }

    /// 빨리 끝나야 할 때예요: `tries`번, 요청마다 `timeout`.
    pub fn with_tries(
        api: &str,
        content: &str,
        token: TokenFn,
        tries: u32,
        timeout: Duration,
    ) -> Self {
        let builder = Client::builder().timeout(timeout);
        let builder = if crate::is_loopback(api) && crate::is_loopback(content) {
            builder.no_proxy()
        } else {
            builder
        };
        DropboxHttp {
            api: api.to_string(),
            content: content.to_string(),
            token,
            client: builder.build().map_err(net_error),
            tries: tries.max(1),
            unit: Duration::from_secs(1),
        }
    }

    /// 백오프의 첫 쉼을 바꿔요(테스트).
    pub fn backoff_unit(mut self, unit: Duration) -> Self {
        self.unit = unit;
        self
    }

    /// `is_rpc`면 JSON 몸통, 아니면 `Dropbox-API-Arg` 머리로 보내요.
    fn call(
        &self,
        is_rpc: bool,
        path: &str,
        arg: &Value,
        root: Option<&str>,
    ) -> Result<Got, SourceError> {
        let fail = |kind: Kind, m: &str| SourceError::service(kind, format!("dropbox: {m}"));
        let client = self.client.as_ref().map_err(|e| fail(Kind::Down, e))?;
        let url = format!("{}{}", if is_rpc { &self.api } else { &self.content }, path);
        let arg_text = ascii_json(arg);
        let root_header = match root {
            Some(r) => Some(
                HeaderValue::from_str(&path_root(r))
                    .map_err(|_| fail(Kind::Down, "the path root cannot be sent in a header"))?,
            ),
            None => None,
        };
        let arg_header = if is_rpc {
            None
        } else {
            Some(
                HeaderValue::from_str(&arg_text)
                    .map_err(|_| fail(Kind::Down, "the argument cannot be sent in a header"))?,
            )
        };
        let mut wait = self.unit;
        let mut retry_after: Option<Duration> = None;
        let mut last = Kind::Busy;
        for k in 0..self.tries {
            if k > 0 {
                std::thread::sleep(retry_after.take().unwrap_or(wait));
                wait = (wait * 2).min(MAX_WAIT);
            }
            let token = (self.token)().ok_or_else(|| fail(Kind::Key, "not connected"))?;
            let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| fail(Kind::Key, "the token cannot be sent in a header"))?;
            auth.set_sensitive(true);
            let mut req = client.post(&url).header(AUTHORIZATION, auth);
            if let Some(h) = &root_header {
                req = req.header("Dropbox-API-Path-Root", h.clone());
            }
            req = match &arg_header {
                None => req
                    .header(CONTENT_TYPE, "application/json")
                    .body(arg_text.clone()),
                Some(h) => req.header("Dropbox-API-Arg", h.clone()),
            };
            let r = req.send().map_err(|e| {
                if e.is_timeout() {
                    fail(Kind::Down, "the server is too slow")
                } else {
                    fail(Kind::Network, &net_error(e))
                }
            })?;
            let status = r.status().as_u16();
            let asked = r
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(|s| Duration::from_secs(s).min(MAX_RETRY_AFTER));
            let bytes = r.bytes().map_err(|e| fail(Kind::Network, &net_error(e)))?;
            if (200..300).contains(&status) {
                return Ok(Got::Body(bytes.to_vec()));
            }
            if status == 401 {
                return Err(fail(Kind::Key, "the connection was refused"));
            }
            if status == 429 {
                last = Kind::Busy;
                retry_after = Some(asked.unwrap_or(wait.min(MAX_RETRY_AFTER)));
                continue;
            }
            if status >= 500 || status == 403 {
                last = Kind::Down;
                continue;
            }
            return Ok(Got::Fail {
                status,
                summary: summary_of(&bytes),
            });
        }
        Err(fail(
            last,
            if last == Kind::Busy {
                "rate limited"
            } else {
                "the server kept failing"
            },
        ))
    }
}

/// 오류 몸통의 `error_summary`(없으면 빈 글)예요.
fn summary_of(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v["error_summary"].as_str().map(str::to_string))
        .unwrap_or_default()
}

impl DropboxApi for DropboxHttp {
    fn rpc(&self, path: &str, arg: &Value, root: Option<&str>) -> Result<Got, SourceError> {
        self.call(true, path, arg, root)
    }

    fn content(&self, path: &str, arg: &Value, root: Option<&str>) -> Result<Got, SourceError> {
        self.call(false, path, arg, root)
    }
}

/// 머리에 실을 수 있게 ASCII만 쓰는 JSON이에요: 0x7f 이상은 \uXXXX(서로게이트 쌍 포함)로 바꿔요.
pub fn ascii_json(v: &Value) -> String {
    let s = serde_json::to_string(v).unwrap_or_else(|_| "null".to_string());
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if (c as u32) >= 0x7f {
            let mut buf = [0u16; 2];
            for unit in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `Dropbox-API-Path-Root` 머리 값이에요.
pub(crate) fn path_root(root: &str) -> String {
    ascii_json(&json!({".tag": "root", "root": root}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Kind;
    use serde_json::{Value, json};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Duration;

    fn token(t: &'static str) -> TokenFn {
        Arc::new(move || Some(t.to_string()))
    }

    fn http(url: &str) -> DropboxHttp {
        DropboxHttp::new(url, url, token("tok")).backoff_unit(Duration::from_millis(1))
    }

    /// 요청을 차례로 받아 `replies`의 날 답을 하나씩 돌려주는 작은 서버예요. 받은 요청(소문자)을 모아 돌려줘요.
    fn serve(replies: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", l.local_addr().unwrap());
        let t = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for reply in replies {
                let (mut s, _) = l.accept().unwrap();
                s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                let head_end = loop {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    let n = s.read(&mut chunk).unwrap();
                    assert!(n > 0, "the client closed before the request head ended");
                    buf.extend_from_slice(&chunk[..n]);
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                while buf.len() < head_end + len {
                    let n = s.read(&mut chunk).unwrap();
                    assert!(n > 0, "the client closed before the body ended");
                    buf.extend_from_slice(&chunk[..n]);
                }
                seen.push(String::from_utf8_lossy(&buf).to_lowercase());
                s.write_all(reply.as_bytes()).unwrap();
                s.shutdown(std::net::Shutdown::Write).unwrap();
            }
            seen
        });
        (url, t)
    }

    fn raw(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn ok(body: &str) -> String {
        raw("200 OK", "", body)
    }

    #[test]
    fn ascii_json_escapes_quotes_backslash_del_and_surrogates() {
        let s = ascii_json(&json!({"path": "/Paper 문서/😃 \"a\\b\"\u{7f}.paper"}));
        assert!(s.is_ascii());
        assert_eq!(
            s,
            r#"{"path":"/Paper \ubb38\uc11c/\ud83d\ude03 \"a\\b\"\u007f.paper"}"#
        );
        assert!(reqwest::header::HeaderValue::from_str(&s).is_ok());
    }

    #[test]
    fn rpc_sends_json_token_and_path_root() {
        let (url, t) = serve(vec![ok(r#"{"entries":[]}"#)]);
        let h = http(&url);
        let v = h
            .rpc("files/list_folder", &json!({"path": ""}), Some("123"))
            .unwrap()
            .json("list")
            .unwrap();
        assert_eq!(v["entries"], json!([]));
        let req = &t.join().unwrap()[0];
        assert!(req.starts_with("post /files/list_folder "));
        assert!(req.contains("authorization: bearer tok"));
        assert!(req.contains(r#"dropbox-api-path-root: {".tag":"root","root":"123"}"#));
        assert!(req.contains("content-type: application/json"));
    }

    #[test]
    fn account_calls_carry_no_path_root() {
        let (url, t) = serve(vec![ok("{}")]);
        http(&url)
            .rpc("users/get_current_account", &Value::Null, None)
            .unwrap();
        let req = &t.join().unwrap()[0];
        assert!(!req.contains("dropbox-api-path-root"));
        assert!(req.ends_with("\r\n\r\nnull"));
    }

    #[test]
    fn content_puts_the_arg_in_a_header_and_returns_bytes() {
        let (url, t) = serve(vec![raw(
            "200 OK",
            "Dropbox-Api-Result: {\"name\":\"a\"}\r\n",
            "# 제목",
        )]);
        let got = http(&url)
            .content(
                "files/export",
                &json!({"path": "id:abc", "export_format": "markdown"}),
                Some("1"),
            )
            .unwrap();
        assert_eq!(got, Got::Body("# 제목".as_bytes().to_vec()));
        let req = &t.join().unwrap()[0];
        assert!(req.contains(r#"dropbox-api-arg: {"path":"id:abc","export_format":"markdown"}"#));
        assert!(!req.contains("content-type"));
    }

    #[test]
    fn a_401_is_key_and_a_409_comes_back_with_its_summary() {
        let (url, _t) = serve(vec![raw(
            "401 Unauthorized",
            "",
            r#"{"error_summary":"expired_access_token/"}"#,
        )]);
        assert_eq!(
            http(&url).rpc("x", &Value::Null, None).unwrap_err().kind(),
            Kind::Key
        );
        let (url, _t) = serve(vec![raw(
            "409 Conflict",
            "",
            r#"{"error_summary":"path/not_found/.."}"#,
        )]);
        assert_eq!(
            http(&url).rpc("x", &Value::Null, None).unwrap(),
            Got::Fail {
                status: 409,
                summary: "path/not_found/..".into()
            }
        );
    }

    #[test]
    fn a_429_honours_retry_after_then_succeeds() {
        let (url, t) = serve(vec![
            raw("429 Too Many Requests", "Retry-After: 0\r\n", "{}"),
            ok("{}"),
        ]);
        let got = http(&url).rpc("x", &Value::Null, None).unwrap();
        assert_eq!(got, Got::Body(b"{}".to_vec()));
        assert_eq!(t.join().unwrap().len(), 2);
    }

    #[test]
    fn rate_limits_that_never_end_are_busy_and_5xx_and_403_are_down() {
        for (status, kind) in [
            ("429 Too Many Requests", Kind::Busy),
            ("503 Service Unavailable", Kind::Down),
            ("403 Forbidden", Kind::Down),
        ] {
            let replies = (0..TRIES)
                .map(|_| {
                    if status.starts_with("429") {
                        raw(status, "Retry-After: 0\r\n", "{}")
                    } else {
                        raw(status, "", "{}")
                    }
                })
                .collect();
            let (url, t) = serve(replies);
            let e = http(&url).rpc("x", &Value::Null, None).unwrap_err();
            assert_eq!(e.kind(), kind, "{status}");
            assert_eq!(t.join().unwrap().len(), TRIES as usize);
        }
    }

    #[test]
    fn failures_are_classified() {
        assert_eq!(failure(409, "reset/.."), Failure::Reset);
        assert_eq!(failure(409, "path/not_found/.."), Failure::NotFound);
        assert_eq!(
            failure(409, "path/restricted_content/"),
            Failure::Restricted
        );
        assert_eq!(failure(409, "unsupported_file/.."), Failure::Unsupported);
        assert_eq!(failure(409, "non_exportable/"), Failure::Unsupported);
        assert_eq!(failure(409, "retry_error/"), Failure::Retry);
        assert_eq!(failure(422, ""), Failure::PathRoot);
        assert_eq!(
            failure(409, "path/malformed_path/"),
            Failure::Other("path/malformed_path".into())
        );
    }

    #[test]
    fn debug_hides_the_token() {
        let h = DropboxHttp::new(API, CONTENT, token("sl.secret"));
        assert!(!format!("{h:?}").contains("secret"));
    }
}
