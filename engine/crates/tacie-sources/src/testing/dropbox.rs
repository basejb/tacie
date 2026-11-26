//! A fake Dropbox (`DropboxApi`): holds the current state of folders and files and the change log. Fetch tests
//! (`dropbox::*`) and engine tests share it. It mimics the order seen on real Dropbox on 2026-11-13 (docs/33
//! 10-0-2).
//!
//! - Cursor `"c<n>"` starts at the nth entry of the change log (recursive from the root). Per-folder cursors are
//!   `"c<n>:<r|f><path_lower>"`. `list_folder` returns the current state (in path order, so parents first) and
//!   `"c<기록 길이>"`, and continues with an `"l<k>"` cursor when a page overflows.
//! - Every mutating function records to the log (except `vanish`). `deleted` entries have only `path_lower`,
//!   `path_display` and `name`, no id.
//!   - Rename: `deleted` (old path) + `file` (new path), same id.
//!   - Folder move: `deleted` for each entry below, the folder, then each entry below at its new path.
//!   - Folder delete: `deleted` for each entry below, then the folder's `deleted`.
//!   - Shared folder unmount (`unmount`): a single folder `deleted` only.
//!   - Shared folder mount (`join`): `deleted <path_lower>`, the folder, then the files.
//! - Folder and file ids all carry the `id:` prefix. Ids of automatically created parent folders are `d<n>` (can be set
//!   with `folder`).
//! - `.paper` is `is_downloadable:false`, `export_info{export_as:"html", export_options:["html","markdown"]}`, and
//!   export returns the text given to `file` whatever the format.
//! - `files/*` is 422 when Path-Root differs from the fake's root. `users/get_current_account` does not check it.
//! - Injected failure statuses are turned into the results `DropboxHttp` would produce: 401 `Key`, 429 `Busy`, 403 and
//!   5xx `Down`, anything else `Got::Fail`.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};

use crate::dropbox::api::{DropboxApi, Got};
use crate::error::{Kind, SourceError};

#[derive(Default)]
struct World {
    /// 폴더와 파일의 지금 모습이에요.
    entries: Vec<Value>,
    log: Vec<Value>,
    /// 파일 id(`id:` 없이) → 내용이에요.
    bodies: HashMap<String, Vec<u8>>,
    /// 이어 받을 목록 쪽이에요: `"l<k>"` → (남은 항목, 마지막 커서).
    listings: HashMap<String, (Vec<Value>, String)>,
    seq: usize,
}

pub struct FakeDropbox {
    /// `file(…, days_ago, …)`의 기준 시각이에요.
    pub now: DateTime<Utc>,
    account_id: String,
    root: Mutex<String>,
    world: Mutex<World>,
    page: AtomicUsize,
    expired: AtomicBool,
    fail_next: Mutex<Option<u16>>,
    recursive_fails: AtomicUsize,
    /// (성공시킬 쪽 수, 그 뒤의 상태)예요.
    after_pages: Mutex<Option<(usize, u16)>>,
    pages_served: AtomicUsize,
    blocked: Mutex<HashMap<String, String>>,
    delay: Mutex<Duration>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    refuse: AtomicBool,
    busy: AtomicBool,
    calls: Mutex<Vec<String>>,
}

fn iso(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn hash(body: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    body.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn body(v: Value) -> Result<Got, SourceError> {
    Ok(Got::Body(v.to_string().into_bytes()))
}

fn fail(status: u16, summary: &str) -> Result<Got, SourceError> {
    Ok(Got::Fail {
        status,
        summary: summary.to_string(),
    })
}

/// 넣은 상태를 `DropboxHttp`가 돌려줄 결과로 바꿔요.
fn status_result(status: u16) -> Result<Got, SourceError> {
    match status {
        401 => Err(SourceError::service(Kind::Key, "dropbox: refused")),
        429 => Err(SourceError::service(Kind::Busy, "dropbox: rate limited")),
        403 | 500.. => Err(SourceError::service(
            Kind::Down,
            "dropbox: the server kept failing",
        )),
        s => fail(s, ""),
    }
}

fn lower(v: &Value) -> &str {
    v["path_lower"].as_str().unwrap_or("")
}

/// `prefix` 자신이거나 그 아래예요(`""`는 모두).
fn under(path: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|r| r.starts_with('/'))
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(p, _)| p)
}

