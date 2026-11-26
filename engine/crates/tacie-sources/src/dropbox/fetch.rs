//! Initial fetch and text fetch from Dropbox (docs/33 6절).
//!
//! - The account (`users/get_current_account`) gives the root (`root_namespace_id`), and every `files/*` call carries
//!   Path-Root. If the saved copy belongs to another account, it is deleted first (only muted folders are kept).
//! - The first listing fetches everything from the root with one recursive listing (`list_folder` + `continue`, in one
//!   go). If the recursive listing keeps failing, it lists each top folder (`reconcile`). Top (depth 1) folders go into
//!   `tops`, files to fetch into `items/<id>.json`, and files whose `content_hash` differs from the saved copy are
//!   added to `pending`. Refetching (reset, root change, weekly reconcile) is `reconcile`.
//! - `pending` is fetched by `WORKERS` workers at once, most recently modified first. Files are written only by the
//!   calling thread, and the state is saved every `SAVE_EVERY` files. Each run fetches only up to `Budget` (file count,
//!   time) and the next sync continues with the rest. Time is wall-clock, so time spent resting on 429 counts too.
//! - Paper is exported (`markdown`, or `plain_text` if not available), PDF and Office files are downloaded and run
//!   through `extract`, and `.md`/`.txt` are UTF-8 text. Files that cannot be fetched keep only the title (`blocked`).
//! - When `still()` is false (the connection changed) it writes no more and stops with `Busy`.
//!
//! File names and content are never put in logs or errors.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use super::api::{DropboxApi, Failure, Got, failure};
use super::cursor::Ended;
use super::reconcile::{SPLIT_AFTER, Why, reconcile_due};
use super::state::{
    FileType, ITEMS, State, Top, WINDOW_DAYS, bare_id, file_type, in_scope, kept_muted, load_state,
    read_item, remove_item, save_state, state_lock, version, write_item,
};
use crate::error::{Kind, SourceError};
use crate::extract::{self, FileKind, MAX_BYTES};
use crate::gdrive::fetch::{Budget, SHEET_LINES, clean, head_lines, iso, parse};

/// 동시에 받는 파일 수예요.
pub const WORKERS: usize = 4;
/// `list_folder` 한 쪽의 항목 수예요.
pub(crate) const LIST_LIMIT: u32 = 2000;
/// 받는 도중 이만큼 받을 때마다 상태를 저장해요.
pub(crate) const SAVE_EVERY: usize = 25;
/// `retry_error`가 이만큼의 동기화에서 이어지면 제목만 남겨요(`retryError`).
pub(crate) const RETRY_LIMIT: u8 = 3;

/// 연결한 계정이에요.
#[derive(Debug, Clone)]
pub(crate) struct Account {
    pub account_id: String,
    pub email: Option<String>,
    pub root: String,
    /// `user` 또는 `team`이에요.
    pub root_tag: String,
    /// 팀 공간에서 내 폴더(소문자)예요.
    pub home_path: Option<String>,
}

/// 목록 하나예요: 항목과 재귀 커서(Whole), 또는 폴더마다의 커서(Split).
#[derive(Debug, Default)]
pub(crate) struct Listing {
    pub entries: Vec<Value>,
    pub cursor: Option<String>,
    pub cursors: BTreeMap<String, String>,
}

pub(crate) struct Run<'a> {
    pub api: &'a dyn DropboxApi,
    pub dir: &'a Path,
    pub still: &'a dyn Fn() -> bool,
    /// 지금 연결의 계정 id예요. 저장본이 다른 계정 것이면 쓰지 않아요.
    pub account: Option<&'a str>,
    pub floor: DateTime<Utc>,
    pub now: DateTime<Utc>,
}

/// 파일 하나를 받은 결과예요.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Text(String),
    /// 제목만 남겨요: 이유.
    Blocked(String),
    /// 지워졌어요.
    Gone,
    /// 지금은 받을 수 없어요(`retry_error`). `pending`에 남겨요.
    Retry,
}

/// 저장하는 종류 이름이에요.
fn type_name(t: FileType, entry: &Value) -> String {
    match t {
        FileType::Paper => "paper".into(),
        FileType::Office(FileKind::Pdf) => "pdf".into(),
        FileType::Office(FileKind::Docx) => "docx".into(),
        FileType::Office(FileKind::Pptx) => "pptx".into(),
        FileType::Office(FileKind::Xlsx) => "xlsx".into(),
        FileType::Text => entry["name"]
            .as_str()
            .and_then(|n| n.rsplit_once('.'))
            .map_or("txt".into(), |(_, e)| e.to_ascii_lowercase()),
    }
}

/// Paper를 받을 형식이에요: `markdown`, 없으면 `plain_text`.
fn export_format(entry: &Value) -> Option<&'static str> {
    let options = entry["export_info"]["export_options"].as_array()?;
    ["markdown", "plain_text"]
        .into_iter()
        .find(|f| options.iter().any(|o| o == f))
}

