//! The source for note folders on this Mac (docs/34). When the app sends the folders the user picked with
//! `POST /notes/folders`, they are kept in `raw/notes/folders.json`, and on refresh the `.md` files in the folders are
//! read and each file becomes one document (`notes/<폴더 id>/<경로>`). A folder with `.obsidian/` at the top is treated
//! as an Obsidian vault, and its cards open in Obsidian.
//!
//! - Folders and files starting with a dot (`.obsidian`, `.trash`, `.git`) and Obsidian's "Excluded files" (path
//!   prefixes in `userIgnoreFilters` of `app.json`) are left out.
//! - The body of each file is read up to `MAX_BYTES`; larger or non-UTF-8 files get only the title.
//! - iCloud files not downloaded yet (`SF_DATALESS`) are read too. Reading makes macOS download them (docs/34 0절). If
//!   one file takes longer than `FETCH_WAIT`, only the title goes in this time and it tries again on the next refresh.
//! - `acl` is `["*"]` (the index is per account, same as past meetings).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tacie_index::RawChunk;

use crate::doc::{Doc, windows_at};
use crate::gdrive::docs::{join, markdown_lines, md, table_cuts, ymd};
use crate::store::{Stored, read_json, write_json};

/// 고른 폴더를 두는 파일이에요(`raw/notes` 아래).
pub const FILE: &str = "folders.json";
/// 조각의 `src`예요.
pub const SRC: &str = "notes";
/// 본문을 읽는 파일 크기 상한이에요.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
/// 한 폴더에서 읽는 파일 수 상한이에요(잘못 고른 홈 폴더 같은 것).
pub const MAX_FILES: usize = 20_000;
/// 아직 받지 않은 iCloud 파일 하나를 기다리는 시간이에요.
pub const FETCH_WAIT: Duration = Duration::from_secs(30);

const SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// 앱이 보낸 폴더 하나예요.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// 절대 경로예요.
    pub path: String,
    /// 옵시디언 vault ID(16글자)예요. 앱이 `obsidian.json`에서 찾았을 때만 와요.
    #[serde(default)]
    pub vault_id: Option<String>,
    /// 화면과 카드에 쓰는 이름이에요. 없으면 폴더 이름이에요.
    #[serde(default)]
    pub name: Option<String>,
}

impl Folder {
    fn root(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }

    /// 조각 경로에 쓰는 id예요: vault ID, 없으면 경로의 해시 16글자예요.
    pub fn id(&self) -> String {
        match &self.vault_id {
            Some(v) if !v.is_empty() => v.clone(),
            _ => hex::encode(&Sha256::digest(self.path.as_bytes())[..8]),
        }
    }

    pub fn label(&self) -> String {
        self.name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| {
                self.root()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.path.clone())
            })
    }

    /// 맨 위에 `.obsidian/`이 있으면 옵시디언 vault예요.
    pub fn obsidian(&self) -> bool {
        self.root().join(".obsidian").is_dir()
    }
}

/// 폴더 안에서 찾은 노트 파일 하나예요.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// 폴더 안 경로(`/`로 이음)예요.
    pub rel: String,
    pub full: PathBuf,
    pub size: u64,
    /// 수정 시각(UNIX 초)이에요.
    pub mtime: i64,
}

fn read(raw_dir: &Path) -> Option<Map<String, Value>> {
    match read_json(&raw_dir.join(FILE)) {
        Ok(Stored::Value(Value::Object(m))) => Some(m),
        _ => None,
    }
}

