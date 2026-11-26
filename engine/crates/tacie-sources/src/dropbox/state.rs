//! Dropbox raw state (`state.json`) and file text (`items/<id>.json`) (docs/33 10절).
//!
//! Shape of a raw file: `id`, `name`, `path_lower`, `path_display`, `server_modified`, `rev`, `content_hash`, `size`,
//! `type` (`paper`|`pdf`|`docx`|`pptx`|`xlsx`|`md`|`txt`), `export_format` (`markdown`|`plain_text`|null),
//! `text` (text|null), `blocked` (reason|null), `got` (version of the fetched text|null). When both `got` and
//! `blocked` are null the file has not been fetched yet, so it produces no chunks.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SourceError;
use crate::extract::FileKind;
use crate::gdrive::fetch::{parse, safe};
use crate::store::{Stored, read_json, write_json};

pub(crate) const STATE: &str = "state.json";
pub(crate) const ITEMS: &str = "items";
/// 받는 범위예요(지난 2년).
pub(crate) const WINDOW_DAYS: i64 = 730;
/// 재조정 간격이에요(일).
pub(crate) const RECONCILE_DAYS: i64 = 7;

/// 맨 위 폴더 하나예요.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Top {
    pub path_lower: String,
    pub name: String,
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
    /// root_namespace_id예요.
    #[serde(default)]
    pub root: Option<String>,
    /// `user` 또는 `team`이에요.
    #[serde(default)]
    pub root_tag: Option<String>,
    /// 팀 공간에서 내 폴더(소문자 경로)예요.
    #[serde(default)]
    pub home_path: Option<String>,
    /// 재귀 하나로 받을 때의 커서예요.
    #[serde(default)]
    pub cursor: Option<String>,
    /// 폴더마다의 커서예요(뿌리는 `""`). 비어 있지 않으면 Split이에요.
    #[serde(default)]
    pub cursors: BTreeMap<String, String>,
    #[serde(default)]
    pub whole_failures: u32,
    /// 맨 위 폴더 id(`id:` 없이) → 경로예요.
    #[serde(default)]
    pub tops: BTreeMap<String, Top>,
    /// 끈 폴더 id예요(정렬, 중복 없음).
    #[serde(default)]
    pub muted: Vec<String>,
    /// 다시 켠 폴더 id예요.
    #[serde(default)]
    pub rescan: BTreeSet<String>,
    /// 받을 파일 id예요.
    #[serde(default)]
    pub pending: Vec<String>,
    /// 커서 도중의 지울 후보예요.
    #[serde(default)]
    pub doomed: BTreeSet<String>,
    /// 재조정에서 빠진 횟수예요.
    #[serde(default)]
    pub gone: BTreeMap<String, u8>,
    /// 받기가 `retry_error`로 끝난 횟수예요(파일 id → 동기화 수).
    #[serde(default)]
    pub retries: BTreeMap<String, u8>,
    #[serde(default)]
    pub initial_done: bool,
    #[serde(default)]
    pub reconciled_at: Option<String>,
    #[serde(default)]
    pub fetched_at: Option<String>,
    #[serde(default)]
    pub lost_since: Option<String>,
    /// 끈 맨 위 폴더가 이번 커서 읽기·다시 받기에서 옮겨지기 전 경로예요. 그 아래 저장본이 새 경로로 다시 오면 받지
    /// 않고 경로만 고쳐요. 커서를 끝까지 읽거나 다시 받기를 마치면 비워요(도중에 멈춰도 남아요).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub muted_moved: BTreeSet<String>,
    /// 옵시디언 vault 폴더(소문자 경로)예요: 바로 아래에 `.obsidian` 폴더가 있어요. 그 vault를 노트 출처가 받으면
    /// Dropbox 쪽은 넣지 않아요(docs/34 6절).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub vaults: BTreeSet<String>,
}

impl State {
    /// 폴더마다 커서를 쓰는지예요.
    pub fn split(&self) -> bool {
        !self.cursors.is_empty()
    }

    /// 끈 폴더 중 `tops`에 있는 것의 경로예요(`muted` 순서).
    pub fn muted_paths(&self) -> Vec<String> {
        self.muted
            .iter()
            .filter_map(|id| self.tops.get(id).map(|t| t.path_lower.clone()))
            .collect()
    }

    /// 이 경로의 맨 위 폴더가 끈 폴더인지예요. 경로가 아니라 그 경로에 있는 맨 위 폴더의 id로 봐요.
    pub fn muted_top(&self, path_lower: &str) -> bool {
        top_of(path_lower).is_some_and(|top| {
            self.tops
                .iter()
                .any(|(id, t)| t.path_lower == top && self.muted.contains(id))
        })
    }

    /// 저장본의 경로가 끈 폴더 아래였는지예요: 지금 끈 폴더이거나, 끈 폴더가 옮겨지기 전 경로(`muted_moved`)예요.
    pub fn was_muted(&self, path_lower: &str) -> bool {
        self.muted_top(path_lower)
            || top_of(path_lower).is_some_and(|top| self.muted_moved.contains(top))
    }

