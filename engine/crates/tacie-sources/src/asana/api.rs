//! Asana REST calls. `AsanaHttp` is the real API; tests plug in the fake from `testing::asana`.
//!
//! - Each request reads the current token through `TokenFn`. The engine never refreshes the token; the app does.
//! - One Mac calls at most 120 times a minute (`Pacer`). After 10 minutes without a 429 the limit doubles, up to 1,200;
//!   a 429 drops it back to 120. On a 429 it waits `Retry-After` seconds and retries the same request (however many
//!   times it waits, it does not end the sync).
//! - 401 is `Key`; 5xx that still fails after exponential backoff is `Down`. Other 4xx come back as a `Reply` so the
//!   caller reads the status (404 for a deleted item, 403 for a blocked workspace, 400 for an expired offset).
//! - Every response's `Date` header is read into `Reply.date` (the reference time for incremental fetches).
//!
//! The token lives only in the `Authorization` header (marked sensitive) and never shows in errors, logs or `Debug`.
//! Bodies are not put in error text either.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use crate::error::{Kind, SourceError};
use crate::gdrive::api::TokenFn;
use crate::net::net_error;

/// Asana API 주소예요. 테스트는 로컬 가짜를 가리켜요.
pub const API: &str = "https://app.asana.com/api/1.0/";
/// 호출 하나를 몇 번까지 하는지예요(5xx를 기다리며). 1, 2, 4, 8, 16, 32초를 쉬어요.
pub const TRIES: u32 = 7;
/// 요청 하나를 기다리는 시간이에요.
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// 백오프 한 번의 최대예요.
const MAX_WAIT: Duration = Duration::from_secs(64);
/// 429로 쉬는 시간의 최대예요(`Retry-After`가 이상하게 길어도 이만큼만 쉬어요).
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);
/// 한 요청이 429를 몇 번까지 받아 보는지예요(끝없이 돌지 않게 하는 안전 상한이에요).
const MAX_429: u32 = 30;
/// 1분에 보내는 요청의 시작 한도예요.
pub const START_CAP: usize = 120;
/// 한도가 올라갈 수 있는 끝이에요.
pub const MAX_CAP: usize = 1_200;
/// 429 없이 이만큼 지나면 한도를 두 배로 올려요.
pub const RAISE_AFTER: Duration = Duration::from_secs(600);
/// `/batch` 한 번에 담는 요청 수예요(Asana의 한도).
pub const BATCH_SIZE: usize = 10;

/// 시계예요. 속도 조절과 쉬기를 이 안에서 해서 테스트가 진짜로 기다리지 않게 해요.
pub trait Clock: Send + Sync {
    /// 시작 이후 지난 시간이에요(줄어들지 않아요).
    fn now(&self) -> Duration;
    fn sleep(&self, d: Duration);
}

