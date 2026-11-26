//! Atlassian REST calls and the point budget. `AtlassianHttp` is the real API; tests plug in the fake from
//! `testing::atlassian`.
//!
//! - Each request reads the current token through `TokenFn`. The engine never refreshes the token; the app does.
//! - 401 is `Key`. 5xx waits a few times with exponential backoff (up to 64 seconds), then becomes `Down`.
//! - A 429 has two causes. If `RateLimit-Reason` contains `burst`, or there is no reason, it is the burst limit: wait
//!   `Retry-After` and ask again (`Busy` if it never succeeds). Any other reason is the hourly point limit, where
//!   waiting does not help. It returns `Busy` ("quota until <time>") at once, and the caller reads when it lifts with
//!   `quota_until` and rests until then.
//! - Other 4xx are a failure of one item, so they are not thrown as errors but returned as `Got::Fail` (for POST, the
//!   status in `Answer`).
//!
//! The token lives only in the `Authorization` header (marked sensitive) and never shows in errors, logs or `Debug`.
//! Titles and content are not put in error text either.

use std::fmt;
use std::time::Duration;

use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::Value;

use crate::error::{Kind, SourceError};
use crate::gdrive::api::{Got, TokenFn};
use crate::net::net_error;

/// Atlassian 게이트웨이 주소예요. 테스트는 로컬 가짜를 가리켜요.
pub const API: &str = "https://api.atlassian.com/";
/// 호출 하나를 몇 번까지 하는지예요(순간 한도·5xx를 기다리며). 1, 2, 4, 8, 16, 32초를 쉬어요.
pub const TRIES: u32 = 7;
/// 요청 하나를 기다리는 시간이에요.
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// 백오프 한 번의 최대예요.
const MAX_WAIT: Duration = Duration::from_secs(64);
/// 한 Mac이 시간당 쓸 수 있는 포인트예요(처음 받기만 이 안에서 해요).
pub const POINTS_PER_HOUR: u32 = 5_000;
/// 시간당 한도 오류 글의 앞부분이에요. `quota_until`이 이 글 뒤의 시각을 읽어요.
const QUOTA_PREFIX: &str = "atlassian: quota until ";

/// POST 하나의 답이에요(report-accounts는 204/200과 `Cycle-Period`초를 줘요).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
    /// `Cycle-Period` 머리(초)예요. 없으면 None이에요.
    pub cycle_period: Option<u64>,
}

/// Atlassian 호출이에요. `path`는 `base` 뒤 경로로, 예: `ex/jira/<cloud>/rest/api/3/search/jql`.
pub trait AtlassianApi: Send + Sync {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Got, SourceError>;
    fn post(&self, path: &str, body: &Value) -> Result<Answer, SourceError>;
    /// An email and API token against the site (the open edition), not an OAuth token through the gateway.
    fn is_basic(&self) -> bool {
        false
    }
}

/// With an API token the site answers itself: `ex/jira/{cloud}/…` and `ex/confluence/{cloud}/…` (whose rest starts
/// with `wiki/`) become `{site}/…`. Paths only the OAuth gateway has (`oauth/…`, `app/…`) give None.
pub fn basic_url(site_url: &str, path: &str) -> Option<String> {
    let rest = path
        .strip_prefix("ex/jira/")
        .or_else(|| path.strip_prefix("ex/confluence/"))?;
    let (_cloud, rest) = rest.split_once('/')?;
    Some(format!("{}/{}", site_url.trim_end_matches('/'), rest))
}

/// 시간당 포인트 장부예요: 요청 1, 글·댓글 하나마다 1, 사용자 하나마다 2. `state.json`에 그대로 남아요.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Points {
    pub used: u32,
    /// 쓴 시각의 `timestamp / 3600`이에요. 시간이 바뀌면 `used`가 0으로 돌아가요.
    pub hour: i64,
}

