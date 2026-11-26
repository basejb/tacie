//! Fetching repo content (spec section 5): the branch head commit (`head_commit`), that commit's tree (`tree`, in
//! place of `ls-files`/`ls-tree`), and file bytes (`download`: the blobs API 4 at a time, or for many files one
//! tarball pinned to that commit). It makes no clone and writes nothing to disk.
//!
//! Every fetched byte string has its git blob SHA (`blob_sha`) compared with the SHA in the tree. From the tarball
//! only matching files are used and the rest are filled in from the blobs API (`export-ignore`, `export-subst`, LFS).
//! If the blobs API returns different bytes, it is down.
//!
//! Tarball entry rules (spec 5-3): only regular files are read (links, hard links, folders and pax headers are
//! skipped). The first path segment (`{owner}-{repo}-{sha}/`) is stripped (its name is not checked: every file used is
//! verified by SHA, so a different name cannot bring in wrong bytes), and entries with no first segment, with an empty
//! segment, `.` or `..` in the rest of the path, or with an absolute path are skipped. Only paths in the wanted files
//! (`want`) are read into memory, and only when they do not exceed `MAX_FILE`.
//!
//! The tarball is a 303 to codeload, and that URL's query carries a temporary `token` (appendix A-3). So errors while
//! reading the body use fixed text, not the error's own text.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use flate2::read::GzDecoder;
use serde_json::Value;
use sha1::{Digest, Sha1};
use tar::{Archive, EntryType};

use super::GitHub;
use super::http::{Accept, TARBALL_TIMEOUT, kind_of, segment};
use crate::error::{Kind, SourceError};
use crate::repo;

/// blobs API를 한 번에 몇 개씩 부르는지예요(스펙 3절 12).
pub const BLOB_WORKERS: usize = 4;
/// 받을 파일이 이 수 이하면 tarball을 쓰지 않아요(4개씩 동시에 30 s 안쪽).
pub const TARBALL_MIN: usize = 40;
/// 원하는 파일의 이 분의 1 이하만 받으면 tarball을 쓰지 않아요(tarball이 받는 것의 4분의 3 이상을 버리면 blobs가
/// 나아요).
pub const TARBALL_SHARE: usize = 4;
/// tarball 항목 하나를 메모리에 읽는 한도예요(GitHub blob 한도 100 MB). 넘으면 읽지 않고 blobs API에 맡겨요.
pub const MAX_FILE: u64 = 100 * 1024 * 1024;

const NOT_MATCHING: &str = "GitHub sent a file that does not match its tree";
const CUT: &str = "GitHub could not be reached: the download was cut";
const UNREADABLE: &str = "GitHub sent a tarball that cannot be read";
const TOO_LARGE: &str = "GitHub's tree is too large";
/// 저장소는 보이는데 브랜치가 없을 때의 글이에요.
pub const BRANCH_GONE: &str = "the branch is gone";

/// 트리의 blob 항목 하나예요(`type == "blob"`만, 서브모듈은 `commit`이라 없어요).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub mode: String,
    pub sha: String,
}

/// 커밋 하나의 트리예요. `entries`는 경로 바이트 순서예요(`ls-files`, `ls-tree -r`와 같아요).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub commit: String,
    pub entries: Vec<Entry>,
}

impl Tree {
    /// 색인할 수 있는 파일이에요: 모드 100644·100755(`git ls-files -s` 대신), 경로 바이트 순서예요.
    pub fn files(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| e.mode == "100644" || e.mode == "100755")
            .map(|e| e.path.as_str())
            .collect()
    }

    /// 모든 blob의 {경로: SHA}예요(링크 포함, `git ls-tree -r` 대신).
    pub fn blobs(&self) -> BTreeMap<String, String> {
        self.entries
            .iter()
            .map(|e| (e.path.clone(), e.sha.clone()))
            .collect()
    }

    /// 심볼릭 링크(모드 120000)예요.
    pub fn links(&self) -> HashSet<String> {
        self.entries
            .iter()
            .filter(|e| e.mode == "120000")
            .map(|e| e.path.clone())
            .collect()
    }
}

