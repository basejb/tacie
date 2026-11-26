//! Slack: reads only channels the Tacie bot was invited to; one thread, or one group of messages that follow each
//! other within 10 minutes, is one document.
//!
//! Inviting the bot is the scope: channels without the bot are not read. Private channels put their member list in
//! the document's acl, so only members see the cards. Bot messages and join/leave notices are left out. The token is
//! `bot_token` in `{config_dir}/slack/config.json`.
//!
//! - Fetching (`fetch_with`): fetches messages, threads and private channel members from the last 180 days and
//!   replaces `slack.json` whole (written to `slack.json.tmp` first, then renamed). If one channel fails the rest are
//!   still fetched (docs/09 5절): public channels use the saved copy, and private channels are left out of this fetch
//!   (a stale member list must not open cards). If every attempted channel fails, it is Err.
//! - Calls (`SlackApi`): `SlackHttp` is the real Web API; tests plug in a fake. Paging (`limit=200`, the given
//!   arguments, then `cursor`) is done by `fetch_with`. The next page of a list is asked for only after the previous
//!   page's entries are used up.
//! - Documents (`documents`) and chunks (`chunks`): text goes through `clean` (mentions to names, links to labels, five
//!   regexes, then Python `html.unescape` and `strip`), dates and times are in KST (+9), and timestamps are read like
//!   Python `datetime.fromtimestamp`, rounding microseconds half to even.
//!
//! The token lives only in the `Authorization` header (marked sensitive) and never shows in errors, logs or `Debug`.
//! Text (message bodies) is never put in logs or errors: log lines have channel names and counts, and errors have only
//! Slack's error code and what did not match. Every failure is `Other` (down).
//!
//! Rules for raw data and failures:
//! - If raw data (Slack's answer or a saved `slack.json`) has a shape Slack does not send and Tacie does not write
//!   (non-string ids, names, ts or text, a message that is not an object, a non-string person name, a non-integer
//!   Retry-After, non-string channel ids, names, team_url or members, a null name for a person after the fifth, a
//!   non-string cursor or token, and the like), it fails with a `Json` error (test
//!   `documents_fail_where_python_raises`). Messages that are not kept (bots, notices, empty text) are not checked.
//! - ts is read as a Rust `f64` after trimming surrounding whitespace: underscores (`1_000.5`) and full-width digits
//!   are rejected.
//! - When a fetch fails, `slack.json.tmp` is deleted, and the written file is fsynced before the rename.
//! - The 30 s timeout covers the whole request.
//! - Error text: if the token file has no `bot_token` or it is not a string, `slack/config.json has no bot_token`; if
//!   the token cannot be put in a header, `slack {method}: the bot token cannot be sent in a header`; if the answer is
//!   not JSON, `slack {method}: the answer is not JSON`; connection failures use reqwest's text (with the URL
//!   removed). When `ok` is false, a non-string `error` is written as JSON text (strings as is, null as `None`,
//!   booleans as `True`/`False`).

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use chrono::{DateTime, Datelike, FixedOffset, Utc};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderValue, RETRY_AFTER};
use serde_json::{Map, Value};
use tacie_index::{RawChunk, pyjson};

use crate::doc::{Doc, windows};
use crate::error::SourceError;
use crate::github::now_iso;
use crate::net::net_error;
use crate::py;

/// Slack Web API 주소예요. 테스트는 로컬 HTTP 가짜를 가리켜요.
pub const API: &str = "https://slack.com/api/";
/// 이만큼 지난 글까지 받아요(일).
pub const DAYS: i64 = 180;
/// 문서에 넣지 않는 글 종류예요(들어옴·나감, 주제, 봇, 고정, 알림).
pub const SKIP_SUBTYPES: [&str; 10] = [
    "channel_join",
    "channel_leave",
    "channel_topic",
    "channel_purpose",
    "channel_name",
    "bot_message",
    "channel_archive",
    "channel_unarchive",
    "pinned_item",
    "reminder_add",
];
/// 이보다 가까운 글(스레드가 아닌 것)은 한 문서예요(초).
pub const BURST_GAP: f64 = 600.0;
/// 호출 하나를 몇 번까지 하는지예요(호출 제한 429를 기다리며).
pub const TRIES: usize = 6;
/// 요청 하나를 기다리는 시간이에요.
pub const TIMEOUT: Duration = Duration::from_secs(30);
const NO_TOKEN: &str = "no Slack token: ~/.config/slack/config.json (docs/05)";
const UNKNOWN: &str = "누군가";
/// KST예요(+9).
const KST_SECS: i32 = 9 * 3600;

static MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<@([UW][A-Z0-9]+)(?:\|[^>]*)?>").unwrap());
static CHANNEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<#[CG][A-Z0-9]+\|([^>]*)>").unwrap());
static SPECIAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!(here|channel|everyone)[^>]*>").unwrap());
static LABELED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(https?://[^|>]+)\|([^>]+)>").unwrap());
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<(https?://[^>]+)>").unwrap());

fn bad(what: &str) -> SourceError {
    SourceError::Json(format!("slack.json: {what}"))
}

/// Slack Web API 호출 하나예요: `ok`인 답만 돌려줘요.
pub trait SlackApi {
    fn call(&self, method: &str, params: &[(&str, String)]) -> Result<Value, SourceError>;
}

/// 진짜 Web API예요: `GET {base}{method}?{params}`, `Authorization: Bearer {token}`, 30 s. 429면 `Retry-After`(없으면
/// 5) + 1초를 쉬고 다시 물어 6번까지 해요. `ok`가 거짓이면 `slack {method}: {error}`예요.
pub struct SlackHttp {
    base: String,
    /// None이면 토큰을 머리에 실을 수 없어요(줄바꿈 같은 글자).
    auth: Option<HeaderValue>,
    client: Result<Client, String>,
}

impl fmt::Debug for SlackHttp {
    /// 주소만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SlackHttp")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl SlackHttp {
    pub fn new(base: &str, token: &str) -> Self {
        let auth = HeaderValue::from_str(&format!("Bearer {token}"))
            .ok()
            .map(|mut h| {
                h.set_sensitive(true);
                h
            });
        // loopback 주소(테스트·계약의 가짜)는 프록시를 쓰지 않아요: 프록시로 새면 요청이 프록시에 남아요.
        let builder = Client::builder().timeout(TIMEOUT);
        let builder = if crate::is_loopback(base) {
            builder.no_proxy()
        } else {
            builder
        };
        let client = builder.build().map_err(net_error);
        SlackHttp {
            base: base.to_string(),
            auth,
            client,
        }
    }
}

/// `Retry-After`(없으면 5)에 1을 더한 초예요. 정수가 아니거나 음수(−1보다 작음)거나 나노초로 i64에 들어가지 않게
/// 크면 None이에요.
fn retry_after(header: Option<&HeaderValue>) -> Option<Duration> {
    let text = match header {
        None => "5",
        Some(h) => h.to_str().ok()?,
    };
    let secs = py_int(text)?.checked_add(1)?;
    let secs = u64::try_from(secs).ok()?;
    // 나노초가 i64에 들어가는 만큼만 기다려요
    if secs > i64::MAX as u64 / 1_000_000_000 {
        return None;
    }
    Some(Duration::from_secs(secs))
}

/// Python `int(s)`예요(10진, 앞뒤 공백, 부호, 숫자 사이 밑줄 하나). i64를 넘으면 None이에요.
fn py_int(s: &str) -> Option<i64> {
    let t = py::strip(s);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let ok = !digits.is_empty()
        && !digits.starts_with('_')
        && !digits.ends_with('_')
        && !digits.contains("__")
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'_');
    if !ok {
        return None;
    }
    let n: i64 = digits.replace('_', "").parse().ok()?;
    Some(if neg { -n } else { n })
}

/// 답의 `error`를 Python `str()`로 쓴 글이에요: 없거나 null이면 `None`, 글은 그대로, 참·거짓은
/// `True`·`False`예요.
fn error_text(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "None".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => if *b { "True" } else { "False" }.to_string(),
        Some(other) => pyjson::to_string(other, None),
    }
}

