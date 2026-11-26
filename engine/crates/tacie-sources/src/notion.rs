//! Notion (docs/13): reads only the pages the user allowed Tacie on the Notion consent screen; one page is one
//! document.
//!
//! What was allowed is the scope: the Notion API returns only allowed pages (and the pages and database rows under
//! them). Tacie is a personal app, so the person who allowed them owns the index. So every chunk's acl is everyone
//! (`["*"]`).
//!
//! - Fetching (`fetch_with`): gets all allowed pages with `POST search`, fetches each page's blocks recursively, and
//!   replaces `notion.json` whole. Pages whose `last_edited_time` equals the saved copy do not refetch their blocks
//!   (incremental). Pages that do not show up in search (access removed or deleted) drop out. If one page fails it
//!   uses the saved copy, and drops it if there is none. If the token is rejected (401) the whole fetch fails with
//!   `Key`.
//! - Calls (`NotionApi`): `NotionHttp` is the real API; tests plug in a fake. To respect Notion's limit of 3 requests
//!   per second on average, calls are spaced `GAP` apart, and on 429 it waits `Retry-After`.
//! - Documents and chunks: blocks become lines (heading `#`, list `-`, to-do `[x]`, code fence, table cells `|` …),
//!   and each line gets a link that opens that block (`페이지 url#블록 id`). Chunks are the same 40-line windows as
//!   GitHub and Slack documents (`doc::windows`).
//!
//! The token lives only in the `Authorization` header (marked sensitive) and never shows in errors, logs or `Debug`.
//! Page text is never put in logs or errors.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, FixedOffset};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderValue, RETRY_AFTER};
use serde_json::{Map, Value, json};
use tacie_index::RawChunk;

use crate::doc::{Doc, windows};
use crate::error::{Kind, SourceError};
use crate::github::now_iso;
use crate::net::net_error;
use crate::store::{Stored, read_json, write_json};

/// 노션 API 주소예요. 테스트는 로컬 가짜를 가리켜요.
pub const API: &str = "https://api.notion.com/v1/";
/// 노션 API 판이에요. 검색이 페이지와 데이터베이스 행을 같은 페이지 모양으로 돌려줘요.
pub const VERSION: &str = "2022-06-28";
/// 호출 사이에 띄우는 시간이에요(노션: 평균 초당 3번).
pub const GAP: Duration = Duration::from_millis(340);
/// 호출 하나를 몇 번까지 하는지예요(429를 기다리며).
pub const TRIES: usize = 6;
/// 요청 하나를 기다리는 시간이에요.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// 블록을 이만큼 깊이까지 따라가요.
pub const DEPTH: usize = 8;
/// 노션의 수정 시각은 분 단위라, 지난 받기 앞뒤로 이만큼 안에 고친 페이지는 시각이 같아도 다시 받아요.
pub const MARGIN_SECS: i64 = 120;
/// 원본 파일 이름이에요(`raw/notion/notion.json`).
pub const FILE: &str = "notion.json";
const UNTITLED: &str = "제목 없음";
const KST_SECS: i32 = 9 * 3600;

/// 노션 API 호출 하나예요: `method`(GET·POST) `path`(`search`, `blocks/{id}/children?…`)에 본문을 보내요.
pub trait NotionApi: Send + Sync {
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, SourceError>;
}

/// 진짜 API예요: `Authorization: Bearer {token}`, `Notion-Version: VERSION`, 30 s.
pub struct NotionHttp {
    base: String,
    /// None이면 토큰을 머리에 실을 수 없어요(줄바꿈 같은 글자).
    auth: Option<HeaderValue>,
    client: Result<Client, String>,
    last: Mutex<Option<Instant>>,
}

impl fmt::Debug for NotionHttp {
    /// 주소만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotionHttp")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl NotionHttp {
    pub fn new(base: &str, token: &str) -> Self {
        let auth = HeaderValue::from_str(&format!("Bearer {token}"))
            .ok()
            .map(|mut h| {
                h.set_sensitive(true);
                h
            });
        // loopback 주소(테스트·계약의 가짜)는 프록시를 쓰지 않아요
        let builder = Client::builder().timeout(TIMEOUT);
        let builder = if crate::is_loopback(base) {
            builder.no_proxy()
        } else {
            builder
        };
        NotionHttp {
            base: base.to_string(),
            auth,
            client: builder.build().map_err(net_error),
            last: Mutex::new(None),
        }
    }