/// 커밋 하나의 자를 것이에요: 트리, 받은 파일 바이트(캐시에 맞는 파일은 없어도 돼요), 트리의 기본 그래프
/// (`repo::GRAPH_PATH`) 바이트예요. `repo::chunks`가 체크아웃 대신 이것을 잘라요.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub tree: Tree,
    pub contents: HashMap<String, Vec<u8>>,
    pub graph: Option<Vec<u8>>,
}

/// `commit`의 `Snapshot`이에요: 트리를 받아 `snapshot_of`예요. GitHub에 그 커밋이 없으면 down이에요.
pub fn snapshot(
    gh: &GitHub,
    repo_name: &str,
    commit: &str,
    graph: Option<&Path>,
    skip: &[String],
    cache_path: Option<&Path>,
) -> Result<Snapshot, SourceError> {
    let tree = tree(gh, repo_name, commit)?.ok_or_else(|| {
        SourceError::service(Kind::Down, "GitHub does not have the commit to index")
    })?;
    snapshot_of(gh, repo_name, tree, graph, skip, cache_path)
}

/// 이미 받은 트리(`tree.commit`)의 `Snapshot`이에요: 기본 그래프 blob(`graph`가 없고 트리에 보통 파일로 있을 때),
/// 그리고 조각 캐시(`cache_path`)에 맞지 않는 자를 파일(`repo::missing`, 캐시가 없으면 모두)의 바이트(`download`)예요.
/// 그래프를 먼저 받는 것은 캐시 키의 이름표가 그 바이트로 정해지기 때문이에요. 기본 그래프 자리가 심볼릭 링크(120000)
/// 면 링크를 따라가지 않아서 그래프가 없는 것이에요(옛 체크아웃은 작업 트리에서 링크를 따라 읽었어요).
pub fn snapshot_of(
    gh: &GitHub,
    repo_name: &str,
    tree: Tree,
    graph: Option<&Path>,
    skip: &[String],
    cache_path: Option<&Path>,
) -> Result<Snapshot, SourceError> {
    let commit = tree.commit.clone();
    let commit = commit.as_str();
    let mut default = None;
    if graph.is_none()
        && let Some(e) = tree
            .entries
            .iter()
            .find(|e| e.path == repo::GRAPH_PATH && (e.mode == "100644" || e.mode == "100755"))
    {
        let want = [(e.path.clone(), e.sha.clone())];
        default = download(gh, repo_name, commit, &want, 1)?.remove(repo::GRAPH_PATH);
    }
    let (bytes, _) = repo::graph_bytes(graph, default.as_deref())?;
    let want = match cache_path {
        Some(cache) => repo::missing(&tree, &repo::graph_id(bytes.as_deref()), skip, cache)?,
        None => repo::wanted_files(&tree),
    };
    let contents = download(gh, repo_name, commit, &want, repo::wanted(&tree))?;
    Ok(Snapshot {
        tree,
        contents,
        graph: default,
    })
}

/// git blob SHA-1이에요: `sha1("blob {len}\0" + bytes)`(GitHub API의 `sha`와 같아요).
pub fn blob_sha(bytes: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", bytes.len()).as_bytes());
    h.update(bytes);
    hex::encode(h.finalize())
}

/// tarball 한 번으로 받을지예요: `missing > max(TARBALL_MIN, wanted / TARBALL_SHARE)`.
pub fn use_tarball(missing: usize, wanted: usize) -> bool {
    missing > TARBALL_MIN.max(wanted / TARBALL_SHARE)
}

pub(crate) fn is_sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn json_error(what: &str) -> SourceError {
    SourceError::Json(format!("GitHub's {what} is not in the expected shape"))
}

/// 2xx가 아니면 `kind_of`의 오류예요.
fn ok_or_kind(
    status: u16,
    headers: &HashMap<String, String>,
    body: Option<&Value>,
) -> Result<(), SourceError> {
    match kind_of(status, headers, body) {
        Some((kind, message)) => Err(SourceError::Service { kind, message }),
        None => Ok(()),
    }
}