/// 진짜 시계예요.
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        SystemClock {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// 요청 속도 조절이에요: 지난 1분에 보낸 요청이 한도만큼이면 가장 오래된 것이 1분이 될 때까지 기다려요.
struct Pacer {
    sent: VecDeque<Duration>,
    cap: usize,
    /// 마지막 429나 한도를 올린 때예요.
    since: Duration,
}

impl Pacer {
    fn new(now: Duration) -> Self {
        Pacer {
            sent: VecDeque::new(),
            cap: START_CAP,
            since: now,
        }
    }

    /// 요청 하나를 보내도 될 때까지 쉬고 한 번을 세어요.
    #[cfg(test)]
    fn acquire(&mut self, clock: &dyn Clock) {
        self.acquire_n(clock, 1);
    }

    /// 요청 `n`개(`/batch` 하나는 그 안의 GET 수만큼이에요, Asana가 GET마다 세요)를 보내도 될 때까지 쉬고 `n`번을
    /// 세어요.
    #[cfg(test)]
    fn acquire_n(&mut self, clock: &dyn Clock, n: usize) {
        self.acquire_until(clock, n, None);
    }

    /// `acquire_n`과 같지만 `deadline`(시계 기준)을 넘겨 쉬어야 하면 쉬지 않고 세지도 않고 거짓이에요.
    #[cfg(test)]
    fn acquire_until(&mut self, clock: &dyn Clock, n: usize, deadline: Option<Duration>) -> bool {
        loop {
            let now = clock.now();
            match self.take(now, n) {
                Ok(()) => return true,
                Err(wait) if deadline.is_some_and(|d| now + wait > d) => return false,
                Err(wait) => clock.sleep(wait),
            }
        }
    }

    /// 지금 요청 `n`개를 보내도 되면 세고 Ok예요. 아니면 세지 않고 더 기다릴 시간이에요. 쉬는 것은 부른 쪽이
    /// 자물쇠 밖에서 해요(같은 속도를 쓰는 다른 부르기가 그동안 막히지 않게요).
    fn take(&mut self, now: Duration, n: usize) -> Result<(), Duration> {
        let n = n.max(1);
        while now.saturating_sub(self.since) >= RAISE_AFTER && self.cap < MAX_CAP {
            self.cap = (self.cap * 2).min(MAX_CAP);
            self.since += RAISE_AFTER;
        }
        while self
            .sent
            .front()
            .is_some_and(|t| now.saturating_sub(*t) >= Duration::from_secs(60))
        {
            self.sent.pop_front();
        }
        if self.sent.len() + n <= self.cap.max(n) {
            self.sent.extend(std::iter::repeat_n(now, n));
            return Ok(());
        }
        let oldest = self.sent.len() + n - self.cap.max(n) - 1;
        let wait = Duration::from_secs(60).saturating_sub(now.saturating_sub(self.sent[oldest]));
        Err(wait.max(Duration::from_millis(1)))
    }

    /// 429를 받았어요: 한도가 처음으로 돌아가요.
    fn throttled(&mut self, now: Duration) {
        self.cap = START_CAP;
        self.since = now;
    }
}

/// 한 Mac의 요청 속도예요(시계와 1분 창). `AsanaSource`가 하나를 들고 받기와 30분 확인이 같이 써서, 둘을 합쳐
/// 1분 한도를 지키고 429 없이 10분이 지나면 한도가 올라가요.
pub struct Pace {
    clock: Arc<dyn Clock>,
    pacer: Mutex<Pacer>,
}

impl Pace {
    pub fn new(clock: Arc<dyn Clock>) -> Arc<Pace> {
        let now = clock.now();
        Arc::new(Pace {
            clock,
            pacer: Mutex::new(Pacer::new(now)),
        })
    }

    /// 진짜 시계의 속도예요.
    pub fn system() -> Arc<Pace> {
        Pace::new(Arc::new(SystemClock::new()))
    }

    /// 지금 1분 한도예요.
    pub fn cap(&self) -> usize {
        self.pacer.lock().map(|p| p.cap).unwrap_or(START_CAP)
    }

    /// 지난 1분 안에 센 요청 수예요(테스트).
    #[doc(hidden)]
    pub fn sent(&self) -> usize {
        let now = self.clock.now();
        self.pacer
            .lock()
            .map(|p| {
                p.sent
                    .iter()
                    .filter(|t| now.saturating_sub(**t) < Duration::from_secs(60))
                    .count()
            })
            .unwrap_or(0)
    }
}

impl fmt::Debug for Pace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pace").field("cap", &self.cap()).finish()
    }
}

/// Asana 답 하나예요. 4xx(401·429 빼고)도 오류가 아니라 이렇게 돌려줘요.
#[derive(Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    /// 답의 `Date` 머리예요. 없거나 못 읽으면 None이에요.
    pub date: Option<DateTime<Utc>>,
}

impl fmt::Debug for Reply {
    /// 상태, 몸통 길이, 시각만 보여요. 몸통은 Asana 내용이라 찍지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reply")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .field("date", &self.date)
            .finish()
    }
}