impl Points {
    pub fn charge(&mut self, now: DateTime<Utc>, requests: u32, objects: u32, users: u32) {
        let hour = now.timestamp() / 3600;
        if hour != self.hour {
            self.hour = hour;
            self.used = 0;
        }
        self.used = self
            .used
            .saturating_add(requests)
            .saturating_add(objects)
            .saturating_add(users.saturating_mul(2));
    }

    /// 이 시간에 남은 포인트예요.
    pub fn left(&self, now: DateTime<Utc>) -> u32 {
        if now.timestamp() / 3600 != self.hour {
            return POINTS_PER_HOUR;
        }
        POINTS_PER_HOUR.saturating_sub(self.used)
    }
}

/// 시간당 한도 오류에서 풀리는 때를 읽어요. 그 오류가 아니면 None이에요.
pub fn quota_until(e: &SourceError) -> Option<DateTime<Utc>> {
    let SourceError::Service {
        kind: Kind::Busy,
        message,
    } = e
    else {
        return None;
    };
    let t = message.strip_prefix(QUOTA_PREFIX)?;
    DateTime::parse_from_rfc3339(t.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// 시간당 한도로 쉬는 가장 긴 시간이에요. `X-RateLimit-Reset`이 이보다 멀면 지금부터 이만큼만 쉬어요.
pub(crate) const MAX_PAUSE_HOURS: i64 = 24;

/// 다음 UTC 정각이에요(`X-RateLimit-Reset`이 없을 때 쓰는 풀리는 때예요).
fn next_hour(now: DateTime<Utc>) -> DateTime<Utc> {
    let top = Utc
        .with_ymd_and_hms(now.year(), now.month(), now.day(), now.hour(), 0, 0)
        .single()
        .unwrap_or(now);
    top + chrono::Duration::hours(1)
}

/// 한 번 부른 결과예요.
struct Reply {
    status: u16,
    body: Vec<u8>,
    headers: HeaderMap,
}

/// 진짜 API예요: `{base}{path}`, `Authorization: Bearer {지금 토큰}`.
pub struct AtlassianHttp {
    base: String,
    token: TokenFn,
    client: Result<Client, String>,
    tries: u32,
    /// 백오프의 첫 쉼이에요(테스트는 짧게 줘요).
    unit: Duration,
    /// `(email, site_url)`: Basic auth against the site instead of Bearer through the gateway.
    basic: Option<(String, String)>,
}

impl fmt::Debug for AtlassianHttp {
    /// 주소만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AtlassianHttp")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl AtlassianHttp {
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
        AtlassianHttp {
            base: base.to_string(),
            token,
            client: builder.build().map_err(net_error),
            tries: tries.max(1),
            unit: Duration::from_secs(1),
            basic: None,
        }
    }

    /// Sends `Basic base64(email:token)` to the site itself (`basic_url`) instead of `Bearer` to the gateway.
    pub fn basic(mut self, email: &str, site_url: &str) -> Self {
        self.basic = Some((email.to_string(), site_url.to_string()));
        self
    }

    /// The URL a gateway path goes to: the gateway, or the site with an API token.
    fn url(&self, path: &str) -> Result<String, SourceError> {
        match &self.basic {
            None => Ok(format!("{}{}", self.base, path)),
            Some((_, site)) => basic_url(site, path).ok_or_else(|| {
                // Down, not Key: Key would mark a working pasted connection refused
                SourceError::service(Kind::Down, "atlassian: not available with an API token")
            }),
        }
    }

    /// 백오프의 첫 쉼을 바꿔요(테스트).
    pub fn backoff_unit(mut self, unit: Duration) -> Self {
        self.unit = unit;
        self
    }

    /// 요청 하나를 보내고, 순간 한도·5xx는 쉬며 다시 해요. 2xx와 그 밖의 4xx는 그대로 돌려줘요.
    fn run(&self, build: impl Fn(&Client) -> RequestBuilder) -> Result<Reply, SourceError> {
        let fail = |kind: Kind, m: &str| SourceError::service(kind, format!("atlassian: {m}"));
        let client = self.client.as_ref().map_err(|e| fail(Kind::Down, e))?;
        let mut wait = self.unit;
        let mut pause = Duration::ZERO;
        let mut last = Kind::Busy;
        for k in 0..self.tries {
            if k > 0 {
                std::thread::sleep(pause);
            }
            let token = (self.token)().ok_or_else(|| fail(Kind::Key, "not connected"))?;
            let value = match &self.basic {
                None => format!("Bearer {token}"),
                Some((email, _)) => {
                    use base64::Engine as _;
                    let pair = format!("{email}:{token}");
                    format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD.encode(pair)
                    )
                }
            };
            let mut auth = HeaderValue::from_str(&value)
                .map_err(|_| fail(Kind::Key, "the token cannot be sent in a header"))?;
            auth.set_sensitive(true);
            let r = build(client)
                .header(AUTHORIZATION, auth)
                .send()
                .map_err(|e| fail(Kind::Network, &net_error(e)))?;
            let status = r.status().as_u16();
            let headers = r.headers().clone();
            let body = r
                .bytes()
                .map_err(|e| fail(Kind::Network, &net_error(e)))?
                .to_vec();
            if (200..300).contains(&status) {
                return Ok(Reply {
                    status,
                    body,
                    headers,
                });
            }
            if status == 401 {
                return Err(fail(Kind::Key, "the connection was refused"));
            }
            // 쉬는 시간은 백오프를 두 배씩 늘리되, 서버가 `Retry-After`를 주면 그만큼이에요.
            pause = wait;
            wait = (wait * 2).min(MAX_WAIT);
            if status == 429 {
                // 이유에 `quota`가 있을 때만 시간당 한도로 봐요. 순간 한도나 모르는 이유는 쉬었다 다시 물어요.
                let reason = header(&headers, "ratelimit-reason").to_lowercase();
                if reason.contains("quota") {
                    let now = Utc::now();
                    let until =
                        DateTime::parse_from_rfc3339(&header(&headers, "x-ratelimit-reset"))
                            .map(|d| d.with_timezone(&Utc))
                            .ok()
                            // 지난 시각이거나 없으면 다음 UTC 정각이에요.
                            .filter(|d| *d > now)
                            .unwrap_or_else(|| next_hour(now))
                            // 이상하게 먼 값에 오래 멈추지 않게 하루로 잘라요.
                            .min(now + chrono::Duration::hours(MAX_PAUSE_HOURS));
                    return Err(SourceError::service(
                        Kind::Busy,
                        format!(
                            "{QUOTA_PREFIX}{}",
                            until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                        ),
                    ));
                }
                if let Ok(s) = header(&headers, "retry-after").parse::<u64>() {
                    pause = Duration::from_secs(s).min(MAX_WAIT);
                }
                last = Kind::Busy;
                continue;
            }
            if status >= 500 {
                last = Kind::Down;
                continue;
            }
            return Ok(Reply {
                status,
                body,
                headers,
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

fn header(h: &HeaderMap, name: &str) -> String {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string()
}

impl AtlassianApi for AtlassianHttp {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Got, SourceError> {
        let mut url = self.url(path)?;
        if !query.is_empty() {
            let q = form_urlencoded::Serializer::new(String::new())
                .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())))
                .finish();
            url.push('?');
            url.push_str(&q);
        }
        let r = self.run(|c| c.get(&url))?;
        if (200..300).contains(&r.status) {
            Ok(Got::Body(r.body))
        } else {
            // 오류 몸통에는 제목이나 내용이 섞일 수 있어서 이유는 비워 둬요.
            Ok(Got::Fail {
                status: r.status,
                reason: String::new(),
            })
        }
    }

    fn post(&self, path: &str, body: &Value) -> Result<Answer, SourceError> {
        let url = self.url(path)?;
        let r = self.run(|c| {
            c.post(&url)
                .header(CONTENT_TYPE, "application/json")
                .body(body.to_string())
        })?;
        Ok(Answer {
            status: r.status,
            body: r.body,
            cycle_period: header(&r.headers, "cycle-period").parse().ok(),
        })
    }

    fn is_basic(&self) -> bool {
        self.basic.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::github::{FakeGitHub, Reply as Fake};
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn http(code: u16, headers: &[(&str, &str)], body: Value) -> Fake {
        Fake::Http {
            code,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.to_string().into_bytes(),
        }
    }

    fn fake(
        token: TokenFn,
        f: impl Fn(&str) -> Fake + Send + Sync + 'static,
    ) -> (Arc<FakeGitHub>, AtlassianHttp) {
        let g = FakeGitHub::start();
        g.status_with(move |p, _| f(p));
        let h = AtlassianHttp::new(&g.url(), token).backoff_unit(Duration::from_millis(1));
        (g, h)
    }

    #[test]
    fn basic_url_maps_gateway_paths_to_the_site() {
        let s = "https://myteam.atlassian.net";
        assert_eq!(
            basic_url(s, "ex/jira/abc/rest/api/3/myself").as_deref(),
            Some("https://myteam.atlassian.net/rest/api/3/myself")
        );
        assert_eq!(
            basic_url(s, "ex/confluence/abc/wiki/api/v2/spaces").as_deref(),
            Some("https://myteam.atlassian.net/wiki/api/v2/spaces")
        );
        assert_eq!(
            basic_url("https://myteam.atlassian.net/", "ex/jira/abc/rest/x").as_deref(),
            Some("https://myteam.atlassian.net/rest/x")
        );
        assert_eq!(basic_url(s, "oauth/token/accessible-resources"), None);
        assert_eq!(basic_url(s, "app/report-accounts/"), None);
    }

    #[test]
    fn basic_sends_email_and_token_to_the_site() {
        let (url, t) = serve(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        ]);
        let h = AtlassianHttp::new(API, fixed()).basic("a@b.c", &url);
        assert!(h.is_basic());
        h.get("ex/jira/c/rest/api/3/myself", &[]).unwrap();
        let seen = t.join().unwrap();
        assert!(
            seen[0].starts_with("get /rest/api/3/myself "),
            "{}",
            seen[0]
        );
        use base64::Engine as _;
        let want = base64::engine::general_purpose::STANDARD
            .encode(format!("a@b.c:{}", FakeGitHub::TOKEN))
            .to_lowercase();
        assert_eq!(auth_of(&seen[0]), format!("basic {want}"));
    }

    #[test]
    fn basic_refuses_oauth_only_paths_without_sending() {
        let h = AtlassianHttp::new(API, fixed()).basic("a@b.c", "http://127.0.0.1:9/");
        let e = h.get("oauth/token/accessible-resources", &[]).unwrap_err();
        // not Key: that would mark the pasted connection refused
        assert_eq!(e.kind(), Kind::Down);
    }

    fn fixed() -> TokenFn {
        Arc::new(|| Some(FakeGitHub::TOKEN.to_string()))
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// 요청을 차례로 받아 `replies`의 날 답(머리까지)을 하나씩 돌려주는 작은 서버예요. FakeGitHub는 머리를 안 남기고
    /// graphql 밖의 POST를 답하지 않아서 써요. 받은 요청(소문자 머리와 몸통)을 모아 돌려줘요.
    fn serve(replies: Vec<&'static str>) -> (String, std::thread::JoinHandle<Vec<String>>) {
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

    fn auth_of(req: &str) -> &str {
        req.lines()
            .find_map(|l| l.strip_prefix("authorization:"))
            .unwrap_or("")
            .trim()
    }

    #[test]
    fn reads_the_token_on_every_try() {
        let (url, t) = serve(vec![
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        ]);
        let reads = Arc::new(AtomicUsize::new(0));
        let r2 = reads.clone();
        let h = AtlassianHttp::new(
            &url,
            Arc::new(move || Some(format!("tok{}", r2.fetch_add(1, Ordering::SeqCst)))),
        )
        .backoff_unit(Duration::from_millis(1));
        h.get("x", &[]).unwrap();
        let seen = t.join().unwrap();
        // 첫 요청은 첫 토큰, 다시 한 둘째 요청은 새 토큰이에요.
        assert_eq!(auth_of(&seen[0]), "bearer tok0");
        assert_eq!(auth_of(&seen[1]), "bearer tok1");
    }

    /// 하루로 잘린 풀리는 때예요(`before`는 부르기 전 시각, 초 단위로 적혀요).
    fn assert_capped(until: DateTime<Utc>, before: DateTime<Utc>) {
        let day = chrono::Duration::hours(MAX_PAUSE_HOURS);
        assert!(
            until >= before + day - chrono::Duration::seconds(1),
            "{until}"
        );
        assert!(until <= Utc::now() + day, "{until}");
    }

    #[test]
    fn a_near_reset_is_kept() {
        let reset = (Utc::now() + chrono::Duration::minutes(30))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let (_g, h) = fake(fixed(), move |_| {
            http(
                429,
                &[
                    ("RateLimit-Reason", "jira-quota-global-based"),
                    ("X-RateLimit-Reset", reset.as_str()),
                ],
                json!({}),
            )
        });
        let until = quota_until(&h.get("x", &[]).unwrap_err()).unwrap();
        assert!(until < Utc::now() + chrono::Duration::hours(1));
    }

    #[test]
    fn quota_429_becomes_busy_with_its_reset() {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let (_g, h) = fake(fixed(), move |_| {
            n2.fetch_add(1, Ordering::SeqCst);
            http(
                429,
                &[
                    ("RateLimit-Reason", "jira-quota-global-based"),
                    ("X-RateLimit-Reset", "2099-11-12T06:00:00Z"),
                ],
                json!({}),
            )
        });
        let before = Utc::now();
        let e = h.get("x", &[]).unwrap_err();
        assert_eq!(e.kind(), Kind::Busy);
        // 하루보다 먼 풀리는 때는 지금부터 하루로 잘라요.
        assert_capped(quota_until(&e).unwrap(), before);
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn quota_until_ignores_other_errors() {
        let e = SourceError::service(Kind::Busy, "atlassian: rate limited");
        assert_eq!(quota_until(&e), None);
        assert_eq!(quota_until(&SourceError::service(Kind::Down, "x")), None);
    }

    #[test]
    fn burst_429_backs_off_and_retries() {
        let n = AtomicUsize::new(0);
        let (_g, h) = fake(fixed(), move |_| {
            if n.fetch_add(1, Ordering::SeqCst) == 0 {
                http(
                    429,
                    &[
                        ("RateLimit-Reason", "jira-burst-based"),
                        ("Retry-After", "0"),
                    ],
                    json!({}),
                )
            } else {
                Fake::Json(json!({"ok": true}))
            }
        });
        assert_eq!(
            h.get("x", &[]).unwrap(),
            Got::Body(b"{\"ok\":true}".to_vec())
        );
    }

    #[test]
    fn a_429_without_reason_that_never_ends_is_busy_without_a_reset() {
        let (_g, h) = fake(fixed(), |_| http(429, &[("Retry-After", "0")], json!({})));
        let e = h.get("x", &[]).unwrap_err();
        assert_eq!(e.kind(), Kind::Busy);
        assert_eq!(quota_until(&e), None);
    }

    #[test]
    fn a_quota_without_a_reset_waits_for_the_next_hour() {
        let (_g, h) = fake(fixed(), |_| {
            http(
                429,
                &[("RateLimit-Reason", "jira-quota-tenant-based")],
                json!({}),
            )
        });
        let until = quota_until(&h.get("x", &[]).unwrap_err()).unwrap();
        assert!(until > Utc::now());
        assert!(until <= Utc::now() + chrono::Duration::hours(1));
        assert_eq!(until.minute(), 0);
    }

    #[test]
    fn unauthorized_is_key_and_5xx_is_down() {
        let (_g, h) = fake(fixed(), |_| http(401, &[], json!({})));
        assert_eq!(h.get("x", &[]).unwrap_err().kind(), Kind::Key);
        let (_g, h) = fake(fixed(), |_| http(503, &[], json!({})));
        assert_eq!(h.get("x", &[]).unwrap_err().kind(), Kind::Down);
    }

    #[test]
    fn other_4xx_is_a_fail_value() {
        let (_g, h) = fake(fixed(), |_| {
            http(404, &[], json!({"message": "secret title"}))
        });
        assert_eq!(
            h.get("x", &[]).unwrap(),
            Got::Fail {
                status: 404,
                reason: String::new()
            }
        );
    }

    #[test]
    fn post_returns_status_and_cycle_period() {
        let (url, t) = serve(vec![
            "HTTP/1.1 204 No Content\r\nCycle-Period: 604800\r\nConnection: close\r\n\r\n",
        ]);
        let h = AtlassianHttp::new(&url, fixed());
        let a = h
            .post("app/report-accounts/", &json!({"accounts": []}))
            .unwrap();
        assert_eq!(
            a,
            Answer {
                status: 204,
                body: Vec::new(),
                cycle_period: Some(604_800)
            }
        );
        let req = t.join().unwrap().remove(0);
        assert!(req.starts_with("post /app/report-accounts/"), "{req}");
        assert!(req.contains("authorization: bearer"), "{req}");
        assert!(req.ends_with("{\"accounts\":[]}"), "{req}");
    }

    #[test]
    fn an_unknown_rate_limit_reason_backs_off_instead_of_pausing() {
        let n = AtomicUsize::new(0);
        let (_g, h) = fake(fixed(), move |_| {
            if n.fetch_add(1, Ordering::SeqCst) == 0 {
                http(
                    429,
                    &[("RateLimit-Reason", "Something-New"), ("Retry-After", "0")],
                    json!({}),
                )
            } else {
                Fake::Json(json!({"ok": true}))
            }
        });
        assert!(h.get("x", &[]).is_ok());
    }

    #[test]
    fn the_reason_is_matched_without_case() {
        let (_g, h) = fake(fixed(), |_| {
            http(
                429,
                &[
                    ("RateLimit-Reason", "Jira-QUOTA-Global-Based"),
                    ("X-RateLimit-Reset", "2099-01-01T00:00:00Z"),
                ],
                json!({}),
            )
        });
        let before = Utc::now();
        assert_capped(quota_until(&h.get("x", &[]).unwrap_err()).unwrap(), before);
    }

    #[test]
    fn a_reset_in_the_past_waits_for_the_next_hour() {
        let (_g, h) = fake(fixed(), |_| {
            http(
                429,
                &[
                    ("RateLimit-Reason", "jira-quota-global-based"),
                    ("X-RateLimit-Reset", "2020-01-01T00:00:00Z"),
                ],
                json!({}),
            )
        });
        let until = quota_until(&h.get("x", &[]).unwrap_err()).unwrap();
        assert!(until > Utc::now() && until <= Utc::now() + chrono::Duration::hours(1));
    }

    #[test]
    fn points_reset_each_hour() {
        let mut p = Points::default();
        p.charge(at("2026-11-12T05:59:00Z"), 10, 4_970, 5);
        assert_eq!(p.used, 4_990);
        assert_eq!(p.left(at("2026-11-12T05:59:30Z")), 10);
        assert_eq!(p.left(at("2099-11-12T06:00:00Z")), POINTS_PER_HOUR);
        p.charge(at("2099-11-12T06:00:00Z"), 1, 0, 0);
        assert_eq!(p.used, 1);
    }

    #[test]
    fn debug_hides_the_token() {
        let h = AtlassianHttp::new(API, Arc::new(|| Some("secret-token".into())));
        assert!(!format!("{h:?}").contains("secret"));
    }

    #[test]
    fn without_a_token_it_is_key() {
        let h = AtlassianHttp::new(API, Arc::new(|| None));
        assert_eq!(h.get("x", &[]).unwrap_err().kind(), Kind::Key);
    }
}
