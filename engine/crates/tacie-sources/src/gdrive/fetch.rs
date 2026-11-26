//! Fetching from Google Drive (docs/19 10절, 15-1절).
//!
//! - There is one scope for "My Drive" (`my`) and one for each shared drive I belong to (`drive:<id>`). Each scope
//!   keeps its own Changes token and whether its initial fetch is finished (`done`).
//! - Initial fetch (scopes not `done`): gets a new Changes token **first**, fetches the listing to the end, then
//!   exports only files whose `modifiedTime` differs from the saved copy. Each run does only up to `Budget` (file
//!   count, time); if unfinished, it reports `more` and the next sync continues. Only when finished is it `done` (so
//!   it does not move to change-only fetching after stopping midway and miss the remaining files).
//! - Changes (scopes that are `done`): processes `changes.list` page by page from the saved token and saves the token.
//!   If a batch fills up before a page is finished, that page's token is not saved (looking again next time, files
//!   already fetched have the same `modifiedTime` and are skipped).
//!   If the token is rejected (400, 404, 410), that scope's initial fetch runs again.
//! - Adding and change checks use the same test (`in_scope`): a fetched type, not in the trash, within 2 years; and
//!   for My Drive no `driveId` and (I am the owner or it is not a Shared with me file), for a shared drive that
//!   `driveId`.
//! - The shared drive list is fetched every sync. Files of drives that disappeared are deleted (nothing is deleted if
//!   the list is empty or more than 20% (at least 5) go missing). Muted drives are not fetched and their raw data is
//!   kept.
//! - Every 7 days each scope's listing is fetched again and matched against the saved copy (same 20% guard).
//! - One file failing: 404 and other 403 permission errors delete it. `exportSizeLimitExceeded`, `cannotExportFile`,
//!   `canDownload` false and other 4xx keep only the title (`blocked`). Docs over 10MB are fetched once more as
//!   `text/plain`.
//! - PDF, Word, PowerPoint and Excel files (issue #31) are downloaded as originals (`alt=media`) instead of exported,
//!   and `extract` pulls the text out. Files larger than `extract::MAX_BYTES` (20MB), with no text (scans) or
//!   unreadable keep only the title. Excel uses only the first `SHEET_LINES` lines, like Sheets.
//! - Re-export happens only when `modifiedTime` changed (`version` goes up even when only sharing settings or the name
//!   change).
//! - When `still()` is false (the connection changed) it writes no more files and stops with `Busy`.
//!
//! Content (titles, bodies) is never put in logs or errors.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{Kind, SourceError};
use crate::extract::{self, FileKind};
use crate::gdrive::api::{GDriveApi, Got};
use crate::store::{Stored, read_json, write_json};

pub(crate) const WINDOW_DAYS: i64 = 730;
pub(crate) const RECONCILE_DAYS: i64 = 7;
pub(crate) const STATE: &str = "state.json";
pub(crate) const ITEMS: &str = "items";
/// 시트는 앞 이만큼의 줄만 써요(docs/19 14절).
pub(crate) const SHEET_LINES: usize = 2000;
/// 동기화 한 번에 export하는 파일 수와 시간이에요(15-1절).
pub const BATCH_FILES: usize = 500;
pub const BATCH_TIME: StdDuration = StdDuration::from_secs(10 * 60);
pub(crate) const MY: &str = "my";

pub(crate) const DOC: &str = "application/vnd.google-apps.document";
pub(crate) const SLIDES: &str = "application/vnd.google-apps.presentation";
pub(crate) const SHEET: &str = "application/vnd.google-apps.spreadsheet";
/// 원본을 받아 글자를 뽑는 종류예요(`extract`).
pub(crate) const PDF: &str = "application/pdf";
pub(crate) const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
pub(crate) const PPTX: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation";
pub(crate) const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

const FILE_FIELDS: &str = "id,name,mimeType,driveId,modifiedTime,version,size,webViewLink,ownedByMe,sharedWithMeTime,trashed,capabilities(canDownload),lastModifyingUser(displayName)";

/// 범위 하나의 상태예요.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Scope {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub done: bool,
}

/// `state.json`이에요.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct State {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub scopes: BTreeMap<String, Scope>,
    /// 내가 속한 공유 드라이브예요: `{id, name}`.
    #[serde(default)]
    pub drives: Vec<Value>,
    #[serde(default)]
    pub muted: Vec<String>,
    #[serde(default)]
    pub lost_since: Option<String>,
    #[serde(default)]
    pub reconciled_at: Option<String>,
    #[serde(default)]
    pub fetched_at: Option<String>,
    /// 받는 종류의 판이에요(`KINDS`). 이보다 낮으면 범위마다 처음 받기를 다시 해요.
    #[serde(default)]
    pub kinds: u32,
}

/// 받는 종류의 판이에요. 1: Docs·Slides·Sheets, 2: PDF·Word·PowerPoint·Excel을 더했어요(이슈 #31). 종류가 늘면
/// 올려요: 이미 받은 범위는 바뀐 것만 받아서, 그대로면 예전부터 있던 새 종류 파일을 영영 받지 않아요(7일 재조정은
/// 지우기만 해요). 다시 하는 처음 받기는 `modifiedTime`이 같은 파일을 다시 export하지 않아요.
pub(crate) const KINDS: u32 = 2;

impl State {
    /// 모든 범위의 처음 받기를 끝냈는지예요.
    pub fn initial_done(&self) -> bool {
        !self.scopes.is_empty() && self.scopes.values().all(|s| s.done)
    }
}