/// 브랜치 머리의 커밋 SHA예요: `GET repos/{r}/git/ref/heads/{branch}`의 `object.sha`. 브랜치 이름은 `/` 사이를
/// 조각마다 인코딩해요. 그 이름의 브랜치가 꼭 있어야 해요: 404면 `repos/{r}`에 한 번 더 물어 저장소가 보이면 down
/// (`BRANCH_GONE`), 아니면 key예요. 40자 16진수가 아니면 down이에요. `timeout`이 없으면 요청 하나의 시간이에요.
pub fn head_commit(
    gh: &GitHub,
    repo: &str,
    branch: &str,
    timeout: Option<Duration>,
) -> Result<String, SourceError> {
    // 브랜치 이름은 `branch::valid_branch`를 지난 것이지만, 여기서도 경로를 벗어나는 조각은 막아요
    if branch
        .split('/')
        .any(|part| part.is_empty() || part.starts_with('.'))
    {
        return Err(SourceError::service(
            Kind::Key,
            "the branch name is not a valid git branch name",
        ));
    }
    let segments: Vec<String> = branch.split('/').map(segment).collect();
    let path = format!("repos/{repo}/git/ref/heads/{}", segments.join("/"));
    let (status, headers, body) = gh.status(&path, &[], timeout)?;
    if let Err(e) = ok_or_kind(status, &headers, body.as_ref()) {
        // 404는 브랜치가 없거나 저장소를 볼 수 없는 거예요. 저장소에 한 번 물어 보이면 브랜치를 지운 거라 down이에요
        // (클론 때 지운 브랜치의 fetch가 down이던 것과 같아요: 보통 갱신은 지난 코드로 가요). 저장소도 실패하면
        // 처음 오류(key)예요
        if status == 404
            && matches!(
                gh.status(&format!("repos/{repo}"), &[], timeout),
                Ok((200, _, _))
            )
        {
            return Err(SourceError::service(Kind::Down, BRANCH_GONE));
        }
        return Err(e);
    }
    let sha = body
        .as_ref()
        .and_then(|b| b.get("object")?.get("sha")?.as_str())
        .filter(|s| is_sha(s))
        .ok_or_else(|| SourceError::service(Kind::Down, "GitHub's branch answer has no commit"))?;
    Ok(sha.to_string())
}

/// 한 트리 요청의 (truncated, 항목들)이에요. 404·422(없는 커밋)면 None이에요.
fn tree_page(gh: &GitHub, path: &str) -> Result<Option<(bool, Vec<Value>)>, SourceError> {
    let (status, headers, body) = gh.status(path, &[], None)?;
    if status == 404 || status == 422 {
        return Ok(None);
    }
    ok_or_kind(status, &headers, body.as_ref())?;
    let body = body.ok_or_else(|| json_error("tree"))?;
    let truncated = body
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let Some(Value::Array(items)) = body.get("tree") else {
        return Err(json_error("tree"));
    };
    Ok(Some((truncated, items.clone())))
}

/// 트리 항목 하나의 (경로, 모드, 종류, SHA)예요.
fn item(v: &Value) -> Result<(&str, &str, &str, &str), SourceError> {
    let field = |k: &str| v.get(k).and_then(Value::as_str);
    match (field("path"), field("mode"), field("type"), field("sha")) {
        (Some(p), Some(m), Some(t), Some(s)) if !p.is_empty() && is_sha(s) => Ok((p, m, t, s)),
        _ => Err(json_error("tree")),
    }
}

/// 커밋의 모든 blob 항목이에요: `GET repos/{r}/git/trees/{commit}?recursive=1`. `truncated`면 `recursive` 없이
/// 맨 위부터 하위 트리를 하나씩 내려가요(요청 수 = 폴더 수(맨 위 포함) + 1). 커밋이 없으면(404·422) None이에요.
/// 항목은 경로 바이트로 정렬해요(문서가 순서를 보장하지 않아요).
pub fn tree(gh: &GitHub, repo: &str, commit: &str) -> Result<Option<Tree>, SourceError> {
    let Some((truncated, items)) =
        tree_page(gh, &format!("repos/{repo}/git/trees/{commit}?recursive=1"))?
    else {
        return Ok(None);
    };
    let mut entries = Vec::new();
    if !truncated {
        for v in &items {
            let (path, mode, kind, sha) = item(v)?;
            if kind == "blob" {
                entries.push(Entry {
                    path: path.to_string(),
                    mode: mode.to_string(),
                    sha: sha.to_string(),
                });
            }
        }
    } else {
        // (앞 경로, 트리 SHA 또는 커밋 SHA)
        let mut todo: Vec<(String, String)> = vec![(String::new(), commit.to_string())];
        while let Some((prefix, sha)) = todo.pop() {
            let Some((cut, items)) = tree_page(gh, &format!("repos/{repo}/git/trees/{sha}"))?
            else {
                if prefix.is_empty() {
                    return Ok(None);
                }
                return Err(SourceError::service(
                    Kind::Down,
                    "GitHub lost a folder of the tree",
                ));
            };
            if cut {
                // 한 폴더도 잘려 오면 더 나눌 수 없어요. 조용히 빼지 않아요
                return Err(SourceError::service(Kind::Down, TOO_LARGE));
            }
            for v in &items {
                let (name, mode, kind, sha) = item(v)?;
                let path = format!("{prefix}{name}");
                match kind {
                    "blob" => entries.push(Entry {
                        path,
                        mode: mode.to_string(),
                        sha: sha.to_string(),
                    }),
                    "tree" => todo.push((format!("{path}/"), sha.to_string())),
                    _ => {}
                }
            }
        }
    }
    entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    Ok(Some(Tree {
        commit: commit.to_string(),
        entries,
    }))
}

