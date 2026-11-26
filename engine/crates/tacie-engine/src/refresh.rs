//! Brings the index up to date (the refresher `Refresher`, `rebuild_target` for `/index/rebuild`, `describe` for
//! the connect screen).
//!
//! Repo code comes from the GitHub API (spec 5절): the commit at the branch head (`content::head_commit`), its tree,
//! and the bytes of files not in the chunk cache (`content::snapshot_of`). No clone is made, git is not launched,
//! and the user's checkout is not touched. The new index is written in full next to the current one (`index.new`),
//! then swapped in whole, `index` → `index.old` and `index.new` → `index`, and only then does the engine answer
//! from the new index. If this is cut off in between, `recover` at the next start rolls it back. Other repos picked
//! in the app each have their own slot name (`data/sources/repos/<owner>__<name>`, the old clone location) and
//! GitHub raw data (`data/sources/github-repos/<owner>__<name>`), and `data/sources/repos.json` remembers for each
//! repo its slot, raw data and graph, the last branch, and whether it had Slack. The slot name
//! (`sources.repo.path`, `path` in repos.json) keeps its old value, but the engine never reads, creates or deletes
//! that folder (spec 9절). Clones made by the old path stay where they are.
//!
//! Refresh runs as synchronous code on one std thread (`refresh`). Embedding is done by `Engine::embed_passages`,
//! which releases the same lock as `/ask` every `EMBED_BATCH` (2) items, so a question does not wait for the whole
//! refresh. The body runs inside `catch_unwind`, so even a panic ends as `failed` (`internal error`, `down`) and
//! the previous index keeps answering.
//!
//! The status is always replaced with a new value, never mutated (the server may be turning the previous one into
//! JSON at that moment). The shape and key order are fixed: start `{state: "running", step: "repo", started_at}`, a
//! stage changes `step` in place and progress appends `progress: {done, total}`, end
//! `{state: "done", finished_at, chunks, embedded, commit(first 8 chars), warnings}` (followed by `unchanged: true`
//! when nothing changed), failure `{state: "failed", finished_at, error, error_kind}`.
//!
//! Rules to keep:
//! - Code comes from the GitHub API instead of a clone (`git fetch`, `checkout`) (spec 5절). When fetching fails: a
//!   switch, `fresh`, key (except when gh could not give a token), or an index with no code to continue from fails;
//!   anything else goes on with the previous code (`warnings: ["repo"]`). A deleted branch is down, like the old
//!   `git fetch` (key if the repo itself is not visible either), and the default branch is GitHub's current value,
//!   not the `origin/HEAD` from clone time.
//!   `fresh` fetches without looking at the chunk cache, and deletes the GitHub raw data right before gather after
//!   fetching it again.
//! - If `start` cannot launch the thread (`std::thread::Builder::spawn` fails), it sets the status to `failed`
//!   (down) and returns false.
//! - A panic is `failed`/`internal error`/`down` (the status does not stay at `running`).
//! - When meta.json or repos.json has an odd shape (`sources`, `sources.repo`, `sources.github` or a repos.json
//!   entry is not an object; `path`, `raw`, `graph` or `commit` is not a string; a required key is missing), it
//!   fails with `Other` (down) naming that spot.
//!   If meta's `repo` is not a string, it is treated as having no name (no entry for that meta is kept in
//!   repos.json).
//!   With no name (in neither the target nor meta), it fails with `the index names no repo` before fetching code.
//! - The `error` text of an IO error is Rust's text (`No such file or directory (os error 2)`).
//! - If `sources.github.repo` is a truthy value that is not a string, it is treated as having no GitHub repo.
//! - Comparing the saved `repo_id` with the `id` in GitHub's answer, and comparing chunks for "nothing changed",
//!   use JSON value equality (`1` and `1.0` differ). Files Tacie writes have no such values.
//! - `repo_key` is `to_lowercase`. GitHub names are ASCII, so this is enough.
//! - `data` in `Places` is the absolute path of `--data-dir`, with symlinks not resolved.
//!   All paths of one `Places` are built from it, so `rel` does not drift.
//! - `clear` panicking is also `failed`/`internal error`/`down` (the status does not stay at `clearing`). If meta's
//!   `repo` is not a string, it is treated as having no name and the cache is not deleted.
//! - `under_raw` treats a path it cannot resolve (one that does not exist) as false (nothing to delete). Deleting
//!   removes the resolved path (the same place as the path written in meta). If the written path itself is a link,
//!   nothing is deleted.
//! - `check`: with odd meta (`sources` or `sources.repo` not an object, `path` or `branch` not a string), what
//!   cannot be asked is left null (if `sources.repo` is not an object, the answer is as if nothing was asked).
//!   `lost` in the status is 0 only when the number is 0 (`false` is not 0). The parentheses in
//!   `check: ETags not saved (…)` are the Rust IO error text with `(os error N)` removed; for an error with no
//!   number, it is that text.
//! - An index with no `sources.repo` in meta (a seed index, `seed_index`) still does a refresh that switches to
//!   another repo (`/index/rebuild {repo}`). With no repo to switch to, it fails with `NO_REPO`.
//! - `describe`: files whose size cannot be read are all skipped.
//!   `sources`, `sources.repo` or `sources.github` that is not an object is treated as empty.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::panic::{AssertUnwindSafe, Location, PanicHookInfo, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Once, PoisonError, TryLockError};
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use tacie_embed::PASSAGE_TOKENS;
use tacie_index::{EmbCache, Index, MODEL, RawChunk, write_index};
use tacie_sources::asana::{self, AsanaFetch, AsanaSource};
use tacie_sources::atlassian::{self, AtlassianFetch, AtlassianSource};
use tacie_sources::dropbox::{self, DropboxFetch, DropboxSource};
use tacie_sources::error::{Kind, SourceError};
use tacie_sources::gather::{Gather, gather};
use tacie_sources::gdrive::{self, GDriveFetch, GDriveSource};
use tacie_sources::github::content::{self, Snapshot};
use tacie_sources::github::fetch::fetch as github_fetch;
use tacie_sources::github::gh_token::is_not_logged_in;
use tacie_sources::github::items::{load_state, save_state};
use tacie_sources::github::probe::changed as github_changed;
use tacie_sources::github::{GitHub, valid_branch, valid_repo_name};
use tacie_sources::linear::{self, LinearFetch, LinearSource};
use tacie_sources::meeting;
use tacie_sources::notes;
use tacie_sources::notion::{self, NotionFetch, NotionSource};
use tacie_sources::slack::SlackFetch;
use tacie_sources::store::{swap_in, write_json};
use tacie_sources::{py, repo};

use crate::pipeline::Engine;

/// 갱신이나 지우기가 도는 상태예요: 그동안은 새 갱신을 시작하지 않아요.
const BUSY: [&str; 2] = ["running", "clearing"];
/// 이만큼 이상 임베딩하는 동기화는 `PARTIAL_EVERY`개씩 나눠서 검색에 넣어요.
const PARTIAL_MIN: usize = 500;
const PARTIAL_EVERY: usize = 500;
/// 중간 색인 벡터의 캐시 태그 꼬리예요(동기화가 끝나면 지워요).
const PARTIAL_TAG: &str = "\u{1f}partial";
/// 처음 중간 색인을 넣을 때 동기화 전 색인을 두는 자리(`index.keep`)예요. 동기화가 멈추면 이것으로 돌아가요.
const KEEP: &str = ".keep";
/// 동기화 중에 GitHub 연결이 바뀌었어요(연결 해제): 멈추고 동기화 전 색인으로 돌아가요.
const CANCELLED: &str = "the GitHub connection changed during the sync";
/// 멈췄는지 보는 간격이에요(조각 수): 임베딩 중에도 연결 해제가 몇 초 안에 닿아요.
const WATCH_EVERY: usize = 16;
/// `/index/check`가 노션을 다시 받을 때로 치는 간격이에요(초).
const NOTION_EVERY: i64 = 15 * 60;
/// Linear를 싸게 물을 수 없을 때(확인 질의가 실패) 받은 지 이만큼 지나면 받아요.
const LINEAR_EVERY: i64 = 15 * 60;
/// Drive 변경을 물을 수 없을 때(확인 호출이 실패) 받은 지 이만큼 지나면 받아요.
const GDRIVE_EVERY: i64 = 15 * 60;
/// Atlassian 변경을 물을 수 없을 때(확인 호출이 실패) 받은 지 이만큼 지나면 받아요.
const ATLASSIAN_EVERY: i64 = 15 * 60;
/// Asana 변경을 물을 수 없을 때(확인 호출이 실패) 받은 지 이만큼 지나면 받아요.
const ASANA_EVERY: i64 = 15 * 60;
/// Dropbox 변경을 물을 수 없을 때(확인 호출이 실패) 받은 지 이만큼 지나면 받아요.
const DROPBOX_EVERY: i64 = 15 * 60;
/// Asana 받기가 몫(3,000번·10분)을 다 써서 멈췄을 때 이만큼 뒤에 이어요(Asana 한도는 분 단위예요).
const ASANA_RESUME: i64 = 60;
/// 30분 확인의 Linear 질의예요: 네 목록에서 가장 늦게 고친 것 하나씩이에요. 이슈·문서는 `sort`로 방향을 정하고,
/// 댓글·업데이트는 `orderBy`뿐이라 방향을 모르는 대로 둬요(docs/18 16-1절).
const LINEAR_CHECK: &str = "query LinearCheck { issues(first: 1, includeArchived: true, sort: [{updatedAt: {order: Descending}}]) { nodes { updatedAt } } documents(first: 1, includeArchived: true, sort: [{updatedAt: {order: Descending}}]) { nodes { updatedAt } } comments(first: 1, includeArchived: true, orderBy: updatedAt) { nodes { updatedAt } } projectUpdates(first: 1, includeArchived: true, orderBy: updatedAt) { nodes { updatedAt } } }";
const NO_REPO: &str = "no repository yet: choose one with /index/rebuild {repo, branch}";
/// 패닉한 갱신의 `error`예요. 패닉 글은 상태에 넣지 않아요(무엇이 들어 있을지 몰라요).
const INTERNAL: &str = "internal error";
/// 갱신 스레드의 이름이에요. 패닉 훅이 이 이름으로 갱신의 패닉을 알아봐요.
const THREAD: &str = "refresh";

/// 갱신 스레드의 패닉이 stderr에 남기는 줄이에요: 자리만이고 패닉 글(payload)은 넣지 않아요. 글자 경계에서 자른
/// 슬라이스 같은 패닉은 이슈·슬랙 글을 담을 수 있어요.
pub fn refresh_panic_line(location: Option<&Location<'_>>) -> String {
    match location {
        Some(at) => format!("refresh: panicked at {}:{}", at.file(), at.line()),
        None => "refresh: panicked".to_string(),
    }
}

/// 패닉 훅을 한 번만 이어 걸어요: 갱신 스레드의 패닉은 `refresh_panic_line`만 찍고, 다른 스레드는 앞 훅에 맡겨요.
fn install_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
            if std::thread::current().name() == Some(THREAD) {
                // eprintln!은 stderr에 쓰지 못하면 패닉해요. 훅 안의 패닉은 프로세스를 끝내서 쓰기 실패는 버려요.
                let _ = writeln!(std::io::stderr(), "{}", refresh_panic_line(info.location()));
            } else {
                previous(info);
            }
        }));
    });
}

/// 지우기가 지워도 되는 자리예요: 저장소의 GitHub 원본, 곧 `data/sources/github`이거나
/// `data/sources/github-repos` 바로 아래 폴더 하나예요. 잘못된 기록(`..`, 바깥을 가리키는 링크)이 다른 곳에 닿으면 안
/// 돼요. 경로와 `data/sources`를 모두 풀어서(`fs::canonicalize`) 비교하고, 없는 경로는 false예요(지울 것도 없어요).
pub fn under_raw(places: &Places, path: &Path) -> bool {
    raw_to_clear(places, path).is_some()
}

/// `under_raw`인 경로를 푼 것이에요. 지우기는 이 풀린 자리를 지워요(적힌 경로가 링크면 지우기는 아무것도 지우지 않아요).
fn raw_to_clear(places: &Places, path: &Path) -> Option<PathBuf> {
    let path = fs::canonicalize(path).ok()?;
    let raw = fs::canonicalize(places.raw()).ok()?;
    (path == raw.join("github") || path.parent() == Some(raw.join("github-repos").as_path()))
        .then_some(path)
}

/// IO 오류의 설명 글이에요: `… (os error N)`의 번호 꼬리를 떼요.
fn strerror(e: &std::io::Error) -> String {
    let text = e.to_string();
    match text.find(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

/// 연결 화면이 보여 주는 색인이에요: 종류마다 다른 문서 수, 디스크의 크기, 저장소와
/// 브랜치, GitHub이 저장소를 볼 수 없어 그 문서를 뺐는지(`github_lost`)예요. 키 순서는 고정이에요.
///
/// 크기는 폴더 바로 아래 파일의 크기 합이에요(링크는 따라가요). 목록과 크기 사이에 사라진 파일(meta.json을 제자리에
/// 쓰는 동안의 meta.json+tmp)은 건너뛰어요. 객체가 아닌 `sources`·`sources.repo`·`sources.github`는 빈 것으로 봐요.
pub fn describe(
    raw: &[RawChunk],
    meta: &Map<String, Value>,
    index_dir: &Path,
) -> Map<String, Value> {
    let paths = |srcs: &[&str]| {
        raw.iter()
            .filter(|c| {
                c.get("src")
                    .and_then(Value::as_str)
                    .is_some_and(|s| srcs.contains(&s))
            })
            .map(|c| c.get("path").map(Value::to_string).unwrap_or_default())
            .collect::<std::collections::HashSet<String>>()
            .len()
    };
    let size: u64 = match fs::read_dir(index_dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter_map(|e| fs::metadata(e.path()).ok())
            .filter(fs::Metadata::is_file)
            .map(|m| m.len())
            .sum(),
        Err(_) => 0,
    };
    let part = |m: &Map<String, Value>, key: &str| -> Map<String, Value> {
        match m.get(key) {
            Some(Value::Object(o)) => o.clone(),
            _ => Map::new(),
        }
    };
    let sources = part(meta, "sources");
    let repo = part(&sources, "repo");
    let github = part(&sources, "github");
    let get = |m: &Map<String, Value>, key: &str| m.get(key).cloned().unwrap_or(Value::Null);
    let mut out = Map::new();
    out.insert("files".into(), Value::from(paths(&["code", "docs"])));
    out.insert("github_items".into(), Value::from(paths(&["github"])));
    out.insert("bytes".into(), Value::from(size));
    out.insert("repo".into(), get(meta, "repo"));
    out.insert("branch".into(), get(&repo, "branch"));
    out.insert("symbols_missing".into(), get(&repo, "symbols_missing"));
    out.insert(
        "github_lost".into(),
        Value::Bool(github.get("lost").is_some_and(py::truthy)),
    );
    out
}

/// 지금 로컬 시각이에요(`%Y-%m-%dT%H:%M:%S%z` 꼴).
pub fn now() -> String {
    chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%z")
        .to_string()
}

/// 씨앗 색인의 meta예요: 저장소도 커밋도 없고 방금 비운 것처럼 `cleared_at`이 있어요. 그래서 `/index`가
/// `cleared: true`를 주고 앱은 자동 동기화를 하지 않아요. 키 순서는 이대로예요.
pub fn seed_meta() -> Map<String, Value> {
    let mut meta = Map::new();
    meta.insert("repo".into(), Value::Null);
    meta.insert("commit".into(), Value::Null);
    meta.insert("built_at".into(), Value::Null);
    meta.insert("cleared_at".into(), Value::from(now()));
    meta
}

/// `index_dir` 자리에 아무것도 없을 때만(심볼릭 링크도 따라가지 않고 NotFound) 빈 씨앗 색인을 써요: 조각 `[]`,
/// (0, 1024) 벡터, `seed_meta`. 옆자리 `index.new`에 다 쓴 뒤 이름 바꾸기 한 번으로 넣어서 반만 쓴 `index`는 없어요.
/// 썼으면 true예요. 무엇이든 있으면(폴더, 파일, 깨진 링크, 깨진 색인) 손대지 않고 false예요: 깨진 색인을 씨앗으로
/// 덮으면 사용자의 색인이 조용히 사라져요. 있는지 볼 수 없으면(NotFound가 아닌 오류) Err예요. `recover` 뒤에 불러요.
pub fn seed_index(index_dir: &Path) -> anyhow::Result<bool> {
    match fs::symlink_metadata(index_dir) {
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("look at {}", index_dir.display())));
        }
    }
    if let Some(parent) = index_dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = beside(index_dir, ".new");
    match fs::remove_dir_all(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(anyhow::Error::new(e).context(format!("remove {}", tmp.display()))),
    }
    // 조각이 없으니 임베딩을 부르지 않아요. 불리면 패닉하지 않고 실패해요.
    write_index(
        &tmp,
        &[],
        &mut |_| Err(anyhow::anyhow!("seed has no chunks")),
        &seed_meta(),
        None,
        None,
    )?;
    fs::rename(&tmp, index_dir)
        .with_context(|| format!("move {} to {}", tmp.display(), index_dir.display()))?;
    Ok(true)
}