impl Reply {
    /// 몸통을 JSON으로 읽어요. 2xx가 아니거나 JSON이 아니면 `Down`이에요(상태는 글에 있어요).
    pub fn json(self, what: &str) -> Result<Value, SourceError> {
        if !(200..300).contains(&self.status) {
            return Err(SourceError::service(
                Kind::Down,
                format!("asana: {what} failed {}", self.status),
            ));
        }
        serde_json::from_slice(&self.body)
            .map_err(|_| SourceError::service(Kind::Down, format!("asana: {what} is not JSON")))
    }
}

/// Asana 호출이에요. `path`는 `base` 뒤 경로로, 예: `tasks/123/stories`(앞의 `/`는 있어도 없어도 돼요).
pub trait AsanaApi: Send + Sync {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply, SourceError>;
    /// `/batch`로 GET을 모아 보내요(10개씩 묶음). 답은 `gets`와 같은 순서로 하나씩이고, 일부가 404여도 오류가 아니라
    /// 그 자리의 `Reply`예요.
    fn batch(&self, gets: &[String]) -> Result<Vec<Reply>, SourceError>;
}

/// `/batch`의 GET 하나예요. `path?opt_fields=a,b`는 `relative_path`와 `options.fields`로 나누고, 다른 쿼리는 `Down`이에요.
pub fn batch_action(path: &str) -> Result<Value, SourceError> {
    let (p, q) = path.split_once('?').unwrap_or((path, ""));
    let mut action =
        json!({"method": "get", "relative_path": format!("/{}", p.trim_start_matches('/'))});
    if q.is_empty() {
        return Ok(action);
    }
    let mut fields = None;
    for (k, v) in form_urlencoded::parse(q.as_bytes()) {
        if k != "opt_fields" || fields.is_some() {
            return Err(SourceError::service(
                Kind::Down,
                "asana: batch path with unsupported query",
            ));
        }
        fields = Some(
            v.split(',')
                .filter(|f| !f.is_empty())
                .map(|f| Value::String(f.to_string()))
                .collect::<Vec<_>>(),
        );
    }
    if let Some(f) = fields {
        action["options"] = json!({"fields": f});
    }
    Ok(action)
}

/// 진짜 API예요: `{base}{path}`, `Authorization: Bearer {지금 토큰}`.
pub struct AsanaHttp {
    base: String,
    token: TokenFn,
    client: Result<Client, String>,
    tries: u32,
    /// 백오프의 첫 쉼이에요(테스트는 짧게 줘요).
    unit: Duration,
    pace: Arc<Pace>,
    /// 이 시각(시계 기준)을 넘겨 쉬어야 하면 쉬지 않고 `Busy`예요(30분 확인).
    deadline: Option<Duration>,
}

impl fmt::Debug for AsanaHttp {
    /// 주소만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsanaHttp")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl AsanaHttp {
    pub fn new(base: &str, token: TokenFn) -> Self {
        Self::with_tries(base, token, TRIES, TIMEOUT)
    }

    /// 빨리 끝나야 할 때예요: `tries`번, 요청마다 `timeout`.
    pub fn with_tries(base: &str, token: TokenFn, tries: u32, timeout: Duration) -> Self {
        let builder = Client::builder().timeout(timeout);
        let builder = if crate::is_loopback(base) {
            builder.no_proxy()
        } else {
            builder
        };
        let mut base = base.to_string();
        if !base.ends_with('/') {
            base.push('/');
        }
        AsanaHttp {
            base,
            token,
            client: builder.build().map_err(net_error),
            tries: tries.max(1),
            unit: Duration::from_secs(1),
            pace: Pace::system(),
            deadline: None,
        }
    }

    /// 백오프의 첫 쉼을 바꿔요(테스트).
    pub fn backoff_unit(mut self, unit: Duration) -> Self {
        self.unit = unit;
        self
    }