impl SlackApi for SlackHttp {
    fn call(&self, method: &str, params: &[(&str, String)]) -> Result<Value, SourceError> {
        let fail = |what: &str| SourceError::Other(format!("slack {method}: {what}"));
        let client = self.client.as_ref().map_err(|e| fail(e))?;
        let auth = self
            .auth
            .clone()
            .ok_or_else(|| fail("the bot token cannot be sent in a header"))?;
        let url = format!("{}{method}", self.base);
        for _ in 0..TRIES {
            let mut req = client.get(&url).header(AUTHORIZATION, auth.clone());
            if !params.is_empty() {
                req = req.query(params);
            }
            let r = req.send().map_err(|e| fail(&net_error(e)))?;
            if r.status().as_u16() == 429 {
                let wait = retry_after(r.headers().get(RETRY_AFTER))
                    .ok_or_else(|| fail("Retry-After is not a number of seconds to wait"))?;
                std::thread::sleep(wait);
                continue;
            }
            let body = r.bytes().map_err(|e| fail(&net_error(e)))?;
            let d: Value =
                serde_json::from_slice(&body).map_err(|_| fail("the answer is not JSON"))?;
            let Some(m) = d.as_object() else {
                return Err(fail("the answer is not a JSON object"));
            };
            if !m.get("ok").is_some_and(py::truthy) {
                return Err(fail(&error_text(m.get("error"))));
            }
            return Ok(d);
        }
        Err(fail("rate limited"))
    }
}

/// 봇 토큰이에요(`{config_dir}/slack/config.json`의 `bot_token`).
pub fn token(config_dir: &Path) -> Result<String, SourceError> {
    let f = config_dir.join("slack/config.json");
    if !f.exists() {
        return Err(SourceError::Other(NO_TOKEN.to_string()));
    }
    let v: Value = serde_json::from_str(&fs::read_to_string(&f)?)
        .map_err(|_| SourceError::Json("slack/config.json is not JSON".to_string()))?;
    match v.get("bot_token") {
        Some(Value::String(t)) => Ok(t.clone()),
        _ => Err(SourceError::Other(
            "slack/config.json has no bot_token".to_string(),
        )),
    }
}

/// 한 쪽에 200개씩 `cursor`를 따라 끝까지 묻고, 쪽마다 `key`의 항목을 차례로 `each`에 줘요.
/// 다음 쪽은 앞 쪽의 항목을 다 쓴 뒤에 물어요.
fn pages(
    api: &dyn SlackApi,
    method: &str,
    key: &str,
    params: &[(&str, String)],
    each: &mut dyn FnMut(Value) -> Result<(), SourceError>,
) -> Result<(), SourceError> {
    let shape = |what: &str| SourceError::Json(format!("slack {method}: {what}"));
    let mut cursor: Option<String> = None;
    loop {
        let mut all = Vec::with_capacity(params.len() + 2);
        all.push(("limit", "200".to_string()));
        all.extend(params.iter().cloned());
        if let Some(c) = &cursor {
            all.push(("cursor", c.clone()));
        }
        let mut d = api.call(method, &all)?;
        // 이 쪽의 항목을 먼저 다 쓰고 다음 커서를 봐요
        match d.get_mut(key).map(Value::take) {
            None => {}
            Some(Value::Array(items)) => {
                for it in items {
                    each(it)?;
                }
            }
            Some(_) => return Err(shape(&format!("{key} is not a list"))),
        }
        cursor = match d.get("response_metadata").filter(|v| py::truthy(v)) {
            None => None,
            Some(Value::Object(meta)) => match meta.get("next_cursor") {
                Some(Value::String(c)) if !c.is_empty() => Some(c.clone()),
                Some(v) if py::truthy(v) => return Err(shape("next_cursor is not text")),
                _ => None,
            },
            Some(_) => return Err(shape("response_metadata is not an object")),
        };
        if cursor.is_none() {
            return Ok(());
        }
    }
}

/// 저장된 채널이에요(id → 채널). 파일이 없으면 비어요.
fn saved_channels(out: &Path) -> Result<HashMap<String, Value>, SourceError> {
    let Some(raw) = read_raw_file(out)? else {
        return Ok(HashMap::new());
    };
    let Some(channels) = raw.get("channels").and_then(Value::as_array) else {
        return Err(bad("there is no channel list"));
    };
    let mut saved = HashMap::new();
    for c in channels {
        let Some(id) = c.get("id").and_then(Value::as_str) else {
            return Err(bad("a saved channel has no id"));
        };
        saved.insert(id.to_string(), c.clone()); // 같은 id는 나중 것이에요
    }
    Ok(saved)
}

