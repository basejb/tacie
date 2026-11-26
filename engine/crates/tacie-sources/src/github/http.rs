//! GitHub REST client (spec section 4): one request (`get`, `post_json`), paging (`pages`), the error table
//! (`kind_of`), and URL building (`with_query`, `segment`). The token is passed in at call time and carried only in the
//! `Authorization` header (`set_sensitive`). URLs, the token and GitHub's `message` are never put in error text (fixed
//! text, spec 4-4).
//!
//! Building a `blocking::Client` on a tokio runtime thread panics (debug). So `Http::new` only reads the URL, and the
//! client is built on the first request (on the refresh thread or in `spawn_blocking`). Dropping it on a runtime
//! thread is fine (it only waits for the thread).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::Url;
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use super::rate_limited;
use crate::error::{Kind, SourceError};
use crate::net::{is_loopback, net_error};

/// GitHub REST 기본 주소예요. 테스트와 계약은 `TACIE_GITHUB_API_URL`로 loopback 가짜를 가리켜요.
pub const API: &str = "https://api.github.com/";
/// 모든 요청의 `User-Agent`예요. 없으면 GitHub가 403이에요(부록 A-5).
pub const USER_AGENT: &str = concat!("Tacie/", env!("CARGO_PKG_VERSION"));
/// `X-GitHub-Api-Version`이에요(gh가 보내던 값).
pub const API_VERSION: &str = "2022-11-28";
/// 연결을 맺는 시간이에요.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 목록 한 쪽, 문서, GraphQL, trees, blobs 요청 하나의 시간이에요.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// tarball 하나를 받는 시간이에요.
pub const TARBALL_TIMEOUT: Duration = Duration::from_secs(300);

/// 쪽 크기예요(gh `--paginate`처럼 첫 주소에만 붙여요. 다음 쪽 주소에는 GitHub가 넣어 줘요).
const PER_PAGE: &str = "100";
/// 경로 조각 하나에서 그대로 두는 글자예요: 영숫자와 `-._~`(RFC 3986 unreserved). `/`도 인코딩해요.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
const UNREACHABLE: &str = "GitHub could not be reached";
/// 401의 글이에요. gh 모드는 이 오류를 보고 빌린 토큰을 잊어요.
pub const BAD_CREDENTIALS: &str = "GitHub answered HTTP 401 (bad credentials)";

/// 받을 몸통의 모양이에요(`Accept` 머리).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accept {
    /// JSON 답이에요. gh가 보내던 `*/*`예요.
    Json,
    /// blob의 날 바이트예요(`application/vnd.github.raw+json`).
    Raw,
    /// tarball이에요(`*/*`, 리다이렉트 뒤 codeload가 gzip을 줘요).
    Tarball,
}

impl Accept {
    fn header(self) -> &'static str {
        match self {
            Accept::Json | Accept::Tarball => "*/*",
            Accept::Raw => "application/vnd.github.raw+json",
        }
    }
}

/// 어디로 물을지예요.
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    /// 기본 주소 뒤의 경로예요(쿼리를 붙일 수 있어요, 앞 `/`는 떼요).
    Path(&'a str),
    /// 앞 답의 `Link: rel="next"` 주소예요. 기본 주소와 scheme·host·port가 같을 때만 따라가요.
    Next(&'a Url),
}

/// 답 하나예요. 머리 이름은 소문자이고 같은 이름이 여럿이면 `, `로 이어요. `next`는 `Link`의 `rel="next"`예요
/// (origin은 아직 보지 않았어요. 따라갈 때 `Target::Next`가 봐요). `Debug`에는 주소를 넣지 않아요.
#[derive(Clone)]
pub struct Answer {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub next: Option<Url>,
}

impl fmt::Debug for Answer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Answer")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .field("has_next", &self.next.is_some())
            .finish_non_exhaustive()
    }
}

/// GitHub REST에 닿는 클라이언트예요. 기본 주소 하나와 reqwest 클라이언트 하나를 가져요.
pub struct Http {
    base: Url,
    /// 처음 요청할 때 만들어요(모듈 설명).
    client: OnceLock<Client>,
}

