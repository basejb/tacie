//! Linear GraphQL calls (docs/18 9절). `LinearHttp` is the real API; tests plug in a fake.
//!
//! Linear returns GraphQL errors as 400 or 200. So it first reads `errors[].extensions.code` in the body (auth errors
//! are `Key`, limits are `Busy`) and falls back to the HTTP status only when the body is not GraphQL. Validation
//! errors such as "no such field" and partly failed answers are not thrown as errors but returned as a `Reply`, so the
//! caller (`fetch`) can drop the field and ask again or use the data it got.
//!
//! The token lives only in the `Authorization` header (marked sensitive) and never shows in errors, logs or `Debug`.
//! Query text and answer content are not put in error text either.

use std::fmt;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue, RETRY_AFTER};
use serde_json::{Value, json};

use crate::error::{Kind, SourceError};
use crate::net::net_error;

/// Linear GraphQL 주소예요. 테스트는 로컬 가짜를 가리켜요.
pub const API: &str = "https://api.linear.app/graphql";
/// 호출 하나를 몇 번까지 하는지예요(429를 기다리며).
pub const TRIES: u32 = 6;
/// 요청 하나를 기다리는 시간이에요.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// GraphQL 답이에요: `data`(없을 수 있어요)와 `errors`(없으면 빈 목록).
#[derive(Debug, Clone, Default)]
pub struct Reply {
    pub data: Option<Value>,
    pub errors: Vec<Value>,
}

/// Linear GraphQL 질의 하나예요.
pub trait LinearApi: Send + Sync {
    fn query(&self, query: &str, vars: &Value) -> Result<Reply, SourceError>;
}

/// 진짜 API예요: `POST {base}`, `Authorization: Bearer {token}`.
pub struct LinearHttp {
    base: String,
    auth: Option<HeaderValue>,
    client: Result<Client, String>,
    tries: u32,
}

impl fmt::Debug for LinearHttp {
    /// 주소만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinearHttp")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

/// The `Authorization` value: a personal API key (`lin_api_…`) goes as it is, an OAuth token as `Bearer`
/// (Linear refuses a personal key with `Bearer`).
pub(crate) fn auth_value(token: &str) -> String {
    if token.starts_with("lin_api_") {
        token.to_string()
    } else {
        format!("Bearer {token}")
    }
}

impl LinearHttp {
    pub fn new(base: &str, token: &str) -> Self {
        Self::with_tries(base, token, TRIES, TIMEOUT)
    }