/// 사람의 이름이에요: `profile.display_name`, `real_name`, `name` 중 처음으로 참인 값이에요(모두 거짓이면 `name`
/// 그대로, 없으면 null).
fn display_name(u: &Map<String, Value>) -> Result<Value, SourceError> {
    let profile = match u.get("profile") {
        None => None,
        Some(Value::Object(p)) => Some(p),
        Some(_) => {
            return Err(SourceError::Json(
                "slack users.list: a member's profile is not an object".to_string(),
            ));
        }
    };
    for v in [
        profile.and_then(|p| p.get("display_name")),
        u.get("real_name"),
    ] {
        if let Some(v) = v.filter(|v| py::truthy(v)) {
            return Ok(v.clone());
        }
    }
    Ok(u.get("name").cloned().unwrap_or(Value::Null))
}

/// 글이어야 하는 필드예요.
fn text_field<'a>(
    m: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<&'a str, SourceError> {
    m.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| SourceError::Json(what.to_string()))
}

/// 지금 − `days`일의 유닉스 시각을 Python `str(float)` 모양으로 쓴 글이에요(마이크로초 정수를 10⁶으로 나눈 가장
/// 가까운 소수).
fn oldest(days: i64) -> String {
    let micros = Utc::now()
        .timestamp_micros()
        .saturating_sub(days.saturating_mul(86_400_000_000));
    pyjson::to_string(&Value::from(micros as f64 / 1e6), None)
}

/// 봇이 멤버인 채널마다 지난 `days`일을 받아 `raw/slack.json`을 통째로 바꿔요. 모듈 설명을 보세요.
pub fn fetch_with(api: &dyn SlackApi, raw: &Path, days: i64) -> Result<(), SourceError> {
    fs::create_dir_all(raw)?;
    let out = raw.join("slack.json");
    let saved = saved_channels(&out)?;
    let team = api.call("auth.test", &[])?;
    let mut users = Map::new();
    pages(api, "users.list", "members", &[], &mut |u| {
        let Some(u) = u.as_object() else {
            return Err(SourceError::Json(
                "slack users.list: a member is not an object".to_string(),
            ));
        };
        let id = text_field(u, "id", "slack users.list: a member has no id")?;
        users.insert(id.to_string(), display_name(u)?); // 같은 id는 나중 것이에요
        Ok(())
    })?;
    let oldest = oldest(days);
    let (mut channels, mut failed, mut tried) = (Vec::new(), Vec::new(), 0usize);
    let list = [
        ("types", "public_channel,private_channel".to_string()),
        ("exclude_archived", "true".to_string()),
    ];
    pages(api, "conversations.list", "channels", &list, &mut |ch| {
        let Some(ch) = ch.as_object() else {
            return Err(SourceError::Json(
                "slack conversations.list: a channel is not an object".to_string(),
            ));
        };
        if !ch.get("is_member").is_some_and(py::truthy) {
            return Ok(());
        }
        tried += 1;
        let private = ch.get("is_private").is_some_and(py::truthy);
        let e = match channel(api, ch, &oldest) {
            Ok(c) => {
                channels.push(c);
                return Ok(());
            }
            Err(e) => e,
        };
        let name = text_field(
            ch,
            "name",
            "slack conversations.list: a channel has no name",
        )?;
        let label = format!("{}{name}", if private { "🔒" } else { "#" });
        let kept = if private {
            None
        } else {
            let id = text_field(ch, "id", "slack conversations.list: a channel has no id")?;
            saved.get(id)
        };
        if let Some(kept) = kept {
            channels.push(kept.clone());
        }
        eprintln!(
            "slack {label}: failed, {} ({})",
            if kept.is_some() {
                "using the saved copy"
            } else {
                "left out"
            },
            py::take_chars(&e.to_string(), 120)
        );
        failed.push(label);
        Ok(())
    })?;
    if tried > 0 && failed.len() == tried {
        return Err(SourceError::Other(format!(
            "slack: every channel failed ({})",
            failed.join(", ")
        )));
    }
    let Some(team_url) = team.get("url") else {
        return Err(SourceError::Json(
            "slack auth.test: the answer has no url".to_string(),
        ));
    };
    let mut data = Map::new();
    data.insert("team_url".into(), team_url.clone());
    data.insert("users".into(), Value::Object(users));
    data.insert("channels".into(), Value::Array(channels));
    data.insert(
        "failed".into(),
        Value::Array(failed.into_iter().map(Value::from).collect()),
    );
    data.insert("fetched_at".into(), Value::from(now_iso()));
    write_whole(&out, &Value::Object(data))
}

