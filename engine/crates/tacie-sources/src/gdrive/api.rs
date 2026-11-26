//! Google Drive REST calls (docs/19 10절, 15-1절). `GDriveHttp` is the real API; tests plug in a fake.
//!
//! - Each request reads the current token through `TokenFn`. Drive tokens last 1 hour, so the app refreshes them
//!   during long fetches.
//! - 401 is `Key`. 429, 403 `userRateLimitExceeded`/`rateLimitExceeded` and 5xx wait a few times with exponential
//!   backoff (up to 64 seconds), then become `Busy` (limit) or `Down` (5xx).
//! - Other 4xx are a failure of one file, so they are not thrown as errors but returned as `Got::Fail` (the caller
//!   deletes the file or keeps only the title).
//!
//! The token lives only in the `Authorization` header (marked sensitive) and never shows in errors, logs or `Debug`.
//! File names and content are not put in error text either.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde_json::Value;

use crate::error::{Kind, SourceError};
use crate::net::net_error;

/// Drive REST v3 주소예요. 테스트는 로컬 가짜를 가리켜요.
pub const API: &str = "https://www.googleapis.com/drive/v3/";
/// 호출 하나를 몇 번까지 하는지예요(한도·5xx를 기다리며). 1, 2, 4, 8, 16, 32초를 쉬어요.
pub const TRIES: u32 = 7;
/// 요청 하나를 기다리는 시간이에요. export는 큰 문서가 느려요.
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// 백오프 한 번의 최대예요.
const MAX_WAIT: Duration = Duration::from_secs(64);

/// 요청마다 지금 토큰을 읽는 함수예요. None이면 연결이 없어요.
pub type TokenFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// GET 하나의 답이에요.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Got {
    /// 2xx의 몸통이에요.
    Body(Vec<u8>),
    /// 한도·인증·서버 오류가 아닌 4xx예요: 상태와 Google 오류의 `reason`(없으면 빈 글).
    Fail { status: u16, reason: String },
}

impl Got {
    /// 몸통을 JSON으로 읽어요. 실패(`Fail`)나 JSON이 아니면 `Down`이에요.
    pub fn json(self, what: &str) -> Result<Value, SourceError> {
        match self {
            Got::Body(b) => serde_json::from_slice(&b).map_err(|_| {
                SourceError::service(Kind::Down, format!("gdrive: {what} is not JSON"))
            }),
            Got::Fail { status, reason } => Err(SourceError::service(
                Kind::Down,
                format!("gdrive: {what} failed {status} {reason}"),
            )),
        }
    }
}

/// Drive GET 하나예요. `path`는 `files`, `files/<id>/export`, `changes` 같은 것이에요.
pub trait GDriveApi: Send + Sync {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Got, SourceError>;
}

/// 진짜 API예요: `GET {base}{path}?{query}`, `Authorization: Bearer {지금 토큰}`.
pub struct GDriveHttp {
    base: String,
    token: TokenFn,
    client: Result<Client, String>,
    tries: u32,
    /// 백오프의 첫 쉼이에요(테스트는 짧게 줘요).
    unit: Duration,
}

impl fmt::Debug for GDriveHttp {
    /// 주소만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GDriveHttp")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl GDriveHttp {
    pub fn new(base: &str, token: TokenFn) -> Self {
        Self::with_tries(base, token, TRIES, TIMEOUT)
    }

    /// 30분 확인처럼 빨리 끝나야 할 때예요: `tries`번, 요청마다 `timeout`.
    pub fn with_tries(base: &str, token: TokenFn, tries: u32, timeout: Duration) -> Self {
        let builder = Client::builder().timeout(timeout);
        let builder = if crate::is_loopback(base) {
            builder.no_proxy()
        } else {
            builder
        };
        GDriveHttp {
            base: base.to_string(),
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
}

/// Google 오류 몸통의 `error.errors[0].reason`(없으면 `error.status`)이에요.
pub(crate) fn reason_of(body: &[u8]) -> String {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let e = &v["error"];
    e["errors"][0]["reason"]
        .as_str()
        .or_else(|| e["status"].as_str())
        .unwrap_or("")
        .to_string()
}

/// 쉬었다 다시 물을 403 이유예요(한도).
pub(crate) fn rate_limited(status: u16, reason: &str) -> bool {
    status == 429
        || (status == 403
            && matches!(
                reason,
                "userRateLimitExceeded" | "rateLimitExceeded" | "RESOURCE_EXHAUSTED"
            ))
}

impl GDriveApi for GDriveHttp {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Got, SourceError> {
        let fail = |kind: Kind, m: &str| SourceError::service(kind, format!("gdrive: {m}"));
        let client = self.client.as_ref().map_err(|e| fail(Kind::Down, e))?;
        let mut url = format!("{}{}", self.base, path);
        if !query.is_empty() {
            let q = form_urlencoded::Serializer::new(String::new())
                .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())))
                .finish();
            url.push('?');
            url.push_str(&q);
        }
        let mut wait = self.unit;
        let mut last = Kind::Busy;
        for k in 0..self.tries {
            if k > 0 {
                std::thread::sleep(wait);
                wait = (wait * 2).min(MAX_WAIT);
            }
            let token = (self.token)().ok_or_else(|| fail(Kind::Key, "not connected"))?;
            let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| fail(Kind::Key, "the token cannot be sent in a header"))?;
            auth.set_sensitive(true);
            let r = client
                .get(&url)
                .header(AUTHORIZATION, auth)
                .send()
                .map_err(|e| fail(Kind::Network, &net_error(e)))?;
            let status = r.status().as_u16();
            let bytes = r.bytes().map_err(|e| fail(Kind::Network, &net_error(e)))?;
            if (200..300).contains(&status) {
                return Ok(Got::Body(bytes.to_vec()));
            }
            if status == 401 {
                return Err(fail(Kind::Key, "the connection was refused"));
            }
            let reason = reason_of(&bytes);
            if rate_limited(status, &reason) {
                last = Kind::Busy;
                continue;
            }
            if status >= 500 {
                last = Kind::Down;
                continue;
            }
            return Ok(Got::Fail { status, reason });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::github::{FakeGitHub, Reply};
    use serde_json::json;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn token(t: &'static str) -> TokenFn {
        Arc::new(move || Some(t.to_string()))
    }