fn name_of(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, n)| n)
}

fn deleted(e: &Value) -> Value {
    json!({".tag": "deleted", "name": e["name"], "path_lower": e["path_lower"],
        "path_display": e["path_display"]})
}

fn cursor_for(n: usize, path: &str, recursive: bool) -> String {
    if path.is_empty() && recursive {
        format!("c{n}")
    } else {
        format!("c{n}:{}{path}", if recursive { 'r' } else { 'f' })
    }
}

/// `"c<n>[:<r|f><path>]"` → (n, path, recursive)예요.
fn parse_cursor(c: &str) -> Option<(usize, String, bool)> {
    let rest = c.strip_prefix('c')?;
    match rest.split_once(':') {
        None => Some((rest.parse().ok()?, String::new(), true)),
        Some((n, scope)) => {
            let recursive = scope.starts_with('r');
            Some((n.parse().ok()?, scope.get(1..)?.to_string(), recursive))
        }
    }
}

/// 이 목록(경로, 재귀)에 들어가는 항목인지예요.
fn in_listing(path_lower: &str, path: &str, recursive: bool) -> bool {
    if recursive {
        under(path_lower, path)
    } else {
        parent(path_lower) == path && path_lower != path
    }
}

impl World {
    fn next(&mut self) -> usize {
        self.seq += 1;
        self.seq
    }

    fn find(&self, id: &str) -> Option<Value> {
        let full = format!("id:{id}");
        self.entries
            .iter()
            .find(|e| e["id"] == full.as_str())
            .cloned()
    }

    fn at(&self, path_lower: &str) -> Option<Value> {
        self.entries
            .iter()
            .find(|e| lower(e) == path_lower)
            .cloned()
    }

    /// 항목을 넣거나 같은 id를 바꾸고 기록에 남겨요.
    fn put(&mut self, e: Value) {
        self.entries.retain(|x| x["id"] != e["id"]);
        self.entries.push(e.clone());
        self.log.push(e);
    }

    /// 없는 부모 폴더를 만들어요(기록에 남겨요).
    fn parents(&mut self, path_display: &str) {
        let p = parent(path_display);
        if p.is_empty() || self.at(&p.to_lowercase()).is_some() {
            return;
        }
        self.parents(p);
        let id = format!("d{}", self.next());
        self.put(folder_entry(&id, p));
    }

    /// 아래 항목(자신 빼고)이에요, 경로 순서로.
    fn below(&self, path_lower: &str) -> Vec<Value> {
        let mut out: Vec<Value> = self
            .entries
            .iter()
            .filter(|e| lower(e) != path_lower && under(lower(e), path_lower))
            .cloned()
            .collect();
        out.sort_by(|a, b| lower(a).cmp(lower(b)));
        out
    }
}

fn folder_entry(id: &str, path_display: &str) -> Value {
    json!({".tag": "folder", "id": format!("id:{id}"), "name": name_of(path_display),
        "path_lower": path_display.to_lowercase(), "path_display": path_display})
}

impl FakeDropbox {
    /// 뿌리 `"100"`, 계정 `"dbid:A"`예요.
    pub fn at(now: DateTime<Utc>) -> FakeDropbox {
        FakeDropbox {
            now,
            account_id: "dbid:A".into(),
            root: Mutex::new("100".into()),
            world: Mutex::default(),
            page: AtomicUsize::new(2000),
            expired: AtomicBool::new(false),
            fail_next: Mutex::default(),
            recursive_fails: AtomicUsize::new(0),
            after_pages: Mutex::default(),
            pages_served: AtomicUsize::new(0),
            blocked: Mutex::default(),
            delay: Mutex::new(Duration::ZERO),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
            refuse: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            calls: Mutex::default(),
        }
    }