    /// 앞 호출에서 `GAP`이 지날 때까지 기다려요.
    fn pace(&self) {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = *last {
            let since = at.elapsed();
            if since < GAP {
                std::thread::sleep(GAP - since);
            }
        }
        *last = Some(Instant::now());
    }
}

impl NotionApi for NotionHttp {
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, SourceError> {
        let what = path.split(['/', '?']).next().unwrap_or(path);
        let fail = |kind: Kind, m: &str| SourceError::service(kind, format!("notion {what}: {m}"));
        let client = self.client.as_ref().map_err(|e| fail(Kind::Down, e))?;
        let auth = self
            .auth
            .clone()
            .ok_or_else(|| fail(Kind::Key, "the token cannot be sent in a header"))?;
        let url = format!("{}{path}", self.base);
        for _ in 0..TRIES {
            self.pace();
            let mut req = if method == "POST" {
                client.post(&url)
            } else {
                client.get(&url)
            };
            req = req
                .header(AUTHORIZATION, auth.clone())
                .header("Notion-Version", VERSION);
            if let Some(b) = body {
                req = req
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(b.to_string());
            }
            let r = req.send().map_err(|e| fail(Kind::Network, &net_error(e)))?;
            let status = r.status().as_u16();
            if status == 429 {
                let secs = r
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|h| h.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(2)
                    .min(60);
                std::thread::sleep(Duration::from_secs(secs + 1));
                continue;
            }
            let bytes = r.bytes().map_err(|e| fail(Kind::Network, &net_error(e)))?;
            let d: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let code = d["code"].as_str().unwrap_or("").to_string();
            return match status {
                200..=299 => Ok(d),
                401 => Err(fail(Kind::Key, "the connection was refused")),
                403 | 404 => Err(fail(Kind::Down, &format!("{status} {code}"))),
                500..=599 => Err(fail(Kind::Down, &format!("{status} {code}"))),
                _ => Err(fail(Kind::Down, &format!("{status} {code}"))),
            };
        }
        Err(fail(Kind::Busy, "rate limited"))
    }
}

/// 리치 텍스트 배열의 글이에요(`plain_text`를 이어요).
fn plain(v: &Value) -> String {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["plain_text"].as_str())
        .collect()
}