    /// 시계를 바꿔요(테스트). 속도 조절의 기준도 이 시계로 다시 시작해요.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.pace = Pace::new(clock);
        self
    }

    /// 다른 부르기와 같은 속도(1분 창, 한도)를 써요.
    pub fn with_pace(mut self, pace: Arc<Pace>) -> Self {
        self.pace = pace;
        self
    }

    /// 지금부터 `limit`이 지나도록 쉬어야 하면(1분 한도, 429, 5xx) 쉬지 않고 `Busy`로 돌아가요.
    pub fn give_up_after(mut self, limit: Duration) -> Self {
        self.deadline = Some(self.pace.clock.now() + limit);
        self
    }

    /// 지금 1분 한도예요(테스트).
    pub fn cap(&self) -> usize {
        self.pace.cap()
    }

    /// `d`를 쉬어요. 끝 시각을 넘기면 쉬지 않고 `Busy`예요.
    fn rest(&self, d: Duration) -> Result<(), SourceError> {
        if self
            .deadline
            .is_some_and(|end| self.pace.clock.now() + d > end)
        {
            return Err(out_of_time());
        }
        self.pace.clock.sleep(d);
        Ok(())
    }

    /// 요청 하나를 속도에 맞춰 보내고, 429와 5xx는 쉬며 다시 해요. 2xx와 그 밖의 4xx는 그대로 돌려줘요.
    /// `weight`는 속도 조절에서 세는 요청 수예요(`/batch`는 그 안의 GET 수).
    fn run(
        &self,
        weight: usize,
        build: impl Fn(&Client) -> RequestBuilder,
    ) -> Result<Reply, SourceError> {
        let fail = |kind: Kind, m: &str| SourceError::service(kind, format!("asana: {m}"));
        let client = self.client.as_ref().map_err(|e| fail(Kind::Down, e))?;
        let mut wait = self.unit;
        let mut pause = Duration::ZERO;
        let mut failures = 0u32;
        let mut throttles = 0u32;
        loop {
            if !pause.is_zero() {
                self.rest(pause)?;
            }
            let token = (self.token)().ok_or_else(|| fail(Kind::Key, "not connected"))?;
            let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| fail(Kind::Key, "the token cannot be sent in a header"))?;
            auth.set_sensitive(true);
            // 자물쇠는 셀 때만 잡고, 쉬는 동안은 놓아요(30분 확인이 받기의 쉼 뒤에 줄 서지 않게요)
            loop {
                let now = self.pace.clock.now();
                let got = match self.pace.pacer.lock() {
                    Ok(mut p) => p.take(now, weight),
                    Err(_) => Ok(()),
                };
                match got {
                    Ok(()) => break,
                    Err(wait) => self.rest(wait)?,
                }
            }
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
            if status == 401 {
                return Err(fail(Kind::Key, "the connection was refused"));
            }
            if status == 429 {
                if let Ok(mut p) = self.pace.pacer.lock() {
                    p.throttled(self.pace.clock.now());
                }
                throttles += 1;
                // 부른 쪽이 지금까지를 저장하고 `more`로 돌아가요(다음 실행이 이어 받아요).
                if throttles >= MAX_429 {
                    return Err(fail(Kind::Busy, "rate limited"));
                }
                pause = header(&headers, "retry-after")
                    .parse::<u64>()
                    .map(Duration::from_secs)
                    .unwrap_or(Duration::from_secs(60))
                    .min(MAX_RETRY_AFTER);
                continue;
            }
            if status >= 500 {
                failures += 1;
                if failures >= self.tries {
                    return Err(fail(Kind::Down, "the server kept failing"));
                }
                pause = wait;
                wait = (wait * 2).min(MAX_WAIT);
                continue;
            }
            let date = DateTime::parse_from_rfc2822(&header(&headers, "date"))
                .ok()
                .map(|d| d.with_timezone(&Utc));
            return Ok(Reply { status, body, date });
        }
    }
}

/// 정한 시간 안에 보낼 수 없어요(30분 확인). 다음 확인이 이어요.
fn out_of_time() -> SourceError {
    SourceError::service(Kind::Busy, "asana: out of time")
}

fn header(h: &HeaderMap, name: &str) -> String {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string()
}