/// `state.json`, `items/`와 지우기를 함께 막는 자물쇠예요(Linear와 같아요). 엔진에 Drive 원본은 하나라 프로세스에
/// 하나예요.
static STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn state_lock() -> std::sync::MutexGuard<'static, ()> {
    STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn iso(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(crate) fn parse(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

pub(crate) fn load_state(dir: &Path) -> State {
    match read_json(&dir.join(STATE)) {
        Ok(Stored::Value(v)) => serde_json::from_value(v).unwrap_or_default(),
        _ => State::default(),
    }
}

pub(crate) fn save_state(dir: &Path, s: &State) -> Result<(), SourceError> {
    fs::create_dir_all(dir)?;
    let mut s = s.clone();
    s.version = 1;
    write_json(
        &dir.join(STATE),
        &serde_json::to_value(&s).map_err(|e| SourceError::Json(e.to_string()))?,
        None,
    )?;
    Ok(())
}

/// Drive id는 영문·숫자·`-`·`_`예요. 그 밖의 글자가 있으면 파일 이름으로 쓰지 않아요.
pub(crate) fn safe(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn item_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(ITEMS).join(format!("{id}.json"))
}

pub(crate) fn read_item(dir: &Path, id: &str) -> Option<Value> {
    if !safe(id) {
        return None;
    }
    match read_json(&item_path(dir, id)) {
        Ok(Stored::Value(v)) => Some(v),
        _ => None,
    }
}

/// 저장된 파일 id예요.
pub(crate) fn item_ids(dir: &Path) -> Vec<String> {
    let mut ids: Vec<String> = fs::read_dir(dir.join(ITEMS))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(str::to_string)
        })
        .collect();
    ids.sort();
    ids
}

fn scope_key(drive: Option<&str>) -> String {
    drive.map_or_else(|| MY.to_string(), |d| format!("drive:{d}"))
}

fn drive_of(scope: &str) -> Option<&str> {
    scope.strip_prefix("drive:")
}

/// 받는 종류의 export 형식이에요.
pub(crate) fn export_mime(mime: &str) -> Option<&'static str> {
    match mime {
        DOC => Some("text/markdown"),
        SLIDES => Some("text/plain"),
        SHEET => Some("text/csv"),
        _ => None,
    }
}

/// 받는 종류인지예요: export하는 Google 문서이거나 글자를 뽑는 파일이에요.
pub(crate) fn takes(mime: &str) -> bool {
    export_mime(mime).is_some() || FileKind::of_mime(mime).is_some()
}

/// 이 파일이 이 범위에서 받을 것인지예요. 넣을 때와 변경을 볼 때 같이 써요.
pub(crate) fn in_scope(f: &Value, scope: &str, floor: DateTime<Utc>) -> bool {
    let kind = f["mimeType"].as_str().is_some_and(takes);
    let fresh = f["modifiedTime"]
        .as_str()
        .and_then(parse)
        .is_some_and(|m| m > floor);
    let trashed = f["trashed"].as_bool() == Some(true);
    let place = match drive_of(scope) {
        Some(d) => f["driveId"].as_str() == Some(d),
        None => {
            f["driveId"].as_str().is_none()
                && (f["ownedByMe"].as_bool() == Some(true) || f["sharedWithMeTime"].is_null())
        }
    };
    kind && fresh && !trashed && place
}

/// export 한 번의 몫이에요.
#[derive(Debug, Clone)]
pub struct Budget {
    left: usize,
    until: Instant,
}

impl Budget {
    pub fn new(files: usize, time: StdDuration) -> Self {
        Budget {
            left: files,
            until: Instant::now() + time,
        }
    }

    pub(crate) fn spent(&self) -> bool {
        self.left == 0 || Instant::now() >= self.until
    }

    pub(crate) fn take(&mut self) {
        self.left = self.left.saturating_sub(1);
    }
}

/// 파일 하나를 넣은 결과예요.
#[derive(Debug, PartialEq, Eq)]
enum Put {
    Done,
    /// 받을 몫이 다 찼어요. 다음 동기화가 받아요.
    Later,
}

/// export의 결과예요.
enum Export {
    Text(String),
    /// 제목만 남겨요: 이유(`exportSizeLimitExceeded` 같은 것).
    Blocked(String),
    /// 지워졌거나 볼 수 없어요.
    Gone,
}

static MD_IMAGE_DEF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\[image\d+\]:\s*<?data:[^\n]*\n?").expect("image def"));
static MD_IMAGE_USE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"!\[[^\]\n]*\](\[image\d+\]|\(data:[^)\n]*\))").expect("image use")
});