/// 비교와 repos.json이 쓰는 저장소 이름이에요: GitHub 이름은 대소문자를 가리지 않아요(O/Old는 o/old). 이름이 ASCII라
/// 소문자로 바꾸기만 해요.
pub fn repo_key(name: &str) -> String {
    name.to_lowercase()
}

/// `t` 다음의 UTC 정각이에요(Atlassian 시간당 예산이 새로 생기는 때).
fn next_utc_hour(t: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp((t.timestamp().div_euclid(3600) + 1) * 3600, 0).unwrap_or(t)
}

/// 폴더 이름 뒤에 붙인 옆자리예요(`index` → `index.new`, `old` → `old+tmp`).
fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// 데이터 자리예요. meta.json과 repos.json의 `data/…` 경로는 `--data-dir` 아래, 그 밖의 상대 경로는 그
/// 부모 아래, 절대 경로는 그대로예요. 쓸 때도 같은 꼴로 써요.
#[derive(Debug, Clone)]
pub struct Places {
    /// `data`의 부모예요.
    pub root: PathBuf,
    /// `--data-dir`의 절대 경로예요.
    pub data: PathBuf,
}

impl Places {
    pub fn new(data_dir: &Path) -> Self {
        let data = std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        let root = data
            .parent()
            .map_or_else(|| data.clone(), Path::to_path_buf);
        Places { root, data }
    }

    /// 출처 원본 폴더예요(`data/sources`).
    pub fn raw(&self) -> PathBuf {
        self.data.join("sources")
    }

    /// meta.json·repos.json이 적은 경로의 실제 자리예요.
    pub fn resolve(&self, rel: &str) -> PathBuf {
        let p = Path::new(rel);
        if p.is_absolute() {
            p.to_path_buf()
        } else if let Ok(rest) = p.strip_prefix("data") {
            self.data.join(rest)
        } else {
            self.root.join(p)
        }
    }

    /// 경로를 meta.json·repos.json이 적는 꼴로 써요: `data` 아래면 `data/…`, `root` 아래면 그 상대
    /// 경로, 아니면 전체예요.
    pub fn rel(&self, path: &Path) -> String {
        if let Ok(rest) = path.strip_prefix(&self.data) {
            if rest.as_os_str().is_empty() {
                return "data".to_string();
            }
            Path::new("data").join(rest).to_string_lossy().into_owned()
        } else if let Ok(rest) = path.strip_prefix(&self.root) {
            rest.to_string_lossy().into_owned()
        } else {
            path.to_string_lossy().into_owned()
        }
    }

    /// 저장소의 조각 캐시예요(파일 → 조각, blob으로).
    pub fn chunk_cache(&self, name: &str) -> PathBuf {
        self.data
            .join("cache/chunks")
            .join(format!("{}.json", repo_key(name).replace('/', "__")))
    }

    /// 임베딩 캐시가 사는 곳이에요.
    pub fn emb_cache_root(&self) -> PathBuf {
        self.data.join("cache/emb")
    }
}

/// 무엇을 갱신할지예요(`{"repo"?, "branch"?, "fresh"?}`). 비면 색인의 저장소와 브랜치예요.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Target {
    pub repo: Option<String>,
    pub branch: Option<String>,
    /// 처음부터 다시 받아요(GitHub 원본, 조각 캐시, 옛 색인의 벡터를 쓰지 않아요).
    pub fresh: bool,
}

/// `POST /index/rebuild`가 묻는 것이에요: 객체가 아니거나, `repo`가 null도 `owner/name`도
/// 아니거나, `branch`가 null도 브랜치 이름도 아니거나, `fresh`가 null도 참·거짓도 아니면 None이에요.
pub fn rebuild_target(body: &Value) -> Option<Target> {
    let body = body.as_object()?;
    let get = |k: &str| body.get(k).filter(|v| !v.is_null());
    let repo = match get("repo") {
        None => None,
        Some(Value::String(r)) if valid_repo_name(r) => Some(r.clone()),
        Some(_) => return None,
    };
    let branch = match get("branch") {
        None => None,
        Some(Value::String(b)) if valid_branch(b) => Some(b.clone()),
        Some(_) => return None,
    };
    let fresh = match get("fresh") {
        None => false,
        Some(Value::Bool(f)) => *f,
        Some(_) => return None,
    };
    Some(Target {
        repo,
        branch,
        fresh,
    })
}

/// 색인 폴더를 여는 함수예요(시작할 때의 mask와 viewer로 `Index::open`).
pub type OpenIndex = Arc<dyn Fn(&Path) -> anyhow::Result<Index> + Send + Sync>;

/// 갱신이 받는 출처예요.
pub struct Sources {
    pub github: Arc<GitHub>,
    /// 슬랙 받기예요. 진짜 것은 reqwest blocking이라 갱신 스레드에서만 불러요.
    pub slack: Arc<dyn SlackFetch>,
    /// 노션이에요(docs/13): 앱이 넘긴 연결로 받아요. 저장소가 아니라 사람의 것이라 저장소를 바꿔도 따라가요.
    pub notion: Arc<NotionSource>,
    /// Linear예요(docs/18): 노션처럼 사람의 것이라 저장소를 바꿔도 따라가요.
    pub linear: Arc<LinearSource>,
    /// Google Drive예요(docs/19): Linear처럼 사람의 것이라 저장소를 바꿔도 따라가요.
    pub gdrive: Arc<GDriveSource>,
    /// Jira·Confluence예요(docs/31): Drive처럼 사람의 것이라 저장소를 바꿔도 따라가요.
    pub atlassian: Arc<AtlassianSource>,
    /// Asana예요(docs/32): Atlassian처럼 사람의 것이라 저장소를 바꿔도 따라가요.
    pub asana: Arc<AsanaSource>,
    /// Dropbox예요(docs/33): Drive처럼 사람의 것이라 저장소를 바꿔도 따라가요.
    pub dropbox: Arc<DropboxSource>,
}

/// 한 번에 갱신 하나를 뒤에서 해요. 바꿔 넣을 때까지 엔진은 옛 색인으로 답해요.
/// 갱신 하나가 끝날 때 GPU 임베더를 내려요(`Engine::release_gpu`).
struct ReleaseGpu<'a>(&'a Engine);

impl Drop for ReleaseGpu<'_> {
    fn drop(&mut self) {
        self.0.release_gpu();
    }
}

pub struct Refresher {
    engine: Arc<Engine>,
    index_dir: PathBuf,
    places: Places,
    sources: Sources,
    open_index: OpenIndex,
    status: Mutex<Arc<Value>>,
    /// 지우기가 도는 동안 잡아요. `start`는 잡히지 않으면 시작하지 않아요.
    lock: Mutex<()>,
    /// 테스트용: 바꾼 상태를 모두 남겨요.
    log: Option<Mutex<Vec<Arc<Value>>>>,
    /// 나눠서 검색에 넣는 기준과 묶음 크기예요(`PARTIAL_MIN`, `PARTIAL_EVERY`; 테스트는 작게 줘요).
    partial: (usize, usize),
    /// GitHub에서 받는 갱신이 시작할 때의 연결 세대예요. 세대가 바뀌면 갱신을 멈춰요(`cancelled`).
    watch: Mutex<Option<u64>>,
    /// 이번 갱신이 시작한 때예요. 단계 줄과 끝 줄에 그때부터 걸린 시간을 적어요(어느 단계가 느린지 로그로 봐요).
    began: Mutex<Option<Instant>>,
    /// 갱신 중에 개인 출처(Linear 연결·끈 팀)가 바뀌었어요: 이 갱신이 끝나면 한 번 더 돌려요.
    pending_personal: std::sync::atomic::AtomicBool,
    /// 갱신 중에 앱이 회의 목록을 보냈어요: 이 갱신이 끝나면 회의 몫을 다시 맞춰요.
    pending_meetings: std::sync::atomic::AtomicBool,
    /// 갱신 중에 노트 폴더가 바뀌었어요(docs/34): 이 갱신이 끝나면 노트 몫을 다시 맞춰요.
    pending_notes: std::sync::atomic::AtomicBool,
    /// Atlassian 처음 받기가 시간당 예산에 막혀 멈췄을 때 다시 받을 수 있는 때예요. 바로 다시 돌리면 정각까지 헛돌아서,
    /// 이때가 지난 뒤에 30분 확인이 이어 받기를 걸어요.
    atlassian_resume: Mutex<Option<DateTime<Utc>>>,
    /// Asana 받기가 몫을 다 써서 멈췄을 때 이어 받을 때예요(1분 뒤). 이때가 지난 뒤에 30분 확인이 이어 받기를 걸어요.
    asana_resume: Mutex<Option<DateTime<Utc>>>,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 갱신 본문이 실패한 이유예요(`error`, `error_kind`): 메시지(200자)와 종류예요.
fn failure(e: &SourceError) -> (String, Kind) {
    (py::take_chars(&e.to_string(), 200).to_string(), e.kind())
}

fn other(message: impl Into<String>) -> SourceError {
    SourceError::Other(message.into())
}

/// GitHub과 이야기하다 난 오류예요(답의 종류, 모양이 틀린 답). 로컬 파일 오류(IO)는 아니에요.
fn from_github(e: &SourceError) -> bool {
    matches!(e, SourceError::Service { .. } | SourceError::Json(_))
}

/// 없으면 빈 것인 객체예요: 거짓 값은 빈 객체, 객체가 아닌 참 값은 오류예요.
fn object_or_empty(v: Option<&Value>, what: &str) -> Result<Map<String, Value>, SourceError> {
    match v {
        Some(Value::Object(m)) => Ok(m.clone()),
        Some(v) if py::truthy(v) => Err(other(format!("{what} is not an object"))),
        _ => Ok(Map::new()),
    }
}

/// 꼭 있어야 하는 글 값이에요(경로로 쓰는 자리).
fn text_of<'a>(m: &'a Map<String, Value>, key: &str, what: &str) -> Result<&'a str, SourceError> {
    match m.get(key) {
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(other(format!("{what} has a {key} that is not text"))),
        None => Err(other(format!("{what} has no {key}"))),
    }
}

/// 참이면 글이어야 하는 값이에요. 거짓 값은 None이에요.
fn truthy_text<'a>(
    m: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<Option<&'a str>, SourceError> {
    match m.get(key) {
        Some(v) if py::truthy(v) => v
            .as_str()
            .map(Some)
            .ok_or_else(|| other(format!("{what} has a {key} that is not text"))),
        _ => Ok(None),
    }
}

/// 만들던 폴더예요: 끝까지 가지 못하면(오류든 패닉이든) 지워요.
struct Scratch<'a>(Option<&'a Path>);

impl Scratch<'_> {
    fn keep(mut self) {
        self.0 = None;
    }
}

impl Drop for Scratch<'_> {
    fn drop(&mut self) {
        if let Some(p) = self.0 {
            let _ = fs::remove_dir_all(p);
        }
    }
}

impl Refresher {
    pub fn new(
        engine: Arc<Engine>,
        index_dir: PathBuf,
        places: Places,
        sources: Sources,
        open_index: OpenIndex,
    ) -> Self {
        install_panic_hook();
        Refresher {
            engine,
            index_dir,
            places,
            sources,
            open_index,
            status: Mutex::new(Arc::new(json!({"state": "idle"}))),
            lock: Mutex::new(()),
            log: None,
            partial: (PARTIAL_MIN, PARTIAL_EVERY),
            watch: Mutex::new(None),
            began: Mutex::new(None),
            pending_personal: std::sync::atomic::AtomicBool::new(false),
            pending_meetings: std::sync::atomic::AtomicBool::new(false),
            pending_notes: std::sync::atomic::AtomicBool::new(false),
            atlassian_resume: Mutex::new(None),
            asana_resume: Mutex::new(None),
        }
    }

    /// 상태를 바꿀 때마다 기록해요(테스트용).
    /// 나눠서 검색에 넣는 기준과 묶음 크기를 바꿔요(테스트).
    pub fn with_partial(mut self, min: usize, every: usize) -> Self {
        self.partial = (min, every.max(1));
        self
    }

    pub fn with_status_log(mut self) -> Self {
        self.log = Some(Mutex::new(Vec::new()));
        self
    }

    /// 지금 상태예요. 늘 새 값으로 바뀌니 받은 것은 그대로예요.
    pub fn status(&self) -> Arc<Value> {
        locked(&self.status).clone()
    }

    /// 기록한 상태들이에요(`with_status_log`가 아니면 비어요).
    pub fn status_log(&self) -> Vec<Arc<Value>> {
        self.log
            .as_ref()
            .map(|l| locked(l).clone())
            .unwrap_or_default()
    }