impl fmt::Debug for Http {
    /// 기본 주소만 보여요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Http")
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

impl Http {
    /// `base`는 끝이 `/`가 아니면 붙여요. loopback 주소(테스트·계약의 가짜)는 프록시를 쓰지 않고, 그 밖은
    /// reqwest `system-proxy`(환경 변수와 macOS 설정)를 따라요.
    pub fn new(base: &str) -> Result<Http, SourceError> {
        let mut text = base.to_string();
        if !text.ends_with('/') {
            text.push('/');
        }
        let bad =
            || SourceError::service(Kind::Down, "the GitHub API address is not a web address");
        let url = Url::parse(&text).map_err(|_| bad())?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(bad());
        }
        Ok(Http {
            base: url,
            client: OnceLock::new(),
        })
    }

    /// reqwest 클라이언트예요. 처음 부를 때 만들어요.
    fn client(&self) -> Result<&Client, SourceError> {
        if let Some(c) = self.client.get() {
            return Ok(c);
        }
        let builder = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT);
        let builder = if is_loopback(self.base.as_str()) {
            builder.no_proxy()
        } else {
            builder
        };
        let client = builder.build().map_err(unreachable)?;
        // 두 스레드가 같이 만들면 먼저 넣은 것을 써요
        let _ = self.client.set(client);
        Ok(self.client.get().expect("the client was just set"))
    }

    /// 기본 주소예요(끝이 `/`).
    pub fn base(&self) -> &str {
        self.base.as_str()
    }

    /// GET 한 번이에요. 상태가 무엇이든 `Answer`로 돌려주고, 연결 실패·시간 초과(`timeout`은 요청 전체)는
    /// network, 다른 origin의 `Next`는 down, 머리에 실을 수 없는 토큰은 key예요. `headers`는 기본 머리를 바꿔요
    /// (`If-None-Match`, 다른 `Accept`).
    pub fn get(
        &self,
        token: &str,
        target: Target<'_>,
        accept: Accept,
        headers: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<Answer, SourceError> {
        let url = match target {
            Target::Path(p) => self.url(p)?,
            Target::Next(u) => {
                if u.origin() != self.base.origin() {
                    return Err(SourceError::service(
                        Kind::Down,
                        "GitHub pointed the next page at another server",
                    ));
                }
                u.clone()
            }
        };
        let req = self.client()?.get(url);
        self.send(req, token, accept, headers, timeout)
    }

    /// `POST {base}{path}`, JSON 몸통이에요(GraphQL). 답은 `get`과 같아요.
    pub fn post_json(
        &self,
        token: &str,
        path: &str,
        body: &Value,
        timeout: Duration,
    ) -> Result<Answer, SourceError> {
        let url = self.url(path)?;
        let req = self
            .client()?
            .post(url)
            .header(CONTENT_TYPE, "application/json; charset=utf-8")
            .body(body.to_string());
        self.send(req, token, Accept::Json, &[], timeout)
    }

    /// 목록 전부예요: 첫 주소에 `per_page=100`과 `extra`를 붙이고 `Link: rel="next"`를 끝까지 따라가 쪽마다 배열을
    /// 이어요. 2xx가 아니면 `kind_of`의 오류, JSON 배열이 아니면 down이에요. `timeout`은 쪽 하나의 시간이에요.
    /// 부르는 쪽은 `path`에 `per_page`를 넣지 않아요(여기서 붙여요). 이미 따라간 `next` 주소가 다시 오면 끝없이
    /// 돌지 않고 down이에요.
    pub fn pages(
        &self,
        token: &str,
        path: &str,
        extra: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<Vec<Value>, SourceError> {
        self.pages_seen(token, path, extra, timeout, &|_, _| {})
    }

    /// `pages`와 같고, 쪽마다 받은 (상태, 몸통)을 `seen`에 먼저 보여 줘요(토큰 거절 가르기, `GitHub::judge`).
    pub fn pages_seen(
        &self,
        token: &str,
        path: &str,
        extra: &[(&str, &str)],
        timeout: Duration,
        seen_answer: &dyn Fn(u16, &[u8]),
    ) -> Result<Vec<Value>, SourceError> {
        let mut query = vec![("per_page", PER_PAGE)];
        query.extend_from_slice(extra);
        let first = with_query(path, &query);
        let mut items = Vec::new();
        let mut answer = self.get(token, Target::Path(&first), Accept::Json, &[], timeout)?;
        let mut seen: HashSet<Url> = HashSet::new();
        loop {
            seen_answer(answer.status, &answer.body);
            let body = checked(&answer)?;
            match body {
                Value::Array(page) => items.extend(page),
                _ => {
                    return Err(SourceError::Json(
                        "a page of GitHub's list is not a list".to_string(),
                    ));
                }
            }
            let Some(next) = answer.next.take() else {
                return Ok(items);
            };
            if !seen.insert(next.clone()) {
                return Err(SourceError::service(
                    Kind::Down,
                    "GitHub pointed the next page back at a page already read",
                ));
            }
            answer = self.get(token, Target::Next(&next), Accept::Json, &[], timeout)?;
        }
    }

    /// 기본 주소 뒤에 경로를 붙여요. 붙인 주소가 기본 주소 밖(다른 origin)이면 down이에요.
    fn url(&self, path: &str) -> Result<Url, SourceError> {
        let url = Url::parse(&format!("{}{}", self.base, path.trim_start_matches('/')))
            .ok()
            .filter(|u| u.origin() == self.base.origin())
            .ok_or_else(|| SourceError::service(Kind::Down, "a GitHub path is not valid"))?;
        Ok(url)
    }

    fn send(
        &self,
        req: RequestBuilder,
        token: &str,
        accept: Accept,
        headers: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<Answer, SourceError> {
        let r = self.start(req, token, accept, headers, timeout)?;
        let status = r.status().as_u16();
        let out = headers_of(&r);
        let next = match out.get("link") {
            Some(link) => next_link(r.url(), link)?,
            None => None,
        };
        let body = r.bytes().map_err(unreachable)?.to_vec();
        Ok(Answer {
            status,
            headers: out,
            body,
            next,
        })
    }

    /// GET 한 번의 몸통을 흘려 읽어요(tarball): 머리까지 받고 몸통은 읽는 쪽이 `body`로 읽어요. 리다이렉트(tarball은
    /// codeload로 303)는 reqwest가 따라가고, 다른 host·port로 갈 때 `Authorization`을 떼요. 실패는 `get`과 같아요.
    /// `timeout`은 몸통을 다 읽을 때까지예요. 몸통을 읽다 난 오류의 글은 주소(codeload의 임시 `token`)를 담을 수
    /// 있으니 그대로 찍지 않아요.
    pub fn open(
        &self,
        token: &str,
        path: &str,
        accept: Accept,
        timeout: Duration,
    ) -> Result<Opened, SourceError> {
        let url = self.url(path)?;
        let req = self.client()?.get(url);
        let r = self.start(req, token, accept, &[], timeout)?;
        Ok(Opened {
            status: r.status().as_u16(),
            headers: headers_of(&r),
            body: Box::new(r),
        })
    }

    /// 머리를 붙여 보내고 답의 머리까지 받아요.
    fn start(
        &self,
        req: RequestBuilder,
        token: &str,
        accept: Accept,
        headers: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<Response, SourceError> {
        let mut auth = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            SourceError::service(Kind::Key, "the GitHub token cannot be sent in a header")
        })?;
        auth.set_sensitive(true);
        let mut map = HeaderMap::new();
        map.insert(ACCEPT, HeaderValue::from_static(accept.header()));
        map.insert(
            HeaderName::from_static("x-github-api-version"),
            HeaderValue::from_static(API_VERSION),
        );
        for (k, v) in headers {
            let (Ok(k), Ok(v)) = (HeaderName::try_from(*k), HeaderValue::from_str(v)) else {
                return Err(SourceError::Other(
                    "a request header is not valid".to_string(),
                ));
            };
            map.insert(k, v);
        }
        map.insert(AUTHORIZATION, auth);
        req.headers(map)
            .timeout(timeout)
            .send()
            .map_err(unreachable)
    }
}

/// 흘려 읽는 답이에요(`Http::open`). `Debug`에는 주소를 넣지 않아요.
pub struct Opened {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Box<dyn Read + Send>,
}

impl fmt::Debug for Opened {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Opened")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// 답의 머리예요: 이름은 소문자이고 같은 이름이 여럿이면 `, `로 이어요.
fn headers_of(r: &Response) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for (k, v) in r.headers() {
        let v = String::from_utf8_lossy(v.as_bytes()).into_owned();
        out.entry(k.as_str().to_string())
            .and_modify(|old| {
                old.push_str(", ");
                old.push_str(&v);
            })
            .or_insert(v);
    }
    out
}

/// 2xx면 JSON 몸통, 아니면 `kind_of`의 오류예요. 2xx인데 JSON이 아니면 down이에요.
pub fn checked(a: &Answer) -> Result<Value, SourceError> {
    let body: Option<Value> = serde_json::from_slice(&a.body).ok();
    if let Some((kind, message)) = kind_of(a.status, &a.headers, body.as_ref()) {
        return Err(SourceError::Service { kind, message });
    }
    body.ok_or_else(|| SourceError::Json("GitHub's answer is not JSON".to_string()))
}

fn unreachable(e: reqwest::Error) -> SourceError {
    SourceError::service(Kind::Network, format!("{UNREACHABLE}: {}", net_error(e)))
}

/// `Link` 머리의 `rel="next"` 주소예요(`<url>; rel="next", <url>; rel="last"`). 주소 안의 `,`(쿼리 값)로 나누지 않게
/// `<…>`를 먼저 읽어요. 상대 주소는 답한 주소에 붙여요. `rel="next"`가 있는데 주소를 읽을 수 없으면 목록을 조용히
/// 자르지 않고 down이에요(글에 주소는 넣지 않아요).
fn next_link(answered: &Url, link: &str) -> Result<Option<Url>, SourceError> {
    let bad = || SourceError::service(Kind::Down, "GitHub's next page link is not valid");
    let mut rest = link;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let close = after.find('>').ok_or_else(bad)?;
        let url = &after[..close];
        let params_end = after[close + 1..]
            .find('<')
            .map_or(after.len(), |i| close + 1 + i);
        let params = &after[close + 1..params_end];
        let is_next = params.split([';', ',']).any(|p| {
            p.trim()
                .strip_prefix("rel=")
                .is_some_and(|r| r.trim_matches('"').split_whitespace().any(|r| r == "next"))
        });
        if is_next {
            return answered.join(url.trim()).map(Some).map_err(|_| bad());
        }
        rest = &after[params_end..];
    }
    Ok(None)
}