/// BOM과 CRLF를 정리해요.
pub(crate) fn clean(text: &str) -> String {
    text.trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

/// Docs Markdown의 그림을 지워요: 본문의 `![][imageN]`과 끝의 `[imageN]: <data:…>` 줄이에요(docs/19 7-1절).
pub(crate) fn strip_images(md: &str) -> String {
    let out = MD_IMAGE_DEF.replace_all(md, "");
    MD_IMAGE_USE.replace_all(&out, "").into_owned()
}

static MD_ESCAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\([\\`*_{}\[\]()#+\-.!|>~<])").expect("md escape"));

/// Docs Markdown의 이스케이프(`\#`, `\-`, `\*` …)를 풀고 줄 끝 공백(Markdown 줄바꿈 표시)을 지워요. 사람이 `#`을 글자로
/// 쓴 줄도 제목처럼 읽혀서 그 자리에서 조각이 나뉘어요(검색에는 그 편이 나아요).
pub(crate) fn unescape_md(md: &str) -> String {
    md.split('\n')
        .map(|l| MD_ESCAPE.replace_all(l.trim_end(), "$1").into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 시트 CSV의 앞 `SHEET_LINES`줄이에요.
pub(crate) fn head_lines(text: &str, n: usize) -> String {
    text.split('\n').take(n).collect::<Vec<_>>().join("\n")
}

/// 받기 진행이에요: (export한 파일, 받을 파일).
struct Progress<'p> {
    done: usize,
    total: usize,
    report: &'p mut dyn FnMut(usize, usize),
}

impl Progress<'_> {
    fn need(&mut self, n: usize) {
        self.total += n;
        (self.report)(self.done, self.total.max(1));
    }
    fn one(&mut self) {
        self.done += 1;
        (self.report)(self.done.min(self.total), self.total.max(1));
    }
}

struct Run<'a> {
    api: &'a dyn GDriveApi,
    dir: &'a Path,
    still: &'a dyn Fn() -> bool,
    /// 지금 연결의 계정 id예요. 저장본이 다른 계정 것이면 쓰지 않아요.
    account: Option<&'a str>,
    floor: DateTime<Utc>,
}

/// 토큰 거절이에요(범위의 처음 받기를 다시 해요).
struct TokenRejected;

impl Run<'_> {
    fn guard(&self) -> Result<(), SourceError> {
        if (self.still)() {
            Ok(())
        } else {
            Err(SourceError::service(
                Kind::Busy,
                "gdrive: connection changed",
            ))
        }
    }

    fn write(&self, id: &str, v: &Value) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        fs::create_dir_all(self.dir.join(ITEMS))?;
        write_json(&item_path(self.dir, id), v, None)?;
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<(), SourceError> {
        if !safe(id) {
            return Ok(());
        }
        let _held = state_lock();
        self.guard()?;
        match fs::remove_file(item_path(self.dir, id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// 받기의 상태를 써요. 끈 드라이브와 `lost_since`는 다른 쪽(앱의 연결, 갱신)이 받는 도중에 바꿀 수 있어서 디스크의
    /// 것을 그대로 둬요.
    fn save(&self, s: &State) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        let disk = load_state(self.dir);
        if let (Some(ours), Some(theirs)) = (self.account, disk.account_id.as_deref())
            && ours != theirs
        {
            return Err(SourceError::service(
                Kind::Busy,
                "gdrive: the saved files are another account's",
            ));
        }
        let mut s = s.clone();
        if s.account_id.is_none() {
            s.account_id = disk.account_id.clone().or(self.account.map(str::to_string));
        }
        s.muted = disk.muted;
        s.lost_since = disk.lost_since;
        save_state(self.dir, &s)
    }

    fn json(&self, path: &str, q: &[(&str, String)], what: &str) -> Result<Value, SourceError> {
        self.api.get(path, q)?.json(what)
    }

    /// 내가 속한 공유 드라이브를 끝까지 받아요.
    fn drives(&self) -> Result<Vec<Value>, SourceError> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut q = vec![
                ("pageSize", "100".to_string()),
                ("fields", "nextPageToken,drives(id,name)".to_string()),
            ];
            if let Some(p) = &page {
                q.push(("pageToken", p.clone()));
            }
            let d = self.json("drives", &q, "drives")?;
            for v in d["drives"].as_array().into_iter().flatten() {
                if let Some(id) = v["id"].as_str().filter(|i| safe(i)) {
                    out.push(json!({"id": id, "name": v["name"].as_str().unwrap_or("")}));
                }
            }
            page = d["nextPageToken"].as_str().map(str::to_string);
            if page.is_none() {
                return Ok(out);
            }
        }
    }

    fn start_token(&self, scope: &str) -> Result<String, SourceError> {
        let mut q = vec![("supportsAllDrives", "true".to_string())];
        if let Some(d) = drive_of(scope) {
            q.push(("driveId", d.to_string()));
        }
        let d = self.json("changes/startPageToken", &q, "start token")?;
        d["startPageToken"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| SourceError::service(Kind::Down, "gdrive: no start token"))
    }

    /// 범위의 파일 목록(2년 안, 받는 종류, 휴지통 아님)을 끝까지 받아요. 내 드라이브는 `in_scope`로 한 번 더 걸러요.
    fn list(&self, scope: &str) -> Result<Vec<Value>, SourceError> {
        let kinds = [DOC, SLIDES, SHEET, PDF, DOCX, PPTX, XLSX]
            .iter()
            .map(|m| format!("mimeType = '{m}'"))
            .collect::<Vec<_>>()
            .join(" or ");
        let q = format!(
            "trashed = false and modifiedTime > '{}' and ({kinds})",
            self.floor.format("%Y-%m-%dT%H:%M:%S")
        );
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut p = vec![
                ("q", q.clone()),
                ("pageSize", "1000".to_string()),
                ("fields", format!("nextPageToken,files({FILE_FIELDS})")),
            ];
            match drive_of(scope) {
                Some(d) => {
                    p.push(("corpora", "drive".to_string()));
                    p.push(("driveId", d.to_string()));
                    p.push(("includeItemsFromAllDrives", "true".to_string()));
                    p.push(("supportsAllDrives", "true".to_string()));
                }
                None => p.push(("corpora", "user".to_string())),
            }
            if let Some(t) = &page {
                p.push(("pageToken", t.clone()));
            }
            let d = self.json("files", &p, "list")?;
            out.extend(
                d["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|f| in_scope(f, scope, self.floor))
                    .cloned(),
            );
            page = d["nextPageToken"].as_str().map(str::to_string);
            if page.is_none() {
                return Ok(out);
            }
        }
    }

    /// 파일 하나의 글이에요.
    fn export(&self, f: &Value, id: &str) -> Result<Export, SourceError> {
        let mime = f["mimeType"].as_str().unwrap_or("");
        if f["capabilities"]["canDownload"].as_bool() == Some(false) && takes(mime) {
            return Ok(Export::Blocked("canDownload".into()));
        }
        if let Some(kind) = FileKind::of_mime(mime) {
            return self.download(f, id, kind);
        }
        let Some(to) = export_mime(mime) else {
            return Ok(Export::Gone);
        };
        let path = format!("files/{id}/export");
        let first = self.api.get(&path, &[("mimeType", to.to_string())])?;
        let got = match first {
            Got::Fail { reason, .. } if mime == DOC && reason == "exportSizeLimitExceeded" => self
                .api
                .get(&path, &[("mimeType", "text/plain".to_string())])?,
            g => g,
        };
        Ok(match got {
            Got::Body(b) => {
                let t = clean(&String::from_utf8_lossy(&b));
                Export::Text(match mime {
                    DOC => strip_images(&t),
                    SHEET => head_lines(&t, SHEET_LINES),
                    _ => t,
                })
            }
            Got::Fail { status: 404, .. } => Export::Gone,
            Got::Fail {
                status: 403,
                reason,
            } if !matches!(
                reason.as_str(),
                "exportSizeLimitExceeded" | "cannotExportFile" | "cannotDownloadFile"
            ) =>
            {
                Export::Gone
            }
            Got::Fail { reason, status } => Export::Blocked(if reason.is_empty() {
                status.to_string()
            } else {
                reason
            }),
        })
    }

    /// PDF·Office 파일 하나의 글이에요: 원본을 받아 글자를 뽑아요. 실패는 그 파일의 제목만 남겨요.
    fn download(&self, f: &Value, id: &str, kind: FileKind) -> Result<Export, SourceError> {
        let size = f["size"].as_str().and_then(|s| s.parse::<usize>().ok());
        if size.is_some_and(|n| n > extract::MAX_BYTES) {
            return Ok(Export::Blocked(extract::Fail::TooLarge.reason().into()));
        }
        let got = self.api.get(
            &format!("files/{id}"),
            &[("alt", "media".to_string()), ("supportsAllDrives", "true".to_string())],
        )?;
        Ok(match got {
            Got::Body(b) => match extract::text(b, kind) {
                Ok(t) if kind == FileKind::Xlsx => Export::Text(head_lines(&t, SHEET_LINES)),
                Ok(t) => Export::Text(t),
                Err(e) => Export::Blocked(e.reason().into()),
            },
            Got::Fail { status: 404, .. } => Export::Gone,
            Got::Fail { status: 403, reason } if reason != "cannotDownloadFile" => Export::Gone,
            Got::Fail { reason, status } => Export::Blocked(if reason.is_empty() {
                status.to_string()
            } else {
                reason
            }),
        })
    }

    /// 범위 안의 파일 하나를 넣어요: 저장본과 `modifiedTime`이 같으면 정보만 고치고, 다르면 export해요.
    fn put(
        &self,
        f: &Value,
        scope: &str,
        budget: &mut Budget,
        progress: &mut Progress<'_>,
    ) -> Result<Put, SourceError> {
        let Some(id) = f["id"].as_str().filter(|i| safe(i)) else {
            return Ok(Put::Done);
        };
        let old = read_item(self.dir, id);
        let mut v = json!({
            "id": id,
            "name": f["name"].as_str().unwrap_or(""),
            "mimeType": f["mimeType"],
            "scope": scope,
            "driveId": f["driveId"],
            "modifiedTime": f["modifiedTime"],
            "version": f["version"],
            "webViewLink": f["webViewLink"],
            "who": f["lastModifyingUser"]["displayName"],
        });
        if let Some(o) = &old
            && o["modifiedTime"] == f["modifiedTime"]
        {
            v["text"] = o["text"].clone();
            v["blocked"] = o["blocked"].clone();
            if *o != v {
                self.write(id, &v)?;
            }
            return Ok(Put::Done);
        }
        if budget.spent() {
            return Ok(Put::Later);
        }
        budget.take();
        let got = self.export(f, id)?;
        progress.one();
        match got {
            Export::Text(t) => {
                v["text"] = Value::String(t);
                v["blocked"] = Value::Null;
            }
            Export::Blocked(why) => {
                eprintln!("gdrive: a file kept by title only ({why})");
                v["text"] = Value::Null;
                v["blocked"] = Value::String(why);
            }
            Export::Gone => return self.remove(id).map(|()| Put::Done),
        }
        self.write(id, &v)?;
        Ok(Put::Done)
    }

    /// 저장본에서 이 범위의 파일 id예요.
    fn stored_in(&self, scope: &str) -> Vec<String> {
        item_ids(self.dir)
            .into_iter()
            .filter(|id| read_item(self.dir, id).is_some_and(|v| v["scope"] == scope))
            .collect()
    }

    /// 목록에 없는 저장본을 지워요. 목록이 비었거나 너무 많이 빠지면 지우지 않고 거짓이에요.
    fn prune(&self, scope: &str, listed: &[Value]) -> Result<bool, SourceError> {
        let remote: HashSet<&str> = listed.iter().filter_map(|f| f["id"].as_str()).collect();
        let stored = self.stored_in(scope);
        let missing: Vec<&String> = stored
            .iter()
            .filter(|id| !remote.contains(id.as_str()))
            .collect();
        if missing.is_empty() {
            return Ok(true);
        }
        if remote.is_empty() || missing.len() as f64 > (stored.len() as f64 * 0.2).max(5.0) {
            eprintln!(
                "gdrive: prune skipped: {} of {} missing",
                missing.len(),
                stored.len()
            );
            return Ok(false);
        }
        for id in missing {
            self.remove(id)?;
        }
        Ok(true)
    }

    /// 범위의 처음 받기예요. 다 받았으면 `done`이에요.
    fn initial(
        &self,
        s: &mut State,
        scope: &str,
        budget: &mut Budget,
        progress: &mut Progress<'_>,
    ) -> Result<bool, SourceError> {
        let token = self.start_token(scope)?;
        let files = self.list(scope)?;
        {
            // 목록까지 받았으면 저장본이 있는 것으로 쳐요(첫 묶음이 아무것도 export하지 못했어도 진행을 보여요)
            let _held = state_lock();
            self.guard()?;
            fs::create_dir_all(self.dir.join(ITEMS))?;
        }
        self.prune(scope, &files)?;
        let need = files
            .iter()
            .filter(|f| {
                f["id"]
                    .as_str()
                    .and_then(|id| read_item(self.dir, id))
                    .is_none_or(|o| o["modifiedTime"] != f["modifiedTime"])
            })
            .count();
        progress.need(need);
        let mut later = false;
        for f in &files {
            if self.put(f, scope, budget, progress)? == Put::Later {
                later = true;
            }
        }
        let done = !later;
        s.scopes.insert(
            scope.to_string(),
            Scope {
                token: Some(token),
                done,
            },
        );
        self.save(s)?;
        Ok(done)
    }

    fn changes_query(scope: &str, token: &str, fields: &str) -> Vec<(&'static str, String)> {
        let mut q = vec![
            ("pageToken", token.to_string()),
            ("pageSize", "1000".to_string()),
            ("includeRemoved", "true".to_string()),
            ("supportsAllDrives", "true".to_string()),
            ("includeItemsFromAllDrives", "true".to_string()),
            ("fields", fields.to_string()),
        ];
        match drive_of(scope) {
            Some(d) => q.push(("driveId", d.to_string())),
            None => q.push(("restrictToMyDrive", "true".to_string())),
        }
        q
    }

    /// 변경 한 쪽이에요. 토큰이 거절되면 `TokenRejected`예요.
    fn change_page(
        &self,
        scope: &str,
        token: &str,
        fields: &str,
    ) -> Result<Result<Value, TokenRejected>, SourceError> {
        match self
            .api
            .get("changes", &Self::changes_query(scope, token, fields))?
        {
            Got::Fail {
                status: 400 | 404 | 410,
                ..
            } => Ok(Err(TokenRejected)),
            g => g.json("changes").map(Ok),
        }
    }

    /// 변경 하나를 저장본에 반영해요.
    fn apply(
        &self,
        c: &Value,
        scope: &str,
        budget: &mut Budget,
        progress: &mut Progress<'_>,
    ) -> Result<Put, SourceError> {
        if c["changeType"].as_str() == Some("drive") {
            return Ok(Put::Done); // 공유 드라이브 자체는 목록(`drives`)으로 맞춰요
        }
        let Some(id) = c["fileId"].as_str().filter(|i| safe(i)) else {
            return Ok(Put::Done);
        };
        let f = &c["file"];
        if c["removed"].as_bool() == Some(true) || f.is_null() {
            self.remove(id)?;
            return Ok(Put::Done);
        }
        if in_scope(f, scope, self.floor) {
            if read_item(self.dir, id).is_none_or(|o| o["modifiedTime"] != f["modifiedTime"]) {
                progress.need(1);
            }
            return self.put(f, scope, budget, progress);
        }
        // 범위 밖이 됐어요(휴지통, 다른 드라이브로 옮김, 2년 지남, 공유 문서함 파일): 이 범위의 저장본이면 지워요
        if read_item(self.dir, id).is_some_and(|o| o["scope"] == scope) {
            self.remove(id)?;
        }
        Ok(Put::Done)
    }

    /// 범위의 변경을 받아요. 한 쪽을 끝낼 때마다 토큰을 저장해요. 다 받았으면 참, 몫이 차서 멈췄으면 거짓이에요.
    fn changes(
        &self,
        s: &mut State,
        scope: &str,
        budget: &mut Budget,
        progress: &mut Progress<'_>,
    ) -> Result<Result<bool, TokenRejected>, SourceError> {
        let fields = format!(
            "nextPageToken,newStartPageToken,changes(changeType,removed,fileId,driveId,file({FILE_FIELDS}))"
        );
        loop {
            let Some(token) = s.scopes.get(scope).and_then(|x| x.token.clone()) else {
                return Ok(Err(TokenRejected));
            };
            let page = match self.change_page(scope, &token, &fields)? {
                Ok(p) => p,
                Err(r) => return Ok(Err(r)),
            };
            for c in page["changes"].as_array().into_iter().flatten() {
                if self.apply(c, scope, budget, progress)? == Put::Later {
                    return Ok(Ok(false));
                }
            }
            let next = page["nextPageToken"]
                .as_str()
                .or(page["newStartPageToken"].as_str())
                .map(str::to_string);
            let last = page["nextPageToken"].is_null();
            if let Some(n) = next {
                s.scopes.entry(scope.to_string()).or_default().token = Some(n);
                self.save(s)?;
            }
            if last {
                return Ok(Ok(true));
            }
        }
    }

    /// 공유 드라이브 목록을 맞춰요: 없어진 드라이브의 파일과 범위를 지워요(가드 안에서).
    fn sync_drives(&self, s: &mut State, new: Vec<Value>) -> Result<(), SourceError> {
        let ids = |v: &[Value]| -> HashSet<String> {
            v.iter()
                .filter_map(|d| d["id"].as_str().map(str::to_string))
                .collect()
        };
        let old = ids(&s.drives);
        let now = ids(&new);
        let gone: Vec<&String> = old.difference(&now).collect();
        let careful = !gone.is_empty()
            && (now.is_empty() || gone.len() as f64 > (old.len() as f64 * 0.2).max(5.0));
        if careful {
            eprintln!(
                "gdrive: {} shared drives vanished, nothing removed",
                gone.len()
            );
            let mut keep = new;
            keep.extend(
                s.drives
                    .iter()
                    .filter(|d| d["id"].as_str().is_some_and(|i| !now.contains(i)))
                    .cloned(),
            );
            s.drives = keep;
            return self.save(s);
        }
        for d in &gone {
            let scope = scope_key(Some(d));
            for id in self.stored_in(&scope) {
                self.remove(&id)?;
            }
            s.scopes.remove(&scope);
        }
        s.drives = new;
        self.save(s)
    }

    /// 7일마다 범위의 목록을 다시 받아 저장본과 맞춰요. 모두 맞췄으면 참이에요.
    fn reconcile(&self, scopes: &[String]) -> Result<bool, SourceError> {
        let mut clean = true;
        for scope in scopes {
            let files = self.list(scope)?;
            clean &= self.prune(scope, &files)?;
        }
        // 2년이 지난 것이에요
        for id in item_ids(self.dir) {
            if let Some(v) = read_item(self.dir, &id)
                && v["modifiedTime"]
                    .as_str()
                    .and_then(parse)
                    .is_none_or(|m| m <= self.floor)
            {
                self.remove(&id)?;
            }
        }
        Ok(clean)
    }
}

/// 받을 범위예요: 내 드라이브와 끄지 않은 공유 드라이브.
fn active_scopes(s: &State) -> Vec<String> {
    let mut out = vec![MY.to_string()];
    out.extend(
        s.drives
            .iter()
            .filter_map(|d| d["id"].as_str())
            .filter(|id| !s.muted.iter().any(|m| m == id))
            .map(|id| scope_key(Some(id))),
    );
    out
}

/// Drive에서 받아 `raw_dir`(state.json, items/)을 고쳐요. 몫이 차서 다 받지 못했으면 `Ok(true)`(다음 동기화가 이어
/// 받아요)예요. `progress(done, total)`은 export한 파일과 받을 파일이에요.
pub fn fetch_with(
    api: &dyn GDriveApi,
    raw_dir: &Path,
    now: DateTime<Utc>,
    account: Option<&str>,
    still: &dyn Fn() -> bool,
    budget: &mut Budget,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<bool, SourceError> {
    let run = Run {
        api,
        dir: raw_dir,
        still,
        account,
        floor: now - Duration::days(WINDOW_DAYS),
    };
    let mut progress = Progress {
        done: 0,
        total: 0,
        report: progress,
    };
    let mut s = load_state(raw_dir);
    if s.kinds < KINDS {
        for scope in s.scopes.values_mut() {
            scope.done = false;
        }
        s.kinds = KINDS;
    }
    let drives = run.drives()?;
    run.sync_drives(&mut s, drives)?;
    let scopes = active_scopes(&s);
    let mut more = false;
    for scope in &scopes {
        let done = s.scopes.get(scope).is_some_and(|x| x.done);
        let finished = if done {
            match run.changes(&mut s, scope, budget, &mut progress)? {
                Ok(f) => f,
                Err(TokenRejected) => {
                    eprintln!("gdrive: change token refused, listing again");
                    run.initial(&mut s, scope, budget, &mut progress)?
                }
            }
        } else {
            run.initial(&mut s, scope, budget, &mut progress)?
        };
        more |= !finished;
    }
    let due = s
        .reconciled_at
        .as_deref()
        .and_then(parse)
        .is_none_or(|r| now - r >= Duration::days(RECONCILE_DAYS));
    if !more && due && s.initial_done() && run.reconcile(&scopes)? {
        s.reconciled_at = Some(iso(now));
    }
    s.fetched_at = Some(iso(now));
    run.save(&s)?;
    (progress.report)(progress.total.max(1), progress.total.max(1));
    Ok(more)
}

/// 30분 확인이에요: 받을 변경(받는 종류이면서 범위 안, 또는 저장본에 있는 파일의 지움·범위 밖)이 있거나, 처음 받기를
/// 끝내지 않은 범위나 공유 드라이브 목록이 바뀌었으면 참이에요. 토큰은 저장하지 않아요.
pub fn changed(
    api: &dyn GDriveApi,
    raw_dir: &Path,
    now: DateTime<Utc>,
) -> Result<bool, SourceError> {
    let never = || false;
    let run = Run {
        api,
        dir: raw_dir,
        still: &never,
        account: None,
        floor: now - Duration::days(WINDOW_DAYS),
    };
    let s = load_state(raw_dir);
    if s.kinds < KINDS && !s.scopes.is_empty() {
        return Ok(true);
    }
    let drives = run.drives()?;
    let ids = |v: &[Value]| -> HashSet<String> {
        v.iter()
            .filter_map(|d| d["id"].as_str().map(str::to_string))
            .collect()
    };
    if ids(&drives) != ids(&s.drives) {
        return Ok(true);
    }
    let fields = "nextPageToken,newStartPageToken,changes(changeType,removed,fileId,file(mimeType,driveId,modifiedTime,trashed,ownedByMe,sharedWithMeTime))";
    let stored: HashMap<String, Value> = item_ids(raw_dir)
        .into_iter()
        .filter_map(|id| read_item(raw_dir, &id).map(|v| (id, v)))
        .collect();
    for scope in active_scopes(&s) {
        let Some(Scope {
            token: Some(mut token),
            done: true,
        }) = s.scopes.get(&scope).cloned()
        else {
            return Ok(true);
        };
        loop {
            let page = match run.change_page(&scope, &token, fields)? {
                Ok(p) => p,
                Err(TokenRejected) => return Ok(true),
            };
            for c in page["changes"].as_array().into_iter().flatten() {
                if c["changeType"].as_str() == Some("drive") {
                    continue;
                }
                let id = c["fileId"].as_str().unwrap_or("");
                let old = stored.get(id);
                let f = &c["file"];
                let relevant = if c["removed"].as_bool() == Some(true) || f.is_null() {
                    old.is_some()
                } else if in_scope(f, &scope, run.floor) {
                    old.is_none_or(|o| o["modifiedTime"] != f["modifiedTime"])
                } else {
                    old.is_some_and(|o| o["scope"] == scope.as_str())
                };
                if relevant {
                    return Ok(true);
                }
            }
            match page["nextPageToken"].as_str() {
                Some(n) => token = n.to_string(),
                None => break,
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::gdrive::FakeDrive;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap()
    }
    fn t(days_ago: i64) -> String {
        iso(now() - Duration::days(days_ago))
    }
    type Fake = FakeDrive;
    fn fake() -> Fake {
        FakeDrive::at(now())
    }

    fn run(fake: &Fake, dir: &Path) -> Result<bool, SourceError> {
        run_with(fake, dir, Budget::new(BATCH_FILES, BATCH_TIME))
    }

    fn run_with(fake: &Fake, dir: &Path, mut b: Budget) -> Result<bool, SourceError> {
        fetch_with(
            fake,
            dir,
            now(),
            Some("acct"),
            &|| true,
            &mut b,
            &mut |_, _| {},
        )
    }

    #[test]
    fn the_first_fetch_exports_each_kind_and_strips_images() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.file("doc1", DOC, 3, None);
        f.file("sl1", SLIDES, 3, None);
        f.file("sh1", SHEET, 3, None);
        f.file("old", DOC, 800, None);
        assert!(!run(&f, d.path()).unwrap());
        assert_eq!(item_ids(d.path()), ["doc1", "sh1", "sl1"]);
        let doc = read_item(d.path(), "doc1").unwrap();
        let text = doc["text"].as_str().unwrap();
        assert!(text.contains("# Title doc1") && text.contains("## Part"));
        assert!(!text.contains("data:") && !text.contains("![]"), "{text}");
        assert_eq!(read_item(d.path(), "sl1").unwrap()["text"], "plain sl1\n");
        assert_eq!(read_item(d.path(), "sh1").unwrap()["text"], "a,b\n1,2\n");
        assert!(load_state(d.path()).initial_done());
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/fixtures/extract/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn pdf_and_office_files_are_downloaded_and_read() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        for (id, mime, file) in [("p", PDF, "policy.pdf"), ("w", DOCX, "doc.docx"), ("x", XLSX, "sheet.xlsx"),
                                 ("scan", PDF, "scan.pdf")] {
            f.file(id, mime, 3, None);
            f.media.lock().unwrap().insert(id.into(), fixture(file));
        }
        // too large to read: the title only, without downloading
        f.file("big", PPTX, 3, None);
        f.upsert(json!({"id": "big", "name": "big", "mimeType": PPTX, "modifiedTime": t(3), "ownedByMe": true,
                        "trashed": false, "size": (extract::MAX_BYTES + 1).to_string()}));
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["big", "p", "scan", "w", "x"]);
        assert!(read_item(d.path(), "p").unwrap()["text"].as_str().unwrap().contains("담당자는 지윤 님이에요"));
        assert!(read_item(d.path(), "w").unwrap()["text"].as_str().unwrap().contains("| 재시도 횟수 | 5 |"));
        assert!(read_item(d.path(), "x").unwrap()["text"].as_str().unwrap().contains("## 결정"));
        let scan = read_item(d.path(), "scan").unwrap();
        assert!(scan["text"].is_null());
        assert_eq!(scan["blocked"], "noText");
        assert_eq!(read_item(d.path(), "big").unwrap()["blocked"], "tooLarge");
        assert!(!f.exports.lock().unwrap().iter().any(|e| e == "big media"));
    }

    #[test]
    fn a_drive_fetched_before_pdf_and_office_gets_its_older_files() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.file("doc1", DOC, 3, None);
        f.file("p", PDF, 300, None);
        f.media.lock().unwrap().insert("p".into(), fixture("policy.pdf"));
        run(&f, d.path()).unwrap();
        // as an engine from before PDF and Office left it: done, with the Docs only
        let mut s = load_state(d.path());
        s.kinds = 1;
        save_state(d.path(), &s).unwrap();
        crate::gdrive::fetch::tests::forget(d.path(), "p");
        assert!(changed(&f, d.path(), now()).unwrap(), "older kinds count as a change");
        f.exports.lock().unwrap().clear();
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["doc1", "p"]);
        // the Doc did not change, so it is not exported again
        assert_eq!(*f.exports.lock().unwrap(), ["p media"]);
        assert_eq!(load_state(d.path()).kinds, KINDS);
        assert!(load_state(d.path()).initial_done());
    }

    fn forget(dir: &Path, id: &str) {
        std::fs::remove_file(dir.join(ITEMS).join(format!("{id}.json"))).unwrap();
    }

    #[test]
    fn files_shared_with_me_are_left_out_but_others_files_in_my_drive_are_kept() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.upsert(
            json!({"id": "shared", "name": "s", "mimeType": DOC, "modifiedTime": t(1),
            "ownedByMe": false, "sharedWithMeTime": t(1), "trashed": false}),
        );
        f.upsert(
            json!({"id": "placed", "name": "p", "mimeType": DOC, "modifiedTime": t(1),
            "ownedByMe": false, "trashed": false}),
        );
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["placed"]);
    }

    #[test]
    fn a_small_budget_stops_early_and_the_next_sync_carries_on() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        for i in 0..5 {
            f.file(&format!("d{i}"), DOC, 1, None);
        }
        let more = run_with(&f, d.path(), Budget::new(2, BATCH_TIME)).unwrap();
        assert!(more);
        assert_eq!(item_ids(d.path()).len(), 2);
        assert!(!load_state(d.path()).initial_done());
        // A file changed between the two syncs is still caught (the list runs again).
        f.file("late", DOC, 0, None);
        let more = run_with(&f, d.path(), Budget::new(10, BATCH_TIME)).unwrap();
        assert!(!more);
        assert_eq!(item_ids(d.path()).len(), 6);
        assert_eq!(
            f.exports.lock().unwrap().len(),
            6,
            "nothing is exported twice"
        );
    }

    #[test]
    fn changes_update_remove_and_trash() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.file("a", DOC, 5, None);
        f.file("b", DOC, 5, None);
        f.file("c", SHEET, 5, None);
        run(&f, d.path()).unwrap();
        f.exports.lock().unwrap().clear();
        f.edit("a", 0);
        f.delete("b");
        let mut c = f.get("c").unwrap();
        c["trashed"] = json!(true);
        f.upsert(c);
        f.file("new", SLIDES, 0, None);
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["a", "new"]);
        assert_eq!(f.exports.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_rename_without_an_edit_updates_the_title_without_exporting() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.file("a", DOC, 5, None);
        run(&f, d.path()).unwrap();
        f.exports.lock().unwrap().clear();
        let mut a = f.get("a").unwrap();
        a["name"] = json!("renamed");
        a["version"] = json!("2");
        f.upsert(a);
        run(&f, d.path()).unwrap();
        assert!(f.exports.lock().unwrap().is_empty());
        let a = read_item(d.path(), "a").unwrap();
        assert_eq!(a["name"], "renamed");
        assert!(a["text"].as_str().unwrap().contains("Title a"));
    }

    #[test]
    fn changes_outside_the_scope_are_ignored_or_removed() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.file("a", DOC, 5, None);
        run(&f, d.path()).unwrap();
        // Shared with me: never stored.
        f.upsert(
            json!({"id": "sw", "name": "s", "mimeType": DOC, "modifiedTime": t(0),
            "ownedByMe": false, "sharedWithMeTime": t(0), "trashed": false}),
        );
        // A pdf.
        f.file("pdf", "application/pdf", 0, None);
        // Edited long ago... moved out of the window by a restore of an old copy.
        let mut a = f.get("a").unwrap();
        a["modifiedTime"] = json!(t(900));
        f.upsert(a);
        run(&f, d.path()).unwrap();
        assert!(item_ids(d.path()).is_empty());
    }

    #[test]
    fn a_refused_change_token_lists_again() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.file("a", DOC, 5, None);
        run(&f, d.path()).unwrap();
        *f.reject_tokens.lock().unwrap() = true;
        f.file("b", DOC, 0, None);
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["a", "b"]);
    }

    #[test]
    fn blocked_files_keep_the_title_and_gone_files_are_removed() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.file("big", DOC, 1, None);
        f.huge.lock().unwrap().insert("big".into());
        f.file("nodl", SHEET, 1, None);
        let mut n = f.get("nodl").unwrap();
        n["capabilities"]["canDownload"] = json!(false);
        f.upsert(n);
        f.file("cannot", SLIDES, 1, None);
        f.fails
            .lock()
            .unwrap()
            .insert("cannot".into(), (403, "cannotExportFile".into()));
        f.file("gone", DOC, 1, None);
        f.fails
            .lock()
            .unwrap()
            .insert("gone".into(), (404, "notFound".into()));
        f.file("noperm", DOC, 1, None);
        f.fails
            .lock()
            .unwrap()
            .insert("noperm".into(), (403, "insufficientFilePermissions".into()));
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["big", "cannot", "nodl"]);
        let big = read_item(d.path(), "big").unwrap();
        assert_eq!(
            big["text"], "plain big\n",
            "a huge doc falls back to plain text"
        );
        assert_eq!(
            read_item(d.path(), "nodl").unwrap()["blocked"],
            "canDownload"
        );
        assert_eq!(
            read_item(d.path(), "cannot").unwrap()["blocked"],
            "cannotExportFile"
        );
        assert!(read_item(d.path(), "cannot").unwrap()["text"].is_null());
    }

    #[test]
    fn shared_drives_are_fetched_muted_drives_are_skipped_and_gone_drives_removed() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        *f.drives.lock().unwrap() = vec![
            json!({"id": "D1", "name": "Team"}),
            json!({"id": "D2", "name": "Ops"}),
        ];
        f.file("x", DOC, 1, Some("D1"));
        f.file("y", DOC, 1, Some("D2"));
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["x", "y"]);
        assert_eq!(read_item(d.path(), "x").unwrap()["scope"], "drive:D1");

        // Muted: not fetched, but kept.
        {
            let _h = state_lock();
            let mut s = load_state(d.path());
            s.muted = vec!["D2".into()];
            save_state(d.path(), &s).unwrap();
        }
        f.edit("y", 0);
        f.exports.lock().unwrap().clear();
        run(&f, d.path()).unwrap();
        assert!(f.exports.lock().unwrap().is_empty());
        assert_eq!(item_ids(d.path()), ["x", "y"]);

        // Left D1: its files go.
        *f.drives.lock().unwrap() = vec![json!({"id": "D2", "name": "Ops"})];
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["y"]);
        assert!(!load_state(d.path()).scopes.contains_key("drive:D1"));
    }

    #[test]
    fn an_empty_drive_list_removes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        *f.drives.lock().unwrap() = vec![json!({"id": "D1", "name": "Team"})];
        f.file("x", DOC, 1, Some("D1"));
        run(&f, d.path()).unwrap();
        f.drives.lock().unwrap().clear();
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["x"]);
    }

    #[test]
    fn a_new_shared_drive_is_fetched_like_a_first_fetch() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.file("a", DOC, 1, None);
        run(&f, d.path()).unwrap();
        *f.drives.lock().unwrap() = vec![json!({"id": "D9", "name": "New"})];
        f.vanish("a"); // keep "a" in the files list out of the picture for the new drive
        f.file("a", DOC, 1, None);
        f.files.lock().unwrap().push(
            json!({"id": "z", "name": "z", "mimeType": DOC, "modifiedTime": t(2),
            "driveId": "D9", "trashed": false}),
        );
        run(&f, d.path()).unwrap();
        assert!(item_ids(d.path()).contains(&"z".to_string()));
    }

    #[test]
    fn the_weekly_reconcile_removes_vanished_files_within_the_guard() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        for i in 0..10 {
            f.file(&format!("d{i}"), DOC, 1, None);
        }
        run(&f, d.path()).unwrap();
        f.vanish("d0");
        {
            let _h = state_lock();
            let mut s = load_state(d.path());
            s.reconciled_at = Some(t(8));
            save_state(d.path(), &s).unwrap();
        }
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()).len(), 9);
        // Everything vanishing at once is not trusted.
        f.files.lock().unwrap().clear();
        {
            let _h = state_lock();
            let mut s = load_state(d.path());
            s.reconciled_at = Some(t(8));
            save_state(d.path(), &s).unwrap();
        }
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()).len(), 9);
    }

    #[test]
    fn a_changed_connection_stops_writing() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.file("a", DOC, 1, None);
        let mut b = Budget::new(BATCH_FILES, BATCH_TIME);
        let e = fetch_with(
            &f,
            d.path(),
            now(),
            Some("acct"),
            &|| false,
            &mut b,
            &mut |_, _| {},
        )
        .unwrap_err();
        assert_eq!(e.kind(), Kind::Busy);
        assert!(item_ids(d.path()).is_empty());
    }

    #[test]
    fn another_accounts_saved_files_are_not_written_over() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.file("a", DOC, 1, None);
        run(&f, d.path()).unwrap();
        let mut b = Budget::new(BATCH_FILES, BATCH_TIME);
        let e = fetch_with(
            &f,
            d.path(),
            now(),
            Some("other"),
            &|| true,
            &mut b,
            &mut |_, _| {},
        )
        .unwrap_err();
        assert_eq!(e.kind(), Kind::Busy);
    }

    #[test]
    fn the_check_sees_only_changes_that_matter() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.file("a", DOC, 1, None);
        assert!(changed(&f, d.path(), now()).unwrap(), "never fetched");
        run(&f, d.path()).unwrap();
        assert!(!changed(&f, d.path(), now()).unwrap());
        f.file("png", "image/png", 0, None);
        f.upsert(
            json!({"id": "sw", "name": "s", "mimeType": DOC, "modifiedTime": t(0),
            "ownedByMe": false, "sharedWithMeTime": t(0), "trashed": false}),
        );
        assert!(
            !changed(&f, d.path(), now()).unwrap(),
            "an image and a shared file do not count"
        );
        f.edit("a", 0);
        assert!(changed(&f, d.path(), now()).unwrap());
        run(&f, d.path()).unwrap();
        assert!(!changed(&f, d.path(), now()).unwrap());
        *f.drives.lock().unwrap() = vec![json!({"id": "D1", "name": "T"})];
        assert!(changed(&f, d.path(), now()).unwrap(), "a new shared drive");
    }

    #[test]
    fn markdown_images_are_stripped() {
        let md = "a ![][image1] b ![alt](data:image/png;base64,xx) c\n\n[image1]: <data:image/png;base64,QQ==>\n[image2]: <data:image/jpeg;base64,QQ==>";
        assert_eq!(strip_images(md), "a  b  c\n\n");
        assert_eq!(clean("\u{feff}a\r\nb\rc"), "a\nb\nc");
        assert_eq!(head_lines("1\n2\n3", 2), "1\n2");
        assert_eq!(
            unescape_md("\\# 제목  \n\\- 항목 \\*굵게\\* 1\\.5  \nC:\\\\path"),
            "# 제목\n- 항목 *굵게* 1.5\nC:\\path"
        );
    }

    #[test]
    fn ids_with_odd_characters_are_not_used_as_file_names() {
        assert!(safe("1AbC-_x"));
        assert!(!safe("../x") && !safe("") && !safe("a/b"));
    }
}