/// 경로가 맨 위(깊이 1)인지예요. 폴더에만 써요(`top_of`는 점이 든 이름을 파일로 봐요).
pub(crate) fn depth_one(path_lower: &str) -> bool {
    path_lower
        .strip_prefix('/')
        .is_some_and(|r| !r.is_empty() && !r.contains('/'))
}

impl Run<'_> {
    pub(crate) fn guard(&self) -> Result<(), SourceError> {
        if (self.still)() {
            Ok(())
        } else {
            Err(SourceError::service(
                Kind::Busy,
                "dropbox: connection changed",
            ))
        }
    }

    fn write(&self, id: &str, v: &Value) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        write_item(self.dir, id, v)
    }

    pub(crate) fn remove(&self, id: &str) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        remove_item(self.dir, id)
    }

    /// 받기의 상태를 써요. 끈 폴더, 다시 켠 폴더, `lost_since`는 다른 쪽(앱의 연결, 갱신)이 받는 도중에 바꿀 수 있어서
    /// 디스크의 것을 그대로 둬요.
    pub(crate) fn save(&self, s: &State) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        let disk = load_state(self.dir);
        if let (Some(ours), Some(theirs)) = (self.account, disk.account_id.as_deref())
            && ours != theirs
        {
            return Err(SourceError::service(
                Kind::Busy,
                "dropbox: the saved files are another account's",
            ));
        }
        let mut s = s.clone();
        s.muted = disk.muted;
        s.rescan = disk.rescan;
        s.lost_since = disk.lost_since;
        save_state(self.dir, &s)
    }

    pub(crate) fn account(&self) -> Result<Account, SourceError> {
        let v = self
            .api
            .rpc("users/get_current_account", &Value::Null, None)?
            .json("account")?;
        let info = &v["root_info"];
        let (Some(account_id), Some(root)) =
            (v["account_id"].as_str(), info["root_namespace_id"].as_str())
        else {
            return Err(SourceError::service(Kind::Down, "dropbox: no account id"));
        };
        Ok(Account {
            account_id: account_id.to_string(),
            email: v["email"].as_str().map(str::to_string),
            root: root.to_string(),
            root_tag: info[".tag"].as_str().unwrap_or("user").to_string(),
            home_path: info["home_path"].as_str().map(str::to_lowercase),
        })
    }

    /// 목록 전체예요. 재귀 하나로 받다가 `SPLIT_AFTER`번 실패하면(디스크에 세요) 같은 호출 안에서 폴더마다 받아요.
    /// 한 번 폴더마다 받기 시작하면 계속 그렇게 받아요. 뿌리 바뀜(422)은 서버 탈이 아니라서 세지 않아요.
    pub(crate) fn list_all(&self, s: &mut State) -> Result<Listing, SourceError> {
        let root = s.root.clone().unwrap_or_default();
        if s.split() || s.whole_failures >= SPLIT_AFTER {
            return self.list_split(&root);
        }
        match self.list_pages("", true, &root) {
            Ok(Ok((entries, cursor))) => {
                s.whole_failures = 0;
                Ok(Listing {
                    entries,
                    cursor: Some(cursor),
                    cursors: BTreeMap::new(),
                })
            }
            Ok(Err(_)) => Err(root_changed()),
            Err(e) if e.kind() == Kind::Down => {
                s.whole_failures += 1;
                self.save_whole_failures(s.whole_failures)?;
                if s.whole_failures >= SPLIT_AFTER {
                    self.list_split(&root)
                } else {
                    Err(e)
                }
            }
            Err(e) => Err(e),
        }
    }

    /// 실패 수만 디스크에 써요. 목록을 끝까지 받기 전에는 메모리의 상태(새 뿌리 같은 것)를 쓰지 않아요.
    fn save_whole_failures(&self, n: u32) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        let mut disk = load_state(self.dir);
        disk.whole_failures = n;
        save_state(self.dir, &disk)
    }

    /// 한 폴더의 목록을 끝까지 받아요: (항목, 마지막 커서). 뿌리가 바뀌었으면(422) `Down`이에요.
    pub(crate) fn list_from(
        &self,
        path: &str,
        recursive: bool,
        root: &str,
    ) -> Result<(Vec<Value>, String), SourceError> {
        self.list_pages(path, recursive, root)?
            .map_err(|_| root_changed())
    }

    /// `list_from`과 같되 뿌리 바뀜(422)을 `Ok(Err(Ended::RootChanged))`로 알려요.
    fn list_pages(
        &self,
        path: &str,
        recursive: bool,
        root: &str,
    ) -> Result<Result<(Vec<Value>, String), Ended>, SourceError> {
        let arg = json!({"path": path, "recursive": recursive, "include_deleted": false,
            "include_non_downloadable_files": true, "limit": LIST_LIMIT});
        let mut got = self.api.rpc("files/list_folder", &arg, Some(root))?;
        let mut entries = Vec::new();
        loop {
            if let Got::Fail { status: 422, .. } = got {
                return Ok(Err(Ended::RootChanged));
            }
            let v = got.json("list")?;
            entries.extend(v["entries"].as_array().into_iter().flatten().cloned());
            let Some(cursor) = v["cursor"].as_str().map(str::to_string) else {
                return Err(SourceError::service(Kind::Down, "dropbox: no cursor"));
            };
            if v["has_more"].as_bool() != Some(true) {
                return Ok(Ok((entries, cursor)));
            }
            got = self.api.rpc(
                "files/list_folder/continue",
                &json!({"cursor": cursor}),
                Some(root),
            )?;
        }
    }

    /// 목록의 파일 하나를 저장본에 맞춰요. 받을 내용이 바뀌었으면 `pending`에 더해요.
    pub(crate) fn upsert(&self, s: &mut State, entry: &Value) -> Result<(), SourceError> {
        let Some(id) = entry["id"].as_str().and_then(bare_id) else {
            return Ok(());
        };
        let old = read_item(self.dir, id);
        if !in_scope(entry, self.floor, &s.muted_paths()) {
            let kept = old.as_ref().is_some_and(|o| {
                kept_muted(s, self.floor, entry, o["path_lower"].as_str().unwrap_or(""))
            });
            match old {
                // 끈 폴더 안의 저장본은 글을 그대로 두고 받지 않아요. 끈 폴더 이름이 바뀌었으면 경로만 고쳐요
                Some(mut o) if kept => {
                    if o["path_lower"] != entry["path_lower"] {
                        for k in ["name", "path_lower", "path_display"] {
                            o[k] = entry[k].clone();
                        }
                        self.write(id, &o)?;
                    }
                }
                old => {
                    if old.is_some() {
                        self.remove(id)?;
                    }
                }
            }
            s.pending.retain(|p| p != id);
            s.retries.remove(id);
            return Ok(());
        }
        let Some(kind) = file_type(entry) else {
            return Ok(());
        };
        let ver = version(entry);
        let was = |k: &str| old.as_ref().map_or(Value::Null, |o| o[k].clone());
        let mut v = json!({
            "id": id,
            "name": entry["name"],
            "path_lower": entry["path_lower"],
            "path_display": entry["path_display"],
            "server_modified": entry["server_modified"],
            "rev": entry["rev"],
            "content_hash": entry["content_hash"],
            "size": entry["size"],
            "type": type_name(kind, entry),
            "export_format": if kind == FileType::Paper { json!(export_format(entry)) } else { Value::Null },
            "text": was("text"),
            "blocked": was("blocked"),
            "got": was("got"),
        });
        let size = entry["size"].as_u64().unwrap_or(0);
        if size > MAX_BYTES as u64 {
            v["text"] = Value::Null;
            v["blocked"] = json!(extract::Fail::TooLarge.reason());
            v["got"] = json!(ver);
            s.pending.retain(|p| p != id);
            s.retries.remove(id);
        } else if v["got"].as_str() != ver.as_deref() && !s.pending.iter().any(|p| p == id) {
            s.pending.push(id.to_string());
        }
        if old.as_ref() != Some(&v) {
            self.write(id, &v)?;
        }
        Ok(())
    }

    /// 목록의 폴더 하나예요: 맨 위 폴더면 `tops`에 넣고, 맨 위에서 더 아래로 옮겨졌으면 `tops`에서 빼요.
    pub(crate) fn upsert_folder(&self, s: &mut State, entry: &Value) {
        let (Some(id), Some(path)) = (
            entry["id"].as_str().and_then(bare_id),
            entry["path_lower"].as_str(),
        ) else {
            return;
        };
        if let Some(vault) = path.strip_suffix("/.obsidian")
            && !vault.is_empty()
        {
            s.vaults.insert(vault.to_string());
        }
        if depth_one(path) {
            if s.tops.get(id).is_some_and(|t| t.path_lower != path) {
                s.remember_muted_path(id);
            }
            s.tops.insert(
                id.to_string(),
                Top {
                    path_lower: path.to_string(),
                    name: entry["name"].as_str().unwrap_or("").to_string(),
                },
            );
        } else {
            s.tops.remove(id);
        }
    }

    /// 받은 결과를 저장본에 써요(부른 스레드에서만).
    fn apply(&self, s: &mut State, item: &Value, got: Outcome) -> Result<(), SourceError> {
        let id = item["id"].as_str().unwrap_or("");
        let mut v = item.clone();
        let got = match got {
            Outcome::Retry => {
                let n = s.retries.entry(id.to_string()).or_default();
                *n = n.saturating_add(1);
                if *n < RETRY_LIMIT {
                    return Ok(());
                }
                Outcome::Blocked("retryError".into())
            }
            other => other,
        };
        s.retries.remove(id);
        match got {
            Outcome::Text(t) => {
                v["text"] = json!(t);
                v["blocked"] = Value::Null;
            }
            Outcome::Blocked(why) => {
                eprintln!("dropbox: a file kept by title only ({why})");
                v["text"] = Value::Null;
                v["blocked"] = json!(why);
            }
            Outcome::Gone => {
                self.remove(id)?;
                s.pending.retain(|p| p != id);
                return Ok(());
            }
            Outcome::Retry => unreachable!("handled above"),
        }
        v["got"] = json!(version(item));
        self.write(id, &v)?;
        s.pending.retain(|p| p != id);
        Ok(())
    }

    /// `pending`을 최근 수정 순으로 `WORKERS`개씩 받아요. 남은 것이 있으면 참이에요.
    pub(crate) fn download_pending(
        &self,
        s: &mut State,
        budget: &mut Budget,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<bool, SourceError> {
        let root = s.root.clone().unwrap_or_default();
        let mut queue: Vec<Value> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        s.pending.retain(|id| {
            if !seen.insert(id.clone()) {
                return false;
            }
            match read_item(self.dir, id) {
                // 이미 이 내용을 받았어요
                Some(v) if !v["got"].is_null() && v["got"].as_str() == version(&v).as_deref() => {
                    false
                }
                Some(v) => {
                    queue.push(v);
                    true
                }
                None => false,
            }
        });
        let modified = |v: &Value| v["server_modified"].as_str().and_then(parse);
        queue.sort_by_key(|v| std::cmp::Reverse(modified(v)));
        let total = queue.len();
        if total == 0 {
            return Ok(false);
        }
        progress(0, total);
        let mut error: Option<SourceError> = None;
        let (job_tx, job_rx) = mpsc::channel::<usize>();
        let job_rx = Mutex::new(job_rx);
        let (res_tx, res_rx) = mpsc::channel::<(usize, Result<Outcome, SourceError>)>();
        std::thread::scope(|scope| {
            for _ in 0..WORKERS.min(total) {
                let (res_tx, job_rx, queue, root, api) =
                    (res_tx.clone(), &job_rx, &queue, &root, self.api);
                scope.spawn(move || {
                    loop {
                        let next = job_rx.lock().unwrap_or_else(PoisonError::into_inner).recv();
                        let Ok(i) = next else { return };
                        if res_tx.send((i, fetch_one(api, root, &queue[i]))).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(res_tx);
            let (mut next, mut busy, mut done) = (0, 0, 0);
            loop {
                while error.is_none() && busy < WORKERS && next < total && !budget.spent() {
                    budget.take();
                    if job_tx.send(next).is_err() {
                        break;
                    }
                    next += 1;
                    busy += 1;
                }
                if busy == 0 {
                    break;
                }
                let Ok((i, got)) = res_rx.recv() else { break };
                busy -= 1;
                done += 1;
                if let Err(e) = got.and_then(|o| self.apply(s, &queue[i], o)) {
                    error.get_or_insert(e);
                }
                progress(done, total);
                if done % SAVE_EVERY == 0
                    && error.is_none()
                    && let Err(e) = self.save(s)
                {
                    error = Some(e);
                }
            }
            drop(job_tx);
        });
        if let Some(e) = error {
            let _ = self.save(s);
            return Err(e);
        }
        Ok(!s.pending.is_empty())
    }
}

fn root_changed() -> SourceError {
    SourceError::service(Kind::Down, "dropbox: the path root changed")
}

/// 파일 하나의 글을 받아요. 파일 하나의 실패는 `Outcome`이고, 연결·한도·뿌리 바뀜은 오류예요.
pub(crate) fn fetch_one(
    api: &dyn DropboxApi,
    root: &str,
    item: &Value,
) -> Result<Outcome, SourceError> {
    let id = item["id"].as_str().unwrap_or("");
    let path = format!("id:{id}");
    let utf8 = |b: Vec<u8>| match String::from_utf8(b) {
        Ok(t) => Outcome::Text(clean(&t)),
        Err(_) => Outcome::Blocked("notUtf8".into()),
    };
    let got = match item["type"].as_str().unwrap_or("") {
        "paper" => {
            let Some(format) = item["export_format"].as_str() else {
                return Ok(Outcome::Blocked("noTextExport".into()));
            };
            match api.content(
                "files/export",
                &json!({"path": path, "export_format": format}),
                Some(root),
            )? {
                Got::Body(b) => return Ok(utf8(b)),
                fail => fail,
            }
        }
        "md" | "txt" => match api.content("files/download", &json!({"path": path}), Some(root))? {
            Got::Body(b) if b.len() > MAX_BYTES => {
                return Ok(Outcome::Blocked(extract::Fail::TooLarge.reason().into()));
            }
            Got::Body(b) if b.contains(&0) => return Ok(Outcome::Blocked("binary".into())),
            Got::Body(b) => return Ok(utf8(b)),
            fail => fail,
        },
        t => {
            let Some(kind) = FileKind::of_extension(t) else {
                return Ok(Outcome::Blocked("unknownType".into()));
            };
            match api.content("files/download", &json!({"path": path}), Some(root))? {
                Got::Body(b) => {
                    return Ok(match extract::text(b, kind) {
                        Ok(t) if kind == FileKind::Xlsx => {
                            Outcome::Text(head_lines(&t, SHEET_LINES))
                        }
                        Ok(t) => Outcome::Text(t),
                        Err(f) => Outcome::Blocked(f.reason().into()),
                    });
                }
                fail => fail,
            }
        }
    };
    let Got::Fail { status, summary } = got else {
        unreachable!("bodies returned above")
    };
    Ok(match failure(status, &summary) {
        Failure::NotFound => Outcome::Gone,
        Failure::Restricted => Outcome::Blocked("restricted_content".into()),
        Failure::Unsupported => Outcome::Blocked("unsupported_file".into()),
        Failure::Retry => Outcome::Retry,
        Failure::Reset => Outcome::Blocked("reset".into()),
        Failure::Other(t) => Outcome::Blocked(t),
        Failure::PathRoot => {
            return Err(SourceError::service(
                Kind::Down,
                "dropbox: the path root changed",
            ));
        }
    })
}

/// Dropbox에서 받아 `raw_dir`(state.json, items/)을 고쳐요. 몫이 차서 다 받지 못했으면 `Ok(true)`(다음 동기화가 이어
/// 받아요)예요. `relist`면 처음 받기를 끝내지 않은 저장본은 목록부터 다시 받아요. `progress(done, total)`은 받은 파일과
/// 받을 파일이에요.
#[allow(clippy::too_many_arguments)]
pub fn fetch_with(
    api: &dyn DropboxApi,
    raw_dir: &Path,
    now: DateTime<Utc>,
    account: Option<&str>,
    still: &dyn Fn() -> bool,
    relist: bool,
    budget: &mut Budget,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<bool, SourceError> {
    let run = Run {
        api,
        dir: raw_dir,
        still,
        account,
        floor: now - Duration::days(WINDOW_DAYS),
        now,
    };
    let mut s = load_state(raw_dir);
    let acct = run.account()?;
    if account.is_some_and(|a| a != acct.account_id) {
        return Err(SourceError::service(
            Kind::Busy,
            "dropbox: the connection is another account's",
        ));
    }
    if s.account_id
        .as_deref()
        .is_some_and(|a| a != acct.account_id)
    {
        // 다른 계정의 저장본이에요: 끈 폴더만 남기고 지워요
        let _held = state_lock();
        run.guard()?;
        match std::fs::remove_dir_all(raw_dir.join(ITEMS)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        s = State {
            muted: s.muted,
            account_id: Some(acct.account_id.clone()),
            ..State::default()
        };
        save_state(raw_dir, &s)?;
    }
    s.account_id = Some(acct.account_id.clone());
    s.account = acct.email.clone();
    s.root_tag = Some(acct.root_tag.clone());
    s.home_path = acct.home_path.clone();
    if s.root.as_deref().is_some_and(|r| r != acct.root) {
        // 뿌리가 바뀌었어요(팀에 들어감): 옛 커서는 읽지 않아요
        s.root = Some(acct.root.clone());
        run.relist(&mut s, Why::Root)?;
    } else if !s.has_cursor() || (relist && !s.initial_done) {
        s.root = Some(acct.root.clone());
        run.relist(&mut s, Why::Initial)?;
    } else {
        match run.changes(&mut s)? {
            Ended::Done => {}
            Ended::Reset => run.relist(&mut s, Why::Reset)?,
            Ended::RootChanged => {
                s.root = Some(run.account()?.root);
                run.relist(&mut s, Why::Root)?;
            }
        }
    }
    for folder in s.rescan.clone() {
        run.rescan(&mut s, &folder)?;
    }
    let mut total = 0;
    let more = run.download_pending(&mut s, budget, &mut |d, t| {
        total = t;
        progress(d, t);
    })?;
    if !more && s.pending.is_empty() && !s.initial_done {
        s.initial_done = true;
        s.reconciled_at = Some(iso(now));
    }
    if !more && reconcile_due(&s, now) {
        // 받은 것을 먼저 저장해요: 재조정이 실패해도 그대로 남아요
        run.save(&s)?;
        run.relist(&mut s, Why::Weekly)?;
    }
    s.fetched_at = Some(iso(now));
    run.save(&s)?;
    progress(total.max(1), total.max(1));
    Ok(more)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dropbox::state::{item_ids, load_state, read_item, save_state};
    use crate::error::Kind;
    use crate::gdrive::fetch::{BATCH_FILES, BATCH_TIME, Budget};
    use crate::testing::dropbox::FakeDropbox;
    use chrono::TimeZone;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration as StdDuration, Instant};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 13, 0, 0, 0).unwrap()
    }

    fn run(f: &FakeDropbox, dir: &Path) -> Result<bool, SourceError> {
        run_with(f, dir, Budget::new(BATCH_FILES, BATCH_TIME))
    }

    fn run_with(f: &FakeDropbox, dir: &Path, mut b: Budget) -> Result<bool, SourceError> {
        fetch_with(
            f,
            dir,
            now(),
            Some("dbid:A"),
            &|| true,
            true,
            &mut b,
            &mut |_, _| {},
        )
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/extract/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn text(dir: &Path, id: &str) -> String {
        read_item(dir, id).unwrap()["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn the_first_fetch_lists_with_path_root_and_reads_each_type() {
        let f = FakeDropbox::at(now());
        f.file("p", "/시험/note.pdf", 3, &fixture("policy.pdf"));
        f.file("w", "/시험/doc.docx", 3, &fixture("doc.docx"));
        f.file("x", "/시험/sheet.xlsx", 3, &fixture("sheet.xlsx"));
        f.file(
            "m",
            "/시험/결정-기록.md",
            3,
            "# 결정\n금요일 배포 금지".as_bytes(),
        );
        f.file(
            "pp",
            "/Paper 문서/😃 내 Paper 문서.paper",
            3,
            b"# Paper\n- [ ] \xEC\x9D\xBC\n![](https://paper-attachments.dropboxusercontent.com/s_1.png)",
        );
        let d = tempfile::tempdir().unwrap();
        assert!(!run(&f, d.path()).unwrap());
        assert_eq!(item_ids(d.path()), ["m", "p", "pp", "w", "x"]);
        assert!(text(d.path(), "p").contains("담당자는 지윤 님이에요"));
        assert!(text(d.path(), "w").contains("| 재시도 횟수 | 5 |"));
        assert!(text(d.path(), "x").contains("## 결정"));
        assert_eq!(text(d.path(), "m"), "# 결정\n금요일 배포 금지");
        let pp = read_item(d.path(), "pp").unwrap();
        assert_eq!(pp["export_format"], "markdown");
        assert_eq!(pp["type"], "paper");
        assert_eq!(pp["path_display"], "/Paper 문서/😃 내 Paper 문서.paper");
        assert_eq!(pp["got"], pp["content_hash"]);
        assert!(
            f.calls()
                .iter()
                .any(|c| c == "content files/export id:pp markdown")
        );
        assert!(f.calls().iter().any(|c| c == "content files/download id:p"));
        let s = load_state(d.path());
        assert_eq!(
            (s.root.as_deref(), s.root_tag.as_deref()),
            (Some("100"), Some("user"))
        );
        assert_eq!(s.account_id.as_deref(), Some("dbid:A"));
        assert!(s.initial_done && s.cursor.is_some() && s.pending.is_empty());
        assert!(
            s.reconciled_at.is_some(),
            "a fresh first listing counts as the first reconcile"
        );
        let tops: Vec<&str> = s.tops.values().map(|t| t.path_lower.as_str()).collect();
        assert_eq!(tops.len(), 2);
        assert!(tops.contains(&"/시험") && tops.contains(&"/paper 문서"));
    }

    #[test]
    fn a_root_folder_with_a_dot_is_still_a_top_and_root_files_are_not() {
        let f = FakeDropbox::at(now());
        f.folder("v12", "/v1.2");
        f.file("a", "/v1.2/a.md", 1, b"a");
        f.file("r", "/root.md", 1, b"r");
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        let s = load_state(d.path());
        assert_eq!(s.tops.len(), 1);
        assert_eq!(s.tops["v12"].path_lower, "/v1.2");
        assert_eq!(item_ids(d.path()), ["a", "r"]);
    }

    #[test]
    fn out_of_scope_files_are_not_fetched_and_large_ones_keep_their_title() {
        let f = FakeDropbox::at(now());
        f.file("old", "/a/old.md", 731, b"old");
        f.file("png", "/a/pic.png", 1, b"png");
        f.file("doc", "/a/old.doc", 1, b"doc");
        f.file("big", "/a/big.pptx", 1, &vec![0u8; MAX_BYTES + 1]);
        let d = tempfile::tempdir().unwrap();
        assert!(!run(&f, d.path()).unwrap());
        assert_eq!(item_ids(d.path()), ["big"]);
        let big = read_item(d.path(), "big").unwrap();
        assert!(big["text"].is_null());
        assert_eq!(big["blocked"], "tooLarge");
        assert_eq!(big["got"], big["content_hash"]);
        assert!(!f.calls().iter().any(|c| c.starts_with("content")));
        assert!(load_state(d.path()).initial_done);
    }

    #[test]
    fn text_files_must_be_utf8_without_nul() {
        let f = FakeDropbox::at(now());
        f.file("u", "/a/u.txt", 1, b"\xff\xfe");
        f.file("n", "/a/n.md", 1, b"a\0b");
        f.file("t", "/a/t.txt", 1, "\u{feff}a\r\nb".as_bytes());
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        assert_eq!(read_item(d.path(), "u").unwrap()["blocked"], "notUtf8");
        assert_eq!(read_item(d.path(), "n").unwrap()["blocked"], "binary");
        let t = read_item(d.path(), "t").unwrap();
        assert_eq!(t["text"], "a\nb");
        assert!(t["blocked"].is_null());
    }

    #[test]
    fn paper_without_markdown_or_plain_text_is_title_only() {
        let f = FakeDropbox::at(now());
        f.file("pp", "/a/회의.paper", 1, b"# x");
        f.paper_options("pp", &["html"]);
        f.file("pt", "/a/메모.paper", 1, b"plain");
        f.paper_options("pt", &["html", "plain_text"]);
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        let pp = read_item(d.path(), "pp").unwrap();
        assert_eq!(pp["blocked"], "noTextExport");
        assert!(pp["export_format"].is_null());
        assert!(!f.calls().iter().any(|c| c.contains("id:pp")));
        assert_eq!(read_item(d.path(), "pt").unwrap()["text"], "plain");
        assert!(
            f.calls()
                .iter()
                .any(|c| c == "content files/export id:pt plain_text")
        );
    }

    #[test]
    fn blocked_gone_and_not_ready_files() {
        let f = FakeDropbox::at(now());
        f.file("r", "/a/r.md", 1, b"r");
        f.file("u", "/a/u.pdf", 1, b"u");
        f.file("g", "/a/g.md", 1, b"g");
        f.file("e", "/a/e.paper", 1, b"# e");
        f.blocked("r", "path/restricted_content/");
        f.blocked("u", "unsupported_file/");
        f.blocked("g", "path/not_found/");
        f.blocked("e", "retry_error/");
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        assert_eq!(
            read_item(d.path(), "r").unwrap()["blocked"],
            "restricted_content"
        );
        assert_eq!(
            read_item(d.path(), "u").unwrap()["blocked"],
            "unsupported_file"
        );
        assert!(read_item(d.path(), "g").is_none());
        let e = read_item(d.path(), "e").unwrap();
        assert!(e["got"].is_null() && e["blocked"].is_null());
        let s = load_state(d.path());
        assert_eq!(s.pending, ["e"]);
        assert!(!s.initial_done);
        f.clear_failures();
        assert!(!run(&f, d.path()).unwrap());
        assert_eq!(text(d.path(), "e"), "# e");
        assert!(load_state(d.path()).initial_done);
    }

    #[test]
    fn a_small_budget_stops_early_newest_first_and_the_next_fetch_carries_on() {
        let f = FakeDropbox::at(now());
        for i in 1..=7 {
            f.file(
                &format!("f{i}"),
                &format!("/a/f{i}.md"),
                i,
                format!("# {i}").as_bytes(),
            );
        }
        let d = tempfile::tempdir().unwrap();
        assert!(run_with(&f, d.path(), Budget::new(3, StdDuration::from_secs(60))).unwrap());
        let got: Vec<String> = item_ids(d.path())
            .into_iter()
            .filter(|id| !read_item(d.path(), id).unwrap()["got"].is_null())
            .collect();
        assert_eq!(got, ["f1", "f2", "f3"]);
        let s = load_state(d.path());
        assert_eq!(s.pending.len(), 4);
        assert!(!s.initial_done);
        assert!(!run(&f, d.path()).unwrap());
        for i in 1..=7 {
            assert_eq!(text(d.path(), &format!("f{i}")), format!("# {i}"));
        }
        assert!(load_state(d.path()).initial_done);
    }

    #[test]
    fn same_content_hash_is_not_downloaded_again() {
        let f = FakeDropbox::at(now());
        f.file("m", "/a/m.md", 1, b"m");
        f.file("p", "/a/p.pdf", 1, &fixture("policy.pdf"));
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        let mut s = load_state(d.path());
        s.cursor = None;
        save_state(d.path(), &s).unwrap();
        f.clear_calls();
        run(&f, d.path()).unwrap();
        assert!(f.calls().iter().any(|c| c == "rpc files/list_folder  true"));
        assert!(
            !f.calls().iter().any(|c| c.starts_with("content")),
            "{:?}",
            f.calls()
        );
        assert_eq!(text(d.path(), "m"), "m");
    }

    #[test]
    fn four_downloads_run_at_once() {
        let f = FakeDropbox::at(now());
        for i in 0..8 {
            f.file(&format!("f{i}"), &format!("/a/f{i}.md"), 1, b"x");
        }
        // 300 ms each: four at once take about 0.6 s, one at a time 2.4 s. The 1.5 s line leaves a slow CI runner room
        // (100 ms each against 0.4 s failed at 0.44 s on GitHub's macOS runner, 2026-11-26)
        f.download_delay(StdDuration::from_millis(300));
        let d = tempfile::tempdir().unwrap();
        let t = Instant::now();
        run(&f, d.path()).unwrap();
        assert!(
            t.elapsed() < StdDuration::from_millis(1500),
            "{:?}",
            t.elapsed()
        );
        assert_eq!(f.max_in_flight(), WORKERS);
        assert_eq!(item_ids(d.path()).len(), 8);
    }

    #[test]
    fn another_accounts_saved_files_are_wiped_first() {
        let f = FakeDropbox::at(now());
        f.file("m", "/a/m.md", 1, b"m");
        let d = tempfile::tempdir().unwrap();
        save_state(
            d.path(),
            &State {
                account_id: Some("dbid:B".into()),
                muted: vec!["keep".into()],
                ..Default::default()
            },
        )
        .unwrap();
        crate::dropbox::state::write_item(d.path(), "old", &json!({"id": "old"})).unwrap();
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["m"]);
        let s = load_state(d.path());
        assert_eq!(s.account_id.as_deref(), Some("dbid:A"));
        assert_eq!(s.muted, ["keep"]);
    }

    #[test]
    fn a_changed_connection_stops_writing() {
        let f = FakeDropbox::at(now());
        for i in 0..3 {
            f.file(&format!("f{i}"), &format!("/a/f{i}.md"), 1, b"x");
        }
        let d = tempfile::tempdir().unwrap();
        // the connection changes once the listing is over and the first download starts
        let switched = AtomicBool::new(false);
        let still = || {
            if f.calls().iter().any(|c| c.starts_with("content")) {
                switched.store(true, Ordering::SeqCst);
            }
            !switched.load(Ordering::SeqCst)
        };
        let mut b = Budget::new(BATCH_FILES, BATCH_TIME);
        let e = fetch_with(
            &f,
            d.path(),
            now(),
            Some("dbid:A"),
            &still,
            true,
            &mut b,
            &mut |_, _| {},
        )
        .unwrap_err();
        assert_eq!(e.kind(), Kind::Busy);
        assert_eq!(item_ids(d.path()).len(), 3, "the listing was written");
        for id in item_ids(d.path()) {
            assert!(read_item(d.path(), &id).unwrap()["got"].is_null(), "{id}");
        }
    }

    #[test]
    fn a_wrong_path_root_is_down() {
        let f = FakeDropbox::at(now());
        let d = tempfile::tempdir().unwrap();
        let r = Run {
            api: &f,
            dir: d.path(),
            still: &|| true,
            account: None,
            floor: now(),
            now: now(),
        };
        assert_eq!(r.list_from("", true, "999").unwrap_err().kind(), Kind::Down);
        assert_eq!(r.list_from("", true, "100").unwrap().1, "c0");
    }

    #[test]
    fn a_file_that_keeps_asking_to_retry_is_kept_by_title_after_three_runs() {
        let f = FakeDropbox::at(now());
        f.file("e", "/a/e.paper", 1, b"# e");
        f.file("m", "/a/m.md", 1, b"m");
        f.blocked("e", "retry_error/");
        let d = tempfile::tempdir().unwrap();
        assert!(run(&f, d.path()).unwrap());
        assert!(run(&f, d.path()).unwrap());
        assert_eq!(load_state(d.path()).retries["e"], 2);
        assert!(!run(&f, d.path()).unwrap());
        let e = read_item(d.path(), "e").unwrap();
        assert_eq!(e["blocked"], "retryError");
        assert_eq!(e["got"], e["content_hash"]);
        let s = load_state(d.path());
        assert!(s.pending.is_empty() && s.retries.is_empty() && s.initial_done);
        assert_eq!(text(d.path(), "m"), "m");
    }

    #[test]
    fn a_pending_file_already_fetched_is_not_downloaded_again() {
        let f = FakeDropbox::at(now());
        f.file("m", "/a/m.md", 1, b"m");
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        let mut s = load_state(d.path());
        s.pending = vec!["m".into()];
        save_state(d.path(), &s).unwrap();
        f.clear_calls();
        assert!(!run(&f, d.path()).unwrap());
        assert!(!f.calls().iter().any(|c| c.starts_with("content")));
        assert!(load_state(d.path()).pending.is_empty());
    }

    #[test]
    fn another_connected_account_writes_nothing() {
        let f = FakeDropbox::at(now());
        f.file("m", "/a/m.md", 1, b"m");
        let d = tempfile::tempdir().unwrap();
        let mut b = Budget::new(BATCH_FILES, BATCH_TIME);
        let e = fetch_with(
            &f,
            d.path(),
            now(),
            Some("dbid:Z"),
            &|| true,
            true,
            &mut b,
            &mut |_, _| {},
        )
        .unwrap_err();
        assert_eq!(e.kind(), Kind::Busy);
        assert!(item_ids(d.path()).is_empty());
        assert!(!d.path().join("state.json").exists());
    }

    #[test]
    fn an_unknown_stored_type_is_kept_by_title() {
        let f = FakeDropbox::at(now());
        let item = json!({"id": "q", "type": "hwp"});
        assert_eq!(
            fetch_one(&f, "100", &item).unwrap(),
            Outcome::Blocked("unknownType".into())
        );
        assert!(f.calls().is_empty());
    }
}