/// 저장된 폴더 목록이에요.
pub fn folders(raw_dir: &Path) -> Vec<Folder> {
    read(raw_dir)
        .and_then(|m| m.get("folders").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 고른 폴더 목록을 저장해요. 저장된 목록과 같으면 쓰지 않고 false예요.
pub fn save(raw_dir: &Path, list: &[Folder]) -> std::io::Result<bool> {
    let _one = SAVING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if read(raw_dir).is_some() && folders(raw_dir) == list {
        return Ok(false);
    }
    let saved_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    write_json(
        &raw_dir.join(FILE),
        &json!({"folders": list, "saved_at": saved_at}),
        None,
    )?;
    Ok(true)
}

/// 고른 폴더를 지워요(연결 해제). 다음 갱신이 노트 조각을 빼요.
pub fn forget(raw_dir: &Path) -> std::io::Result<()> {
    match fs::remove_file(raw_dir.join(FILE)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// 옵시디언 "제외한 파일" 중 경로 앞부분인 것이에요. `/…/` 꼴(정규식)은 아직 따르지 않아요.
fn ignored_prefixes(root: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(root.join(".obsidian").join("app.json")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    v["userIgnoreFilters"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .filter(|s| !(s.len() > 1 && s.starts_with('/') && s.ends_with('/')))
                .map(|s| s.trim_start_matches('/').to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn mtime_of(m: &fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// 폴더 안의 `.md` 파일이에요(경로 순). 심볼릭 링크는 따라가지 않아요. 폴더가 없으면 비어요.
pub fn scan(folder: &Folder) -> Vec<Note> {
    let root = folder.root();
    let skip = ignored_prefixes(&root);
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            let full = e.path();
            let rel = full
                .strip_prefix(&root)
                .map(|p| {
                    p.components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .unwrap_or_default();
            if skip.iter().any(|s| {
                let s = s.trim_end_matches('/');
                rel == s || rel.starts_with(&format!("{s}/"))
            }) {
                continue;
            }
            if ft.is_dir() {
                stack.push(full);
            } else if ft.is_file() && name.to_lowercase().ends_with(".md") {
                let Ok(m) = e.metadata() else { continue };
                out.push(Note {
                    rel,
                    full,
                    size: m.len(),
                    mtime: mtime_of(&m),
                });
                if out.len() >= MAX_FILES {
                    stack.clear();
                    break;
                }
            }
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

/// 폴더들의 파일 목록 지문이에요(경로·크기·수정 시각). 같으면 다시 읽지 않아요.
pub fn signature(list: &[Folder]) -> String {
    let mut h = Sha256::new();
    for f in list {
        h.update(f.path.as_bytes());
        h.update([0]);
        h.update(f.vault_id.as_deref().unwrap_or("").as_bytes());
        h.update([0]);
        h.update(f.label().as_bytes());
        h.update([1]);
        for n in scan(f) {
            h.update(n.rel.as_bytes());
            h.update(format!("\0{}\0{}\n", n.size, n.mtime).as_bytes());
        }
    }
    hex::encode(&h.finalize()[..16])
}

#[cfg(target_os = "macos")]
fn dataless(path: &Path) -> bool {
    use std::os::macos::fs::MetadataExt;
    // SF_DATALESS: 아직 받지 않은 iCloud 파일이에요
    fs::symlink_metadata(path).is_ok_and(|m| m.st_flags() & 0x4000_0000 != 0)
}

#[cfg(not(target_os = "macos"))]
fn dataless(_: &Path) -> bool {
    false
}

/// 파일 글이에요. 너무 크거나 UTF-8이 아니거나, 받아 오는 데 `wait`보다 오래 걸리면 None이에요.
pub fn read_text(note: &Note, wait: Duration) -> Option<String> {
    if note.size > MAX_BYTES {
        return None;
    }
    let bytes = if dataless(&note.full) {
        // 받아 오는 동안 읽기가 멈춰요: 따로 읽고 기다리는 시간을 둬요. 늦으면 그 스레드는 혼자 끝나게 둬요.
        let (tx, rx) = std::sync::mpsc::channel();
        let path = note.full.clone();
        std::thread::Builder::new()
            .name("notes-fetch".into())
            .spawn(move || {
                let _ = tx.send(fs::read(path));
            })
            .ok()?;
        rx.recv_timeout(wait).ok()?.ok()?
    } else {
        fs::read(&note.full).ok()?
    };
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// 머리말(frontmatter)에서 쓰는 것이에요.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Front {
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
        .unwrap_or(s);
    s.to_string()
}

fn inline_list(v: &str) -> Vec<String> {
    let v = v.trim();
    let inner = v.strip_prefix('[').and_then(|x| x.strip_suffix(']'));
    let parts: Vec<&str> = match inner {
        Some(i) => i.split(',').collect(),
        None if v.is_empty() => Vec::new(),
        None => v.split(',').collect(),
    };
    parts
        .into_iter()
        .map(unquote)
        .map(|s| s.trim_start_matches('#').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 글 맨 앞의 `---` 머리말을 떼어요: (머리말, 나머지 글)이에요. 머리말이 없으면 기본값과 글 그대로예요.
pub fn split_front(text: &str) -> (Front, &str) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (Front::default(), text);
    };
    let mut end = None;
    let mut pos = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" || line.trim_end() == "..." {
            end = Some((pos, pos + line.len()));
            break;
        }
        pos += line.len();
    }
    let Some((yaml_end, body_start)) = end else {
        return (Front::default(), text);
    };
    let yaml = &rest[..yaml_end];
    let mut front = Front::default();
    let mut list: Option<&str> = None;
    for line in yaml.lines() {
        if let Some(item) = line.trim_start().strip_prefix("- ")
            && line.starts_with([' ', '-'])
        {
            let v = unquote(item).trim_start_matches('#').to_string();
            match list {
                Some("tags") if !v.is_empty() => front.tags.push(v),
                Some("aliases") if !v.is_empty() => front.aliases.push(v),
                _ => {}
            }
            continue;
        }
        list = None;
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let k = k.trim();
        match k {
            "title" => {
                let t = unquote(v);
                if !t.is_empty() {
                    front.title = Some(t);
                }
            }
            "tags" | "tag" => {
                if v.trim().is_empty() {
                    list = Some("tags");
                } else {
                    front.tags.extend(inline_list(v));
                }
            }
            "aliases" | "alias" => {
                if v.trim().is_empty() {
                    list = Some("aliases");
                } else {
                    front.aliases.extend(inline_list(v));
                }
            }
            _ => {}
        }
    }
    (front, &rest[body_start..])
}

static EMBED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[\[[^\]\n]*\]\]").unwrap());
static WIKI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\]\n]*)\]\]").unwrap());
static IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[[^\]\n]*\]\([^)\n]*\)").unwrap());
static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)%%.*?%%").unwrap());

/// 옵시디언 문법을 읽는 글로 바꿔요: `![[…]]`·그림·`%%주석%%`은 지우고, `[[노트|별칭]]`은 별칭(없으면 노트 이름)만
/// 남겨요.
pub fn plain(text: &str) -> String {
    let t = COMMENT.replace_all(text, "");
    let t = EMBED.replace_all(&t, "");
    let t = IMAGE.replace_all(&t, "");
    WIKI.replace_all(&t, |c: &regex::Captures| {
        let inner = &c[1];
        match inner.split_once('|') {
            Some((_, alias)) => alias.to_string(),
            None => inner.split('#').next().unwrap_or(inner).to_string(),
        }
    })
    .into_owned()
}

fn enc(s: &str) -> String {
    utf8_percent_encode(s, SET).to_string()
}

/// 카드가 여는 주소예요. 옵시디언 vault이고 vault ID를 알면(이 Mac의 옵시디언이 그 vault를 연 적이 있어요)
/// `obsidian://open`, 아니면 `file://`(기본 앱)예요.
pub fn link(folder: &Folder, rel: &str, obsidian: bool) -> String {
    let vault = folder.vault_id.as_deref().filter(|v| !v.is_empty());
    if let (true, Some(vault)) = (obsidian, vault) {
        format!("obsidian://open?vault={}&file={}", enc(vault), enc(rel))
    } else {
        let full = folder.root().join(rel);
        let path: Vec<String> = full.to_string_lossy().split('/').map(enc).collect();
        format!("file://{}", path.join("/"))
    }
}

fn iso(mtime: i64) -> String {
    chrono::DateTime::from_timestamp(mtime, 0)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// 노트 하나의 문서와 창 시작 줄이에요. `text`가 None이면(못 읽음) 제목 한 줄이 본문이에요.
pub fn document(
    folder: &Folder,
    obsidian: bool,
    note: &Note,
    text: Option<&str>,
) -> (Doc, Vec<usize>) {
    let stem = note
        .rel
        .rsplit('/')
        .next()
        .unwrap_or(&note.rel)
        .trim_end_matches(".md")
        .trim_end_matches(".MD")
        .to_string();
    let (front, body_text) = text.map_or((Front::default(), ""), split_front);
    let title = front.title.clone().unwrap_or(stem);
    let url = link(folder, &note.rel, obsidian);
    let when = iso(note.mtime);
    let folders: Vec<&str> = note
        .rel
        .rsplit_once('/')
        .map_or("", |(p, _)| p)
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    let edited = if when.is_empty() {
        String::new()
    } else {
        format!("{} 수정", md(&when))
    };
    let place = if obsidian { "Obsidian" } else { "노트" };
    let mut head = format!(
        "notes: {title}\nmeta: {}",
        join(&[place, &folder.label(), &folders.join(" / "), &edited])
    );
    // 별칭과 태그는 머리에 붙여요: 조각마다 앞에 붙어서 어느 창이든 그 말로 찾혀요
    if !front.aliases.is_empty() {
        head.push_str(&format!("\naliases: {}", front.aliases.join(", ")));
    }
    if !front.tags.is_empty() {
        let tags: Vec<String> = front.tags.iter().map(|t| format!("#{t}")).collect();
        head.push_str(&format!("\ntags: {}", tags.join(" ")));
    }
    let (all, mut starts) = markdown_lines(&plain(body_text));
    starts.extend(table_cuts(&all, &starts));
    let mut body: Vec<(String, String)> = all.into_iter().map(|(l, _)| (l, url.clone())).collect();
    if body.iter().all(|(l, _)| l.trim().is_empty()) {
        body = vec![(title.clone(), url.clone())];
        starts.clear();
    }
    (
        Doc {
            path: format!("notes/{}/{}", folder.id(), note.rel),
            title,
            url,
            author: None,
            at: ymd(&when),
            acl: Some(vec!["*".to_string()]),
            head,
            body,
        },
        starts,
    )
}

/// 갱신의 노트 몫이에요: 고른 폴더의 조각과 info(`{folders: [{id, name, obsidian, notes, missing}], notes, chunks,
/// saved_at, signature}`)예요. 폴더 파일이 없으면 둘 다 비어요.
pub fn gather(raw_dir: &Path) -> (Vec<RawChunk>, Option<Value>) {
    gather_with(raw_dir, FETCH_WAIT)
}

pub fn gather_with(raw_dir: &Path, wait: Duration) -> (Vec<RawChunk>, Option<Value>) {
    let Some(raw) = read(raw_dir) else {
        return (Vec::new(), None);
    };
    let list = folders(raw_dir);
    let mut chunks = Vec::new();
    let mut infos = Vec::new();
    let mut total = 0;
    let mut late = 0;
    for f in &list {
        let missing = !f.root().is_dir();
        let obsidian = f.obsidian();
        let notes = scan(f);
        for n in &notes {
            let text = read_text(n, wait);
            if text.is_none() && n.size <= MAX_BYTES && dataless(&n.full) {
                late += 1;
            }
            let (d, starts) = document(f, obsidian, n, text.as_deref());
            chunks.extend(windows_at(&d, SRC, &starts));
        }
        total += notes.len();
        infos.push(json!({
            "id": f.id(),
            "name": f.label(),
            "obsidian": obsidian,
            "notes": notes.len(),
            "missing": missing,
        }));
    }
    let info = json!({
        "folders": infos,
        "notes": total,
        "chunks": chunks.len(),
        "not_downloaded": late,
        "saved_at": raw.get("saved_at").cloned().unwrap_or(Value::Null),
        "signature": signature(&list),
        "read_at": iso(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)),
    });
    (chunks, Some(info))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(dir: &Path, vault: Option<&str>) -> Folder {
        Folder {
            path: dir.to_string_lossy().into_owned(),
            vault_id: vault.map(str::to_string),
            name: Some("일 노트".into()),
        }
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    #[test]
    fn scan_skips_dot_folders_other_types_and_ignored_paths() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "결정/캐시.md", "a");
        write(d.path(), "a.md", "a");
        write(d.path(), "b.canvas", "{}");
        write(
            d.path(),
            ".obsidian/app.json",
            r#"{"userIgnoreFilters": ["보관/", "/정규식/"]}"#,
        );
        write(d.path(), ".trash/old.md", "x");
        write(d.path(), "보관/old.md", "x");
        let rels: Vec<String> = scan(&folder(d.path(), None))
            .into_iter()
            .map(|n| n.rel)
            .collect();
        assert_eq!(rels, vec!["a.md", "결정/캐시.md"]);
    }

    #[test]
    fn front_matter_gives_title_tags_and_aliases_and_is_cut_from_the_body() {
        let t = "---\ntitle: \"캐시 결정\"\ntags: [infra, \"#결정\"]\naliases:\n  - 캐시\n---\n# 왜\n본문\n";
        let (f, rest) = split_front(t);
        assert_eq!(f.title.as_deref(), Some("캐시 결정"));
        assert_eq!(f.tags, vec!["infra", "결정"]);
        assert_eq!(f.aliases, vec!["캐시"]);
        assert_eq!(rest, "# 왜\n본문\n");
        let (f, rest) = split_front("본문만\n---\n");
        assert_eq!(f, Front::default());
        assert_eq!(rest, "본문만\n---\n");
    }

    #[test]
    fn wiki_links_keep_the_visible_words_and_embeds_go() {
        let t =
            "[[회의/10월|10월 회의]]에서 [[캐시#이유]] 정함 ![[그림.png]] ![a](b.png) %%메모%%끝";
        assert_eq!(plain(t), "10월 회의에서 캐시 정함   끝");
    }

    #[test]
    fn a_vault_note_opens_in_obsidian_and_a_plain_folder_note_opens_the_file() {
        let d = tempfile::tempdir().unwrap();
        let v = folder(d.path(), Some("0123456789abcdef"));
        assert_eq!(
            link(&v, "회의/결정 기록.md", true),
            "obsidian://open?vault=0123456789abcdef&file=%ED%9A%8C%EC%9D%98%2F%EA%B2%B0%EC%A0%95%20%EA%B8%B0%EB%A1%9D.md"
        );
        let plainf = Folder {
            path: "/tmp/노트".into(),
            vault_id: None,
            name: None,
        };
        assert_eq!(
            link(&plainf, "a b.md", false),
            "file:///tmp/%EB%85%B8%ED%8A%B8/a%20b.md"
        );
        // a vault Obsidian has never opened on this Mac has no ID: the file opens in its default app
        assert_eq!(
            link(&plainf, "a b.md", true),
            "file:///tmp/%EB%85%B8%ED%8A%B8/a%20b.md"
        );
    }

    #[test]
    fn gather_makes_one_document_per_note_with_head_and_tags() {
        let d = tempfile::tempdir().unwrap();
        let vault = d.path().join("vault");
        write(&vault, ".obsidian/app.json", "{}");
        write(
            &vault,
            "결정/캐시.md",
            "---\ntags: [infra]\n---\n# 캐시\nTTL은 [[지표|지표 회의]]에서 5분으로 정했어요.\n",
        );
        write(&vault, "빈.md", "");
        let raw = d.path().join("raw");
        assert!(save(&raw, &[folder(&vault, Some("0123456789abcdef"))]).unwrap());
        assert!(!save(&raw, &[folder(&vault, Some("0123456789abcdef"))]).unwrap());
        let (chunks, info) = gather(&raw);
        let info = info.unwrap();
        assert_eq!(info["notes"], 2);
        assert_eq!(info["folders"][0]["obsidian"], true);
        let c = chunks
            .iter()
            .find(|c| c["path"] == "notes/0123456789abcdef/결정/캐시.md")
            .unwrap();
        assert_eq!(c["src"], "notes");
        assert_eq!(c["acl"], json!(["*"]));
        let text = c["text"].as_str().unwrap();
        assert!(
            text.starts_with("notes: 캐시\nmeta: Obsidian · 일 노트 · 결정 · "),
            "{text}"
        );
        assert!(text.contains("tags: #infra"), "{text}");
        assert!(text.contains("지표 회의에서 5분으로"), "{text}");
        assert!(
            c["url"]
                .as_str()
                .unwrap()
                .starts_with("obsidian://open?vault=0123456789abcdef&file=")
        );
        // 빈 노트는 제목으로 찾아요
        assert!(
            chunks
                .iter()
                .any(|c| c["path"] == "notes/0123456789abcdef/빈.md")
        );
    }

    #[test]
    fn a_big_or_binary_note_is_searched_by_its_title() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "큰.md",
            &"가".repeat((MAX_BYTES as usize) / 3 + 10),
        );
        fs::write(d.path().join("이진.md"), b"a\0b").unwrap();
        let f = folder(d.path(), None);
        for n in scan(&f) {
            assert!(read_text(&n, FETCH_WAIT).is_none(), "{}", n.rel);
            let (doc, _) = document(&f, false, &n, None);
            assert_eq!(doc.body.len(), 1);
        }
    }

    #[test]
    fn signature_changes_when_a_note_changes() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "a.md", "1");
        let f = vec![folder(d.path(), None)];
        let before = signature(&f);
        assert_eq!(before, signature(&f));
        write(d.path(), "a.md", "12");
        assert_ne!(before, signature(&f));
    }

    #[test]
    fn forget_removes_the_folder_list() {
        let d = tempfile::tempdir().unwrap();
        save(d.path(), &[folder(d.path(), None)]).unwrap();
        forget(d.path()).unwrap();
        assert!(gather(d.path()).1.is_none());
        forget(d.path()).unwrap();
    }
}