    /// 30분 확인처럼 빨리 끝나야 할 때예요: `tries`번, 요청마다 `timeout`.
    pub fn with_tries(base: &str, token: &str, tries: u32, timeout: Duration) -> Self {
        let auth = HeaderValue::from_str(&auth_value(token)).ok().map(|mut h| {
            h.set_sensitive(true);
            h
        });
        let builder = Client::builder().timeout(timeout);
        let builder = if crate::is_loopback(base) {
            builder.no_proxy()
        } else {
            builder
        };
        LinearHttp {
            base: base.to_string(),
            auth,
            client: builder.build().map_err(net_error),
            tries: tries.max(1),
        }
    }
}

/// GraphQL 본문이 아닌 답의 HTTP 상태를 종류로 나눠요.
pub(crate) fn map_status(status: u16) -> Kind {
    match status {
        401 => Kind::Key,
        429 => Kind::Busy,
        _ => Kind::Down,
    }
}

/// GraphQL 오류 목록에서 종류를 아는 것(인증, 한도)을 찾아요.
#[cfg(test)]
pub(crate) fn graphql_kind(errors: &Value) -> Option<Kind> {
    graphql_kind_with(errors, false)
}

/// `FORBIDDEN`은 데이터가 하나도 없을 때만 거절이에요(일부 칸만 볼 수 없는 답은 받은 데이터를 써요).
fn graphql_kind_with(errors: &Value, has_data: bool) -> Option<Kind> {
    let codes = errors
        .as_array()?
        .iter()
        .filter_map(|e| e["extensions"]["code"].as_str());
    let mut found = None;
    for c in codes {
        match c {
            "AUTHENTICATION_ERROR" => return Some(Kind::Key),
            "FORBIDDEN" if !has_data => return Some(Kind::Key),
            "RATELIMITED" => found = Some(Kind::Busy),
            _ => {}
        }
    }
    found
}

/// "Cannot query field \"X\" on type …" 오류의 칸 이름이에요.
pub(crate) fn unknown_field(errors: &Value) -> Option<String> {
    errors.as_array()?.iter().find_map(|e| {
        let m = e["message"].as_str()?;
        let rest = m.strip_prefix("Cannot query field \"")?;
        Some(rest.split('"').next()?.to_string())
    })
}

impl LinearApi for LinearHttp {
    fn query(&self, query: &str, vars: &Value) -> Result<Reply, SourceError> {
        let fail = |kind: Kind, m: &str| SourceError::service(kind, format!("linear: {m}"));
        let client = self.client.as_ref().map_err(|e| fail(Kind::Down, e))?;
        let auth = self
            .auth
            .clone()
            .ok_or_else(|| fail(Kind::Key, "the token cannot be sent in a header"))?;
        let body = json!({"query": query, "variables": vars}).to_string();
        for _ in 0..self.tries {
            let r = client
                .post(&self.base)
                .header(AUTHORIZATION, auth.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(body.clone())
                .send()
                .map_err(|e| fail(Kind::Network, &net_error(e)))?;
            let status = r.status().as_u16();
            let wait = r
                .headers()
                .get(RETRY_AFTER)
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok());
            let bytes = r.bytes().map_err(|e| fail(Kind::Network, &net_error(e)))?;
            let d: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let errors = d.get("errors").cloned().unwrap_or(Value::Null);
            let has_data = d.get("data").is_some_and(|v| !v.is_null());
            let kind = graphql_kind_with(&errors, has_data).or(if status == 429 {
                Some(Kind::Busy)
            } else {
                None
            });
            if kind == Some(Kind::Busy) {
                std::thread::sleep(Duration::from_secs(wait.unwrap_or(1).min(60) + 1));
                continue;
            }
            if kind == Some(Kind::Key) {
                return Err(fail(Kind::Key, "the connection was refused"));
            }
            let graphql = d.get("data").is_some() || errors.is_array();
            if !graphql || status >= 500 {
                return Err(fail(map_status(status), &format!("{status}")));
            }
            return Ok(Reply {
                data: d.get("data").filter(|v| !v.is_null()).cloned(),
                errors: errors.as_array().cloned().unwrap_or_default(),
            });
        }
        Err(fail(Kind::Busy, "rate limited"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::github::FakeGitHub;
    use serde_json::json;

    #[test]
    fn personal_key_goes_without_bearer() {
        assert_eq!(auth_value("lin_api_abc"), "lin_api_abc");
        assert_eq!(auth_value("oauth-token"), "Bearer oauth-token");
    }

    #[test]
    fn statuses_map_to_kinds() {
        assert_eq!(map_status(401), Kind::Key);
        assert_eq!(map_status(503), Kind::Down);
        assert_eq!(map_status(418), Kind::Down);
    }

    #[test]
    fn ratelimited_graphql_error_is_busy() {
        let e = json!([{"message": "slow down", "extensions": {"code": "RATELIMITED"}}]);
        assert_eq!(graphql_kind(&e), Some(Kind::Busy));
    }

    #[test]
    fn auth_error_is_key() {
        let e = json!([{"message": "no", "extensions": {"code": "AUTHENTICATION_ERROR"}}]);
        assert_eq!(graphql_kind(&e), Some(Kind::Key));
        assert_eq!(graphql_kind(&json!([{"message": "x"}])), None);
    }

    #[test]
    fn unknown_field_is_found() {
        let e = json!([{"message": "Cannot query field \"descriptionData\" on type \"Issue\"."}]);
        assert_eq!(unknown_field(&e), Some("descriptionData".to_string()));
        assert_eq!(unknown_field(&json!([{"message": "other"}])), None);
    }

    #[test]
    fn debug_hides_the_token() {
        let h = LinearHttp::new(API, "lin_oauth_secret");
        assert!(!format!("{h:?}").contains("secret"));
    }

    fn fake(
        f: impl Fn(&str, &Value) -> (u16, Value) + Send + Sync + 'static,
    ) -> (std::sync::Arc<FakeGitHub>, LinearHttp) {
        let g = FakeGitHub::start();
        g.graphql(f);
        let h = LinearHttp::new(&format!("{}graphql", g.url()), FakeGitHub::TOKEN);
        (g, h)
    }

    #[test]
    fn http_400_validation_error_returns_the_errors_body() {
        let (_g, h) = fake(|_, _| {
            (
                400,
                json!({"errors": [{"message": "Cannot query field \"x\" on type \"Issue\"."}]}),
            )
        });
        let r = h.query("{ x }", &json!({})).unwrap();
        assert!(r.data.is_none());
        assert_eq!(unknown_field(&Value::Array(r.errors)), Some("x".into()));
    }

    #[test]
    fn http_400_auth_error_is_key() {
        let (_g, h) = fake(|_, _| {
            (
                400,
                json!({"errors": [{"message": "auth", "extensions": {"code": "AUTHENTICATION_ERROR"}}]}),
            )
        });
        assert_eq!(h.query("{ x }", &json!({})).unwrap_err().kind(), Kind::Key);
    }

    #[test]
    fn http_200_auth_error_is_key() {
        let (_g, h) = fake(|_, _| {
            (
                200,
                json!({"data": null, "errors": [{"message": "auth", "extensions": {"code": "AUTHENTICATION_ERROR"}}]}),
            )
        });
        assert_eq!(h.query("{ x }", &json!({})).unwrap_err().kind(), Kind::Key);
    }

    #[test]
    fn http_401_without_graphql_body_is_key() {
        let (_g, h) = fake(|_, _| (401, json!("nope")));
        assert_eq!(h.query("{ x }", &json!({})).unwrap_err().kind(), Kind::Key);
    }

    #[test]
    fn http_429_waits_then_succeeds() {
        let n = std::sync::atomic::AtomicUsize::new(0);
        let (_g, h) = fake(move |_, _| {
            if n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                (429, json!({}))
            } else {
                (200, json!({"data": {"ok": true}}))
            }
        });
        let r = h.query("{ ok }", &json!({})).unwrap();
        assert_eq!(r.data.unwrap()["ok"], true);
    }

    #[test]
    fn http_200_partial_errors_return_data_and_errors() {
        let (g, h) = fake(|q, v| {
            assert!(q.contains("issues"));
            assert_eq!(v["since"], "s");
            (
                200,
                json!({"data": {"issues": {"nodes": []}}, "errors": [{"message": "Entity not found: parent"}]}),
            )
        });
        let r = h.query("{ issues }", &json!({"since": "s"})).unwrap();
        assert!(r.data.is_some());
        assert_eq!(r.errors.len(), 1);
        assert!(g.calls()[0].has_token);
    }

    #[test]
    fn a_forbidden_part_beside_data_is_not_a_refusal() {
        let (_g, h) = fake(|_, _| {
            (
                200,
                json!({"data": {"issues": {"nodes": []}}, "errors": [{"message": "no", "extensions": {"code": "FORBIDDEN"}}]}),
            )
        });
        let r = h.query("{ issues }", &json!({})).unwrap();
        assert!(r.data.is_some());
        let (_g, h) = fake(|_, _| {
            (
                200,
                json!({"data": null, "errors": [{"message": "no", "extensions": {"code": "FORBIDDEN"}}]}),
            )
        });
        assert_eq!(h.query("{ x }", &json!({})).unwrap_err().kind(), Kind::Key);
    }
}