/// 파일 바이트예요: `want`는 (경로, blob SHA)이고, 돌려주는 것에는 `want`의 모든 경로가 있어요(아니면 Err).
/// `wanted`는 종류가 있는 파일의 수(tarball 기준, `use_tarball`)예요. tarball(`repos/{r}/tarball/{commit}`)에서
/// SHA가 맞는 것을 쓰고, 남은 것은 blobs API(`git/blobs/{sha}`, 같은 blob은 한 번)를 `BLOB_WORKERS`개씩 불러요.
pub fn download(
    gh: &GitHub,
    repo: &str,
    commit: &str,
    want: &[(String, String)],
    wanted: usize,
) -> Result<HashMap<String, Vec<u8>>, SourceError> {
    let mut got: HashMap<String, Vec<u8>> = HashMap::new();
    if want.is_empty() {
        eprintln!("repo: 0 files to download");
        return Ok(got);
    }
    let tarball = use_tarball(want.len(), wanted);
    if tarball {
        let map: HashMap<&str, &str> = want.iter().map(|(p, s)| (p.as_str(), s.as_str())).collect();
        got = from_tarball(gh, repo, commit, &map)?;
    }
    // 남은 것: blob SHA마다 한 번
    let mut shas: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for (path, sha) in want {
        if !got.contains_key(path) && seen.insert(sha) {
            shas.push(sha);
        }
    }
    // 받은 길을 한 줄로 남겨요(GitHub 호출 수: tarball 0 또는 1, blob은 SHA마다 한 번)
    eprintln!(
        "repo: {} files to download, tarball {}, {} from it, blobs {}",
        want.len(),
        u8::from(tarball),
        got.len(),
        shas.len()
    );
    let blobs = from_blobs(gh, repo, &shas)?;
    for (path, sha) in want {
        if !got.contains_key(path) {
            let bytes = blobs.get(sha.as_str()).ok_or_else(|| {
                SourceError::service(Kind::Down, "GitHub did not send every file")
            })?;
            got.insert(path.clone(), bytes.clone());
        }
    }
    Ok(got)
}

/// blob들을 `BLOB_WORKERS`개씩 받아요(`std::thread::scope`). 하나라도 실패하면 남은 것은 묻지 않고 그 오류예요.
fn from_blobs<'a>(
    gh: &GitHub,
    repo: &str,
    shas: &[&'a str],
) -> Result<HashMap<&'a str, Vec<u8>>, SourceError> {
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let out: Mutex<HashMap<&'a str, Vec<u8>>> = Mutex::new(HashMap::new());
    let error: Mutex<Option<SourceError>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..BLOB_WORKERS.min(shas.len()) {
            scope.spawn(|| {
                loop {
                    if failed.load(Ordering::SeqCst) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(&sha) = shas.get(i) else { return };
                    let got = gh
                        .bytes(
                            &format!("repos/{repo}/git/blobs/{sha}"),
                            Accept::Raw,
                            gh.timeouts().request,
                        )
                        .and_then(|bytes| {
                            if blob_sha(&bytes) == sha {
                                Ok(bytes)
                            } else {
                                Err(SourceError::service(Kind::Down, NOT_MATCHING))
                            }
                        });
                    match got {
                        Ok(bytes) => {
                            out.lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .insert(sha, bytes);
                        }
                        Err(e) => {
                            failed.store(true, Ordering::SeqCst);
                            error
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .get_or_insert(e);
                            return;
                        }
                    }
                }
            });
        }
    });
    if let Some(e) = error.into_inner().unwrap_or_else(PoisonError::into_inner) {
        return Err(e);
    }
    Ok(out.into_inner().unwrap_or_else(PoisonError::into_inner))
}