    /// 끈 맨 위 폴더 `id`가 옮겨지거나 사라지기 전 경로를 기억해요.
    pub fn remember_muted_path(&mut self, id: &str) {
        if self.muted.iter().any(|m| m == id)
            && let Some(t) = self.tops.get(id)
        {
            self.muted_moved.insert(t.path_lower.clone());
        }
    }

    pub fn has_cursor(&self) -> bool {
        self.cursor.is_some() || self.split()
    }
}

/// 받는 파일의 종류예요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileType {
    Paper,
    Office(FileKind),
    Text,
}

/// `id:abc` → `abc`예요. 파일 이름으로 쓸 수 없는 id는 None이에요.
pub(crate) fn bare_id(id: &str) -> Option<&str> {
    let bare = id.strip_prefix("id:").unwrap_or(id);
    safe(bare).then_some(bare)
}

/// 맨 위 폴더 경로예요: `/a/b/c.md` → `/a`, `/a` → `/a`예요. 맨 위에 놓인 파일(`/c.md`, 마지막 조각에 `.`이 있어요)은
/// 폴더가 아니라서 None이에요.
pub(crate) fn top_of(path_lower: &str) -> Option<&str> {
    let rest = path_lower.strip_prefix('/')?;
    match rest.find('/') {
        Some(0) => None,
        Some(i) => Some(&path_lower[..=i]),
        None if rest.is_empty() || rest.contains('.') => None,
        None => Some(path_lower),
    }
}

/// `prefix` 자신이거나 그 아래예요.
pub(crate) fn under(path_lower: &str, prefix: &str) -> bool {
    path_lower == prefix
        || path_lower
            .strip_prefix(prefix)
            .is_some_and(|r| r.starts_with('/'))
}

/// 바뀌었는지 보는 값이에요: `content_hash`, 없으면 `rev`예요.
pub(crate) fn version(entry: &Value) -> Option<String> {
    ["content_hash", "rev"]
        .iter()
        .find_map(|k| entry[*k].as_str())
        .map(str::to_string)
}

pub(crate) fn file_type(entry: &Value) -> Option<FileType> {
    if entry[".tag"].as_str() != Some("file") {
        return None;
    }
    let name = entry["name"].as_str()?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if ext == "paper" {
        return entry["export_info"].is_object().then_some(FileType::Paper);
    }
    if entry["is_downloadable"].as_bool() == Some(false) {
        return None;
    }
    match ext.as_str() {
        "md" | "txt" => Some(FileType::Text),
        _ => FileKind::of_extension(&ext).map(FileType::Office),
    }
}

/// 받을 파일인지예요: 종류, 나이, 끈 폴더를 봐요. 크기는 보지 않아요(받을 때 `tooLarge`로 남겨요).
pub(crate) fn in_scope(entry: &Value, floor: DateTime<Utc>, muted_paths: &[String]) -> bool {
    if file_type(entry).is_none() {
        return false;
    }
    let fresh = entry["server_modified"]
        .as_str()
        .and_then(parse)
        .is_some_and(|t| t > floor);
    let path = entry["path_lower"].as_str().unwrap_or("");
    let muted = top_of(path).is_some_and(|top| muted_paths.iter().any(|m| m == top));
    fresh && !muted
}

/// 끈 폴더 안의 저장본(옛 경로 `old_path`)을 글 그대로 두는지예요: 새 경로도 끈 폴더 아래이고, 끄지 않았다면 받을
/// 파일이고, 옛 경로가 끈 폴더(옮겨지기 전 경로 포함) 아래였어요. 다른 곳에서 끈 폴더로 옮겨 온 파일은 지워요.
pub(crate) fn kept_muted(s: &State, floor: DateTime<Utc>, entry: &Value, old_path: &str) -> bool {
    s.muted_top(entry["path_lower"].as_str().unwrap_or(""))
        && in_scope(entry, floor, &[])
        && s.was_muted(old_path)
}

static STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn state_lock() -> std::sync::MutexGuard<'static, ()> {
    STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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

pub(crate) fn write_item(dir: &Path, id: &str, v: &Value) -> Result<(), SourceError> {
    if !safe(id) {
        return Err(SourceError::Other("unsafe item id".into()));
    }
    write_json(&item_path(dir, id), v, None)?;
    Ok(())
}