impl AsanaApi for AsanaHttp {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply, SourceError> {
        let mut url = format!("{}{}", self.base, path.trim_start_matches('/'));
        if !query.is_empty() {
            let q = form_urlencoded::Serializer::new(String::new())
                .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())))
                .finish();
            url.push('?');
            url.push_str(&q);
        }
        self.run(1, |c| c.get(&url))
    }

    fn batch(&self, gets: &[String]) -> Result<Vec<Reply>, SourceError> {
        let url = format!("{}batch", self.base);
        let mut out = Vec::with_capacity(gets.len());
        for group in gets.chunks(BATCH_SIZE) {
            let actions = group
                .iter()
                .map(|p| batch_action(p))
                .collect::<Result<Vec<Value>, SourceError>>()?;
            let body = json!({"data": {"actions": actions}}).to_string();
            let r = self.run(group.len(), |c| {
                c.post(&url)
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.clone())
            })?;
            let (date, status) = (r.date, r.status);
            let v = r.json("batch")?;
            let items = v["data"].as_array().filter(|a| a.len() == group.len());
            let Some(items) = items else {
                return Err(SourceError::service(
                    Kind::Down,
                    format!("asana: batch answered the wrong number of replies ({status})"),
                ));
            };
            for item in items {
                out.push(Reply {
                    status: item["status_code"].as_u64().unwrap_or(0) as u16,
                    body: serde_json::to_vec(&item["body"]).unwrap_or_default(),
                    date,
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 시간이 저절로 가지 않고 `sleep`만큼 가는 시계예요.
    struct FakeClock {
        now: Mutex<Duration>,
        slept: Mutex<Vec<Duration>>,
    }

    impl FakeClock {
        fn new() -> Arc<Self> {
            Arc::new(FakeClock {
                now: Mutex::new(Duration::ZERO),
                slept: Mutex::default(),
            })
        }
        fn advance(&self, d: Duration) {
            *self.now.lock().unwrap() += d;
        }
        fn slept(&self) -> Vec<Duration> {
            self.slept.lock().unwrap().clone()
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            *self.now.lock().unwrap()
        }
        fn sleep(&self, d: Duration) {
            self.slept.lock().unwrap().push(d);
            self.advance(d);
        }
    }

    fn fixed() -> TokenFn {
        Arc::new(|| Some("tok".to_string()))
    }

    fn http(url: &str, clock: &Arc<FakeClock>) -> AsanaHttp {
        AsanaHttp::new(url, fixed())
            .backoff_unit(Duration::from_secs(1))
            .with_clock(clock.clone())
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
    fn a_401_is_a_key_error() {
        let (url, t) = serve(vec![raw("401 Unauthorized", "", "{}")]);
        let c = FakeClock::new();
        let e = http(&url, &c).get("users/me", &[]).unwrap_err();
        assert_eq!(e.kind(), Kind::Key);
        t.join().unwrap();
    }

    #[test]
    fn a_429_sleeps_retry_after_and_retries_the_same_request() {
        let (url, t) = serve(vec![
            raw("429 Too Many Requests", "Retry-After: 2\r\n", ""),
            ok("{\"data\":1}"),
        ]);
        let c = FakeClock::new();
        let r = http(&url, &c)
            .get("tasks", &[("project", "9".to_string())])
            .unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(c.slept(), vec![Duration::from_secs(2)]);
        let seen = t.join().unwrap();
        let line = |s: &str| s.lines().next().unwrap().to_string();
        assert_eq!(line(&seen[0]), line(&seen[1]));
        assert!(line(&seen[0]).starts_with("get /tasks?project=9 "));
    }

    #[test]
    fn a_429_returns_the_cap_to_the_start() {
        let (url, t) = serve(vec![
            ok("{}"),
            raw("429 Too Many Requests", "Retry-After: 1\r\n", ""),
            ok("{}"),
        ]);
        let c = FakeClock::new();
        let h = http(&url, &c);
        c.advance(RAISE_AFTER);
        h.get("a", &[]).unwrap();
        assert_eq!(h.cap(), 240);
        h.get("b", &[]).unwrap();
        assert_eq!(h.cap(), START_CAP);
        t.join().unwrap();
    }

    #[test]
    fn a_batch_counts_each_get_against_the_cap() {
        let c = FakeClock::new();
        let mut p = Pacer::new(Duration::ZERO);
        for _ in 0..11 {
            p.acquire_n(c.as_ref(), 10);
        }
        assert_eq!(p.sent.len(), 110);
        assert!(c.slept().is_empty());
        c.advance(Duration::from_secs(1));
        // 120번 한도에 10번이 더 들어가지 않아요: 첫 10번이 1분이 될 때까지 쉬어요.
        p.acquire_n(c.as_ref(), 10);
        assert!(c.slept().is_empty());
        p.acquire_n(c.as_ref(), 10);
        assert_eq!(c.slept(), vec![Duration::from_secs(59)]);
        assert_eq!(p.sent.len(), 20);
    }

    #[test]
    fn the_cap_doubles_every_ten_quiet_minutes_up_to_1200() {
        let mut p = Pacer::new(Duration::ZERO);
        let c = FakeClock::new();
        for expect in [240, 480, 960, 1_200, 1_200] {
            c.advance(RAISE_AFTER);
            p.acquire(c.as_ref());
            assert_eq!(p.cap, expect);
        }
    }

    #[test]
    fn a_503_is_down_after_the_tries() {
        let (url, t) = serve(vec![raw("503 Service Unavailable", "", ""); 3]);
        let c = FakeClock::new();
        let h = AsanaHttp::with_tries(&url, fixed(), 3, TIMEOUT)
            .backoff_unit(Duration::from_secs(1))
            .with_clock(c.clone());
        let e = h.get("tasks/1", &[]).unwrap_err();
        assert_eq!(e.kind(), Kind::Down);
        assert_eq!(
            c.slept(),
            vec![Duration::from_secs(1), Duration::from_secs(2)]
        );
        t.join().unwrap();
    }

    #[test]
    fn other_4xx_come_back_as_replies() {
        let (url, t) = serve(vec![raw(
            "404 Not Found",
            "",
            "{\"errors\":[{\"message\":\"Not a recognized ID\"}]}",
        )]);
        let c = FakeClock::new();
        let r = http(&url, &c).get("tasks/1", &[]).unwrap();
        assert_eq!(r.status, 404);
        let e = r.json("task").unwrap_err();
        assert_eq!(e.kind(), Kind::Down);
        assert!(e.detail().contains("404"));
        t.join().unwrap();
    }

    #[test]
    fn the_date_header_is_read() {
        let (url, t) = serve(vec![raw(
            "200 OK",
            "Date: Thu, 12 Nov 2026 10:20:30 GMT\r\n",
            "{}",
        )]);
        let c = FakeClock::new();
        let r = http(&url, &c).get("users/me", &[]).unwrap();
        assert_eq!(
            r.date,
            Some(
                DateTime::parse_from_rfc3339("2026-11-12T10:20:30Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
        t.join().unwrap();
    }

    #[test]
    fn the_121st_request_in_a_minute_waits() {
        let (url, t) = serve(vec![ok("{}"); 121]);
        let c = FakeClock::new();
        let h = http(&url, &c);
        for _ in 0..120 {
            h.get("x", &[]).unwrap();
        }
        assert!(c.slept().is_empty());
        h.get("x", &[]).unwrap();
        assert_eq!(c.slept(), vec![Duration::from_secs(60)]);
        t.join().unwrap();
    }

    #[test]
    fn the_token_is_read_on_every_try_and_never_shown() {
        let (url, t) = serve(vec![
            raw("429 Too Many Requests", "Retry-After: 0\r\n", ""),
            ok("{}"),
        ]);
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let c = FakeClock::new();
        let h = AsanaHttp::new(
            &url,
            Arc::new(move || Some(format!("tok{}", n2.fetch_add(1, Ordering::SeqCst)))),
        )
        .with_clock(c);
        h.get("x", &[]).unwrap();
        let seen = t.join().unwrap();
        let auth = |s: &str| {
            s.lines()
                .find_map(|l| l.strip_prefix("authorization:"))
                .unwrap()
                .trim()
                .to_string()
        };
        assert_eq!(auth(&seen[0]), "bearer tok0");
        assert_eq!(auth(&seen[1]), "bearer tok1");
        assert!(!format!("{h:?}").contains("tok"));
    }

    #[test]
    fn a_connection_failure_is_a_network_error() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", l.local_addr().unwrap());
        drop(l);
        let c = FakeClock::new();
        let e = http(&url, &c).get("users/me", &[]).unwrap_err();
        assert_eq!(e.kind(), Kind::Network);
    }

    #[test]
    fn a_503_then_a_200_succeeds_after_one_backoff() {
        let (url, t) = serve(vec![raw("503 Service Unavailable", "", ""), ok("{}")]);
        let c = FakeClock::new();
        let r = http(&url, &c).get("users/me", &[]).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(c.slept(), vec![Duration::from_secs(1)]);
        t.join().unwrap();
    }

    #[test]
    fn a_batch_with_the_wrong_reply_count_is_down() {
        let (url, t) = serve(vec![ok("{\"data\":[]}")]);
        let c = FakeClock::new();
        let e = http(&url, &c).batch(&["/tasks/1".to_string()]).unwrap_err();
        assert_eq!(e.kind(), Kind::Down);
        t.join().unwrap();
    }

    #[test]
    fn batch_puts_opt_fields_in_options_and_refuses_other_queries() {
        let (url, t) = serve(vec![ok(
            "{\"data\":[{\"status_code\":200,\"body\":{},\"headers\":{}}]}",
        )]);
        let c = FakeClock::new();
        let h = http(&url, &c);
        h.batch(&["/tasks/1?opt_fields=modified_at,memberships.project".to_string()])
            .unwrap();
        let seen = t.join().unwrap();
        let body: Value = serde_json::from_str(seen[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"data": {"actions": [{"method": "get", "relative_path": "/tasks/1",
                "options": {"fields": ["modified_at", "memberships.project"]}}]}})
        );
        let e = h.batch(&["/tasks?limit=5".to_string()]).unwrap_err();
        assert_eq!(e.kind(), Kind::Down);
        assert!(e.detail().contains("unsupported query"));
    }

    #[test]
    fn a_reply_debug_hides_the_body() {
        let r = Reply {
            status: 200,
            body: b"secret task name".to_vec(),
            date: None,
        };
        let d = format!("{r:?}");
        assert!(!d.contains("secret") && d.contains("body_len"));
    }

    #[test]
    fn batch_of_23_makes_3_posts_in_order_and_a_404_is_a_reply() {
        let reply = |ids: std::ops::Range<u32>| {
            let items: Vec<Value> = ids
                .map(|i| {
                    if i == 12 {
                        json!({"status_code": 404, "body": {"errors": [{"message": "Not a recognized ID"}]}, "headers": {}})
                    } else {
                        json!({"status_code": 200, "body": {"data": {"gid": i.to_string()}}, "headers": {}})
                    }
                })
                .collect();
            ok(&json!({"data": items}).to_string())
        };
        let (url, t) = serve(vec![reply(0..10), reply(10..20), reply(20..23)]);
        let c = FakeClock::new();
        let paths: Vec<String> = (0..23).map(|i| format!("/tasks/{i}")).collect();
        let rs = http(&url, &c).batch(&paths).unwrap();
        assert_eq!(rs.len(), 23);
        for (i, r) in rs.iter().enumerate() {
            if i == 12 {
                assert_eq!(r.status, 404);
            } else {
                assert_eq!(r.status, 200);
                let v: Value = serde_json::from_slice(&r.body).unwrap();
                assert_eq!(v["data"]["gid"], i.to_string());
            }
        }
        let seen = t.join().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen.iter().all(|s| s.starts_with("post /batch ")));
        assert!(seen[0].contains("\"relative_path\":\"/tasks/0\""));
        assert!(seen[2].contains("\"relative_path\":\"/tasks/22\""));
    }

    /// 두 부르기가 한 속도를 쓰면 1분 창을 같이 세요: 앞이 120번을 쓰면 뒤의 첫 요청이 기다려요.
    #[test]
    fn two_calls_on_one_pace_share_the_minute() {
        let (url, t) = serve(vec![ok("{}"); 121]);
        let c = FakeClock::new();
        let pace = Pace::new(c.clone());
        let first = AsanaHttp::new(&url, fixed()).with_pace(pace.clone());
        for _ in 0..120 {
            first.get("x", &[]).unwrap();
        }
        assert_eq!(pace.sent(), 120);
        let second = AsanaHttp::with_tries(&url, fixed(), 1, TIMEOUT).with_pace(pace.clone());
        assert!(c.slept().is_empty());
        second.get("x", &[]).unwrap();
        assert_eq!(c.slept(), vec![Duration::from_secs(60)]);
        t.join().unwrap();
    }

    /// 한도가 10분 동안 429 없이 지나면 오르는 것도 부르기를 새로 만들어도 이어져요.
    #[test]
    fn the_raise_carries_over_to_a_new_call_on_the_same_pace() {
        let (url, t) = serve(vec![ok("{}"); 2]);
        let c = FakeClock::new();
        let pace = Pace::new(c.clone());
        AsanaHttp::new(&url, fixed())
            .with_pace(pace.clone())
            .get("x", &[])
            .unwrap();
        c.advance(RAISE_AFTER);
        let later = AsanaHttp::new(&url, fixed()).with_pace(pace.clone());
        later.get("x", &[]).unwrap();
        assert_eq!(later.cap(), 2 * START_CAP);
        t.join().unwrap();
    }

    /// 끝 시각이 있으면 1분 한도나 429로 쉬어야 할 때 쉬지 않고 `Busy`예요.
    #[test]
    fn a_deadline_gives_up_instead_of_waiting() {
        let (url, t) = serve(vec![ok("{}"); 120]);
        let c = FakeClock::new();
        let pace = Pace::new(c.clone());
        let fetch = AsanaHttp::new(&url, fixed()).with_pace(pace.clone());
        for _ in 0..120 {
            fetch.get("x", &[]).unwrap();
        }
        t.join().unwrap();
        let check = AsanaHttp::with_tries("http://127.0.0.1:9/", fixed(), 1, TIMEOUT)
            .with_pace(pace.clone())
            .give_up_after(Duration::from_secs(20));
        let e = check.get("x", &[]).unwrap_err();
        assert_eq!(e.kind(), Kind::Busy);
        assert!(c.slept().is_empty());
        assert_eq!(pace.sent(), 120);

        let (url, t) = serve(vec![raw("429 Too Many Requests", "Retry-After: 30\r\n", "")]);
        let c = FakeClock::new();
        let check = AsanaHttp::with_tries(&url, fixed(), 1, TIMEOUT)
            .with_clock(c.clone())
            .give_up_after(Duration::from_secs(20));
        assert_eq!(check.get("x", &[]).unwrap_err().kind(), Kind::Busy);
        assert!(c.slept().is_empty());
        t.join().unwrap();
    }

    /// `AsanaSource`는 30분 확인을 몇 번 해도 같은 속도를 써요(받기도 같은 것을 써요).
    #[test]
    fn the_source_keeps_one_pace_across_checks() {
        use crate::asana::state::{Proj, State, Ws, iso, save_state};
        let (url, t) = serve(vec![ok("{\"data\":[]}"); 2]);
        let src = crate::asana::AsanaSource::new(&url);
        src.set(Some("tok".into()), Some("me-1".into()));
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let mut st = State {
            user_gid: Some("me-1".into()),
            initial_done: true,
            reconciled_at: Some(iso(now)),
            workspaces: vec![Ws {
                gid: "w1".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        st.projects.insert(
            "p1".into(),
            Proj {
                ws: "w1".into(),
                cursor: Some(iso(now)),
                ..Default::default()
            },
        );
        save_state(dir.path(), &st).unwrap();
        let before = src.pace().sent();
        for _ in 0..2 {
            assert!(!src.changed(dir.path(), Duration::from_secs(5)).unwrap().unwrap());
        }
        assert_eq!(src.pace().sent(), before + 2);
        t.join().unwrap();
    }
}