/// `slack.json.tmp`에 쓰고(fsync) 바꿔 넣어요. 실패하면 임시 파일을 지워요.
fn write_whole(out: &Path, data: &Value) -> Result<(), SourceError> {
    let tmp = out.with_file_name("slack.json.tmp");
    let written = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(pyjson::to_string(data, None).as_bytes())?;
        rustix::fs::fsync(&f)?;
        drop(f);
        fs::rename(&tmp, out)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(written?)
}

/// 사전 키로 쓸 수 있는 값의 글자예요(목록·객체는 오류예요).
fn hashable(v: &Value, what: &str) -> Result<String, SourceError> {
    match v {
        Value::Array(_) | Value::Object(_) => Err(SourceError::Json(what.to_string())),
        Value::String(s) => Ok(format!("s{s}")),
        other => Ok(format!("v{other}")),
    }
}

/// 채널 하나의 글, 스레드, (비공개면) 멤버예요. 키 순서: id, name, is_private, members, messages,
/// replies.
fn channel(
    api: &dyn SlackApi,
    ch: &Map<String, Value>,
    oldest: &str,
) -> Result<Value, SourceError> {
    let id = text_field(ch, "id", "slack: a channel has no id")?;
    let name = text_field(ch, "name", "slack: a channel has no name")?;
    let shape = |what: &str| SourceError::Json(format!("slack #{name}: {what}"));
    let mut msgs = Vec::new();
    let params = [("channel", id.to_string()), ("oldest", oldest.to_string())];
    pages(
        api,
        "conversations.history",
        "messages",
        &params,
        &mut |m| {
            msgs.push(m);
            Ok(())
        },
    )?;
    let mut replies = Map::new();
    for m in &msgs {
        let Some(m) = m.as_object() else {
            return Err(shape("a message is not an object"));
        };
        if !m.get("reply_count").is_some_and(py::truthy) {
            continue;
        }
        let ts = text_field(m, "ts", "a thread has no ts")?;
        // 답글의 쪽마다 맨 앞에 원글이 다시 와요: 답글은 ts마다 한 번(자리는 처음, 값은 나중 것)
        let mut by_ts: Vec<(String, Value)> = Vec::new();
        let params = [("channel", id.to_string()), ("ts", ts.to_string())];
        pages(
            api,
            "conversations.replies",
            "messages",
            &params,
            &mut |r| {
                let Some(rts) = r.get("ts") else {
                    return Err(shape("a reply has no ts"));
                };
                if rts.as_str() == Some(ts) {
                    return Ok(());
                }
                let key = hashable(rts, "a reply's ts is a list or an object")?;
                match by_ts.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, v)) => *v = r,
                    None => by_ts.push((key, r)),
                }
                Ok(())
            },
        )?;
        let list = by_ts.into_iter().map(|(_, r)| r).collect();
        replies.insert(ts.to_string(), Value::Array(list));
    }
    let private = ch.get("is_private").is_some_and(py::truthy);
    let members = if private {
        let mut members = Vec::new();
        let params = [("channel", id.to_string())];
        pages(api, "conversations.members", "members", &params, &mut |u| {
            members.push(u);
            Ok(())
        })?;
        Value::Array(members)
    } else {
        Value::Null
    };
    eprintln!(
        "slack #{name}: {} messages, {} threads",
        msgs.len(),
        replies.len()
    );
    let mut out = Map::new();
    out.insert("id".into(), Value::from(id));
    out.insert("name".into(), Value::from(name));
    out.insert(
        "is_private".into(),
        ch.get("is_private").cloned().unwrap_or(Value::Bool(false)),
    );
    out.insert("members".into(), members);
    out.insert("messages".into(), Value::Array(msgs));
    out.insert("replies".into(), Value::Object(replies));
    Ok(Value::Object(out))
}