/// 페이지 제목이에요: `title` 종류의 속성이에요.
fn title_of(page: &Value) -> String {
    let t = page["properties"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
        .find(|p| p["type"] == "title")
        .map(|p| plain(&p["title"]))
        .unwrap_or_default();
    let t = t.trim();
    if t.is_empty() {
        UNTITLED.to_string()
    } else {
        t.to_string()
    }
}

/// 데이터베이스 행의 속성 줄이에요(`이름: 값`). 제목과 빈 값은 빼요.
fn property_lines(page: &Value) -> Vec<String> {
    let Some(props) = page["properties"].as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, p) in props {
        let ty = p["type"].as_str().unwrap_or("");
        let v = &p[ty];
        let names = |arr: &Value| -> String {
            arr.as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let text = match ty {
            "rich_text" => plain(v),
            "number" => v.as_f64().map(|n| n.to_string()).unwrap_or_default(),
            "select" | "status" => v["name"].as_str().unwrap_or("").to_string(),
            "multi_select" | "people" => names(v),
            "date" => match (v["start"].as_str(), v["end"].as_str()) {
                (Some(s), Some(e)) => format!("{s} – {e}"),
                (Some(s), None) => s.to_string(),
                _ => String::new(),
            },
            "checkbox" => {
                if v.as_bool() == Some(true) {
                    "예".to_string()
                } else {
                    "아니요".to_string()
                }
            }
            "url" | "email" | "phone_number" => v.as_str().unwrap_or("").to_string(),
            "formula" => match v["type"].as_str() {
                Some("string") => v["string"].as_str().unwrap_or("").to_string(),
                Some("number") => v["number"]
                    .as_f64()
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        let text = text.trim();
        if !text.is_empty() {
            out.push(format!("{name}: {text}"));
        }
    }
    out
}

/// 블록 하나의 줄이에요(들여쓰기 `indent`). 글이 없는 블록(구분선, 표 틀, 단)은 비어요.
fn block_lines(block: &Value, indent: usize) -> Vec<String> {
    let ty = block["type"].as_str().unwrap_or("");
    let d = &block[ty];
    let rich = || plain(&d["rich_text"]);
    let pad = "  ".repeat(indent);
    let one = |s: String| {
        if s.trim().is_empty() {
            Vec::new()
        } else {
            vec![format!("{pad}{s}")]
        }
    };
    match ty {
        "paragraph" | "toggle" => one(rich()),
        "heading_1" => one(format!("# {}", rich())),
        "heading_2" => one(format!("## {}", rich())),
        "heading_3" => one(format!("### {}", rich())),
        "bulleted_list_item" => one(format!("- {}", rich())),
        "numbered_list_item" => one(format!("1. {}", rich())),
        "to_do" => {
            let mark = if d["checked"].as_bool() == Some(true) {
                "[x]"
            } else {
                "[ ]"
            };
            one(format!("{mark} {}", rich()))
        }
        "quote" => one(format!("> {}", rich())),
        "callout" => {
            let icon = d["icon"]["emoji"].as_str().unwrap_or("");
            one(format!("{icon} {}", rich()).trim().to_string())
        }
        "code" => {
            let text = rich();
            if text.trim().is_empty() {
                return Vec::new();
            }
            let lang = d["language"].as_str().unwrap_or("");
            let mut out = vec![format!("{pad}```{lang}")];
            out.extend(text.split('\n').map(|l| format!("{pad}{l}")));
            out.push(format!("{pad}```"));
            out
        }
        "child_page" => one(format!(
            "하위 페이지: {}",
            d["title"].as_str().unwrap_or("")
        )),
        "child_database" => one(format!(
            "데이터베이스: {}",
            d["title"].as_str().unwrap_or("")
        )),
        "table_row" => {
            let cells: Vec<String> = d["cells"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| plain(c).replace('\n', " "))
                .collect();
            one(format!("| {} |", cells.join(" | ")))
        }
        "image" | "file" | "pdf" | "video" | "audio" => one(plain(&d["caption"])),
        "bookmark" | "embed" | "link_preview" => one(d["url"].as_str().unwrap_or("").to_string()),
        "equation" => one(d["expression"].as_str().unwrap_or("").to_string()),
        _ => Vec::new(),
    }
}

/// 노션 링크의 블록 자리예요(`#` 뒤, 하이픈 없는 id).
fn anchor(page_url: &str, block_id: &str) -> String {
    format!("{page_url}#{}", block_id.replace('-', ""))
}

/// 블록 `id` 아래를 모두 받아 (줄, 그 블록 링크)를 더해요. 하위 페이지·데이터베이스 안으로는 들어가지 않아요(그건
/// 그 자체로 검색에 나와요).
fn walk(
    api: &dyn NotionApi,
    id: &str,
    page_url: &str,
    depth: usize,
    out: &mut Vec<(String, String)>,
) -> Result<(), SourceError> {
    let mut cursor: Option<String> = None;
    loop {
        let mut path = format!("blocks/{id}/children?page_size=100");
        if let Some(c) = &cursor {
            path.push_str(&format!("&start_cursor={c}"));
        }
        let d = api.call("GET", &path, None)?;
        for b in d["results"].as_array().into_iter().flatten() {
            let bid = b["id"].as_str().unwrap_or("");
            let url = anchor(page_url, bid);
            for line in block_lines(b, depth) {
                out.push((line, url.clone()));
            }
            let ty = b["type"].as_str().unwrap_or("");
            if b["has_children"].as_bool() == Some(true)
                && depth + 1 < DEPTH
                && ty != "child_page"
                && ty != "child_database"
            {
                walk(api, bid, page_url, depth + 1, out)?;
            }
        }
        match (d["has_more"].as_bool(), d["next_cursor"].as_str()) {
            (Some(true), Some(c)) => cursor = Some(c.to_string()),
            _ => return Ok(()),
        }
    }
}

/// 허용한 페이지 모두예요(검색 결과의 페이지 객체).
fn search_pages(api: &dyn NotionApi) -> Result<Vec<Value>, SourceError> {
    let mut pages = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut body = json!({
            "filter": {"property": "object", "value": "page"},
            "page_size": 100,
        });
        if let Some(c) = &cursor {
            body["start_cursor"] = Value::from(c.as_str());
        }
        let d = api.call("POST", "search", Some(&body))?;
        for p in d["results"].as_array().into_iter().flatten() {
            if p["archived"].as_bool() != Some(true) && p["in_trash"].as_bool() != Some(true) {
                pages.push(p.clone());
            }
        }
        match (d["has_more"].as_bool(), d["next_cursor"].as_str()) {
            (Some(true), Some(c)) => cursor = Some(c.to_string()),
            _ => return Ok(pages),
        }
    }
}

/// 저장본의 페이지예요(id → 저장한 페이지).
fn saved_pages(raw_dir: &Path) -> HashMap<String, Value> {
    match read_raw(raw_dir) {
        Ok(Some(raw)) => raw["pages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| Some((p["id"].as_str()?.to_string(), p.clone())))
            .collect(),
        _ => HashMap::new(),
    }
}

fn lines_value(lines: &[(String, String)]) -> Value {
    Value::Array(
        lines
            .iter()
            .map(|(l, u)| Value::Array(vec![Value::from(l.as_str()), Value::from(u.as_str())]))
            .collect(),
    )
}

/// 허용한 페이지를 받아 `raw_dir/notion.json`을 통째로 바꿔요. 모듈 설명을 보세요.
pub fn fetch_with(
    api: &dyn NotionApi,
    raw_dir: &Path,
    workspace: Option<&str>,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<(), SourceError> {
    let saved = saved_pages(raw_dir);
    let last = read_raw(raw_dir)
        .ok()
        .flatten()
        .and_then(|r| r["fetched_at"].as_str().and_then(seconds));
    let found = search_pages(api)?;
    let (mut fresh, mut reused, mut failed) = (0, 0, 0);
    let mut pages = Vec::new();
    for (i, p) in found.iter().enumerate() {
        progress(i, found.len());
        let Some(id) = p["id"].as_str() else { continue };
        let edited = p["last_edited_time"].as_str().unwrap_or("");
        let url = p["url"].as_str().unwrap_or("");
        // 지난 받기와 같은 분 언저리에 고친 페이지는 그 뒤 고친 것이 같은 시각으로 보일 수 있어요
        let near_last = match (last, seconds(edited)) {
            (Some(l), Some(e)) => e >= l - MARGIN_SECS,
            _ => false,
        };
        if let Some(old) = saved
            .get(id)
            .filter(|o| o["edited"] == edited && !near_last)
        {
            pages.push(old.clone());
            reused += 1;
            continue;
        }
        let mut lines: Vec<(String, String)> = Vec::new();
        if p["parent"]["type"] == "database_id" {
            lines.extend(property_lines(p).into_iter().map(|l| (l, url.to_string())));
        }
        match walk(api, id, url, 0, &mut lines) {
            Ok(()) => {
                fresh += 1;
                pages.push(json!({
                    "id": id,
                    "title": title_of(p),
                    "url": url,
                    "edited": edited,
                    "lines": lines_value(&lines),
                }));
            }
            Err(e) if e.kind() == Kind::Key => return Err(e),
            Err(_) => {
                failed += 1;
                if let Some(old) = saved.get(id) {
                    pages.push(old.clone());
                }
            }
        }
    }
    progress(found.len(), found.len());
    eprintln!(
        "notion: {} pages ({fresh} fetched, {reused} unchanged, {failed} failed)",
        pages.len()
    );
    let raw = json!({
        "fetched_at": now_iso(),
        "workspace": workspace,
        "pages": pages,
    });
    write_json(&raw_dir.join(FILE), &raw, None)?;
    Ok(())
}

/// `raw_dir/notion.json`이에요. 없으면 None이에요.
pub fn read_raw(raw_dir: &Path) -> Result<Option<Value>, SourceError> {
    match read_json(&raw_dir.join(FILE))? {
        Stored::Value(v) => Ok(Some(v)),
        Stored::Missing => Ok(None),
        Stored::Unreadable => Err(SourceError::Json("notion.json is not JSON".to_string())),
    }
}

/// ISO 시각의 유닉스 초예요(노션의 `…Z`, `now_iso`).
fn seconds(iso: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|t| t.timestamp())
}

/// 노션 시각(UTC ISO)의 KST 날짜예요(`YYYY-MM-DD`).
fn kst_date(iso: &str) -> String {
    match DateTime::parse_from_rfc3339(iso) {
        Ok(t) => t
            .with_timezone(&FixedOffset::east_opt(KST_SECS).expect("KST"))
            .format("%Y-%m-%d")
            .to_string(),
        Err(_) => iso.chars().take(10).collect(),
    }
}

/// 저장한 페이지 하나의 문서예요.
pub fn document(page: &Value) -> Doc {
    let id = page["id"].as_str().unwrap_or("");
    let title = page["title"].as_str().unwrap_or(UNTITLED).to_string();
    let url = page["url"].as_str().unwrap_or("").to_string();
    let at = kst_date(page["edited"].as_str().unwrap_or(""));
    let body = page["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| Some((l[0].as_str()?.to_string(), l[1].as_str()?.to_string())))
        .collect();
    Doc {
        path: format!("notion/{id}"),
        head: format!("notion: {title}\nmeta: 노션 · {at} 수정"),
        title,
        url,
        author: None,
        at,
        acl: None,
        body,
    }
}

/// 원본 하나의 조각이에요(`src` = "notion").
pub fn chunks_of(raw: &Value) -> Vec<RawChunk> {
    raw["pages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|p| windows(&document(p), "notion"))
        .collect()
}

/// 노션 받기예요(갱신이 불러요). 연결이 없으면 받지 않고 저장본만 써요.
pub trait NotionFetch: Send + Sync {
    /// 앱이 연결을 넘겼는지예요.
    fn connected(&self) -> bool;
    /// `raw/notion/notion.json`을 새로 받아요. `progress(받은 페이지, 모든 페이지)`예요.
    fn fetch(&self, raw: &Path, progress: &mut dyn FnMut(usize, usize)) -> Result<(), SourceError>;
}

/// 진짜 노션이에요: 앱이 넘긴 토큰(메모리에만)으로 `NotionHttp`를 불러요. 테스트는 `api`에 가짜를 넣어요.
#[derive(Default)]
pub struct NotionSource {
    pub base: String,
    token: RwLock<Option<(String, Option<String>)>>,
    /// 지난 받기에서 노션이 토큰을 거절했어요(앱이 갱신하거나 다시 연결해요). 연결을 바꾸면 잊어요.
    refused: AtomicBool,
    api: Option<Arc<dyn NotionApi>>,
}

impl fmt::Debug for NotionSource {
    /// 주소와 연결 여부만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotionSource")
            .field("base", &self.base)
            .field("connected", &self.connected())
            .finish_non_exhaustive()
    }
}

impl NotionSource {
    pub fn new(base: &str) -> Self {
        NotionSource {
            base: base.to_string(),
            ..Default::default()
        }
    }

    /// 진짜 API 대신 `api`를 부르는 노션이에요(테스트).
    pub fn with_api(api: Arc<dyn NotionApi>) -> Self {
        NotionSource {
            api: Some(api),
            ..Default::default()
        }
    }

    /// 연결을 바꿔요(토큰, 워크스페이스 이름). None이면 연결을 끊어요. 바뀌었으면 참이에요.
    pub fn set(&self, token: Option<&str>, workspace: Option<&str>) -> bool {
        let new = token.map(|t| (t.to_string(), workspace.map(str::to_string)));
        let mut cur = self.token.write().unwrap_or_else(PoisonError::into_inner);
        let changed = *cur != new;
        if changed {
            self.refused.store(false, Ordering::SeqCst);
        }
        *cur = new;
        changed
    }

    /// 지난 받기에서 노션이 토큰을 거절했는지예요.
    pub fn refused(&self) -> bool {
        self.refused.load(Ordering::SeqCst)
    }

    /// 연결된 워크스페이스 이름이에요.
    pub fn workspace(&self) -> Option<String> {
        self.token
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|(_, w)| w.clone())
    }
}

impl NotionFetch for NotionSource {
    fn connected(&self) -> bool {
        self.token
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn fetch(&self, raw: &Path, progress: &mut dyn FnMut(usize, usize)) -> Result<(), SourceError> {
        let Some((token, workspace)) = self
            .token
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        else {
            return Err(SourceError::service(Kind::Key, "notion: not connected"));
        };
        let got = match &self.api {
            Some(api) => fetch_with(api.as_ref(), raw, workspace.as_deref(), progress),
            None => fetch_with(
                &NotionHttp::new(&self.base, &token),
                raw,
                workspace.as_deref(),
                progress,
            ),
        };
        if matches!(&got, Err(e) if e.kind() == Kind::Key) {
            self.refused.store(true, Ordering::SeqCst);
        }
        got
    }
}

/// 갱신의 노션 몫이에요: 연결돼 있으면(그리고 `fetch`거나 저장본이 없으면) 받고, 저장본의 조각과 info
/// (`{workspace, fetched_at, pages, chunks}`)를 돌려줘요. 저장본이 없으면 둘 다 비어요. 받지 못했는데 저장본이
/// 있으면 경고 `notion`을 남기고 저장본으로 가요.
pub fn gather(
    source: &dyn NotionFetch,
    raw_dir: &Path,
    fetch: bool,
    progress: &mut dyn FnMut(usize, usize),
    warnings: &mut Vec<String>,
) -> Result<(Vec<RawChunk>, Option<Value>), SourceError> {
    let file = raw_dir.join(FILE);
    if source.connected() && (fetch || !file.exists()) {
        crate::gather::fetch_or_saved(
            "notion",
            || source.fetch(raw_dir, progress),
            || file.exists(),
            warnings,
        )?;
    }
    let Some(raw) = read_raw(raw_dir)? else {
        return Ok((Vec::new(), None));
    };
    let chunks = chunks_of(&raw);
    let mut info = Map::new();
    info.insert("workspace".into(), raw["workspace"].clone());
    info.insert("fetched_at".into(), raw["fetched_at"].clone());
    info.insert(
        "pages".into(),
        Value::from(raw["pages"].as_array().map_or(0, Vec::len)),
    );
    info.insert("chunks".into(), Value::from(chunks.len()));
    Ok((chunks, Some(Value::Object(info))))
}

/// 받은 지 `secs`초가 지났거나 받은 적이 없는지예요(`/index/check`가 노션을 받을 때인지 봐요).
pub fn due(raw_dir: &Path, secs: i64) -> bool {
    let last = read_raw(raw_dir)
        .ok()
        .flatten()
        .and_then(|r| r["fetched_at"].as_str().and_then(seconds));
    match last {
        Some(l) => chrono::Utc::now().timestamp() - l >= secs,
        None => true,
    }
}

/// 연결을 끊을 때 받은 노션 자료를 지워요. 다음 갱신이 노션 조각을 빼요.
pub fn forget(raw_dir: &Path) -> std::io::Result<()> {
    match fs::remove_file(raw_dir.join(FILE)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 페이지와 블록을 미리 넣은 가짜 노션이에요. 받은 호출을 적어요.
    #[derive(Default)]
    struct Fake {
        pages: Vec<Value>,
        children: HashMap<String, Vec<Value>>,
        refuse: bool,
        broken: Vec<String>,
        calls: Mutex<Vec<String>>,
    }

    impl NotionApi for Fake {
        fn call(&self, method: &str, path: &str, _b: Option<&Value>) -> Result<Value, SourceError> {
            self.calls.lock().unwrap().push(format!("{method} {path}"));
            if self.refuse {
                return Err(SourceError::service(Kind::Key, "notion search: refused"));
            }
            if path == "search" {
                return Ok(json!({"results": self.pages, "has_more": false}));
            }
            let id = path
                .trim_start_matches("blocks/")
                .split('/')
                .next()
                .unwrap();
            if self.broken.iter().any(|b| b == id) {
                return Err(SourceError::service(Kind::Down, "notion blocks: 404"));
            }
            Ok(
                json!({"results": self.children.get(id).cloned().unwrap_or_default(), "has_more": false}),
            )
        }
    }

    fn text(s: &str) -> Value {
        json!([{"plain_text": s}])
    }

    fn page(id: &str, title: &str, edited: &str) -> Value {
        json!({"object": "page", "id": id, "url": format!("https://www.notion.so/{id}"),
               "last_edited_time": edited, "parent": {"type": "workspace"},
               "properties": {"Name": {"type": "title", "title": text(title)}}})
    }

    fn block(id: &str, ty: &str, data: Value, kids: bool) -> Value {
        json!({"id": id, "type": ty, ty: data, "has_children": kids})
    }

    fn fake() -> Fake {
        let mut f = Fake {
            pages: vec![page("p-1", "재시도 정책", "2026-01-01T01:00:00.000Z")],
            ..Default::default()
        };
        f.children.insert(
            "p-1".into(),
            vec![
                block(
                    "b-1",
                    "heading_2",
                    json!({"rich_text": text("재시도")}),
                    false,
                ),
                block(
                    "b-2",
                    "paragraph",
                    json!({"rich_text": text("기본값은 3번이에요.")}),
                    false,
                ),
                block("b-3", "toggle", json!({"rich_text": text("더 보기")}), true),
                block(
                    "b-5",
                    "code",
                    json!({"rich_text": text("retry = 3\nbackoff = 2"), "language": "toml"}),
                    false,
                ),
            ],
        );
        f.children.insert(
            "b-3".into(),
            vec![block(
                "b-4",
                "to_do",
                json!({"rich_text": text("실패 큐 보기"), "checked": true}),
                false,
            )],
        );
        f
    }

    #[test]
    fn pages_become_documents_with_block_links() {
        let dir = tempfile::tempdir().unwrap();
        let f = fake();
        fetch_with(&f, dir.path(), Some("팀"), &mut |_, _| {}).unwrap();
        let raw = read_raw(dir.path()).unwrap().unwrap();
        assert_eq!(raw["workspace"], "팀");
        let doc = document(&raw["pages"][0]);
        assert_eq!(doc.path, "notion/p-1");
        assert_eq!(doc.title, "재시도 정책");
        assert_eq!(doc.at, "2026-01-01");
        let lines: Vec<&str> = doc.body.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(
            lines,
            [
                "## 재시도",
                "기본값은 3번이에요.",
                "더 보기",
                "  [x] 실패 큐 보기",
                "```toml",
                "retry = 3",
                "backoff = 2",
                "```"
            ]
        );
        assert_eq!(doc.body[1].1, "https://www.notion.so/p-1#b2");
        let chunks = chunks_of(&raw);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["src"], "notion");
        assert_eq!(chunks[0]["acl"], json!(["*"]));
        assert!(
            chunks[0]["text"]
                .as_str()
                .unwrap()
                .starts_with("notion: 재시도 정책\nmeta: 노션 · 2026-01-01 수정\n## 재시도")
        );
    }

    #[test]
    fn unchanged_pages_are_not_fetched_again() {
        let dir = tempfile::tempdir().unwrap();
        fetch_with(&fake(), dir.path(), None, &mut |_, _| {}).unwrap();
        let mut f = fake();
        f.pages.push(page("p-2", "", "2026-01-01T02:00:00.000Z"));
        fetch_with(&f, dir.path(), None, &mut |_, _| {}).unwrap();
        let calls = f.calls.lock().unwrap();
        assert!(calls.iter().all(|c| !c.contains("blocks/p-1")), "{calls:?}");
        assert!(calls.iter().any(|c| c.contains("blocks/p-2")));
        let raw = read_raw(dir.path()).unwrap().unwrap();
        assert_eq!(raw["pages"][1]["title"], UNTITLED);
    }

    #[test]
    fn pages_edited_around_the_last_fetch_are_fetched_again() {
        let dir = tempfile::tempdir().unwrap();
        fetch_with(&fake(), dir.path(), None, &mut |_, _| {}).unwrap();
        // 지난 받기가 그 페이지를 고친 바로 그 분이었어요: 같은 분에 또 고쳤을 수 있어요
        let mut raw = read_raw(dir.path()).unwrap().unwrap();
        raw["fetched_at"] = json!("2026-01-01T01:00:30Z");
        write_json(&dir.path().join(FILE), &raw, None).unwrap();
        let f = fake();
        let mut seen = Vec::new();
        fetch_with(&f, dir.path(), None, &mut |d, t| seen.push((d, t))).unwrap();
        assert!(
            f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.contains("blocks/p-1"))
        );
        assert_eq!(seen, [(0, 1), (1, 1)]);
    }

    #[test]
    fn pages_no_longer_allowed_drop_out() {
        let dir = tempfile::tempdir().unwrap();
        fetch_with(&fake(), dir.path(), None, &mut |_, _| {}).unwrap();
        let f = Fake::default();
        fetch_with(&f, dir.path(), None, &mut |_, _| {}).unwrap();
        assert!(chunks_of(&read_raw(dir.path()).unwrap().unwrap()).is_empty());
    }

    #[test]
    fn a_broken_page_keeps_its_saved_copy_and_a_refusal_fails() {
        let dir = tempfile::tempdir().unwrap();
        fetch_with(&fake(), dir.path(), None, &mut |_, _| {}).unwrap();
        let mut f = fake();
        f.pages[0]["last_edited_time"] = json!("2026-01-01T03:00:00.000Z");
        f.broken.push("p-1".into());
        fetch_with(&f, dir.path(), None, &mut |_, _| {}).unwrap();
        assert_eq!(chunks_of(&read_raw(dir.path()).unwrap().unwrap()).len(), 1);

        let refused = Fake {
            refuse: true,
            ..Default::default()
        };
        let e = fetch_with(&refused, dir.path(), None, &mut |_, _| {}).unwrap_err();
        assert_eq!(e.kind(), Kind::Key);
    }

    #[test]
    fn database_rows_carry_their_properties() {
        let mut p = page("r-1", "결제 장애", "2026-01-01T01:00:00.000Z");
        p["parent"] = json!({"type": "database_id", "database_id": "db"});
        p["properties"]["상태"] = json!({"type": "status", "status": {"name": "해결"}});
        p["properties"]["담당"] = json!({"type": "people", "people": [{"name": "도현"}]});
        p["properties"]["메모"] = json!({"type": "rich_text", "rich_text": []});
        let lines = property_lines(&p);
        assert_eq!(lines, ["상태: 해결", "담당: 도현"]);
    }

    struct Off;
    impl NotionFetch for Off {
        fn connected(&self) -> bool {
            false
        }
        fn fetch(&self, _: &Path, _: &mut dyn FnMut(usize, usize)) -> Result<(), SourceError> {
            panic!("not connected: no fetch")
        }
    }

    #[test]
    fn without_a_connection_the_saved_copy_is_still_searched_until_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let mut warnings = Vec::new();
        assert!(
            gather(&Off, dir.path(), true, &mut |_, _| {}, &mut warnings)
                .unwrap()
                .1
                .is_none()
        );
        fetch_with(&fake(), dir.path(), Some("팀"), &mut |_, _| {}).unwrap();
        let (chunks, info) = gather(&Off, dir.path(), true, &mut |_, _| {}, &mut warnings).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(info.unwrap()["pages"], 1);
        forget(dir.path()).unwrap();
        assert!(
            gather(&Off, dir.path(), true, &mut |_, _| {}, &mut warnings)
                .unwrap()
                .0
                .is_empty()
        );
        assert!(warnings.is_empty());
    }
}