    fn http(code: u16, body: Value) -> Reply {
        Reply::Http {
            code,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.to_string().into_bytes(),
        }
    }

    fn fake(f: impl Fn(&str) -> Reply + Send + Sync + 'static) -> (Arc<FakeGitHub>, GDriveHttp) {
        let g = FakeGitHub::start();
        g.status_with(move |p, _| f(p));
        let h = GDriveHttp::new(&g.url(), token(FakeGitHub::TOKEN))
            .backoff_unit(Duration::from_millis(1));
        (g, h)
    }

    #[test]
    fn a_body_comes_back_with_the_query_and_the_token() {
        let (g, h) = fake(|p| {
            assert!(p.starts_with("files?"), "{p}");
            assert!(p.contains("q=trashed+%3D+false"), "{p}");
            Reply::Json(json!({"files": []}))
        });
        let got = h.get("files", &[("q", "trashed = false".into())]).unwrap();
        assert_eq!(got.json("list").unwrap()["files"], json!([]));
        assert!(g.calls()[0].has_token);
    }

    #[test]
    fn unauthorized_is_key() {
        let (_g, h) = fake(|_| http(401, json!({"error": {"code": 401}})));
        assert_eq!(h.get("about", &[]).unwrap_err().kind(), Kind::Key);
    }

    #[test]
    fn a_rate_limit_waits_then_succeeds() {
        let n = AtomicUsize::new(0);
        let (_g, h) = fake(move |_| {
            if n.fetch_add(1, Ordering::SeqCst) < 2 {
                http(
                    403,
                    json!({"error": {"errors": [{"reason": "userRateLimitExceeded"}]}}),
                )
            } else {
                Reply::Json(json!({"ok": true}))
            }
        });
        assert_eq!(
            h.get("x", &[]).unwrap(),
            Got::Body(b"{\"ok\":true}".to_vec())
        );
    }

    #[test]
    fn a_rate_limit_that_never_ends_is_busy_and_5xx_is_down() {
        let (_g, h) = fake(|_| http(429, json!({})));
        assert_eq!(h.get("x", &[]).unwrap_err().kind(), Kind::Busy);
        let (_g, h) = fake(|_| http(503, json!({})));
        assert_eq!(h.get("x", &[]).unwrap_err().kind(), Kind::Down);
    }

    #[test]
    fn a_file_level_failure_is_returned_with_its_reason() {
        let (_g, h) = fake(|_| {
            http(
                403,
                json!({"error": {"errors": [{"reason": "exportSizeLimitExceeded"}]}}),
            )
        });
        assert_eq!(
            h.get("files/a/export", &[]).unwrap(),
            Got::Fail {
                status: 403,
                reason: "exportSizeLimitExceeded".into()
            }
        );
    }

    #[test]
    fn the_token_is_read_on_every_request() {
        let seen = Arc::new(Mutex::new(0usize));
        let s2 = seen.clone();
        let g = FakeGitHub::start();
        g.status_with(|_, _| Reply::Json(json!({})));
        let h = GDriveHttp::new(
            &g.url(),
            Arc::new(move || {
                *s2.lock().unwrap() += 1;
                Some(FakeGitHub::TOKEN.to_string())
            }),
        );
        h.get("a", &[]).unwrap();
        h.get("b", &[]).unwrap();
        assert_eq!(*seen.lock().unwrap(), 2);
    }

    #[test]
    fn without_a_token_it_is_key() {
        let h = GDriveHttp::new(API, Arc::new(|| None));
        assert_eq!(h.get("about", &[]).unwrap_err().kind(), Kind::Key);
    }

    #[test]
    fn debug_hides_the_token() {
        let h = GDriveHttp::new(API, token("ya29.secret"));
        assert!(!format!("{h:?}").contains("secret"));
    }
}