/// 슬랙 표시를 보통 글로 바꿔요: 멘션은 `@이름`(모르면 `@누군가`), 채널은 `#이름`, `<!here>` 같은
/// 것은 `@here`, 링크는 라벨(없으면 주소), 그다음 Python `html.unescape`와 `strip`이에요. 멘션한 사람의 이름이 글이
/// 아니면 Err예요.
pub fn clean(text: &str, users: &Map<String, Value>) -> Result<String, SourceError> {
    let mut named = String::with_capacity(text.len());
    let mut last = 0;
    for m in MENTION.captures_iter(text) {
        let whole = m.get(0).expect("a match has group 0");
        named.push_str(&text[last..whole.start()]);
        named.push('@');
        named.push_str(name_by_id(users, &m[1])?);
        last = whole.end();
    }
    named.push_str(&text[last..]);
    let text = CHANNEL.replace_all(&named, "#${1}");
    let text = SPECIAL.replace_all(&text, "@${1}");
    let text = LABELED.replace_all(&text, "${2}");
    let text = LINK.replace_all(&text, "${1}");
    Ok(py::strip(&py::html_unescape(&text)).to_string())
}

/// `users`에서 찾은 이름이에요. 없으면 `누군가`, 글이 아니면 Err예요.
fn name_by_id<'a>(users: &'a Map<String, Value>, id: &str) -> Result<&'a str, SourceError> {
    match users.get(id) {
        None => Ok(UNKNOWN),
        Some(Value::String(name)) => Ok(name),
        Some(_) => Err(bad("a user's name is not text")),
    }
}

/// 글의 사용자 이름이에요(모르면 `누군가`): 글이 아닌 사용자(null, 수)는 찾을 수 없어 `누군가`이고, 목록·객체는
/// Err예요.
fn name_of<'a>(
    users: &'a Map<String, Value>,
    user: Option<&Value>,
) -> Result<&'a str, SourceError> {
    match user {
        Some(Value::String(id)) => name_by_id(users, id),
        Some(Value::Array(_) | Value::Object(_)) => Err(bad("a message's user is not an id")),
        _ => Ok(UNKNOWN),
    }
}

/// 남길 글이에요: 빼는 종류가 아니고, 봇 글이 아니고, 글이 비지 않았어요.
fn kept(m: &Map<String, Value>) -> Result<bool, SourceError> {
    match m.get("subtype") {
        Some(Value::String(s)) if SKIP_SUBTYPES.contains(&s.as_str()) => return Ok(false),
        Some(Value::Array(_) | Value::Object(_)) => {
            return Err(bad("a message's subtype is a list or an object"));
        }
        _ => {}
    }
    if m.get("bot_id").is_some_and(py::truthy) {
        return Ok(false);
    }
    match m.get("text") {
        Some(Value::String(s)) => Ok(!py::strip(s).is_empty()),
        Some(v) if py::truthy(v) => Err(bad("a message's text is not text")),
        _ => Ok(false),
    }
}

/// 문서에 드는 글의 ts예요: 글이어야 하고, 유한한 수로 읽혀야 해요(NaN·무한은 시각으로 바꿀 수 없어요).
fn ts_of(m: &Map<String, Value>) -> Result<(&str, f64), SourceError> {
    let ts = text_field(m, "ts", "slack.json: a message has no ts")?;
    let t = py::strip(ts);
    let f = if t.is_empty() || t.contains('_') {
        None
    } else {
        t.parse::<f64>().ok()
    };
    match f {
        Some(f) if f.is_finite() => Ok((ts, f)),
        _ => Err(bad("a message's ts is not a time")),
    }
}

/// Python `datetime.fromtimestamp(t, KST)`예요: 마이크로초를 짝수 쪽으로 반올림하고(올림이 초를 넘기면 다음 초),
/// 1–9999년 밖이면 Err예요.
fn when(t: f64) -> Result<DateTime<FixedOffset>, SourceError> {
    let out_of_range = || bad("a message's ts is out of range");
    let whole = t.trunc();
    let us = ((t - whole) * 1e6).round_ties_even();
    let secs = if us >= 1e6 {
        whole + 1.0
    } else if us < 0.0 {
        whole - 1.0
    } else {
        whole
    };
    if !(-9.2e18..9.2e18).contains(&secs) {
        return Err(out_of_range());
    }
    let kst = FixedOffset::east_opt(KST_SECS).expect("KST is a valid offset");
    let utc = DateTime::from_timestamp(secs as i64, 0).ok_or_else(out_of_range)?;
    let local = utc.with_timezone(&kst);
    if !(1..=9999).contains(&utc.year()) || !(1..=9999).contains(&local.year()) {
        return Err(out_of_range());
    }
    Ok(local)
}