    /// 상태를 바꿔요(테스트가 바쁜 상태를 흉내 낼 때 써요).
    #[doc(hidden)]
    pub fn set_status(&self, status: Value) {
        let status = Arc::new(status);
        *locked(&self.status) = status.clone();
        if let Some(log) = &self.log {
            locked(log).push(status);
        }
    }

    /// 지우기처럼 갱신 자물쇠를 잡아요(테스트용, `clear`가 같은 자물쇠를 잡아요). 독이 들어도 잡아요.
    #[doc(hidden)]
    pub fn hold_lock(&self) -> MutexGuard<'_, ()> {
        locked(&self.lock)
    }

    pub fn index_dir(&self) -> &Path {
        &self.index_dir
    }

    /// 갱신 자물쇠를 기다리지 않고 잡아요. 다른 쪽이 잡고 있으면 None이에요. 잡은 채 패닉한 쪽이 있었으면 그대로 써요:
    /// 지키는 값이 없는 자물쇠라 영영 바쁘면 안 돼요.
    fn try_hold(&self) -> Option<MutexGuard<'_, ()>> {
        match self.lock.try_lock() {
            Ok(held) => Some(held),
            Err(TryLockError::Poisoned(p)) => Some(p.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    /// 갱신이나 지우기가 도는 중이에요.
    pub fn busy(&self) -> bool {
        BUSY.contains(&self.status()["state"].as_str().unwrap_or_default())
    }

    pub fn sources(&self) -> &Sources {
        &self.sources
    }

    /// 뒤에서 갱신해요: 색인의 저장소와 브랜치, 또는 `target`이에요. 갱신이나 지우기가 도는 동안은 false예요.
    pub fn start(self: &Arc<Self>, target: Target) -> bool {
        {
            let Some(_held) = self.try_hold() else {
                return false; // 지우기가 잡고 있어요
            };
            if self.busy() {
                return false;
            }
            self.set_status(json!({"state": "running", "step": "repo", "started_at": now()}));
        }
        let this = self.clone();
        let spawned = std::thread::Builder::new()
            .name(THREAD.to_string())
            .spawn(move || {
                this.run(&target);
                // 갱신 중에 Linear 연결이나 끈 팀이 바뀌었으면 한 번 더 돌려요(docs/18 16-1절)
                if this
                    .pending_personal
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    this.start(Target::default());
                }
                if this
                    .pending_meetings
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    this.meetings_changed();
                }
                if this
                    .pending_notes
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    this.notes_changed();
                }
            });
        if let Err(e) = spawned {
            self.set_status(json!({"state": "failed", "finished_at": now(),
                                   "error": "could not start the refresh", "error_kind": "down"}));
            eprintln!("refresh failed: could not start the refresh ({e})");
            return false;
        }
        true
    }

    /// 색인을 비우고 그 저장소의 GitHub 원본을 지워요. 연결(meta의 `sources`)은 남기고, 옛
    /// 클론 자리(`sources.repo.path`)는 건드리지 않아요. 갱신이나 다른 지우기가 도는 동안은 false예요.
    ///
    /// 도는 동안 갱신 자물쇠를 잡고 있어서 `start`와 `check`는 기다리지 않고 물러나요. 막히는 일이라 HTTP 경로는
    /// `spawn_blocking`에서 불러요. 본문은 `catch_unwind` 안이라 패닉도 `failed`로 끝나요(상태가 `clearing`에 남으면
    /// 갱신을 다시 시작할 수 없어요).
    pub fn clear(&self) -> bool {
        let Some(_held) = self.try_hold() else {
            return false;
        };
        if self.busy() {
            return false;
        }
        self.set_status(json!({"state": "clearing", "started_at": now()}));
        let outcome = catch_unwind(AssertUnwindSafe(|| self.clear_body()));
        let failed = match outcome {
            Ok(Ok(cleared_at)) => {
                self.set_status(json!({"state": "idle", "cleared_at": cleared_at}));
                return true;
            }
            Ok(Err(e)) => (py::take_chars(&e.to_string(), 200).to_string(), e.kind()),
            Err(_) => (INTERNAL.to_string(), Kind::Down),
        };
        let (detail, kind) = failed;
        self.set_status(json!({"state": "failed", "finished_at": now(),
                               "error": detail, "error_kind": kind.as_str()}));
        eprintln!("clear failed: {detail}");
        false
    }

    /// `clear`의 본문이에요. 지운 시각(`cleared_at`)을 돌려줘요.
    fn clear_body(&self) -> Result<String, SourceError> {
        let live = self.engine.index.load_full();
        // commit·built_at은 null, cleared_at은 지금이에요: 있는 키는 제자리, 없는 키는 뒤
        let mut meta = live.meta.clone();
        meta.insert("commit".into(), Value::Null);
        meta.insert("built_at".into(), Value::Null);
        let cleared_at = now();
        meta.insert("cleared_at".into(), Value::from(cleared_at.as_str()));
        let tmp = beside(&self.index_dir, ".new");
        let _ = fs::remove_dir_all(&tmp);
        // 조각이 없으니 임베딩을 부르지 않아요
        write_index(
            &tmp,
            &[],
            &mut |texts| self.engine.embed_passages(texts, &mut |_, _| {}),
            &meta,
            None,
            None,
        )?;
        // 손으로 만든 meta는 저장소 이름이 없을 수 있어요: 그러면 지울 저장소 몫도 없어요
        if let Some(name) = meta.get("repo").and_then(Value::as_str) {
            EmbCache::new(&self.places.emb_cache_root(), MODEL, PASSAGE_TOKENS)
                .drop_prefix(&format!("{}#", repo_key(name)))?;
            if let Err(e) = fs::remove_file(self.places.chunk_cache(name))
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(e.into());
            }
        }
        // 지난 갱신의 index.old도, 먼저 지워요: 아래 바꿔 넣기가 끊겨도 그것이 되돌아오면 안 돼요(recover)
        let _ = fs::remove_dir_all(beside(&self.index_dir, ".old"));
        swap_in(&tmp, &self.index_dir)?;
        let opened = (self.open_index)(&self.index_dir)?;
        self.engine.index.store(Arc::new(opened));
        let src = object_or_empty(meta.get("sources"), "meta.json's sources")?;
        if src.get("github").is_some_and(py::truthy) {
            // 끝의 `/`와 가운데 `.`를 떼요: `링크/`는 링크를 따라가 폴더로 보여요
            let recorded: PathBuf = self.raw_of(&src)?.components().collect();
            // 링크면 아무것도 지우지 않아요: 링크가 다른 저장소의
            // 원본을 가리켜도 그대로 둬요
            let is_link = fs::symlink_metadata(&recorded).is_ok_and(|m| m.is_symlink());
            if !is_link && let Some(raw) = raw_to_clear(&self.places, &recorded) {
                let _ = fs::remove_dir_all(raw);
            }
        }
        // Google Drive 원문도 지워요(Google 데이터 정책, docs/19 1-8절·11절). 연결은 남아서 다음 동기화가 다시 받아요
        gdrive::forget(&self.gdrive_dir())?;
        // Jira·Confluence 원문도 지워요. 연결은 남아서 다음 동기화가 다시 받아요. 끈 프로젝트·스페이스는 원본
        // `state.json`에만 있어서 지우기 전에 읽어 두고 되돌려 놓아요(풀리면 다음 동기화가 끈 것을 AI로 보내요)
        let (projects, spaces) = atlassian::muted(&self.atlassian_dir());
        self.forget_atlassian()?;
        if !projects.is_empty() || !spaces.is_empty() {
            atlassian::set_muted(&self.atlassian_dir(), &projects, &spaces)?;
        }
        // Asana 원문도 지워요. 연결은 남아서 다음 동기화가 다시 받아요. 끈 워크스페이스·프로젝트도 Atlassian처럼
        // 되돌려 놓아요
        let (workspaces, projects) = asana::muted(&self.asana_dir());
        self.forget_asana()?;
        if !workspaces.is_empty() || !projects.is_empty() {
            asana::set_muted(&self.asana_dir(), &workspaces, &projects)?;
        }
        // Dropbox 원문도 지워요. 연결은 남아서 다음 동기화가 다시 받아요. 끈 폴더도 Asana처럼 되돌려 놓아요
        let folders = dropbox::muted(&self.dropbox_dir());
        self.forget_dropbox()?;
        if !folders.is_empty() {
            dropbox::set_muted(&self.dropbox_dir(), &folders)?;
        }
        Ok(cleared_at)
    }

    /// 갱신이 새것을 찾을지 싸게 물어요: 브랜치 머리는 `meta.repo`의 `git/ref/heads/…`
    /// 하나(스펙 5-5, 확인의 시간 제한), GitHub은 목록마다 저장된 ETag로 한 번씩(`github::probe::changed`). 갱신이나
    /// 지우기가 도는 동안은 None이에요. 아니면 `{"code": 참·거짓·null, "github": 참·거짓·null, "changed": 참·거짓}`이고,
    /// 알 수 없으면(오프라인, 너무 느림, 저장소 이름이나 브랜치가 없음, GitHub 원본이 없거나 닫힘) null이며 `changed`는
    /// 그것을 아니오로 세요. 클론 자리는 읽지 않아요.
    ///
    /// 상태와 meta는 자물쇠 안에서 읽고, GitHub은 자물쇠 밖에서 물어요(그동안 부른 갱신은 바로 시작해요). ETag는 그
    /// 사이에 갱신이나 지우기가 돌지 않았고(상태는 바뀔 때마다 새 `Arc`예요) 원본 폴더가 아직 있을 때만 저장해요.
    /// 여기서 폴더를 만들지 않아요. 막히는 일이라 HTTP 경로는 `spawn_blocking`에서 불러요.
    pub fn check(&self) -> Option<Value> {
        let nothing = json!({"code": null, "github": null, "changed": false});
        let gh = self.sources.github.as_ref();
        // 노션은 바뀐 것을 싸게 물을 곳이 없어요: 연결돼 있고 받은 지 `NOTION_EVERY`초가 지났으면 바뀐 것으로 쳐요
        let notion_due =
            self.sources.notion.connected() && notion::due(&self.notion_dir(), NOTION_EVERY);
        let linear_on = self.sources.linear.connected();
        let linear_changed = linear_on && self.linear_changed(gh.timeouts().check);
        let gdrive_on = self.sources.gdrive.connected();
        let gdrive_changed = gdrive_on && self.gdrive_changed(gh.timeouts().check);
        let atlassian_on = self.sources.atlassian.connected();
        let atlassian_changed = atlassian_on && self.atlassian_changed(gh.timeouts().check);
        let asana_on = self.sources.asana.connected();
        let asana_changed = asana_on && self.asana_changed(gh.timeouts().check);
        let dropbox_on = self.sources.dropbox.connected();
        let dropbox_changed = dropbox_on && self.dropbox_changed(gh.timeouts().check);
        let pending = self
            .pending_personal
            .load(std::sync::atomic::Ordering::SeqCst);
        let mut only_notion = if notion_due
            || linear_changed
            || gdrive_changed
            || atlassian_changed
            || asana_changed
            || dropbox_changed
            || pending
        {
            json!({"code": null, "github": null, "notion": true, "changed": true})
        } else {
            nothing
        };
        if !notion_due {
            only_notion["notion"] = Value::Bool(false);
        }
        if !self.sources.notion.connected() {
            only_notion
                .as_object_mut()
                .map(|m| m.shift_remove("notion"));
        }
        if linear_on {
            only_notion["linear"] = Value::Bool(linear_changed);
        }
        if gdrive_on {
            only_notion["gdrive"] = Value::Bool(gdrive_changed);
        }
        if atlassian_on {
            only_notion["atlassian"] = Value::Bool(atlassian_changed);
        }
        if asana_on {
            only_notion["asana"] = Value::Bool(asana_changed);
        }
        if dropbox_on {
            only_notion["dropbox"] = Value::Bool(dropbox_changed);
        }
        let (seen, meta, name, branch, gh_repo, raw, state) = {
            let _held = self.try_hold()?; // 지우기가 잡고 있어요
            if self.busy() {
                return None;
            }
            let seen = self.status();
            let meta = self.engine.index.load().meta.clone();
            let src = match meta.get("sources") {
                Some(Value::Object(m)) => m.clone(),
                _ => Map::new(),
            };
            let r = src.get("repo").cloned().unwrap_or(Value::Null);
            // 무엇보다 먼저, GitHub에 묻기 전에: Tacie 연결이 없으면 사용자의 로그인을 빌리지 않아요
            if !gh.connected() || meta.get("cleared_at").is_some_and(py::truthy) || !py::truthy(&r)
            {
                return Some(only_notion);
            }
            let Value::Object(r) = r else {
                return Some(only_notion); // 객체가 아닌 sources.repo: 물을 곳을 몰라요
            };
            let name = meta
                .get("repo")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .map(str::to_string);
            let branch = r
                .get("branch")
                .and_then(Value::as_str)
                .filter(|b| !b.is_empty())
                .map(str::to_string);
            let gh_repo = match src.get("github") {
                Some(Value::Object(g)) => g
                    .get("repo")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                _ => None,
            };
            let raw = self.raw_of(&src).ok();
            let state = match (&gh_repo, &raw) {
                (Some(_), Some(raw)) => Some(load_state(raw)),
                _ => None,
            };
            (seen, meta, name, branch, gh_repo, raw, state)
        };

        // 자물쇠 밖이에요
        let code = match (&name, &branch) {
            // 오프라인, 너무 느림, 브랜치가 없음: 알 수 없어요
            (Some(name), Some(branch)) => {
                content::head_commit(gh, name, branch, Some(gh.timeouts().check))
                    .ok()
                    .map(|head| meta.get("commit").and_then(Value::as_str) != Some(head.as_str()))
            }
            _ => None,
        };
        let (github, etags) = match (&gh_repo, &raw, &state) {
            (Some(repo), Some(raw), Some(state))
                if state.get("lost").and_then(Value::as_f64) == Some(0.0) && raw.is_dir() =>
            {
                github_changed(gh, repo, state)
            }
            _ => (None, Map::new()),
        };
        if let (false, Some(raw)) = (etags.is_empty(), &raw)
            && let Some(_held) = self.try_hold()
            && Arc::ptr_eq(&self.status(), &seen)
            && raw.is_dir()
        {
            let mut st = load_state(raw);
            let mut merged = match st.get("etags") {
                Some(Value::Object(old)) => old.clone(),
                _ => Map::new(),
            };
            for (k, v) in etags {
                merged.insert(k, v);
            }
            st.insert("etags".into(), Value::Object(merged));
            // 디스크가 가득 찼다든지: ETag는 아끼는 것일 뿐이고, 다음 확인이 통째로 물어요
            if let Err(e) = save_state(raw, &st) {
                eprintln!("check: ETags not saved ({})", strerror(&e));
            }
        }
        let mut out = json!({"code": code, "github": github,
                             "changed": code == Some(true) || github == Some(true) || notion_due
                                 || linear_changed || gdrive_changed || atlassian_changed
                                 || asana_changed || dropbox_changed || pending});
        // 노션 칸은 노션이 연결돼 있을 때만 있어요(앱과의 계약은 그대로예요)
        if self.sources.notion.connected() {
            out["notion"] = Value::Bool(notion_due);
        }
        if linear_on {
            out["linear"] = Value::Bool(linear_changed);
        }
        if gdrive_on {
            out["gdrive"] = Value::Bool(gdrive_changed);
        }
        if atlassian_on {
            out["atlassian"] = Value::Bool(atlassian_changed);
        }
        if asana_on {
            out["asana"] = Value::Bool(asana_changed);
        }
        if dropbox_on {
            out["dropbox"] = Value::Bool(dropbox_changed);
        }
        Some(out)
    }

    /// 갱신 스레드의 몸이에요: 패닉도 `failed`로 끝나요.
    fn run(&self, target: &Target) {
        let outcome = catch_unwind(AssertUnwindSafe(|| self.refresh(target)));
        if outcome.is_err() {
            self.set_status(json!({"state": "failed", "finished_at": now(),
                                   "error": INTERNAL, "error_kind": Kind::Down.as_str()}));
            eprintln!("refresh failed: {INTERNAL}");
        }
    }

    fn refresh(&self, target: &Target) {
        *locked(&self.began) = Some(Instant::now());
        // 끝나면(멈추거나 실패하거나 패닉해도) GPU 임베더를 내려요
        let _gpu = ReleaseGpu(&self.engine);
        *locked(&self.watch) = None;
        let keep = beside(&self.index_dir, KEEP);
        let _ = fs::remove_dir_all(&keep); // 앞 갱신이 끝나지 못하고 남긴 것이에요
        let live = self.engine.index.load_full();
        let outcome = self.body(&live, target);
        if self.cancelled() {
            *locked(&self.watch) = None;
            self.put_back();
            self.set_status(json!({"state": "idle", "cancelled": true, "finished_at": now()}));
            eprintln!("refresh: stopped, {CANCELLED}");
            return;
        }
        *locked(&self.watch) = None;
        let _ = fs::remove_dir_all(&keep);
        // GitHub 쪽이 실패해도 노션은 받아요(docs/13): 코드와 GitHub 조각은 지금 색인 그대로, 노션만 새로예요. 실패는
        // 그대로 알려요(앱이 GitHub 문제를 보여 줘요)
        if let Err(e) = &outcome
            && from_github(e)
            && e.kind() != Kind::Model
            && !target.fresh
            && target.repo.is_none()
            && self.personal_on()
            && let Err(n) = self.personal_beside(&live)
        {
            eprintln!(
                "refresh: personal sources not refreshed either ({})",
                py::take_chars(&n.detail(), 120)
            );
        }
        if let Err(e) = outcome {
            // 옛 색인이 계속 답해요. 앱이 이유를 보여 줘요
            let (detail, kind) = failure(&e);
            self.set_status(json!({"state": "failed", "finished_at": now(),
                                   "error": detail, "error_kind": kind.as_str()}));
            eprintln!("refresh failed: {detail}");
        }
    }

    /// GitHub에서 받는 이 갱신이 시작한 뒤 연결이 바뀌었어요.
    fn cancelled(&self) -> bool {
        locked(&self.watch).is_some_and(|g| g != self.sources.github.generation())
    }

    fn stop_if_cancelled(&self) -> Result<(), SourceError> {
        if self.cancelled() {
            return Err(other(CANCELLED));
        }
        Ok(())
    }

    /// 멈춘 갱신이 중간 색인을 넣었으면 동기화 전 색인(`index.keep`)으로 돌아가요. 만들던 폴더도 지워요.
    fn put_back(&self) {
        let _ = fs::remove_dir_all(beside(&self.index_dir, ".new"));
        let _ = fs::remove_dir_all(beside(&self.index_dir, ".part"));
        let keep = beside(&self.index_dir, KEEP);
        if !keep.exists() {
            return;
        }
        if let Err(e) = self.swap_in(&keep) {
            eprintln!("refresh: the index before the sync did not come back ({e})");
            let _ = fs::remove_dir_all(&keep);
        }
    }

    fn step(&self, name: &str) {
        let mut st = self.status().as_object().cloned().unwrap_or_default();
        st.insert("step".into(), Value::from(name));
        self.set_status(Value::Object(st));
        eprintln!("refresh: {name} (at {})", self.took());
    }

    /// 이번 갱신이 시작한 뒤 걸린 시간이에요(`12.3 s`). 로그 줄에만 써요.
    fn took(&self) -> String {
        let secs = locked(&self.began).map_or(0.0, |t| t.elapsed().as_secs_f64());
        format!("{secs:.1} s")
    }

    fn progress(&self, done: usize, total: usize) {
        let mut st = self.status().as_object().cloned().unwrap_or_default();
        st.insert("progress".into(), json!({"done": done, "total": total}));
        self.set_status(Value::Object(st));
    }

    /// 갱신의 본문이에요: 단계와 순서는 바꾸지 않아요.
    fn body(&self, live: &Index, target: &Target) -> Result<(), SourceError> {
        let gh = self.sources.github.as_ref();
        let meta = &live.meta;
        // 저장소 없이 노션만 쓰는 사람이에요(docs/13 0절): 저장소를 고르지 않았고 색인에도 없으면 노션만으로 만들어요
        let asks_repo = target.repo.as_deref().is_some_and(|r| !r.is_empty());
        let has_repo = meta
            .get("sources")
            .and_then(|s| s.get("repo"))
            .is_some_and(py::truthy);
        if !asks_repo && !has_repo {
            if self.personal_on() {
                return self.personal_only(live, target);
            }
            // 노션·Linear만이던 색인에서 연결을 끊었어요: 빈 씨앗으로 돌아가요
            let personal = meta.get("sources").is_some_and(|s| {
                s.get("notion").is_some()
                    || s.get("linear").is_some()
                    || s.get("gdrive").is_some()
                    || s.get("atlassian").is_some()
                    || s.get("asana").is_some()
                    || s.get("dropbox").is_some()
                    || s.get("meeting").is_some()
                    || s.get("notes").is_some()
            });
            if personal {
                self.install(&[], &seed_meta(), true, "notion", &|_| true)?;
                self.set_status(json!({"state": "done", "finished_at": now(), "chunks": 0,
                                       "embedded": 0, "commit": null, "warnings": []}));
                eprintln!("refresh: done, notion and linear disconnected, back to the seed");
                return Ok(());
            }
        }
        // 무엇보다 먼저, GitHub에 묻기 전에: Tacie 연결이 없으면 사용자의 로그인을 빌리지 않아요
        if !gh.connected() {
            return Err(SourceError::service(Kind::Key, "GitHub is not connected"));
        }
        *locked(&self.watch) = Some(gh.generation());
        let src = match meta.get("sources") {
            None => Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => return Err(other("meta.json's sources is not an object")),
        };
        // 저장소가 없는 색인(씨앗)은 다른 저장소로 바꾸는 갱신만 해요(아래 `switch` 길)
        let meta_r = match src.get("repo") {
            Some(Value::Object(r)) if !r.is_empty() => Some(r.clone()),
            Some(v) if py::truthy(v) => {
                return Err(other("meta.json's sources.repo is not an object"));
            }
            _ => None,
        };
        let meta_repo = meta.get("repo").and_then(Value::as_str).map(str::to_string);
        let name = target
            .repo
            .clone()
            .filter(|n| !n.is_empty())
            .or_else(|| meta_repo.clone());
        // O/Old는 색인의 o/old예요: 같은 자리, 그래프, 이름이에요
        let switch = name.as_deref().map(repo_key) != meta_repo.as_deref().map(repo_key);

        let (r, gh_repo, raw, with_slack, rec) = if switch {
            // 다른 저장소예요: 기억한 자리·원본·그래프, 아니면 새 자리예요
            let Some(name) = name.as_deref().filter(|n| valid_repo_name(n)) else {
                return Err(other("repo must be owner/name"));
            };
            let slug = name.replace('/', "__");
            let rec = match self.known()?.get(&repo_key(name)) {
                Some(Value::Object(rec)) if !rec.is_empty() => rec.clone(),
                Some(v) if py::truthy(v) => {
                    return Err(other("repos.json has an entry that is not an object"));
                }
                _ => {
                    let raw = self.places.raw();
                    let mut rec = Map::new();
                    rec.insert(
                        "path".into(),
                        Value::from(self.places.rel(&raw.join("repos").join(&slug))),
                    );
                    rec.insert(
                        "raw".into(),
                        Value::from(self.places.rel(&raw.join("github-repos").join(&slug))),
                    );
                    rec
                }
            };
            // 자리 이름은 옛 값 그대로 적어요(스펙 9절). 그 폴더는 만들지도 읽지도 않아요
            text_of(&rec, "path", "repos.json's entry")?;
            let mut r = Map::new();
            r.insert("path".into(), rec["path"].clone());
            if rec.get("graph").is_some_and(py::truthy) {
                r.insert("graph".into(), rec["graph"].clone());
            }
            // 회사 슬랙은 다른 저장소의 자료에 넣지 않아요. 슬랙이 있던 저장소는 다시 가져가요
            let raw = self
                .places
                .resolve(text_of(&rec, "raw", "repos.json's entry")?);
            let with_slack = rec.get("slack").is_some_and(py::truthy);
            (r, Some(name.to_string()), raw, with_slack, Some(rec))
        } else {
            let Some(r) = meta_r else {
                return Err(other(NO_REPO));
            };
            text_of(&r, "path", "meta.json's sources.repo")?;
            let github = object_or_empty(src.get("github"), "meta.json's sources.github")?;
            let gh_repo = github
                .get("repo")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let raw = self.raw_of(&src)?;
            (r, gh_repo, raw, src.contains_key("slack"), None)
        };

        // 브랜치를 묻지 않았으면 이 저장소를 마지막에 받은 브랜치, 아니면 GitHub의 지금 기본 브랜치예요(스펙 5-5)
        let branch = match target.branch.as_deref().filter(|b| !b.is_empty()) {
            Some(b) => Value::from(b),
            None => match &rec {
                Some(rec) if rec.get("branch").is_some_and(py::truthy) => rec["branch"].clone(),
                Some(_) => {
                    // 바꾸기 길이라 이름은 늘 있어요
                    let name = name.as_deref().unwrap_or_default();
                    gh.json(&format!("repos/{name}"), 1)?
                        .get("default_branch")
                        .filter(|b| b.is_string())
                        .cloned()
                        .ok_or_else(|| {
                            SourceError::service(
                                Kind::Down,
                                "GitHub's repository answer has no default branch",
                            )
                        })?
                }
                None => r
                    .get("branch")
                    .cloned()
                    .ok_or_else(|| other("meta.json's sources.repo has no branch"))?,
            },
        };
        let branch = match branch.as_str() {
            Some(b) if valid_branch(b) => b.to_string(),
            _ => return Err(other("not a branch name")),
        };
        let Some(name) = name else {
            return Err(other("the index names no repo"));
        };
        let fresh = target.fresh;
        let mut warnings: Vec<String> = Vec::new();
        let had_commit = meta.get("commit").is_some_and(py::truthy);
        let same_branch = r.get("branch").and_then(Value::as_str) == Some(branch.as_str());
        let cache = self.places.chunk_cache(&name);
        let graph_dir =
            truthy_text(&r, "graph", "meta.json's sources.repo")?.map(|g| self.places.resolve(g));
        let graph = graph_dir.as_ref().map(|d| d.join("wiring.json"));
        // 색인한 코드의 보통 갱신은 GitHub에 먼저 물어요: 볼 수 없거나 이제 다른 저장소인 것은 받지 않아요(받기가
        // 실패하거나 새 주인의 코드를 가져와요). 색인한 코드가 남고, gather의 fetch가 그 답을 세어 PR·이슈를 빼요
        let gone = match &gh_repo {
            Some(g) if !switch && !fresh && had_commit && same_branch => self.gone(g, &raw),
            _ => false,
        };
        let mut code_error = None;
        let mut fetched = None;
        if !gone {
            // `fresh`는 조각 캐시를 보지 않고 모든 파일을 받아요(캐시는 gather 바로 앞에서 지워요)
            match self.fetch_code(
                &name,
                &branch,
                graph_dir.as_deref(),
                graph.as_deref(),
                (!fresh).then_some(cache.as_path()),
            ) {
                Ok(got) => fetched = Some(got),
                // 로컬 파일(그래프, 조각 캐시)을 읽지 못한 것은 받기 실패가 아니에요: 예전처럼 멈춰요
                Err(e) if !from_github(&e) => return Err(e),
                Err(e) => {
                    // 받기가 실패한 보통 갱신은 코드 없이 가요. 권한을 잃었거나(key), 바꾸거나, 처음부터 받거나, 이어
                    // 갈 코드가 없는 색인(지웠거나 만든 적 없음)은 멈춰요(스펙 5-4). 지운 브랜치는 down이라(저장소는
                    // 보여요, `content::head_commit`) 보통 갱신이 지난 코드로 가요: 옛 `git fetch`의 실패와 같아요
                    // gh가 토큰을 주지 못한 것(gh 로그아웃, 없는 gh)은 권한을 잃은 게 아니라 받기 실패예요: 예전
                    // git 받기가 gh 없이도 사용자의 git 설정으로 받던 것처럼, 보통 갱신은 지난 코드로 가요
                    let lost = e.kind() == Kind::Key && !is_not_logged_in(&e);
                    if switch || fresh || lost || !had_commit {
                        return Err(e);
                    }
                    code_error = Some(e);
                }
            }
        }

        let (commit, code, symbols_missing, snap, skip) = match fetched {
            Some((commit, snap, skip)) => {
                let missing = Value::from(skip.len());
                (commit, None, missing, Some(snap), skip)
            }
            None => {
                let commit = match meta.get("commit") {
                    Some(Value::String(c)) => c.clone(),
                    _ => return Err(other("meta.json's commit is not text")),
                };
                let code: Vec<RawChunk> = live
                    .raw
                    .iter()
                    .filter(|c| {
                        matches!(c.get("src").and_then(Value::as_str), Some("code" | "docs"))
                    })
                    .cloned()
                    .collect();
                warnings.push("repo".to_string());
                // 색인의 조각은 이름표를 그때 그대로 가져요
                let missing = r.get("symbols_missing").cloned().unwrap_or(Value::from(0));
                let why = match &code_error {
                    Some(e) => py::take_chars(&e.detail(), 120).to_string(),
                    None => "GitHub says it is gone, hidden or another repo".to_string(),
                };
                eprintln!("repo: not fetched, keeping the indexed code ({why})");
                (commit, Some(code), missing, None, Vec::new())
            }
        };
        // 이름표를 붙이는 그래프는 받은 코드에만 써요
        let graph = if code.is_some() { None } else { graph };

        self.step("sources");
        if fresh && let Some(g) = &gh_repo {
            self.fresh_raw(g, &raw)?; // 받기가 실패하면 저장본은 그대로예요
        }
        if fresh {
            // 모든 파일을 다시 잘라요: 받기가 다 된 뒤에 지워서, 실패한 `fresh`는 조각 캐시를 남겨요
            if let Err(e) = fs::remove_file(&cache)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(e.into());
            }
        }
        let slack_dir = self.places.raw().join("slack");
        let (mut chunks, mut info) = gather(
            gh,
            self.sources.slack.as_ref(),
            Gather {
                snapshot: snap.as_ref(),
                graph: graph.as_deref(),
                skip: &skip,
                github_repo: gh_repo.as_deref(),
                github_raw: &raw,
                slack_raw: with_slack.then_some(slack_dir.as_path()),
                fetch: !fresh,
                code,
                cached: Some(&cache),
            },
            &mut warnings,
        )?;
        if let Some(e) = code_error {
            // 다른 출처도 모두 저장본으로 갔으면(또는 없으면) 모든 출처가 실패한 거예요: 예전처럼 실패해요
            let extras = [("github", gh_repo.is_some()), ("slack", with_slack)];
            if extras
                .iter()
                .filter(|(_, on)| *on)
                .all(|(n, _)| warnings.iter().any(|w| w == n))
            {
                return Err(e);
            }
        }
        if let Some(Value::Object(g)) = info.get_mut("github") {
            g.insert("raw".into(), Value::from(self.places.rel(&raw)));
        }
        let (notion_chunks, notion_info) = self.notion_part(&mut warnings)?;
        chunks.extend(notion_chunks);
        if let Some(n) = notion_info {
            info.insert("notion".into(), n);
        }
        let (linear_chunks, linear_info) = self.linear_part(&mut warnings);
        chunks.extend(linear_chunks);
        if let Some(l) = linear_info {
            info.insert("linear".into(), l);
        }
        let (gdrive_chunks, gdrive_info) = self.gdrive_part(&mut warnings);
        chunks.extend(gdrive_chunks);
        if let Some(g) = gdrive_info {
            info.insert("gdrive".into(), g);
        }
        let (atlassian_chunks, atlassian_info) = self.atlassian_part(&mut warnings);
        chunks.extend(atlassian_chunks);
        if let Some(a) = atlassian_info {
            info.insert("atlassian".into(), a);
        }
        let (asana_chunks, asana_info) = self.asana_part(&mut warnings);
        chunks.extend(asana_chunks);
        if let Some(a) = asana_info {
            info.insert("asana".into(), a);
        }
        let (dropbox_chunks, dropbox_info) = self.dropbox_part(&mut warnings);
        chunks.extend(dropbox_chunks);
        if let Some(d) = dropbox_info {
            info.insert("dropbox".into(), d);
        }
        let (meeting_chunks, meeting_info) = self.meeting_part();
        chunks.extend(meeting_chunks);
        if let Some(m) = meeting_info {
            info.insert("meeting".into(), m);
        }
        let (notes_chunks, notes_info) = self.notes_part();
        chunks.extend(notes_chunks);
        if let Some(n) = notes_info {
            info.insert("notes".into(), n);
        }

        // meta에서 cleared_at을 빼고 repo, commit, built_at, embed, sources를 넣어요:
        // 있는 키는 제자리, 없는 키는 뒤예요
        let mut new_meta = meta.clone();
        new_meta.shift_remove("cleared_at");
        if switch {
            new_meta.insert("repo".into(), Value::from(name.as_str()));
        }
        new_meta.insert("commit".into(), Value::from(commit.as_str()));
        new_meta.insert("built_at".into(), Value::from(now()));
        let embed = json!({"model": MODEL, "tokens": PASSAGE_TOKENS});
        new_meta.insert("embed".into(), embed.clone());
        let mut repo_info = r.clone();
        repo_info.insert("branch".into(), Value::from(branch.as_str()));
        repo_info.insert("symbols_missing".into(), symbols_missing);
        let mut sources = Map::new();
        sources.insert("repo".into(), Value::Object(repo_info));
        for (k, v) in info {
            sources.insert(k, v);
        }
        new_meta.insert("sources".into(), Value::Object(sources));

        // 같은 브랜치에 새것이 없고 같은 방식으로 임베딩했으면(meta가 "embed"를 갖기 전의 색인은 bge-m3, 512로
        // 만들었어요) 색인, 벡터, 캐시는 그대로예요. built_at은 모든 출처가 왔을 때만 옮겨요: 저장본이 섞이면 이번
        // 동기화는 끝나지 않은 거라 meta.json도 그대로예요. "github-lost"는 GitHub의 답이지 끝나지 않은 동기화가 아니에요
        let default_embed = json!({"model": "BAAI/bge-m3", "tokens": PASSAGE_TOKENS});
        let same_embed = *meta.get("embed").unwrap_or(&default_embed) == embed;
        let commit8 = py::take_chars(&commit, 8).to_string();
        if !switch
            && !fresh
            && same_branch
            && meta.get("commit").and_then(Value::as_str) == Some(commit.as_str())
            && same_embed
            && chunks == *live.raw
        {
            if warnings.iter().all(|w| w == "github-lost") {
                write_json(
                    &self.index_dir.join("meta.json"),
                    &Value::Object(new_meta.clone()),
                    Some(1),
                )?;
                self.engine.index.store(Arc::new(live.with_meta(new_meta)));
            }
            self.set_status(
                json!({"state": "done", "finished_at": now(), "chunks": chunks.len(),
                                   "embedded": 0, "commit": commit8, "warnings": warnings,
                                   "unchanged": true}),
            );
            eprintln!("refresh: done, nothing changed, commit {commit8}");
            return Ok(());
        }

        let tag = format!("{}#{branch}", repo_key(&name));
        // 다른 저장소로 바꾸는 중에는 옛 저장소의 조각을 남기지 않아요: 새 meta의 저장소로 링크가 걸려요
        let keep_old = |c: &RawChunk| {
            !switch
                || !matches!(
                    c.get("src").and_then(Value::as_str),
                    Some("code" | "docs" | "github")
                )
        };
        let embedded = self.install(&chunks, &new_meta, fresh, &tag, &keep_old)?;
        if let Some(mut rec) = rec {
            rec.insert("branch".into(), Value::from(branch.as_str()));
            self.remember(meta, &name, rec)?;
        }
        self.set_status(
            json!({"state": "done", "finished_at": now(), "chunks": chunks.len(),
                               "embedded": embedded, "commit": commit8, "warnings": warnings}),
        );
        eprintln!(
            "refresh: done, {} chunks, {embedded} embedded, commit {commit8}, took {}",
            chunks.len(),
            self.took()
        );
        Ok(())
    }

    /// 조각을 임베딩해 새 색인을 옆자리에 쓰고 바꿔 넣어요: 새로 임베딩한 조각 수예요. 바꿔 넣을 때까지 옛 색인이
    /// 답해요. `fresh`면 옛 색인의 벡터를 쓰지 않아요. `tag`는 임베딩 캐시의 이름표예요.
    fn install(
        &self,
        chunks: &[RawChunk],
        new_meta: &Map<String, Value>,
        fresh: bool,
        tag: &str,
        keep_old: &dyn Fn(&RawChunk) -> bool,
    ) -> Result<usize, SourceError> {
        self.step("embed");
        let tmp = beside(&self.index_dir, ".new");
        let _ = fs::remove_dir_all(&tmp);
        let emb_cache = EmbCache::new(&self.places.emb_cache_root(), MODEL, PASSAGE_TOKENS);
        // 중간 색인의 벡터는 색인 전체를 다 쓸 때까지 따로 태그를 달아 둬요
        let partial_tag = format!("{tag}{PARTIAL_TAG}");
        let embedded = write_index(
            &tmp,
            chunks,
            &mut |texts| {
                self.embed_in_parts(texts, chunks, new_meta, &emb_cache, &partial_tag, keep_old)
            },
            new_meta,
            if fresh { None } else { Some(&self.index_dir) },
            Some((&emb_cache, tag)),
        );
        if let Err(e) = emb_cache.drop_prefix(&partial_tag) {
            eprintln!("refresh: the partial vectors' tag stays ({e})");
        }
        let embedded = embedded?;
        self.stop_if_cancelled()?;

        self.step("swap");
        self.swap_in(&tmp)?;
        Ok(embedded)
    }

    /// 다 쓴 색인 폴더를 지금 색인 자리에 넣고 열어요. 옛 색인은 `index.old`로 가요.
    fn swap_in(&self, dir: &Path) -> Result<(), SourceError> {
        let old = beside(&self.index_dir, ".old");
        let _ = fs::remove_dir_all(&old);
        fs::rename(&self.index_dir, &old)?;
        fs::rename(dir, &self.index_dir)?;
        let opened = (self.open_index)(&self.index_dir)?;
        self.engine.index.store(Arc::new(opened));
        Ok(())
    }

    /// 동기화의 임베딩이에요. 큰 것(처음 동기화, 새 저장소, 많이 바뀐 것)은 `PARTIAL_EVERY`개씩 나눠 하고, 한 묶음이
    /// 끝날 때마다 준비된 것을 검색에 넣어요(`publish_partial`). 질문은 동기화가 다 끝나기를 기다리지 않고 받은 만큼
    /// 찾아요. 작은 것은 예전처럼 한 번에 해요.
    ///
    /// 길이가 비슷한 조각끼리 한 배치가 돼야 패딩이 적어서(`tacie_embed` 토크나이저는 배치의 가장 긴 조각에 맞춰
    /// 채워요), 글 길이(바이트)순으로 임베딩하고 원래 순서로 돌려놔요. 그래서 짧은 조각이 먼저 검색에 들어가요.
    fn embed_in_parts(
        &self,
        texts: &[&str],
        chunks: &[RawChunk],
        new_meta: &Map<String, Value>,
        cache: &EmbCache,
        partial_tag: &str,
        keep_old: &dyn Fn(&RawChunk) -> bool,
    ) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut order: Vec<usize> = (0..texts.len()).collect();
        order.sort_by_key(|&i| texts[i].len());
        let sorted: Vec<&str> = order.iter().map(|&i| texts[i]).collect();
        let vectors = self.embed_sorted(&sorted, chunks, new_meta, cache, partial_tag, keep_old)?;
        let mut out = vec![Vec::new(); texts.len()];
        for (i, v) in order.into_iter().zip(vectors) {
            out[i] = v;
        }
        Ok(out)
    }

    /// `embed_in_parts`의 몸이에요: `texts`는 길이순이에요.
    fn embed_sorted(
        &self,
        texts: &[&str],
        chunks: &[RawChunk],
        new_meta: &Map<String, Value>,
        cache: &EmbCache,
        partial_tag: &str,
        keep_old: &dyn Fn(&RawChunk) -> bool,
    ) -> anyhow::Result<Vec<Vec<f32>>> {
        let total = texts.len();
        let (min, every) = self.partial;
        if total < min {
            return self.embed_watched(texts, 0, total);
        }
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(total);
        for part in texts.chunks(every) {
            let base = out.len();
            let vectors = self.embed_watched(part, base, total)?;
            out.extend(vectors);
            if out.len() < total {
                cache.put(part, &out[base..])?;
                let pending: HashSet<&str> = texts[out.len()..].iter().copied().collect();
                match self.publish_partial(chunks, &pending, new_meta, cache, partial_tag, keep_old)
                {
                    Ok(n) => {
                        self.mark_partial();
                        eprintln!(
                            "refresh: {n} chunks searchable while {} wait (at {})",
                            total - out.len(),
                            self.took()
                        );
                    }
                    Err(e) => eprintln!("refresh: no partial index this time ({e})"),
                }
            }
        }
        Ok(out)
    }

    /// `WATCH_EVERY`개씩 임베딩하고, 그 사이마다 연결이 바뀌었는지 봐요. 진행은 `base`부터 `total`까지예요.
    fn embed_watched(
        &self,
        texts: &[&str],
        base: usize,
        total: usize,
    ) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for slice in texts.chunks(WATCH_EVERY) {
            if self.cancelled() {
                anyhow::bail!(CANCELLED);
            }
            let done = base + out.len();
            let vectors = self
                .engine
                .embed_passages(slice, &mut |n, _| self.progress(done + n, total))?;
            out.extend(vectors);
        }
        if texts.is_empty() {
            self.progress(base, total);
        }
        Ok(out)
    }

    /// 준비된 것으로 색인을 써서 바꿔 넣어요: `pending`이 아닌 새 조각, 그리고 새 조각이 하나도 준비되지 않은 문서는
    /// 지금 색인의 조각이에요(보통 동기화가 크게 돌 때도 찾던 것을 잃지 않아요). `keep_old`가 거짓인 옛 조각은 빼요. 벡터는 모두 캐시나 지금 색인에서 와요.
    /// 동기화가 끝나지 않았으니 `built_at`은 지금 색인의 것 그대로예요.
    fn publish_partial(
        &self,
        chunks: &[RawChunk],
        pending: &HashSet<&str>,
        new_meta: &Map<String, Value>,
        cache: &EmbCache,
        partial_tag: &str,
        keep_old: &dyn Fn(&RawChunk) -> bool,
    ) -> anyhow::Result<usize> {
        let text = |c: &RawChunk| {
            c.get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let path = |c: &RawChunk| {
            c.get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let ready: Vec<RawChunk> = chunks
            .iter()
            .filter(|c| !pending.contains(text(c).as_str()))
            .cloned()
            .collect();
        let ready_paths: HashSet<String> = ready.iter().map(path).collect();
        let live = self.engine.index.load_full();
        let mut part = ready;
        part.extend(
            live.raw
                .iter()
                .filter(|c| keep_old(c) && !ready_paths.contains(&path(c)))
                .cloned(),
        );
        let mut meta = new_meta.clone();
        meta.insert(
            "built_at".into(),
            live.meta.get("built_at").cloned().unwrap_or(Value::Null),
        );
        let dir = beside(&self.index_dir, ".part");
        let _ = fs::remove_dir_all(&dir);
        write_index(
            &dir,
            &part,
            &mut |_| Err(anyhow::anyhow!("a partial index embeds nothing")),
            &meta,
            Some(&self.index_dir),
            Some((cache, partial_tag)),
        )?;
        // 이번 동기화의 첫 중간 색인이면 동기화 전 색인을 `index.keep`에 둬요(멈추면 돌아가요)
        let keep = beside(&self.index_dir, KEEP);
        if !keep.exists() {
            fs::rename(&self.index_dir, &keep)?;
            fs::rename(&dir, &self.index_dir)?;
            self.engine
                .index
                .store(Arc::new((self.open_index)(&self.index_dir)?));
        } else {
            self.swap_in(&dir)?;
        }
        Ok(part.len())
    }

    /// 상태에 이번 동기화의 일부를 벌써 찾는다고 적어요(`partial`, 앱이 사용자에게 말해요).
    fn mark_partial(&self) {
        let mut st = self.status().as_object().cloned().unwrap_or_default();
        st.insert("partial".into(), Value::Bool(true));
        self.set_status(Value::Object(st));
    }

    /// 노션 원본 폴더예요(`raw/notion`).
    pub fn notion_dir(&self) -> PathBuf {
        self.places.raw().join("notion")
    }

    /// 노션을 넣을지예요: 연결돼 있거나, 앱이 아직 연결을 넘기지 않았어도 받아 둔 것이 있으면이에요.
    fn notion_on(&self) -> bool {
        self.sources.notion.connected() || self.notion_dir().join(notion::FILE).exists()
    }

    /// 노션의 조각과 info예요(연결돼 있으면 받고, 진행은 `step` "notion"과 `progress`로 보여요).
    fn notion_part(
        &self,
        warnings: &mut Vec<String>,
    ) -> Result<(Vec<RawChunk>, Option<Value>), SourceError> {
        if !self.notion_on() {
            return Ok((Vec::new(), None));
        }
        self.step("notion");
        notion::gather(
            self.sources.notion.as_ref(),
            &self.notion_dir(),
            true,
            &mut |done, total| self.progress(done, total),
            warnings,
        )
    }

    /// Linear 원본 폴더예요(`raw/linear`).
    pub fn linear_dir(&self) -> PathBuf {
        self.places.raw().join("linear")
    }

    /// Linear를 넣을지예요: 연결돼 있거나, 받아 둔 것이 있으면이에요.
    fn linear_on(&self) -> bool {
        self.sources.linear.connected() || self.linear_dir().join("state.json").exists()
    }

    /// 갱신 중에 Linear 연결이나 끈 팀이 바뀌었어요: 이 갱신이 끝나면 한 번 더 돌려요.
    pub fn queue_personal(&self) {
        self.pending_personal
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// 받은 Linear 자료를 지워요(연결 해제, 다른 워크스페이스). 도는 받기는 세대가 바뀌어 더 쓰지 않아요.
    pub fn forget_linear(&self) -> std::io::Result<()> {
        linear::forget(&self.linear_dir())
    }

    /// 30분 확인의 Linear 몫이에요: 가장 늦게 고친 항목이 저장본보다 늦으면 참이에요. 물을 수 없으면 받은 지
    /// `LINEAR_EVERY`가 지났는지로 정해요. 갱신 자물쇠 밖에서 불러요.
    fn linear_changed(&self, timeout: Duration) -> bool {
        let dir = self.linear_dir();
        let Some(latest) = linear::latest(&dir) else {
            return true; // 받은 적이 없어요
        };
        let Some(api) = self.sources.linear.checker(timeout) else {
            return false;
        };
        match api.query(LINEAR_CHECK, &json!({})) {
            Ok(r) => {
                let Some(d) = r.data else {
                    return linear::due(&dir, LINEAR_EVERY);
                };
                ["issues", "documents", "comments", "projectUpdates"]
                    .iter()
                    .any(|k| {
                        d[k]["nodes"][0]["updatedAt"]
                            .as_str()
                            .is_some_and(|u| u > latest.as_str())
                    })
            }
            Err(_) => linear::due(&dir, LINEAR_EVERY),
        }
    }

    /// Linear의 조각과 info예요. **실패해도 갱신을 실패시키지 않아요**(docs/18 16-1절): 저장본도 없이 실패하면
    /// 조각 없이 경고 `linear`와 `error_kind`만 남겨요. 저장본의 워크스페이스가 지금 연결과 다르면 먼저 지워요.
    fn linear_part(&self, warnings: &mut Vec<String>) -> (Vec<RawChunk>, Option<Value>) {
        if !self.linear_on() {
            return (Vec::new(), None);
        }
        let dir = self.linear_dir();
        let ls = self.sources.linear.as_ref();
        if let (Some(now_id), Some(saved_id)) = (ls.workspace_id(), linear::stored_workspace(&dir))
            && now_id != saved_id
            && let Err(e) = linear::forget(&dir)
        {
            eprintln!("refresh: linear: the old workspace was not removed ({e})");
        }
        self.step("linear");
        match linear::gather(
            ls,
            &dir,
            true,
            &mut |done, total| self.progress(done, total),
            warnings,
        ) {
            Ok(got) => got,
            Err(e) => {
                warnings.push("linear".to_string());
                eprintln!(
                    "refresh: linear failed ({})",
                    py::take_chars(&e.detail(), 120)
                );
                let info = json!({"workspace": ls.workspace(), "issues": null, "teams": null, "chunks": 0,
                                  "fetched_at": null, "lost_since": null, "error_kind": e.kind().as_str()});
                (Vec::new(), Some(info))
            }
        }
    }

    /// 사람의 출처(노션, Linear, Drive, Atlassian, Asana, Dropbox) 중 넣을 것이 있는지예요.
    fn personal_on(&self) -> bool {
        self.notion_on()
            || self.linear_on()
            || self.gdrive_on()
            || self.atlassian_on()
            || self.asana_on()
            || self.dropbox_on()
            || self.meeting_on()
            || self.notes_on()
    }

    /// 지난 회의 원본 폴더예요(`raw/meeting`, docs/30). 앱이 `POST /meetings`로 보낸 회의가 있어요.
    pub fn meeting_dir(&self) -> PathBuf {
        self.places.raw().join("meeting")
    }

    /// 지난 회의를 넣을지예요: 앱이 보낸 회의 파일이 있으면이에요(빈 목록이어도요).
    fn meeting_on(&self) -> bool {
        self.meeting_dir().join(meeting::FILE).exists()
    }

    /// 이 Mac에서 오는 출처(`meeting`, `notes`)의 조각과 info예요.
    fn local_part(&self, src: &str) -> (Vec<RawChunk>, Option<Value>) {
        match src {
            "meeting" => self.meeting_part(),
            _ => self.notes_part(),
        }
    }

    fn pending_of(&self, src: &str) -> &std::sync::atomic::AtomicBool {
        match src {
            "meeting" => &self.pending_meetings,
            _ => &self.pending_notes,
        }
    }

    /// 이 Mac에서 오는 출처의 조각이 검색 데이터와 다른지예요. 사용자가 검색 데이터를 지웠으면(씨앗이 아닌데
    /// `cleared_at`) 다시 동기화할 때까지 넣지 않아요.
    fn local_stale(&self, live: &Index, src: &str) -> bool {
        let user_cleared = live.meta.get("cleared_at").is_some_and(py::truthy)
            && live.meta.get("sources").is_some();
        if user_cleared {
            return false;
        }
        let (found, _) = self.local_part(src);
        let now: Vec<&RawChunk> = live
            .raw
            .iter()
            .filter(|c| c.get("src").and_then(Value::as_str) == Some(src))
            .collect();
        found.len() != now.len() || found.iter().zip(now).any(|(a, b)| a != b)
    }

    /// 앱이 회의 목록을 보냈어요(docs/30): 회의 조각만 바꿔 끼워요(`local_changed`).
    pub fn meetings_changed(self: &Arc<Self>) {
        self.local_changed("meeting");
    }

    /// 노트 폴더나 노트가 바뀌었어요(docs/34): 노트 조각만 바꿔 끼워요(`local_changed`).
    pub fn notes_changed(self: &Arc<Self>) {
        self.local_changed("notes");
    }

    /// 이 Mac에서 오는 출처의 조각만 바꿔 끼워요. 다른 출처는 다시 받지 않고(GitHub·노션에 묻지 않아요) 지난
    /// 동기화의 상태도 그대로 둬요. 갱신이나 지우기가 도는 중이면 그게 끝난 뒤에 해요.
    fn local_changed(self: &Arc<Self>, src: &'static str) {
        if self.busy() {
            self.pending_of(src)
                .store(true, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        let this = self.clone();
        let spawned = std::thread::Builder::new()
            .name(THREAD.to_string())
            .spawn(move || {
                // 갱신 자물쇠를 쥐고 해요: 그동안 동기화와 지우기는 시작하지 않아요
                let Some(_held) = this.try_hold() else {
                    this.pending_of(src)
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    return;
                };
                if this.busy() {
                    this.pending_of(src)
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
                let before = this.status();
                *locked(&this.began) = Some(Instant::now());
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    let _gpu = ReleaseGpu(&this.engine);
                    this.swap_local(src)
                }));
                // 넣는 동안 적힌 단계(`embed`, `swap`)는 지워요: 앱에는 동기화로 보이지 않아요
                this.set_status((*before).clone());
                let name = if src == "meeting" {
                    "meetings"
                } else {
                    "notes"
                };
                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        eprintln!("{name}: not put in ({})", py::take_chars(&e.detail(), 120))
                    }
                    Err(_) => eprintln!("{name}: not put in ({INTERNAL})"),
                }
            });
        if let Err(e) = spawned {
            eprintln!("{src}: not put in (could not start: {e})");
        }
    }

    /// 지금 색인에서 그 출처의 조각만 새로 모은 것으로 바꿔요. 그 밖에 남는 게 없으면 빈 씨앗으로 돌아가요.
    fn swap_local(&self, src: &str) -> Result<(), SourceError> {
        let live = self.engine.index.load_full();
        if !self.local_stale(&live, src) {
            return Ok(());
        }
        let (found, info) = self.local_part(src);
        // 씨앗(`cleared_at`이 있고 출처가 없어요)에는 남길 것이 없어요
        let seed = live.meta.get("cleared_at").is_some_and(py::truthy)
            && live.meta.get("sources").is_none();
        let mut chunks: Vec<RawChunk> = live
            .raw
            .iter()
            .filter(|c| !seed && c.get("src").and_then(Value::as_str) != Some(src))
            .cloned()
            .collect();
        let mut sources = match live.meta.get("sources") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        sources.shift_remove(src);
        let name = if src == "meeting" {
            "meetings"
        } else {
            "notes"
        };
        if chunks.is_empty() && found.is_empty() && sources.is_empty() {
            self.install(&[], &seed_meta(), true, "notion", &|_| true)?;
            eprintln!("{name}: none left, back to the seed");
            return Ok(());
        }
        let n = found.len();
        chunks.extend(found);
        if let Some(m) = info {
            sources.insert(src.into(), m);
        }
        let mut new_meta = live.meta.clone();
        if new_meta.get("cleared_at").is_some_and(py::truthy) {
            // 씨앗에서 처음 넣어요(회의·노트만 쓰는 사람, `personal_only`와 같아요)
            new_meta.shift_remove("cleared_at");
            new_meta.insert("built_at".into(), Value::from(now()));
            new_meta.insert(
                "embed".into(),
                json!({"model": MODEL, "tokens": PASSAGE_TOKENS}),
            );
        }
        new_meta.insert("sources".into(), Value::Object(sources));
        let embedded = self.install(&chunks, &new_meta, false, "notion", &|_| true)?;
        eprintln!("{name}: {n} chunks in the search data, {embedded} embedded");
        Ok(())
    }

    /// 지난 회의의 조각과 info예요. 이 Mac의 파일이라 실패하지 않아요.
    fn meeting_part(&self) -> (Vec<RawChunk>, Option<Value>) {
        if !self.meeting_on() {
            return (Vec::new(), None);
        }
        meeting::gather(&self.meeting_dir())
    }

    /// 노트 폴더 목록을 두는 곳이에요(`raw/notes`, docs/34). 앱이 `POST /notes/folders`로 보낸 폴더가 있어요.
    pub fn notes_dir(&self) -> PathBuf {
        self.places.raw().join("notes")
    }

    /// 노트를 넣을지예요: 앱이 보낸 폴더 파일이 있으면이에요(빈 목록이어도요).
    fn notes_on(&self) -> bool {
        self.notes_dir().join(notes::FILE).exists()
    }

    /// 노트의 조각과 info예요. 이 Mac의 파일이라 실패하지 않아요(읽지 못한 파일은 제목만이에요).
    fn notes_part(&self) -> (Vec<RawChunk>, Option<Value>) {
        if !self.notes_on() {
            return (Vec::new(), None);
        }
        notes::gather(&self.notes_dir())
    }

    /// 노트 폴더의 파일이 검색 데이터를 만든 때와 다른지 싸게 봐요(경로·크기·수정 시각의 지문, 파일은 읽지 않아요).
    /// 폴더를 다 뺐는데 노트 조각이 남아 있어도 참이에요.
    pub fn notes_stale_quickly(&self) -> bool {
        let live = self.engine.index.load_full();
        let saved = live
            .meta
            .get("sources")
            .and_then(|s| s.get("notes"))
            .and_then(|n| n.get("signature"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if !self.notes_on() {
            return saved.is_some();
        }
        let now = notes::signature(&notes::folders(&self.notes_dir()));
        saved.as_deref() != Some(now.as_str())
    }

    /// Drive 원본 폴더예요(`raw/gdrive`).
    pub fn gdrive_dir(&self) -> PathBuf {
        self.places.raw().join("gdrive")
    }

    /// Drive를 넣을지예요: 연결돼 있거나, 받아 둔 것이 있으면이에요.
    fn gdrive_on(&self) -> bool {
        self.sources.gdrive.connected() || self.gdrive_dir().join("state.json").exists()
    }

    /// 받은 Drive 자료를 지워요(연결 해제, 다른 계정). 도는 받기는 세대가 바뀌어 더 쓰지 않아요.
    pub fn forget_gdrive(&self) -> std::io::Result<()> {
        gdrive::forget(&self.gdrive_dir())
    }

    /// 30분 확인의 Drive 몫이에요: 받을 변경(받는 종류이면서 범위 안)이 있으면 참이에요. 물을 수 없으면 받은 지
    /// `GDRIVE_EVERY`가 지났는지로 정해요. 갱신 자물쇠 밖에서 불러요.
    fn gdrive_changed(&self, timeout: Duration) -> bool {
        let dir = self.gdrive_dir();
        match self.sources.gdrive.changed(&dir, timeout) {
            None => false,
            Some(Ok(changed)) => changed,
            Some(Err(_)) => gdrive::due(&dir, GDRIVE_EVERY),
        }
    }

    /// Drive의 조각과 info예요. **실패해도 갱신을 실패시키지 않아요**(Linear와 같아요). 저장본의 계정이 지금 연결과
    /// 다르면 먼저 지워요. 한 번에 다 받지 못했으면(처음 받기는 묶음으로 끊어요, docs/19 15-1절) 이 갱신이 끝난 뒤 한
    /// 번 더 돌려요.
    fn gdrive_part(&self, warnings: &mut Vec<String>) -> (Vec<RawChunk>, Option<Value>) {
        if !self.gdrive_on() {
            return (Vec::new(), None);
        }
        let dir = self.gdrive_dir();
        let gs = self.sources.gdrive.as_ref();
        if let (Some(now_id), Some(saved_id)) = (gs.account_id(), gdrive::stored_account(&dir))
            && now_id != saved_id
            && let Err(e) = gdrive::forget(&dir)
        {
            eprintln!("refresh: gdrive: the old account was not removed ({e})");
        }
        self.step("gdrive");
        match gdrive::gather(
            gs,
            &dir,
            true,
            &mut |done, total| self.progress(done, total),
            warnings,
        ) {
            Ok((chunks, info, more)) => {
                if more {
                    eprintln!("refresh: gdrive: more files next time");
                    self.queue_personal();
                }
                (chunks, info)
            }
            Err(e) => {
                warnings.push("gdrive".to_string());
                eprintln!(
                    "refresh: gdrive failed ({})",
                    py::take_chars(&e.detail(), 120)
                );
                let info = json!({"account": gs.account(), "files": null, "drives": null, "chunks": 0,
                                  "fetched_at": null, "initial_done": false, "lost_since": null,
                                  "error_kind": e.kind().as_str()});
                (Vec::new(), Some(info))
            }
        }
    }

    /// Atlassian 원본 폴더예요(`raw/atlassian`).
    pub fn atlassian_dir(&self) -> PathBuf {
        self.places.raw().join("atlassian")
    }

    /// Atlassian을 넣을지예요: 연결돼 있거나, 받아 둔 것이 있으면이에요.
    fn atlassian_on(&self) -> bool {
        self.sources.atlassian.connected() || self.atlassian_dir().join("state.json").exists()
    }

    /// 받은 Atlassian 자료를 지워요(연결 해제, 다른 사이트, 검색 데이터 지우기). 도는 받기는 세대가 바뀌어 더 쓰지
    /// 않아요. 기다리던 이어 받기도 잊어요.
    pub fn forget_atlassian(&self) -> std::io::Result<()> {
        *locked(&self.atlassian_resume) = None;
        atlassian::forget(&self.atlassian_dir())
    }

    /// 예산으로 멈춘 처음 받기를 다시 받을 수 있는 때예요. 멈추지 않았으면 None이에요.
    pub fn atlassian_resume(&self) -> Option<DateTime<Utc>> {
        *locked(&self.atlassian_resume)
    }

    /// 이어 받을 때를 바꿔요(테스트가 정각이 지난 것처럼 만들 때 써요).
    #[doc(hidden)]
    pub fn set_atlassian_resume(&self, at: Option<DateTime<Utc>>) {
        *locked(&self.atlassian_resume) = at;
    }

    /// 30분 확인의 Atlassian 몫이에요. 예산으로 멈춘 처음 받기는 이어 받을 때가 지났는지만 봐요(그 전에는 묻지도
    /// 않아요). 받은 적이 없으면 참이고, 아니면 받을 변경이 있으면 참이에요. 물을 수 없으면 받은 지
    /// `ATLASSIAN_EVERY`가 지났는지로 정해요. 갱신 자물쇠 밖에서 불러요.
    fn atlassian_changed(&self, timeout: Duration) -> bool {
        if let Some(at) = self.atlassian_resume() {
            return at <= Utc::now();
        }
        let dir = self.atlassian_dir();
        if atlassian::due(&dir, i64::MAX) {
            return true; // 받은 적이 없어요
        }
        match self.sources.atlassian.changed(&dir, timeout) {
            None => false,
            Some(Ok(changed)) => changed,
            Some(Err(_)) => atlassian::due(&dir, ATLASSIAN_EVERY),
        }
    }

    /// Atlassian의 조각과 info예요. **실패해도 갱신을 실패시키지 않아요**(Drive와 같아요). 저장본의 사이트가 지금
    /// 연결과 다르면 먼저 지워요. 처음 받기가 시간당 예산에 막혔으면(`more`) 바로 다시 돌리지 않고 다시 받을 수 있는
    /// 때(멈춘 때까지, 없으면 다음 UTC 정각)를 적어 둬요. 그때가 지나면 30분 확인이 이어 받기를 걸어요.
    fn atlassian_part(&self, warnings: &mut Vec<String>) -> (Vec<RawChunk>, Option<Value>) {
        if !self.atlassian_on() {
            return (Vec::new(), None);
        }
        let dir = self.atlassian_dir();
        let src = self.sources.atlassian.as_ref();
        if let (Some(now_cloud), Some(saved_cloud)) = (src.cloud(), atlassian::stored_cloud(&dir))
            && now_cloud != saved_cloud
            && let Err(e) = self.forget_atlassian()
        {
            eprintln!("refresh: atlassian: the old site was not removed ({e})");
        }
        self.step("atlassian");
        // 받는 사이 연결이 바뀌면(해제, 다른 사이트) 그 받기의 이어 받기 때는 적지 않아요
        let generation = src.generation();
        match atlassian::gather(
            src,
            &dir,
            true,
            &mut |done, total| self.progress(done, total),
            warnings,
        ) {
            Ok((chunks, info, more)) => {
                let resume = more.then(|| {
                    info.as_ref()
                        .and_then(|i| i["paused_until"].as_str())
                        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                        .map(|t| t.with_timezone(&Utc))
                        .unwrap_or_else(|| next_utc_hour(Utc::now()))
                });
                if let Some(at) = resume {
                    eprintln!(
                        "refresh: atlassian: more items after {}",
                        at.format("%H:%M UTC")
                    );
                }
                if src.generation() == generation {
                    *locked(&self.atlassian_resume) = resume;
                }
                (chunks, info)
            }
            Err(e) => {
                warnings.push("atlassian".to_string());
                eprintln!(
                    "refresh: atlassian failed ({})",
                    py::take_chars(&e.detail(), 120)
                );
                let info = json!({"site": src.site(), "site_url": null, "fetched_at": null,
                                  "issues": null, "pages": null, "chunks": 0, "initial_done": false,
                                  "lost_since": null, "paused_until": src.paused_until(),
                                  "error_kind": e.kind().as_str()});
                (Vec::new(), Some(info))
            }
        }
    }

    /// Asana 원본 폴더예요(`raw/asana`).
    pub fn asana_dir(&self) -> PathBuf {
        self.places.raw().join("asana")
    }

    /// Asana를 넣을지예요: 연결돼 있거나, 받아 둔 것이 있으면이에요.
    fn asana_on(&self) -> bool {
        self.sources.asana.connected() || self.asana_dir().join("state.json").exists()
    }

    /// 받은 Asana 자료를 지워요(연결 해제, 다른 사용자, 검색 데이터 지우기). 도는 받기는 세대가 바뀌어 더 쓰지
    /// 않아요. 기다리던 이어 받기도 잊어요.
    pub fn forget_asana(&self) -> std::io::Result<()> {
        *locked(&self.asana_resume) = None;
        asana::forget(&self.asana_dir())
    }

    /// 몫을 다 써서 멈춘 받기를 이어 받을 때예요. 멈추지 않았으면 None이에요.
    pub fn asana_resume(&self) -> Option<DateTime<Utc>> {
        *locked(&self.asana_resume)
    }

    /// 이어 받을 때를 바꿔요(테스트가 1분이 지난 것처럼 만들 때 써요).
    #[doc(hidden)]
    pub fn set_asana_resume(&self, at: Option<DateTime<Utc>>) {
        *locked(&self.asana_resume) = at;
    }

    /// 30분 확인의 Asana 몫이에요. 몫으로 멈춘 받기는 이어 받을 때가 지났는지만 봐요(그 전에는 묻지도 않아요). 받은
    /// 적이 없으면 참이고, 아니면 받을 변경이 있으면 참이에요. 물을 수 없으면 받은 지 `ASANA_EVERY`가 지났는지로
    /// 정해요. 갱신 자물쇠 밖에서 불러요.
    fn asana_changed(&self, timeout: Duration) -> bool {
        if let Some(at) = self.asana_resume() {
            return at <= Utc::now();
        }
        let dir = self.asana_dir();
        if asana::due(&dir, i64::MAX) {
            return true; // 받은 적이 없어요
        }
        match self.sources.asana.changed(&dir, timeout) {
            None => false,
            Some(Ok(changed)) => changed,
            Some(Err(_)) => asana::due(&dir, ASANA_EVERY),
        }
    }

    /// Asana의 조각과 info예요. **실패해도 갱신을 실패시키지 않아요**(Atlassian과 같아요). 저장본의 사용자가 지금
    /// 연결과 다르면 먼저 지워요. 몫(3,000번·10분)을 다 써서 멈췄으면(`more`) 바로 다시 돌리지 않고 1분 뒤를 적어
    /// 둬요(Asana 한도는 분 단위예요). 그때가 지나면 30분 확인이 이어 받기를 걸어요. 그동안 GitHub·노션·회의는
    /// 기다리지 않아요.
    fn asana_part(&self, warnings: &mut Vec<String>) -> (Vec<RawChunk>, Option<Value>) {
        if !self.asana_on() {
            return (Vec::new(), None);
        }
        let dir = self.asana_dir();
        let src = self.sources.asana.as_ref();
        if let (Some(now_user), Some(saved_user)) = (src.user_gid(), asana::stored_user(&dir))
            && now_user != saved_user
            && let Err(e) = self.forget_asana()
        {
            eprintln!("refresh: asana: the old user's items were not removed ({e})");
        }
        self.step("asana");
        // 받는 사이 연결이 바뀌면(해제, 다른 사용자) 그 받기의 이어 받기 때는 적지 않아요
        let generation = src.generation();
        match asana::gather(
            src,
            &dir,
            true,
            &mut |done, total| self.progress(done, total),
            warnings,
        ) {
            Ok((chunks, info, more)) => {
                let resume = more.then(|| Utc::now() + chrono::Duration::seconds(ASANA_RESUME));
                if let Some(at) = resume {
                    eprintln!(
                        "refresh: asana: more items after {}",
                        at.format("%H:%M:%S UTC")
                    );
                }
                if src.generation() == generation {
                    *locked(&self.asana_resume) = resume;
                }
                (chunks, info)
            }
            Err(e) => {
                warnings.push("asana".to_string());
                eprintln!(
                    "refresh: asana failed ({})",
                    py::take_chars(&e.detail(), 120)
                );
                let info = json!({"workspaces": null, "tasks": null, "chunks": 0, "fetched_at": null,
                                  "initial_done": false, "lost_since": null,
                                  "error_kind": e.kind().as_str(), "blocked_workspaces": []});
                (Vec::new(), Some(info))
            }
        }
    }

    /// Dropbox 원본 폴더예요(`raw/dropbox`).
    pub fn dropbox_dir(&self) -> PathBuf {
        self.places.raw().join("dropbox")
    }

    /// Dropbox를 넣을지예요: 연결돼 있거나, 받아 둔 것이 있으면이에요.
    fn dropbox_on(&self) -> bool {
        self.sources.dropbox.connected() || self.dropbox_dir().join("state.json").exists()
    }

    /// 받은 Dropbox 자료를 지워요(연결 해제, 다른 계정). 도는 받기는 세대가 바뀌어 더 쓰지 않아요.
    pub fn forget_dropbox(&self) -> std::io::Result<()> {
        dropbox::forget(&self.dropbox_dir())
    }

    /// 30분 확인의 Dropbox 몫이에요: 받을 변경이 있으면 참이에요. 물을 수 없으면 받은 지 `DROPBOX_EVERY`가
    /// 지났는지로 정해요. 갱신 자물쇠 밖에서 불러요.
    fn dropbox_changed(&self, timeout: Duration) -> bool {
        let dir = self.dropbox_dir();
        match self.sources.dropbox.changed(&dir, timeout) {
            None => false,
            Some(Ok(changed)) => changed,
            Some(Err(_)) => dropbox::due(&dir, DROPBOX_EVERY),
        }
    }

    /// Dropbox의 조각과 info예요. **실패해도 갱신을 실패시키지 않아요**(Drive와 같아요). 저장본의 계정이 지금
    /// 연결과 다르면 먼저 지워요. 한 번에 다 받지 못했으면 이 갱신이 끝난 뒤 한 번 더 돌려요.
    fn dropbox_part(&self, warnings: &mut Vec<String>) -> (Vec<RawChunk>, Option<Value>) {
        if !self.dropbox_on() {
            return (Vec::new(), None);
        }
        let dir = self.dropbox_dir();
        let src = self.sources.dropbox.as_ref();
        if let (Some(now_id), Some(saved_id)) = (src.account_id(), dropbox::stored_account(&dir))
            && now_id != saved_id
            && let Err(e) = dropbox::forget(&dir)
        {
            eprintln!("refresh: dropbox: the old account was not removed ({e})");
        }
        self.step("dropbox");
        match dropbox::gather(
            src,
            &dir,
            true,
            &mut |done, total| self.progress(done, total),
            warnings,
        ) {
            Ok((mut chunks, info, more)) => {
                if more {
                    eprintln!("refresh: dropbox: more files next time");
                    self.queue_personal();
                }
                // 노트 출처가 받는 옵시디언 vault는 Dropbox 쪽을 빼요: 같은 노트가 카드 두 장이 되지 않게요(docs/34 6절)
                if self.notes_on() {
                    let local: Vec<String> = notes::folders(&self.notes_dir())
                        .into_iter()
                        .map(|f| f.path)
                        .collect();
                    let skip = dropbox::doc_paths_in_vaults(&dir, &local);
                    if !skip.is_empty() {
                        chunks.retain(|c| {
                            c.get("path")
                                .and_then(Value::as_str)
                                .is_none_or(|p| !skip.contains(p))
                        });
                    }
                }
                (chunks, info)
            }
            Err(e) => {
                warnings.push("dropbox".to_string());
                eprintln!(
                    "refresh: dropbox failed ({})",
                    py::take_chars(&e.detail(), 120)
                );
                let info = json!({"account": src.account(), "files": null, "folders": null, "chunks": 0,
                                  "fetched_at": null, "initial_done": false, "lost_since": null,
                                  "error_kind": e.kind().as_str()});
                (Vec::new(), Some(info))
            }
        }
    }

    /// 코드·GitHub·슬랙 조각은 지금 색인 그대로 두고 노션·Linear만 새로 받아 넣어요(GitHub 쪽이 실패한 갱신). meta의
    /// `sources.notion`·`sources.linear`만 바뀌고 `built_at`은 그대로예요(이번 동기화는 끝나지 않았어요). 노션이
    /// 실패하면 지금 색인의 노션 조각을 그대로 둬요.
    fn personal_beside(&self, live: &Index) -> Result<(), SourceError> {
        let mut warnings = Vec::new();
        let notion = self.notion_part(&mut warnings);
        let (linear_found, linear_info) = self.linear_part(&mut warnings);
        let (gdrive_found, gdrive_info) = self.gdrive_part(&mut warnings);
        let (atlassian_found, atlassian_info) = self.atlassian_part(&mut warnings);
        let (asana_found, asana_info) = self.asana_part(&mut warnings);
        let (dropbox_found, dropbox_info) = self.dropbox_part(&mut warnings);
        let (meeting_found, meeting_info) = self.meeting_part();
        let (notes_found, notes_info) = self.notes_part();
        let keep_notion = notion.is_err();
        let mut chunks: Vec<RawChunk> = live
            .raw
            .iter()
            .filter(|c| match c.get("src").and_then(Value::as_str) {
                Some("notion") => keep_notion,
                Some("linear") | Some("gdrive") | Some("jira") | Some("confluence")
                | Some("asana") | Some("dropbox") | Some("meeting") | Some("notes") => false,
                _ => true,
            })
            .cloned()
            .collect();
        let mut new_meta = live.meta.clone();
        let mut sources = match new_meta.get("sources") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        match notion {
            Ok((found, info)) => {
                chunks.extend(found);
                match info {
                    Some(n) => sources.insert("notion".into(), n),
                    None => sources.shift_remove("notion"),
                };
            }
            Err(e) => eprintln!(
                "refresh: notion kept as it was ({})",
                py::take_chars(&e.detail(), 120)
            ),
        }
        chunks.extend(linear_found);
        match linear_info {
            Some(l) => sources.insert("linear".into(), l),
            None => sources.shift_remove("linear"),
        };
        chunks.extend(gdrive_found);
        match gdrive_info {
            Some(g) => sources.insert("gdrive".into(), g),
            None => sources.shift_remove("gdrive"),
        };
        chunks.extend(atlassian_found);
        match atlassian_info {
            Some(a) => sources.insert("atlassian".into(), a),
            None => sources.shift_remove("atlassian"),
        };
        chunks.extend(asana_found);
        match asana_info {
            Some(a) => sources.insert("asana".into(), a),
            None => sources.shift_remove("asana"),
        };
        chunks.extend(dropbox_found);
        match dropbox_info {
            Some(d) => sources.insert("dropbox".into(), d),
            None => sources.shift_remove("dropbox"),
        };
        chunks.extend(meeting_found);
        match meeting_info {
            Some(m) => sources.insert("meeting".into(), m),
            None => sources.shift_remove("meeting"),
        };
        chunks.extend(notes_found);
        match notes_info {
            Some(n) => sources.insert("notes".into(), n),
            None => sources.shift_remove("notes"),
        };
        if chunks == *live.raw {
            return Ok(());
        }
        new_meta.insert("sources".into(), Value::Object(sources));
        // 임베딩 캐시 이름표는 노션 시절 그대로 "notion"이에요(바꾸면 받아 둔 벡터를 다시 계산해요)
        let embedded = self.install(&chunks, &new_meta, false, "notion", &|_| true)?;
        eprintln!(
            "refresh: notion and linear refreshed beside a failed GitHub sync, {embedded} embedded"
        );
        Ok(())
    }

    /// 저장소 없이 노션·Linear 조각만으로 색인을 만들어요(docs/13 4절, docs/18). meta의 `sources`에는 그 둘만 있어요.
    /// 노션이 실패해도 Linear가 있으면 Linear로 만들어요. 나중에 저장소를 고르면 보통 길이 같이 넣어 다시 만들어요.
    fn personal_only(&self, live: &Index, target: &Target) -> Result<(), SourceError> {
        let mut warnings = Vec::new();
        let notion = self.notion_part(&mut warnings);
        let (linear_chunks, linear_info) = self.linear_part(&mut warnings);
        let (gdrive_chunks, gdrive_info) = self.gdrive_part(&mut warnings);
        let (atlassian_chunks, atlassian_info) = self.atlassian_part(&mut warnings);
        let (asana_chunks, asana_info) = self.asana_part(&mut warnings);
        let (dropbox_chunks, dropbox_info) = self.dropbox_part(&mut warnings);
        let (meeting_chunks, meeting_info) = self.meeting_part();
        let (notes_chunks, notes_info) = self.notes_part();
        let mut chunks = Vec::new();
        let mut sources = Map::new();
        match notion {
            Ok((found, Some(info))) => {
                chunks.extend(found);
                sources.insert("notion".into(), info);
            }
            Ok((_, None)) => {}
            Err(e)
                if linear_info.is_none()
                    && gdrive_info.is_none()
                    && atlassian_info.is_none()
                    && asana_info.is_none()
                    && dropbox_info.is_none()
                    && meeting_info.is_none()
                    && notes_info.is_none() =>
            {
                return Err(e);
            }
            Err(e) => {
                warnings.push("notion".to_string());
                eprintln!(
                    "refresh: notion failed, the other personal sources only ({})",
                    py::take_chars(&e.detail(), 120)
                );
            }
        }
        chunks.extend(linear_chunks);
        if let Some(l) = linear_info {
            sources.insert("linear".into(), l);
        }
        chunks.extend(gdrive_chunks);
        if let Some(g) = gdrive_info {
            sources.insert("gdrive".into(), g);
        }
        chunks.extend(atlassian_chunks);
        if let Some(a) = atlassian_info {
            sources.insert("atlassian".into(), a);
        }
        chunks.extend(asana_chunks);
        if let Some(a) = asana_info {
            sources.insert("asana".into(), a);
        }
        chunks.extend(dropbox_chunks);
        if let Some(d) = dropbox_info {
            sources.insert("dropbox".into(), d);
        }
        chunks.extend(meeting_chunks);
        if let Some(m) = meeting_info {
            sources.insert("meeting".into(), m);
        }
        chunks.extend(notes_chunks);
        if let Some(n) = notes_info {
            sources.insert("notes".into(), n);
        }
        if sources.is_empty() {
            return Err(other(NO_REPO));
        }
        let embed = json!({"model": MODEL, "tokens": PASSAGE_TOKENS});
        let mut new_meta = live.meta.clone();
        new_meta.shift_remove("cleared_at");
        new_meta.insert("built_at".into(), Value::from(now()));
        new_meta.insert("embed".into(), embed.clone());
        new_meta.insert("sources".into(), Value::Object(sources));
        let unchanged = !target.fresh
            && live.meta.get("embed") == Some(&embed)
            && !live.meta.get("cleared_at").is_some_and(py::truthy)
            && chunks == *live.raw;
        if unchanged {
            if warnings.is_empty() {
                write_json(
                    &self.index_dir.join("meta.json"),
                    &Value::Object(new_meta.clone()),
                    Some(1),
                )?;
                self.engine.index.store(Arc::new(live.with_meta(new_meta)));
            }
            self.set_status(
                json!({"state": "done", "finished_at": now(), "chunks": chunks.len(),
                                   "embedded": 0, "commit": null, "warnings": warnings,
                                   "unchanged": true}),
            );
            eprintln!("refresh: done, nothing changed (notion and linear only)");
            return Ok(());
        }
        let embedded = self.install(&chunks, &new_meta, target.fresh, "notion", &|_| true)?;
        self.set_status(
            json!({"state": "done", "finished_at": now(), "chunks": chunks.len(),
                               "embedded": embedded, "commit": null, "warnings": warnings}),
        );
        eprintln!(
            "refresh: done, {} chunks, {embedded} embedded (notion and linear only)",
            chunks.len()
        );
        Ok(())
    }

    /// 색인의 저장소가 GitHub 원본을 두는 곳이에요. 처음 저장소는 `data/sources/github`에 뒀어요.
    fn raw_of(&self, src: &Map<String, Value>) -> Result<PathBuf, SourceError> {
        let github = object_or_empty(src.get("github"), "meta.json's sources.github")?;
        Ok(
            match truthy_text(&github, "raw", "meta.json's sources.github")? {
                Some(raw) => self.places.resolve(raw),
                None => self.places.raw().join("github"),
            },
        )
    }

    /// repos.json이에요: owner/name(소문자) → `{"path", "raw", "graph"?, "branch"?, "slack"?}`. 다른 대소문자로 둔
    /// 옛 키는 읽으면서 소문자로 바꿔요. 없으면 비어요. 읽거나 풀지 못하면 Err예요.
    fn known(&self) -> Result<Map<String, Value>, SourceError> {
        let f = self.places.raw().join("repos.json");
        if !f.exists() {
            return Ok(Map::new());
        }
        let text = fs::read_to_string(&f)?;
        match serde_json::from_str(&text) {
            Ok(Value::Object(m)) => Ok(m.into_iter().map(|(k, v)| (repo_key(&k), v)).collect()),
            Ok(_) => Err(SourceError::Json("repos.json is not an object".to_string())),
            Err(e) => Err(SourceError::Json(format!("repos.json: {e}"))),
        }
    }

    /// 바꾼 저장소와, 지금까지 색인의 저장소(옛 meta에서)를 기억해요: 자리, 브랜치, 슬랙이 있었는지예요. 그래서 돌아오면
    /// 거기서 이어가요.
    fn remember(
        &self,
        meta: &Map<String, Value>,
        name: &str,
        rec: Map<String, Value>,
    ) -> Result<(), SourceError> {
        let mut known = self.known()?;
        let src = object_or_empty(meta.get("sources"), "meta.json's sources")?;
        let was = meta
            .get("repo")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        if let Some(was) = was
            && src.get("repo").is_some_and(py::truthy)
        {
            let Some(r) = src["repo"].as_object() else {
                return Err(other("meta.json's sources.repo is not an object"));
            };
            let mut entry = Map::new();
            entry.insert(
                "path".into(),
                r.get("path")
                    .cloned()
                    .ok_or_else(|| other("meta.json's sources.repo has no path"))?,
            );
            entry.insert(
                "raw".into(),
                Value::from(self.places.rel(&self.raw_of(&src)?)),
            );
            for key in ["graph", "branch"] {
                if r.get(key).is_some_and(py::truthy) {
                    entry.insert(key.into(), r[key].clone());
                }
            }
            if src.contains_key("slack") {
                entry.insert("slack".into(), Value::Bool(true));
            }
            known.insert(repo_key(was), Value::Object(entry));
        }
        known.insert(repo_key(name), Value::Object(rec));
        write_json(
            &self.places.raw().join("repos.json"),
            &Value::Object(known),
            Some(1),
        )?;
        Ok(())
    }

    /// 브랜치 머리의 코드예요(스펙 5-1): 머리 커밋, 그 트리, 그래프가 본 blob과 다른 파일(`skip`, 그래프를 만든 커밋을
    /// GitHub이 모르면 모두), 조각 캐시에 없는 파일의 바이트예요(`cache`가 없으면(`fresh`) 모두). 반쯤 받은 것은
    /// 어디에도 남지 않아요: 바이트는 메모리에만 있고 조각 캐시는 gather가 다 받은 뒤에 써요.
    fn fetch_code(
        &self,
        name: &str,
        branch: &str,
        graph_dir: Option<&Path>,
        graph: Option<&Path>,
        cache: Option<&Path>,
    ) -> Result<(String, Snapshot, Vec<String>), SourceError> {
        let gh = self.sources.github.as_ref();
        let head = content::head_commit(gh, name, branch, None)?;
        // 방금 답한 머리의 트리가 없으면(GitHub이 잃었어요) 받기 실패예요(down)
        let tree = content::tree(gh, name, &head)?.ok_or_else(|| {
            SourceError::service(Kind::Down, "GitHub has no tree for the branch head")
        })?;
        let skip: Vec<String> = match graph_dir {
            Some(dir) => {
                let gb = repo::graph_blobs(gh, name, dir)?;
                tree.blobs()
                    .into_iter()
                    .filter(|(p, b)| gb.as_ref().is_none_or(|gb| gb.get(p) != Some(b)))
                    .map(|(p, _)| p)
                    .collect()
            }
            None => Vec::new(),
        };
        let snap = content::snapshot_of(gh, name, tree, graph, &skip, cache)?;
        Ok((head, snap, skip))
    }

    /// 저장소의 GitHub 원본을 옆(`<이름>+new`)에 처음부터 받아 다 왔을 때만 제자리에 둬요. 404
    /// 한 번은 저장본을 바꾸지도(닫지도) 않아요.
    fn fresh_raw(&self, name: &str, raw: &Path) -> Result<(), SourceError> {
        let tmp = beside(raw, "+new");
        let _ = fs::remove_dir_all(&tmp);
        let scratch = Scratch(Some(&tmp));
        if github_fetch(self.sources.github.as_ref(), name, &tmp)?.lost != 0 {
            return Err(SourceError::service(
                Kind::Key,
                "GitHub cannot see this repository now",
            ));
        }
        scratch.keep();
        swap_in(&tmp, raw)?;
        Ok(())
    }

    /// 코드를 받기 전 GitHub의 말이에요: 사라졌거나 숨었거나(404, 410), 이제 다른 저장소예요(`raw`에 저장된 것과 다른
    /// id의 200). 보기만 해요: 상태는 바꾸지 않아요(github fetch가 세요). 다른 답이나 답이 없으면(오프라인, 너무
    /// 느림) 말이 없는 것이고 코드 받기는 그대로 가요.
    fn gone(&self, name: &str, raw: &Path) -> bool {
        let gh = self.sources.github.as_ref();
        let Ok((status, _, body)) =
            gh.status(&format!("repos/{name}"), &[], Some(gh.timeouts().check))
        else {
            return false;
        };
        if status == 404 || status == 410 {
            return true;
        }
        let known = load_state(raw)
            .get("repo_id")
            .cloned()
            .unwrap_or(Value::Null);
        status == 200
            && !known.is_null()
            && body
                .as_ref()
                .and_then(Value::as_object)
                .and_then(|b| b.get("id"))
                .is_some_and(|id| (id.is_i64() || id.is_u64()) && *id != known)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// 갱신 스레드의 패닉 줄에는 패닉 글이 없어요: 한글을 글자 중간에서 자른 패닉 글(슬라이스한 글을 담아요)도요.
    #[test]
    fn refresh_panic_line_has_no_payload() {
        static SEEN: Mutex<Option<(String, String)>> = Mutex::new(None);
        let name = "refresh-panic-line-test";
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().name() == Some(name) {
                let payload = info
                    .payload()
                    .downcast_ref::<String>()
                    .cloned()
                    .unwrap_or_default();
                *SEEN.lock().unwrap() = Some((refresh_panic_line(info.location()), payload));
            }
        }));
        let text = String::from("한글");
        let joined = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let n = std::hint::black_box(1);
                let _ = &text[..n]; // 글자 경계가 아니에요
            })
            .unwrap()
            .join();
        let _ = std::panic::take_hook();
        std::panic::set_hook(previous);
        assert!(joined.is_err());
        let (line, payload) = SEEN.lock().unwrap().clone().unwrap();
        assert!(payload.contains("'한'"), "{payload}"); // 기본 훅이라면 이 글(자른 글의 글자)이 찍혀요
        assert!(line.starts_with("refresh: panicked at "), "{line}");
        assert!(line.contains("refresh.rs:"), "{line}");
        assert!(
            !line.contains("한") && !line.contains("byte index"),
            "{line}"
        );
        assert_eq!(refresh_panic_line(None), "refresh: panicked");
    }
}