/// 없는 파일을 지우는 건 괜찮아요.
pub(crate) fn remove_item(dir: &Path, id: &str) -> Result<(), SourceError> {
    if !safe(id) {
        return Ok(());
    }
    match fs::remove_file(item_path(dir, id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// 저장된 파일 id예요(정렬).
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

/// 끌 폴더를 정해요(`id:` 접두는 떼요). 바뀌었으면 참이에요. 다시 켠 폴더는 `rescan`에 더해요.
pub fn set_muted(dir: &Path, muted: &[String]) -> Result<bool, SourceError> {
    let _g = state_lock();
    let mut next: Vec<String> = muted
        .iter()
        .filter_map(|id| bare_id(id).map(str::to_string))
        .collect();
    next.sort();
    next.dedup();
    let mut s = load_state(dir);
    if s.muted == next {
        return Ok(false);
    }
    for id in &s.muted {
        if !next.contains(id) {
            s.rescan.insert(id.clone());
        }
    }
    s.muted = next;
    save_state(dir, &s)?;
    Ok(true)
}

pub fn muted(dir: &Path) -> Vec<String> {
    load_state(dir).muted
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use serde_json::json;

    use crate::gdrive::fetch::iso;

    #[test]
    fn ids_lose_their_prefix_and_odd_ids_are_refused() {
        assert_eq!(bare_id("id:a4ayc_80_OEAAAAAAAAAXw"), Some("a4ayc_80_OEAAAAAAAAAXw"));
        assert_eq!(bare_id("id:../x"), None);
        assert_eq!(bare_id(""), None);
    }

    #[test]
    fn top_of_root_file_is_none() {
        assert_eq!(top_of("/영업/제안서/a.pdf"), Some("/영업"));
        assert_eq!(top_of("/a.pdf"), None);
        assert_eq!(top_of("/영업"), Some("/영업")); // 폴더 자신
    }

    #[test]
    fn under_needs_a_slash_boundary() {
        assert!(under("/a", "/a") && under("/a/x.md", "/a"));
        assert!(!under("/ab/x.md", "/a"));
    }

    #[test]
    fn version_prefers_content_hash_then_rev() {
        assert_eq!(version(&json!({"content_hash": "h", "rev": "r"})).as_deref(), Some("h"));
        assert_eq!(version(&json!({"rev": "r"})).as_deref(), Some("r"));
    }

    #[test]
    fn file_type_reads_extensions_case_insensitively_and_refuses_odd_entries() {
        let f = |name: &str, extra: Value| {
            let mut v = json!({".tag": "file", "name": name, "is_downloadable": true});
            v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            v
        };
        assert_eq!(file_type(&f("A.PDF", json!({}))), Some(FileType::Office(FileKind::Pdf)));
        assert_eq!(
            file_type(&f(
                "회의.Paper",
                json!({"is_downloadable": false,
                    "export_info": {"export_as": "html", "export_options": ["html", "markdown"]}})
            )),
            Some(FileType::Paper)
        );
        assert_eq!(file_type(&f("x.paper", json!({"is_downloadable": false}))), None); // export_info 없음
        assert_eq!(file_type(&f("x.docx", json!({"is_downloadable": false}))), None);
        assert_eq!(file_type(&f("notes.MD", json!({}))), Some(FileType::Text));
        for n in ["a.doc", "a.hwp", "a.png", "a.gdoc", "a.rs", "a.markdown"] {
            assert_eq!(file_type(&f(n, json!({}))), None, "{n}");
        }
        assert_eq!(file_type(&json!({".tag": "folder", "name": "a.pdf"})), None);
    }

    #[test]
    fn in_scope_checks_age_type_and_muted_tops() {
        let now = Utc.with_ymd_and_hms(2026, 11, 13, 0, 0, 0).unwrap();
        let floor = now - Duration::days(WINDOW_DAYS);
        let e = |path: &str, days: i64| {
            json!({".tag": "file", "name": "a.md", "path_lower": path,
                "server_modified": iso(now - Duration::days(days)), "is_downloadable": true})
        };
        assert!(in_scope(&e("/a/a.md", 10), floor, &[]));
        assert!(!in_scope(&e("/a/a.md", 731), floor, &[]));
        assert!(!in_scope(&e("/a/a.md", 10), floor, &["/a".into()]));
        assert!(in_scope(&e("/ab/a.md", 10), floor, &["/a".into()]));
    }

    #[test]
    fn state_and_items_round_trip_and_mutes_are_sorted() {
        let d = tempfile::tempdir().unwrap();
        let mut s = State::default();
        s.cursors.insert("".into(), "c1".into());
        s.pending = vec!["b".into(), "a".into()];
        save_state(d.path(), &s).unwrap();
        let back = load_state(d.path());
        assert!(back.split() && back.has_cursor());
        assert_eq!(back.pending, ["b", "a"]);
        write_item(d.path(), "a", &json!({"id": "a"})).unwrap();
        assert_eq!(item_ids(d.path()), ["a"]);
        remove_item(d.path(), "a").unwrap();
        remove_item(d.path(), "a").unwrap(); // 없어도 괜찮아요
        assert!(set_muted(d.path(), &["f2".into(), "f1".into(), "f1".into()]).unwrap());
        assert!(!set_muted(d.path(), &["f1".into(), "f2".into()]).unwrap());
        assert_eq!(muted(d.path()), ["f1", "f2"]);
    }

    #[test]
    fn unmuting_a_folder_queues_a_rescan() {
        let d = tempfile::tempdir().unwrap();
        set_muted(d.path(), &["f1".into(), "f2".into()]).unwrap();
        set_muted(d.path(), &["f2".into()]).unwrap();
        assert_eq!(load_state(d.path()).rescan.into_iter().collect::<Vec<_>>(), ["f1"]);
    }
}