/// 원본 하나의 문서예요: 채널마다 스레드 하나, 또는 10분 안에 이어진 글 묶음 하나가 문서예요.
/// 원본이 기대한 모양이 아니면 Err예요(모듈 설명).
pub fn documents(raw: &Value) -> Result<Vec<Doc>, SourceError> {
    let users = match raw.get("users") {
        Some(Value::Object(users)) => users,
        _ => return Err(bad("there are no users")),
    };
    let Some(channels) = raw.get("channels").and_then(Value::as_array) else {
        return Err(bad("there is no channel list"));
    };
    let mut docs = Vec::new();
    for ch in channels {
        let Some(ch) = ch.as_object() else {
            return Err(bad("a channel is not an object"));
        };
        channel_documents(raw, users, ch, &mut docs)?;
    }
    Ok(docs)
}

fn channel_documents(
    raw: &Value,
    users: &Map<String, Value>,
    ch: &Map<String, Value>,
    docs: &mut Vec<Doc>,
) -> Result<(), SourceError> {
    let Some(private) = ch.get("is_private") else {
        return Err(bad("a channel has no is_private"));
    };
    let acl: Vec<String> = if py::truthy(private) {
        let Some(members) = ch.get("members").and_then(Value::as_array) else {
            return Err(bad("a private channel has no member list"));
        };
        members
            .iter()
            .map(|u| {
                u.as_str()
                    .map(|u| format!("slack:{u}"))
                    .ok_or_else(|| bad("a member is not an id"))
            })
            .collect::<Result<_, _>>()?
    } else {
        vec!["*".to_string()]
    };
    let Some(messages) = ch.get("messages").and_then(Value::as_array) else {
        return Err(bad("a channel has no message list"));
    };
    // 스레드 밖의 글(원글)만, ts 순서로요(같은 ts는 원래 차례)
    let mut top: Vec<(f64, &Map<String, Value>)> = Vec::new();
    for m in messages {
        let Some(m) = m.as_object() else {
            return Err(bad("a message is not an object"));
        };
        if !kept(m)? {
            continue;
        }
        let Some(ts) = m.get("ts") else {
            return Err(bad("a message has no ts"));
        };
        if m.get("thread_ts").unwrap_or(ts) != ts {
            continue;
        }
        top.push((ts_of(m)?.1, m));
    }
    // 모두 유한해요(ts_of). -0.0과 0.0은 같아서 원래 차례예요(안정 정렬)
    top.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let threaded = |m: &Map<String, Value>| m.get("reply_count").is_some_and(py::truthy);
    let mut groups: Vec<Vec<&Map<String, Value>>> = Vec::new();
    let mut burst: Vec<(f64, &Map<String, Value>)> = Vec::new();
    for (t, m) in top {
        if threaded(m) {
            let mut g = vec![m];
            let replies = match ch.get("replies") {
                Some(Value::Object(r)) => r,
                _ => return Err(bad("a channel has no replies object")),
            };
            match replies.get(ts_of(m)?.0) {
                None => {}
                Some(Value::Array(rs)) => {
                    for r in rs {
                        let Some(r) = r.as_object() else {
                            return Err(bad("a reply is not an object"));
                        };
                        if kept(r)? {
                            g.push(r);
                        }
                    }
                }
                Some(_) => return Err(bad("a thread's replies are not a list")),
            }
            groups.push(g);
            continue;
        }
        if burst.last().is_some_and(|(last, _)| t - last > BURST_GAP) {
            groups.push(burst.drain(..).map(|(_, m)| m).collect());
        }
        burst.push((t, m));
    }
    if !burst.is_empty() {
        groups.push(burst.into_iter().map(|(_, m)| m).collect());
    }
    for g in groups {
        docs.push(group_document(raw, users, ch, &acl, &g, threaded(g[0]))?);
    }
    Ok(())
}