    /// 지금 시각 기준의 가짜예요(엔진 테스트).
    pub fn new() -> FakeDropbox {
        Self::at(Utc::now())
    }

    fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().unwrap()
    }

    fn t(&self, days_ago: i64) -> String {
        iso(self.now - chrono::Duration::days(days_ago))
    }

    fn file_entry(&self, w: &mut World, id: &str, path: &str, days_ago: i64, body: &[u8]) -> Value {
        let rev = format!("{:09x}", w.next());
        let mut e = json!({".tag": "file", "id": format!("id:{id}"), "name": name_of(path),
            "path_lower": path.to_lowercase(), "path_display": path, "client_modified": self.t(days_ago),
            "server_modified": self.t(days_ago), "rev": rev, "size": body.len(), "content_hash": hash(body),
            "is_downloadable": true});
        if path.to_lowercase().ends_with(".paper") {
            e["is_downloadable"] = json!(false);
            e["export_info"] = json!({"export_as": "html", "export_options": ["html", "markdown"]});
        }
        w.bodies.insert(id.to_string(), body.to_vec());
        e
    }

    /// 파일 하나를 더하거나 바꿔요. 없는 부모 폴더는 만들어요. `.paper`면 `body`가 export 글이에요.
    pub fn file(&self, id: &str, path: &str, days_ago: i64, body: &[u8]) {
        let mut w = self.world();
        w.parents(path);
        let e = self.file_entry(&mut w, id, path, days_ago, body);
        w.put(e);
    }

    /// 이 id의 폴더를 더해요(저절로 만드는 부모 폴더의 id를 정하고 싶을 때).
    pub fn folder(&self, id: &str, path: &str) {
        let mut w = self.world();
        w.parents(path);
        w.put(folder_entry(id, path));
    }

    /// 내용을 바꿔요: 새 `content_hash`와 `rev`, 지금 수정이에요.
    pub fn edit(&self, id: &str, body: &[u8]) {
        let mut w = self.world();
        let mut e = w.find(id).expect("no such file");
        let rev = format!("{:09x}", w.next());
        e["rev"] = json!(rev);
        e["content_hash"] = json!(hash(body));
        e["size"] = json!(body.len());
        e["server_modified"] = json!(self.t(0));
        w.bodies.insert(id.to_string(), body.to_vec());
        w.put(e);
    }

    /// 내용은 그대로 두고 `server_modified`를 이 날로 고쳐요(새 `rev`).
    pub fn edit_at(&self, id: &str, days_ago: i64) {
        let mut w = self.world();
        let mut e = w.find(id).expect("no such file");
        let rev = format!("{:09x}", w.next());
        e["rev"] = json!(rev);
        e["server_modified"] = json!(self.t(days_ago));
        w.put(e);
    }

    /// 이름을 바꾸거나 옮겨요: 옛 경로 `deleted` + 새 경로 `file`이에요.
    pub fn rename(&self, id: &str, new_path: &str) {
        let mut w = self.world();
        let mut e = w.find(id).expect("no such file");
        w.log.push(deleted(&e));
        w.parents(new_path);
        e["name"] = json!(name_of(new_path));
        e["path_lower"] = json!(new_path.to_lowercase());
        e["path_display"] = json!(new_path);
        w.put(e);
    }

    /// 파일이나 폴더를 지워요. 폴더면 아래 항목마다 `deleted`, 그 뒤 폴더 `deleted`예요.
    pub fn delete(&self, path: &str) {
        let mut w = self.world();
        let pl = path.to_lowercase();
        let Some(e) = w.at(&pl) else { return };
        let mut below = w.below(&pl);
        below.reverse();
        for b in &below {
            w.log.push(deleted(b));
        }
        w.log.push(deleted(&e));
        w.entries.retain(|x| !under(lower(x), &pl));
    }

    /// 폴더를 옮겨요: 아래 항목마다 `deleted`, 옛 폴더 `deleted`, 새 경로의 폴더, 아래 항목마다 새 경로예요(id 그대로,
    /// docs/33 10-0-2 ⑥).
    pub fn move_folder(&self, from: &str, to: &str) {
        let mut w = self.world();
        let fl = from.to_lowercase();
        let Some(mut folder) = w.at(&fl) else { return };
        let from_display = folder["path_display"].as_str().unwrap_or(from).to_string();
        let below = w.below(&fl);
        for b in &below {
            w.log.push(deleted(b));
        }
        w.log.push(deleted(&folder));
        w.parents(to);
        folder["name"] = json!(name_of(to));
        folder["path_lower"] = json!(to.to_lowercase());
        folder["path_display"] = json!(to);
        w.put(folder);
        for mut b in below {
            let display = b["path_display"].as_str().unwrap_or("").to_string();
            let rest = display.get(from_display.len()..).unwrap_or("");
            let moved = format!("{to}{rest}");
            b["path_lower"] = json!(moved.to_lowercase());
            b["path_display"] = json!(moved);
            w.put(b);
        }
    }

    /// 공유 폴더를 떼요(권한을 잃은 것도 같아요): 폴더 `deleted` 하나만 남고 아래 항목은 사라져요.
    pub fn unmount(&self, path: &str) {
        let mut w = self.world();
        let pl = path.to_lowercase();
        let Some(e) = w.at(&pl) else { return };
        w.log.push(deleted(&e));
        w.entries.retain(|x| !under(lower(x), &pl));
    }

    /// 공유 폴더의 내 권한이 바뀌었어요: 폴더 `deleted` 뒤 폴더와 아래 항목 전체가 지금 모습 그대로 다시 와요
    /// (docs/33 10-0-2 ⑥).
    pub fn reshare(&self, path: &str) {
        let mut w = self.world();
        let pl = path.to_lowercase();
        let Some(folder) = w.at(&pl) else { return };
        w.log.push(deleted(&folder));
        let below = w.below(&pl);
        w.put(folder);
        for b in below {
            w.put(b);
        }
    }

    /// 공유 폴더를 붙여요: `deleted <path_lower>`, 폴더, 파일들(id, 이름)이에요. 파일 내용은 이름에서 정해져서 다시
    /// 붙여도 `content_hash`가 같아요.
    pub fn join(&self, path: &str, folder_id: &str, files: &[(&str, &str)]) {
        let mut w = self.world();
        let folder = folder_entry(folder_id, path);
        w.log.push(deleted(&folder));
        w.parents(path);
        w.put(folder);
        for (id, name) in files {
            let body = format!("# {name}\n공유 폴더의 파일이에요\n");
            let e = self.file_entry(&mut w, id, &format!("{path}/{name}"), 1, body.as_bytes());
            w.put(e);
        }
    }

    /// 지우되 변경 기록에는 남기지 않아요(재조정만 알아요).
    pub fn vanish(&self, id: &str) {
        let full = format!("id:{id}");
        self.world().entries.retain(|e| e["id"] != full.as_str());
    }

    /// 뿌리를 바꿔요(팀에 들어감). 옛 뿌리로 오는 `files/*`는 422예요.
    pub fn set_root(&self, root: &str) {
        *self.root.lock().unwrap() = root.to_string();
    }

    /// 한 쪽의 항목 수예요(기본 2000).
    pub fn set_page(&self, n: usize) {
        self.page.store(n.max(1), Ordering::SeqCst);
    }

    /// 지금까지 준 커서를 무효로 해요: `continue`가 409 `reset/`이에요. 다음 `list_folder`부터 다시 돼요.
    pub fn expire_cursors(&self) {
        self.expired.store(true, Ordering::SeqCst);
    }

    /// 다음 호출 하나(무엇이든)가 이 상태예요.
    pub fn fail_next(&self, status: u16) {
        *self.fail_next.lock().unwrap() = Some(status);
    }

    /// `list_folder{path:"", recursive:true}`가 이만큼 503(`Down`)이에요.
    pub fn fail_recursive_root(&self, times: usize) {
        self.recursive_fails.store(times, Ordering::SeqCst);
    }

    /// 지금부터 목록 쪽(`list_folder`·`continue`)을 `pages`개 준 뒤로는 이 상태예요(`clear_failures`까지).
    pub fn fail_after_pages(&self, pages: usize, status: u16) {
        self.pages_served.store(0, Ordering::SeqCst);
        *self.after_pages.lock().unwrap() = Some((pages, status));
    }

    /// 넣은 실패(`fail_next`, `fail_recursive_root`, `fail_after_pages`, `blocked`)를 모두 지워요.
    pub fn clear_failures(&self) {
        *self.fail_next.lock().unwrap() = None;
        self.recursive_fails.store(0, Ordering::SeqCst);
        *self.after_pages.lock().unwrap() = None;
        self.blocked.lock().unwrap().clear();
    }

    /// Paper의 `export_info.export_options`를 바꿔요.
    pub fn paper_options(&self, id: &str, options: &[&str]) {
        let mut w = self.world();
        let mut e = w.find(id).expect("no such file");
        e["export_info"]["export_options"] = json!(options);
        w.put(e);
    }

    /// 내려받기·export 하나가 이만큼 걸려요.
    pub fn download_delay(&self, d: Duration) {
        *self.delay.lock().unwrap() = d;
    }

    /// 동시에 받던 내려받기·export의 최대 수예요.
    pub fn max_in_flight(&self) -> usize {
        self.max_in_flight.load(Ordering::SeqCst)
    }

    /// 모든 호출이 401(`Key`)이에요.
    pub fn refuse(&self, on: bool) {
        self.refuse.store(on, Ordering::SeqCst);
    }

    /// 모든 호출이 한도(`Busy`)예요.
    pub fn busy(&self, on: bool) {
        self.busy.store(on, Ordering::SeqCst);
    }

    /// 이 파일의 내려받기·export가 409와 이 `error_summary`예요.
    pub fn blocked(&self, id: &str, summary: &str) {
        self.blocked
            .lock()
            .unwrap()
            .insert(id.to_string(), summary.to_string());
    }

    /// 부른 것이에요: `"rpc files/list_folder <path> <recursive>"`, `"rpc files/list_folder/continue"`,
    /// `"rpc users/get_current_account"`, `"content files/download id:x"`, `"content files/export id:x markdown"`.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub fn clear_calls(&self) {
        self.calls.lock().unwrap().clear();
    }

    /// 모든 호출 앞의 실패예요.
    fn gate(&self) -> Option<Result<Got, SourceError>> {
        if self.refuse.load(Ordering::SeqCst) {
            return Some(status_result(401));
        }
        if self.busy.load(Ordering::SeqCst) {
            return Some(status_result(429));
        }
        self.fail_next.lock().unwrap().take().map(status_result)
    }

    /// 목록 쪽 하나의 실패예요(`fail_after_pages`).
    fn page_gate(&self) -> Option<Result<Got, SourceError>> {
        let (pages, status) = (*self.after_pages.lock().unwrap())?;
        if self.pages_served.load(Ordering::SeqCst) >= pages {
            return Some(status_result(status));
        }
        self.pages_served.fetch_add(1, Ordering::SeqCst);
        None
    }

    fn wrong_root(&self, root: Option<&str>) -> bool {
        root != Some(self.root.lock().unwrap().as_str())
    }

    fn list_folder(&self, arg: &Value) -> Result<Got, SourceError> {
        let path = arg["path"].as_str().unwrap_or("").to_string();
        let recursive = arg["recursive"].as_bool() == Some(true);
        if path.is_empty() && recursive {
            let left = self.recursive_fails.load(Ordering::SeqCst);
            if left > 0 {
                self.recursive_fails.store(left - 1, Ordering::SeqCst);
                return status_result(503);
            }
        }
        if let Some(r) = self.page_gate() {
            return r;
        }
        self.expired.store(false, Ordering::SeqCst);
        let pl = path.to_lowercase();
        let mut w = self.world();
        if !pl.is_empty() && w.at(&pl).is_none() {
            return fail(409, "path/not_found/..");
        }
        let mut entries: Vec<Value> = w
            .entries
            .iter()
            .filter(|e| in_listing(lower(e), &pl, recursive))
            .cloned()
            .collect();
        entries.sort_by(|a, b| lower(a).cmp(lower(b)));
        let last = cursor_for(w.log.len(), &pl, recursive);
        let page = self.page.load(Ordering::SeqCst);
        if entries.len() <= page {
            return body(json!({"entries": entries, "cursor": last, "has_more": false}));
        }
        let rest = entries.split_off(page);
        let key = format!("l{}", w.next());
        w.listings.insert(key.clone(), (rest, last));
        body(json!({"entries": entries, "cursor": key, "has_more": true}))
    }

    fn continue_folder(&self, arg: &Value) -> Result<Got, SourceError> {
        if let Some(r) = self.page_gate() {
            return r;
        }
        let cursor = arg["cursor"].as_str().unwrap_or("");
        let page = self.page.load(Ordering::SeqCst);
        let mut w = self.world();
        if cursor.starts_with('l') {
            let Some((mut rest, last)) = w.listings.remove(cursor) else {
                return fail(409, "reset/..");
            };
            if rest.len() <= page {
                return body(json!({"entries": rest, "cursor": last, "has_more": false}));
            }
            let more = rest.split_off(page);
            let key = format!("l{}", w.next());
            w.listings.insert(key.clone(), (more, last));
            return body(json!({"entries": rest, "cursor": key, "has_more": true}));
        }
        if self.expired.load(Ordering::SeqCst) {
            return fail(409, "reset/..");
        }
        let Some((from, path, recursive)) = parse_cursor(cursor) else {
            return fail(400, "");
        };
        let mut entries = Vec::new();
        let mut at = from.min(w.log.len());
        let mut has_more = false;
        while at < w.log.len() {
            let e = &w.log[at];
            if in_listing(lower(e), &path, recursive) {
                if entries.len() == page {
                    has_more = true;
                    break;
                }
                entries.push(e.clone());
            }
            at += 1;
        }
        let next = cursor_for(if has_more { at } else { w.log.len() }, &path, recursive);
        body(json!({"entries": entries, "cursor": next, "has_more": has_more}))
    }

    fn file_body(&self, path: &str, arg: &Value) -> Result<Got, SourceError> {
        let id = arg["path"].as_str().unwrap_or("").trim_start_matches("id:");
        if let Some(summary) = self.blocked.lock().unwrap().get(id) {
            return fail(409, summary);
        }
        let w = self.world();
        let Some(e) = w.find(id) else {
            return fail(409, "path/not_found/..");
        };
        let paper = e["export_info"].is_object();
        match path {
            "files/download" if paper => return fail(409, "unsupported_file/.."),
            "files/export" if !paper => return fail(409, "non_exportable/.."),
            "files/export" => {
                let format = arg["export_format"].as_str().unwrap_or("");
                let offered = e["export_info"]["export_options"]
                    .as_array()
                    .is_some_and(|o| o.iter().any(|x| x == format));
                if !offered {
                    return fail(409, "invalid_export_format/..");
                }
            }
            _ => {}
        }
        Ok(Got::Body(w.bodies.get(id).cloned().unwrap_or_default()))
    }
}