/// 답의 실패 종류와 고정 글이에요(스펙 4-4). 2xx면 None이에요. 304(ETag가 맞음)도 여기서는 down이니, ETag를
/// 보내는 쪽(`status`를 쓰는 바뀜 확인)은 304를 먼저 스스로 다루고 이것을 불러요. 글에는 GitHub의 `message`, 주소, 이름을 넣지 않고,
/// 403의 이유만 `message`에서 골라 괄호로 붙여요.
pub fn kind_of(
    status: u16,
    headers: &HashMap<String, String>,
    body: Option<&Value>,
) -> Option<(Kind, String)> {
    if (200..300).contains(&status) {
        return None;
    }
    let said = format!("GitHub answered HTTP {status}");
    let out = if rate_limited(status, headers, body) {
        (Kind::Busy, format!("{said} (rate limit)"))
    } else {
        match status {
            401 => (Kind::Key, BAD_CREDENTIALS.to_string()),
            403 => {
                let message = body
                    .and_then(|b| b.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                let why = [
                    ("saml enforcement", "SAML enforcement"),
                    (
                        "oauth app access restrictions",
                        "OAuth App access restrictions",
                    ),
                    ("not accessible", "not accessible"),
                ]
                .into_iter()
                .find(|(mark, _)| message.contains(mark));
                match why {
                    Some((_, why)) => (Kind::Key, format!("{said} ({why})")),
                    None => (Kind::Key, said),
                }
            }
            404 | 410 => (Kind::Key, said),
            _ => (Kind::Down, said),
        }
    };
    Some(out)
}

/// `path`에 쿼리 값을 더해요: 이미 `?`가 있으면 `&`로 이어요. 값은 `application/x-www-form-urlencoded`예요(`:` →
/// `%3A`, 빈칸 → `+`).
pub fn with_query(path: &str, extra: &[(&str, &str)]) -> String {
    if extra.is_empty() {
        return path.to_string();
    }
    let query = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(extra)
        .finish();
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{path}{sep}{query}")
}

/// 경로 조각 하나(브랜치 이름의 `/` 사이)를 퍼센트 인코딩해요. `/`로 나누는 건 부르는 쪽이에요.
pub fn segment(s: &str) -> String {
    utf8_percent_encode(s, SEGMENT).to_string()
}