fn group_document(
    raw: &Value,
    users: &Map<String, Value>,
    ch: &Map<String, Value>,
    acl: &[String],
    g: &[&Map<String, Value>],
    thread: bool,
) -> Result<Doc, SourceError> {
    let team_url = raw
        .get("team_url")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("there is no team_url"))?;
    let id = text_field(ch, "id", "slack.json: a channel has no id")?;
    let name = text_field(ch, "name", "slack.json: a channel has no name")?;
    let link = |ts: &str, root: Option<&str>| {
        let mut url = format!("{team_url}archives/{id}/p{}", ts.replace('.', ""));
        if let Some(root) = root.filter(|r| !r.is_empty() && *r != ts) {
            url.push_str(&format!("?thread_ts={root}&cid={id}"));
        }
        url
    };
    let root = g[0];
    let mut people: Vec<&str> = Vec::new();
    for m in g {
        let who = name_of(users, m.get("user"))?;
        if !people.contains(&who) {
            people.push(who);
        }
    }
    let (root_ts, root_t) = ts_of(root)?;
    let day = when(root_t)?.format("%Y-%m-%d").to_string();
    let root_text = clean(
        text_field(root, "text", "slack.json: a message has no text")?,
        users,
    )?;
    let first = root_text.split('\n').next().unwrap_or("");
    let mut head = format!(
        "slack: #{name} · {day}\nmeta: {} · {}{}",
        if thread { "스레드" } else { "대화" },
        people[..people.len().min(4)].join(", "),
        if people.len() > 4 { " 외" } else { "" }
    );
    if thread {
        head.push_str(&format!(" · 답글 {}개", g.len() - 1));
    }
    let mut body = Vec::new();
    for m in g {
        let who = name_of(users, m.get("user"))?;
        let (ts, t) = ts_of(m)?;
        let text = clean(
            text_field(m, "text", "slack.json: a message has no text")?,
            users,
        )?;
        let mut lines = text.split('\n');
        let url = link(ts, thread.then_some(root_ts));
        let at = when(t)?.format("%H:%M");
        body.push((
            format!("{who} {at}  {}", lines.next().unwrap_or("")),
            url.clone(),
        ));
        body.extend(lines.map(|l| (format!("  {l}"), url.clone())));
    }
    Ok(Doc {
        path: format!("slack/{id}/{root_ts}"),
        title: format!("#{name} · {}", py::take_chars(first, 40)),
        url: link(root_ts, None),
        author: Some(name_of(users, root.get("user"))?.to_string()),
        at: day,
        acl: Some(acl.to_vec()),
        head,
        body,
    })
}

/// 원본 파일을 읽어요. 없으면 None, UTF-8이나 JSON이 아니면 Err예요.
fn read_raw_file(f: &Path) -> Result<Option<Value>, SourceError> {
    if !f.exists() {
        return Ok(None);
    }
    let text = String::from_utf8(fs::read(f)?).map_err(|_| bad("not UTF-8"))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|_| bad("not JSON"))
}

/// `raw_dir/slack.json`이에요. 없으면 None이에요.
pub fn read_raw(raw_dir: &Path) -> Result<Option<Value>, SourceError> {
    read_raw_file(&raw_dir.join("slack.json"))
}

/// 원본 하나의 조각이에요(`src` = "slack").
pub fn chunks_of(raw: &Value) -> Result<Vec<RawChunk>, SourceError> {
    Ok(documents(raw)?
        .iter()
        .flat_map(|d| windows(d, "slack"))
        .collect())
}

/// `raw_dir/slack.json`의 조각이에요. 파일이 없으면 비어요.
pub fn chunks(raw_dir: &Path) -> Result<Vec<RawChunk>, SourceError> {
    match read_raw(raw_dir)? {
        Some(raw) => chunks_of(&raw),
        None => Ok(Vec::new()),
    }
}

/// 슬랙 받기예요(갱신이 부르고, 테스트는 `testing::FakeSlack`을 넣어요).
pub trait SlackFetch: Send + Sync {
    /// `raw/slack.json`을 새로 받아요.
    fn fetch(&self, raw: &Path) -> Result<(), SourceError>;
}

/// 진짜 슬랙이에요: 토큰을 읽고 `SlackHttp`로 지난 `DAYS`일을 받아요.
#[derive(Debug, Clone)]
pub struct SlackSource {
    /// 토큰 파일이 있는 설정 폴더예요(보통 `~/.config`).
    pub config_dir: PathBuf,
    /// Web API 주소예요(보통 `API`).
    pub base: String,
}

impl SlackSource {
    /// `API`로 부르는 슬랙이에요.
    pub fn new(config_dir: PathBuf) -> Self {
        SlackSource {
            config_dir,
            base: API.to_string(),
        }
    }
}

impl SlackFetch for SlackSource {
    fn fetch(&self, raw: &Path) -> Result<(), SourceError> {
        let token = token(&self.config_dir)?;
        fetch_with(&SlackHttp::new(&self.base, &token), raw, DAYS)
    }
}