impl Default for FakeDropbox {
    fn default() -> Self {
        Self::new()
    }
}

impl DropboxApi for FakeDropbox {
    fn rpc(&self, path: &str, arg: &Value, root: Option<&str>) -> Result<Got, SourceError> {
        let call = match path {
            "files/list_folder" => format!(
                "rpc {path} {} {}",
                arg["path"].as_str().unwrap_or(""),
                arg["recursive"].as_bool() == Some(true)
            ),
            _ => format!("rpc {path}"),
        };
        self.calls.lock().unwrap().push(call);
        if let Some(r) = self.gate() {
            return r;
        }
        if path == "users/get_current_account" {
            let root = self.root.lock().unwrap().clone();
            return body(
                json!({"account_id": self.account_id, "email": "dropbox@example.com",
                "account_type": {".tag": "basic"},
                "root_info": {".tag": "user", "root_namespace_id": root, "home_namespace_id": root}}),
            );
        }
        if path.starts_with("files/") && self.wrong_root(root) {
            return fail(422, "");
        }
        match path {
            "files/list_folder" => self.list_folder(arg),
            "files/list_folder/continue" => self.continue_folder(arg),
            _ => fail(404, ""),
        }
    }

    fn content(&self, path: &str, arg: &Value, root: Option<&str>) -> Result<Got, SourceError> {
        let target = arg["path"].as_str().unwrap_or("");
        let call = match arg["export_format"].as_str() {
            Some(f) => format!("content {path} {target} {f}"),
            None => format!("content {path} {target}"),
        };
        self.calls.lock().unwrap().push(call);
        if let Some(r) = self.gate() {
            return r;
        }
        if self.wrong_root(root) {
            return fail(422, "");
        }
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        let delay = *self.delay.lock().unwrap();
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        let got = self.file_body(path, arg);
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        got
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fake() -> FakeDropbox {
        FakeDropbox::at(Utc.with_ymd_and_hms(2026, 11, 13, 0, 0, 0).unwrap())
    }

    fn list(f: &FakeDropbox, path: &str, recursive: bool) -> Value {
        f.rpc(
            "files/list_folder",
            &json!({"path": path, "recursive": recursive}),
            Some("100"),
        )
        .unwrap()
        .json("list")
        .unwrap()
    }

    /// 커서부터 기록을 `(tag, path_lower, id)`로 읽어요.
    fn since(f: &FakeDropbox, cursor: &str) -> Vec<(String, String, String)> {
        let v = f
            .rpc(
                "files/list_folder/continue",
                &json!({"cursor": cursor}),
                Some("100"),
            )
            .unwrap()
            .json("continue")
            .unwrap();
        v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e[".tag"].as_str().unwrap().to_string(),
                    lower(e).to_string(),
                    e["id"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect()
    }

    fn cursor(f: &FakeDropbox) -> String {
        list(f, "", true)["cursor"].as_str().unwrap().to_string()
    }

    fn t(tag: &str, path: &str, id: &str) -> (String, String, String) {
        (tag.into(), path.into(), id.into())
    }

    #[test]
    fn listing_gives_parents_first_with_prefixed_ids_and_pages() {
        let f = fake();
        f.file("m", "/A/b/M.md", 1, b"m");
        let v = list(&f, "", true);
        let paths: Vec<&str> = v["entries"].as_array().unwrap().iter().map(lower).collect();
        assert_eq!(paths, ["/a", "/a/b", "/a/b/m.md"]);
        assert_eq!(v["entries"][2]["id"], "id:m");
        assert_eq!(v["entries"][2]["path_display"], "/A/b/M.md");
        assert_eq!(v["cursor"], "c3");
        assert_eq!(list(&f, "", false)["entries"].as_array().unwrap().len(), 1);
        f.set_page(2);
        let v = list(&f, "", true);
        assert_eq!(v["has_more"], true);
        let rest = f
            .rpc(
                "files/list_folder/continue",
                &json!({"cursor": v["cursor"]}),
                Some("100"),
            )
            .unwrap()
            .json("c")
            .unwrap();
        assert_eq!(
            (rest["entries"].as_array().unwrap().len(), &rest["cursor"]),
            (1, &json!("c3"))
        );
        assert_eq!(
            f.rpc("files/list_folder", &json!({"path": ""}), Some("1"))
                .unwrap(),
            Got::Fail {
                status: 422,
                summary: String::new()
            }
        );
    }

    #[test]
    fn changes_follow_the_order_seen_on_real_dropbox() {
        let f = fake();
        f.file("x", "/a/x.md", 1, b"x");
        f.folder("fa", "/f");
        f.file("y", "/f/y.md", 1, b"y");
        let c = cursor(&f);
        f.rename("x", "/a/z.md");
        assert_eq!(
            since(&f, &c),
            [t("deleted", "/a/x.md", ""), t("file", "/a/z.md", "id:x")]
        );
        let c = cursor(&f);
        f.move_folder("/f", "/g");
        assert_eq!(
            since(&f, &c),
            [
                t("deleted", "/f/y.md", ""),
                t("deleted", "/f", ""),
                t("folder", "/g", "id:fa"),
                t("file", "/g/y.md", "id:y")
            ]
        );
        let c = cursor(&f);
        f.unmount("/g");
        assert_eq!(since(&f, &c), [t("deleted", "/g", "")]);
        let c = cursor(&f);
        f.join("/g", "fb", &[("y", "y.md")]);
        assert_eq!(
            since(&f, &c),
            [
                t("deleted", "/g", ""),
                t("folder", "/g", "id:fb"),
                t("file", "/g/y.md", "id:y")
            ]
        );
        let c = cursor(&f);
        f.reshare("/g");
        assert_eq!(
            since(&f, &c),
            [
                t("deleted", "/g", ""),
                t("folder", "/g", "id:fb"),
                t("file", "/g/y.md", "id:y")
            ]
        );
        let c = cursor(&f);
        f.delete("/a");
        assert_eq!(
            since(&f, &c),
            [t("deleted", "/a/z.md", ""), t("deleted", "/a", "")]
        );
        let c = cursor(&f);
        f.vanish("y");
        assert!(since(&f, &c).is_empty());
        f.expire_cursors();
        assert_eq!(
            f.rpc(
                "files/list_folder/continue",
                &json!({"cursor": c}),
                Some("100")
            )
            .unwrap(),
            Got::Fail {
                status: 409,
                summary: "reset/..".into()
            }
        );
    }

    #[test]
    fn injected_failures_look_like_the_http_client() {
        let f = fake();
        f.file("p", "/a/p.paper", 1, b"# p");
        f.fail_next(503);
        assert_eq!(
            f.rpc("users/get_current_account", &Value::Null, None)
                .unwrap_err()
                .kind(),
            Kind::Down
        );
        f.fail_recursive_root(1);
        assert_eq!(
            f.rpc(
                "files/list_folder",
                &json!({"path": "", "recursive": true}),
                Some("100")
            )
            .unwrap_err()
            .kind(),
            Kind::Down
        );
        f.fail_after_pages(1, 429);
        list(&f, "", true);
        assert_eq!(
            f.rpc("files/list_folder", &json!({"path": ""}), Some("100"))
                .unwrap_err()
                .kind(),
            Kind::Busy
        );
        f.clear_failures();
        let export = |fmt: &str| {
            f.content(
                "files/export",
                &json!({"path": "id:p", "export_format": fmt}),
                Some("100"),
            )
            .unwrap()
        };
        assert_eq!(export("markdown"), Got::Body(b"# p".to_vec()));
        assert!(matches!(
            export("plain_text"),
            Got::Fail { status: 409, .. }
        ));
        assert!(matches!(
            f.content("files/download", &json!({"path": "id:p"}), Some("100"))
                .unwrap(),
            Got::Fail { status: 409, .. }
        ));
    }
}