/// tarball 하나를 흘려 읽어 `want`의 경로 중 SHA가 맞는 것만 돌려줘요(모듈 설명의 항목 규칙).
fn from_tarball(
    gh: &GitHub,
    repo: &str,
    commit: &str,
    want: &HashMap<&str, &str>,
) -> Result<HashMap<String, Vec<u8>>, SourceError> {
    let body = gh.stream(
        &format!("repos/{repo}/tarball/{commit}"),
        Accept::Tarball,
        TARBALL_TIMEOUT,
    )?;
    let mut archive = Archive::new(GzDecoder::new(body));
    let mut got: HashMap<String, Vec<u8>> = HashMap::new();
    for entry in archive.entries().map_err(read_failed)? {
        let mut entry = entry.map_err(read_failed)?;
        if entry.header().entry_type() != EntryType::Regular {
            continue;
        }
        let raw = entry.path_bytes();
        let Some(path) = inside(&raw) else {
            continue;
        };
        let Some(&sha) = want.get(path) else {
            continue;
        };
        if got.contains_key(path) || entry.size() > MAX_FILE {
            continue;
        }
        let path = path.to_string();
        let mut bytes = Vec::new();
        (&mut entry)
            .take(MAX_FILE)
            .read_to_end(&mut bytes)
            .map_err(read_failed)?;
        if blob_sha(&bytes) == sha {
            got.insert(path, bytes);
        }
    }
    Ok(got)
}

/// tarball 항목의 경로에서 첫 조각을 뗀 저장소 안의 경로예요. 첫 조각이 없거나(맨 위의 파일, 절대 경로), 남은
/// 경로에 빈 조각·`.`·`..`이 있거나, UTF-8이 아니면 None이에요.
fn inside(raw: &[u8]) -> Option<&str> {
    let s = std::str::from_utf8(raw).ok()?;
    let (top, rest) = s.split_once('/')?;
    if top.is_empty() || top == "." || top == ".." || rest.is_empty() {
        return None;
    }
    if rest
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    Some(rest)
}

/// tarball을 읽다 난 오류예요. 안의 글(주소가 들어 있을 수 있어요)은 쓰지 않아요: 연결이 끊기거나 늦거나 몸통이
/// 일찍 끝났으면 network, 그 밖(gzip·tar가 아님)은 down이에요.
fn read_failed(e: io::Error) -> SourceError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = e.get_ref().map(|r| r as _);
    let mut from_network = false;
    while let Some(s) = source {
        if s.is::<reqwest::Error>() {
            from_network = true;
            break;
        }
        source = s.source();
    }
    if from_network
        || matches!(
            e.kind(),
            io::ErrorKind::TimedOut
                | io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::BrokenPipe
        )
    {
        SourceError::service(Kind::Network, CUT)
    } else {
        SourceError::service(Kind::Down, UNREADABLE)
    }
}

#[cfg(test)]
mod tests {
    use super::inside;

    #[test]
    fn inside_strips_the_top_folder_and_refuses_escapes() {
        assert_eq!(inside(b"o-n-abc/a/b.py"), Some("a/b.py"));
        assert_eq!(inside("o-n-abc/한글.md".as_bytes()), Some("한글.md"));
        for bad in [
            &b"a.py"[..],
            b"/abs",
            b"/o-n-abc/a.py",
            b"o-n-abc/",
            b"o-n-abc/../a.py",
            b"o-n-abc/a/../../b",
            b"o-n-abc/./a.py",
            b"o-n-abc//a.py",
            b"../a.py",
            b"./a.py",
            b"o-n-abc/a\xff.py",
        ] {
            assert_eq!(inside(bad), None, "{}", String::from_utf8_lossy(bad));
        }
    }
}
