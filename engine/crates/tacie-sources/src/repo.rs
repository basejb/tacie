//! Cuts one commit's files (`Snapshot`: tree, bytes, default graph): code by content-defined boundaries (`pieces`),
//! Markdown split by headings first and then cut at the same boundaries (docs/17 1절). Chunk text is built only with
//! the Python rules in `py`, so it matches the Python chunker used in research (eval/chunk_churn.py `cdc_line`) down
//! to the character (golden files come from eval/chunk_golden.py).
//! File order is the byte order of tree paths (`git ls-files`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use blake2::{Blake2b, digest::consts::U8};
use regex::Regex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tacie_index::RawChunk;

use crate::error::{Kind, SourceError};
use crate::github::GitHub;
use crate::github::content::{self, Snapshot, Tree};
use crate::py;
use crate::store::{self, Stored};

/// 내용 기준 경계예요: 줄(앞뒤 공백을 뗀 것)의 해시가 `CUT_EVERY`로 나누어떨어지면 그 줄 뒤에서 잘라요. 경계가 그
/// 줄 하나에만 달려 있어서, 한 줄을 고쳐도 그 근처 경계만 움직이고 나머지 조각은 글이 그대로예요(임베딩 캐시가
/// 맞아요). 조각은 `MIN_LINES`줄 이상이고, `MAX_LINES`줄이나 `CAP_CHARS`자(줄 끝 포함)에 닿으면 해시와 상관없이
/// 잘라요. 값은 잰 그대로예요(docs/17 1-2, 1-4).
pub const MIN_LINES: usize = 12;
pub const MAX_LINES: usize = 60;
pub const CAP_CHARS: usize = 1600;
pub const CUT_EVERY: u64 = 24;
pub const MAX_CHARS: usize = 2000;
/// 경계, 머리, 빼는 규칙, 이름표 규칙이 바뀌면 올려요. 그러면 옛 캐시 항목을 쓰지 않아요.
/// 1: 40줄 창을 30줄씩(2026-10-28까지). 2: 내용 기준 경계.
pub const CHUNKER_VERSION: u32 = 2;

const CODE_EXT: [&str; 11] = [
    ".ts", ".tsx", ".mjs", ".js", ".rs", ".swift", ".py", ".tf", ".yml", ".yaml", ".css",
];
const DOC_SKIP: [&str; 4] = [
    "docs/09_DB",
    "docs/api/",
    "docs/superpowers/plans/assets",
    "graft/",
];
const EXTRA_DIRS: [&str; 2] = ["infra/", "apps/desktop/src-tauri/"];
/// 보통 파일이에요. 심볼릭 링크(120000)와 서브모듈(160000)은 읽지 않아요.
const FILE_MODES: [&str; 2] = ["100644", "100755"];
/// `graph`를 주지 않으면 쓰는 트리 안의 Graft 배선 그래프예요.
pub const GRAPH_PATH: &str = "graft/.graph/wiring.json";

static SKIP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(__tests__|\.test\.|\.spec\.|/drizzle/|\.d\.ts$|^graft/|pnpm-lock\.yaml$)")
        .unwrap()
});
/// Python `re.match(r"^(#{1,4})\s+(.*)", line)`예요. Python `\s`는 `str.isspace()`라 `\x1c`–`\x1f`가 더 들어가요.
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#{1,4})[\s\x1c-\x1f]+(.*)").unwrap());

/// 파일 하나를 맡는 자르개예요. 한 파일은 많아야 하나에 들어가요.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Code,
    Extra,
    Doc,
    None,
}

impl FileKind {
    fn of(path: &str) -> FileKind {
        if is_code(path) {
            FileKind::Code
        } else if is_extra(path) {
            FileKind::Extra
        } else if is_doc(path) {
            FileKind::Doc
        } else {
            FileKind::None
        }
    }

    fn name(self) -> &'static str {
        match self {
            FileKind::Code => "code",
            FileKind::Extra => "extra",
            FileKind::Doc => "doc",
            FileKind::None => "none",
        }
    }

    fn from_name(name: &str) -> Option<FileKind> {
        [
            FileKind::Code,
            FileKind::Extra,
            FileKind::Doc,
            FileKind::None,
        ]
        .into_iter()
        .find(|k| k.name() == name)
    }
}

fn is_code(path: &str) -> bool {
    CODE_EXT.iter().any(|e| path.ends_with(e)) && !SKIP.is_match(path)
}

/// infra와 데스크톱 앱 설정 JSON에는 진짜 설정(메일 스팸 규칙, Tauri 창 등)이 들어 있어요.
fn is_extra(path: &str) -> bool {
    EXTRA_DIRS.iter().any(|d| path.starts_with(d)) && path.ends_with(".json")
}

fn is_doc(path: &str) -> bool {
    path.ends_with(".md") && !DOC_SKIP.iter().any(|d| path.starts_with(d)) && path != "CLAUDE.md"
}

/// Python `e.strerror or type(e).__name__`에 가까운 이유예요(`(os error N)`을 떼요).
fn reason(e: &std::io::Error) -> String {
    let text = e.to_string();
    match text.rfind(" (os error ") {
        Some(at) if text.ends_with(')') => text[..at].to_string(),
        _ => text,
    }
}

/// 실패해도 되는 캐시 쓰기예요(디스크가 꽉 찼거나 쓸 수 없는 폴더): 다음에 다시 자를 뿐이에요.
fn save(path: &Path, v: &Value) {
    if let Err(e) = store::write_json(path, v, None) {
        eprintln!(
            "cache: {} not written ({})",
            path.file_name().unwrap_or_default().to_string_lossy(),
            reason(&e)
        );
    }
}

/// 심볼 그래프를 만든 커밋의 blob 지도예요(링크 포함, 그 커밋 트리의 `{경로: blob sha}`). 다시 묻지 않게
/// graph_dir/blobs.json에 둬요. graph.json이 없거나(깨진 JSON 포함) 그 커밋을 GitHub이 모르면(트리 404·422, SHA가
/// 아닌 값) `Ok(None)`(과 stderr 한 줄)이에요: 이름표 없이 가요. graph.json·blobs.json을 읽지 못하거나(폴더, 권한)
/// GitHub에 닿지 못하면 Err예요: 갱신은 그것을 코드 받기 실패로 봐요. blobs.json을 쓰지 못하는 것은 실패가
/// 아니에요(다음에 다시 만들어요).
pub fn graph_blobs(
    gh: &GitHub,
    repo_name: &str,
    graph_dir: &Path,
) -> Result<Option<BTreeMap<String, String>>, SourceError> {
    let commit = match store::read_json(&graph_dir.join("graph.json"))? {
        Stored::Value(Value::Object(made)) => match made.get("commit") {
            Some(Value::String(c)) if !c.is_empty() => c.clone(),
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };
    if let Stored::Value(Value::Object(kept)) = store::read_json(&graph_dir.join("blobs.json"))?
        && kept.get("commit").and_then(Value::as_str) == Some(commit.as_str())
        && let Some(Value::Object(blobs)) = kept.get("blobs")
    {
        // 글이 아닌 값은 어떤 blob과도 같지 않아요(Python `gb.get(p) != cur[p]`): 빼도 같아요
        return Ok(Some(
            blobs
                .iter()
                .filter_map(|(p, b)| Some((p.clone(), b.as_str()?.to_string())))
                .collect(),
        ));
    }
    let tree = if content::is_sha(&commit) {
        content::tree(gh, repo_name, &commit)?
    } else {
        None // 주소에 넣을 수 없는 값이에요
    };
    let Some(tree) = tree else {
        eprintln!(
            "symbol graph: commit {} is not on GitHub, chunks get no symbol labels",
            py::take_chars(&commit, 8)
        );
        return Ok(None);
    };
    let blobs = tree.blobs();
    let mut kept = Map::new();
    kept.insert("commit".into(), Value::String(commit));
    kept.insert(
        "blobs".into(),
        Value::Object(
            blobs
                .iter()
                .map(|(p, b)| (p.clone(), Value::String(b.clone())))
                .collect(),
        ),
    );
    save(&graph_dir.join("blobs.json"), &Value::Object(kept));
    Ok(Some(blobs))
}

/// 경로 → `[(시작 줄, 이름)]`(그래프의 노드 순서)이에요. 창에 이름표를 붙여요.
type Starts = HashMap<String, Vec<(i64, String)>>;

fn bad_graph(graph: &str, what: &str) -> SourceError {
    SourceError::Json(format!("symbol graph {graph}: {what}"))
}

/// Python `int(s)`예요(앞뒤 공백, 부호, 숫자 사이 `_`).
fn py_int(s: &str) -> Option<i64> {
    let s = py::strip(s);
    let (neg, digits) = match s.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let n: i64 = digits.replace('_', "").parse().ok()?;
    Some(if neg { -n } else { n })
}

/// Graft 배선 그래프(`name`은 오류 글에 쓰는 이름)에서 파일마다 심볼 시작 줄이에요. `skip`의 파일은 그래프를 만든
/// 뒤 바뀌어서 그 범위가 옛 줄을 가리켜요: 이름표를 붙이지 않아요.
fn symbol_starts(bytes: &[u8], name: &str, skip: &[String]) -> Result<Starts, SourceError> {
    let graph = name;
    let text = std::str::from_utf8(bytes).map_err(|_| bad_graph(graph, "not UTF-8"))?;
    let wiring: Value = serde_json::from_str(text).map_err(|e| bad_graph(graph, &e.to_string()))?;
    let nodes = wiring
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| bad_graph(graph, "no nodes"))?;
    let skip: HashSet<&str> = skip.iter().map(String::as_str).collect();
    let mut starts = Starts::new();
    for n in nodes {
        let kind = n
            .get("kind")
            .ok_or_else(|| bad_graph(graph, "a node has no kind"))?;
        if kind == "file" {
            continue;
        }
        let span = n.get("span").unwrap_or(&Value::Null);
        if !py::truthy(span) {
            continue;
        }
        let path = n
            .get("path")
            .ok_or_else(|| bad_graph(graph, "a node has no path"))?;
        // 글이 아닌 경로는 어떤 파일과도 같지 않아요(Python도 이름표를 붙이지 못해요)
        let Some(path) = path.as_str() else { continue };
        if skip.contains(path) {
            continue;
        }
        let span = span
            .as_str()
            .ok_or_else(|| bad_graph(graph, "a span is not text"))?;
        // "L12-L30" → 12: Python `int(span.split("-")[0][1:])`
        let mut first = span.split('-').next().unwrap_or_default().chars();
        first.next();
        let a = py_int(first.as_str())
            .ok_or_else(|| bad_graph(graph, &format!("bad span {span:?}")))?;
        let name = n
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| bad_graph(graph, "a node has no name"))?;
        starts
            .entry(path.to_string())
            .or_default()
            .push((a, name.to_string()));
    }
    Ok(starts)
}

/// 파일 글이에요(Python `read_text(errors="ignore")`). 스냅숏에 바이트가 없으면 down이에요(경로만, 내용 없이).
fn read(snap: &Snapshot, path: &str) -> Result<String, SourceError> {
    snap.contents
        .get(path)
        .map(|bytes| py::decode_ignore(bytes))
        .ok_or_else(|| SourceError::service(Kind::Down, format!("the snapshot lacks {path}")))
}

fn chunk(src: &str, path: &str, line: usize, text: String) -> RawChunk {
    let mut c = Map::new();
    c.insert("src".into(), Value::from(src));
    c.insert("path".into(), Value::from(path));
    c.insert("line".into(), Value::from(line));
    c.insert("text".into(), Value::String(text));
    c
}

/// 줄 하나가 경계인지예요: Python `int.from_bytes(blake2b(line.strip(), digest_size=8), "big") % CUT_EVERY == 0`.
fn cuts_after(line: &str) -> bool {
    let digest = Blake2b::<U8>::digest(py::strip(line).as_bytes());
    u64::from_be_bytes(digest.into()) % CUT_EVERY == 0
}

/// 줄들을 조각 범위 `[a, b)`로 나눠요. 빈 줄들이면 `[(0, 0)]`이에요(Python `seg_cdc_line`).
fn pieces(lines: &[&str]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut start, mut size) = (0, 0);
    for (i, line) in lines.iter().enumerate() {
        let n = i - start + 1;
        size += line.chars().count() + 1; // Python len은 글자 수예요
        if (n >= MIN_LINES && cuts_after(line)) || n >= MAX_LINES || size >= CAP_CHARS {
            out.push((start, i + 1));
            (start, size) = (i + 1, 0);
        }
    }
    if start < lines.len() || out.is_empty() {
        out.push((start, lines.len()));
    }
    out
}

/// 범위 `[a, b)`의 글이에요(Python `"\n".join(lines[a:b]).strip()`).
fn body(lines: &[&str], (a, b): (usize, usize)) -> String {
    py::strip(&lines[a..b].join("\n")).to_string()
}

fn cut_text(head: &str, body: &str) -> String {
    py::take_chars(&format!("{head}\n{body}"), MAX_CHARS).to_string()
}

fn code_chunks(snap: &Snapshot, path: &str, starts: &Starts) -> Result<Vec<RawChunk>, SourceError> {
    let text = read(snap, path)?;
    let lines = py::splitlines(&text);
    let labels = starts.get(path).map(Vec::as_slice).unwrap_or_default();
    let mut out = Vec::new();
    for (a, b) in pieces(&lines) {
        let body = body(&lines, (a, b));
        if body.is_empty() {
            continue;
        }
        // 이름표는 시작 줄(1부터)이 이 조각의 줄 안에 있는 심볼이에요
        let (from, to) = (a as i64, b as i64);
        let names: Vec<&str> = labels
            .iter()
            .filter(|(s, _)| from < *s && *s <= to)
            .take(6)
            .map(|(_, name)| name.as_str())
            .collect();
        let mut head = format!("file: {path}");
        if !names.is_empty() {
            head.push_str("\nsymbols: ");
            head.push_str(&names.join(", "));
        }
        out.push(chunk("code", path, a + 1, cut_text(&head, &body)));
    }
    Ok(out)
}

fn extra_chunks(snap: &Snapshot, path: &str) -> Result<Vec<RawChunk>, SourceError> {
    let text = read(snap, path)?;
    let lines = py::splitlines(&text);
    let mut out = Vec::new();
    for (a, b) in pieces(&lines) {
        let body = body(&lines, (a, b));
        if !body.is_empty() {
            out.push(chunk(
                "code",
                path,
                a + 1,
                cut_text(&format!("file: {path}"), &body),
            ));
        }
    }
    Ok(out)
}

fn doc_chunks(snap: &Snapshot, path: &str) -> Result<Vec<RawChunk>, SourceError> {
    let text = read(snap, path)?;
    let lines = py::splitlines(&text);
    let mut out = Vec::new();
    let mut trail: Vec<String> = Vec::new();
    let mut sec: Vec<&str> = Vec::new();
    let mut sec_start = 1;

    let flush = |out: &mut Vec<RawChunk>, trail: &[String], sec: &[&str], sec_start: usize| {
        let head = format!("doc: {path}\nsection: {}", trail.join(" > "));
        for (a, b) in pieces(sec) {
            let body = body(sec, (a, b));
            if !body.is_empty() {
                out.push(chunk("docs", path, sec_start + a, cut_text(&head, &body)));
            }
        }
    };

    for (i, &line) in lines.iter().enumerate() {
        if let Some(m) = HEADING.captures(line) {
            flush(&mut out, &trail, &sec, sec_start);
            let level = m[1].len();
            trail.truncate(level - 1);
            trail.push(py::strip(&m[2]).to_string());
            sec = vec![line];
            sec_start = i + 1;
        } else {
            sec.push(line);
        }
    }
    flush(&mut out, &trail, &sec, sec_start);
    Ok(out)
}

fn cut(
    snap: &Snapshot,
    path: &str,
    kind: FileKind,
    starts: &Starts,
) -> Result<Vec<RawChunk>, SourceError> {
    match kind {
        FileKind::Code => code_chunks(snap, path, starts),
        FileKind::Extra => extra_chunks(snap, path),
        FileKind::Doc => doc_chunks(snap, path),
        FileKind::None => Ok(Vec::new()),
    }
}

/// 캐시 항목이 쓸 만한지예요(Python: kind가 넷 중 하나이고 chunks가 목록). 조각이 객체가 아니면 다시 잘라요.
fn valid_hit(hit: &Value) -> Option<(FileKind, Vec<RawChunk>)> {
    let kind = FileKind::from_name(hit.get("kind")?.as_str()?)?;
    let chunks = hit
        .get("chunks")?
        .as_array()?
        .iter()
        .map(|c| c.as_object().cloned())
        .collect::<Option<Vec<_>>>()?;
    Some((kind, chunks))
}

/// 조각 캐시의 키예요: `"{CHUNKER_VERSION}:{path}:{blob}:{label}"`(`label`은 `graph_id`나 `-`).
pub fn cache_key(path: &str, blob: &str, label: &str) -> String {
    format!("{CHUNKER_VERSION}:{path}:{blob}:{label}")
}

/// 그래프 바이트의 sha256 앞 16자예요. 그래프가 없으면 `-`예요.
pub fn graph_id(graph: Option<&[u8]>) -> String {
    match graph {
        Some(bytes) => hex::encode(Sha256::digest(bytes))[..16].to_string(),
        None => "-".to_string(),
    }
}

/// 자를 파일(종류가 None이 아닌 것)의 (경로, blob)이에요. 트리의 경로 바이트 순서예요.
/// 자르개가 읽는 경로예요(코드, 설정 JSON, 문서). 모드는 보지 않아요.
pub fn is_wanted(path: &str) -> bool {
    FileKind::of(path) != FileKind::None
}

pub(crate) fn wanted_files(tree: &Tree) -> Vec<(String, String)> {
    tree.entries
        .iter()
        .filter(|e| {
            FILE_MODES.contains(&e.mode.as_str()) && FileKind::of(&e.path) != FileKind::None
        })
        .map(|e| (e.path.clone(), e.sha.clone()))
        .collect()
}

/// 종류가 None이 아닌 파일 수예요(tarball 기준, `content::use_tarball`).
pub fn wanted(tree: &Tree) -> usize {
    wanted_files(tree).len()
}

/// 파일의 이름표 자리예요: 이름표는 코드 창에만, `skip`에 없는 파일에만 붙어요.
fn label<'a>(path: &str, graph_id: &'a str, skipped: &HashSet<&str>) -> &'a str {
    if FileKind::of(path) == FileKind::Code && !skipped.contains(path) {
        graph_id
    } else {
        "-"
    }
}

fn read_cache(cache_path: &Path) -> Result<Map<String, Value>, SourceError> {
    Ok(match store::read_json(cache_path)? {
        Stored::Value(Value::Object(m)) => m,
        _ => Map::new(),
    })
}

/// 캐시에 쓸 만한 항목이 없어서 바이트가 필요한 파일(종류가 None이 아닌 것)의 (경로, blob)이에요.
pub fn missing(
    tree: &Tree,
    graph_id: &str,
    skip: &[String],
    cache_path: &Path,
) -> Result<Vec<(String, String)>, SourceError> {
    let old = read_cache(cache_path)?;
    let skipped: HashSet<&str> = skip.iter().map(String::as_str).collect();
    Ok(wanted_files(tree)
        .into_iter()
        .filter(|(path, blob)| {
            let key = cache_key(path, blob, label(path, graph_id, &skipped));
            old.get(&key).and_then(valid_hit).is_none()
        })
        .collect())
}

/// 이름표에 쓸 그래프예요: `graph`(로컬 wiring.json)를 주면 그 파일(없으면 그래프 없음), 아니면 스냅숏의 기본
/// 그래프(`GRAPH_PATH`)예요. (바이트, 오류 글의 이름)이에요.
pub(crate) fn graph_bytes(
    graph: Option<&Path>,
    default: Option<&[u8]>,
) -> Result<(Option<Vec<u8>>, String), SourceError> {
    match graph {
        Some(path) => {
            let name = path.display().to_string();
            if path.exists() {
                Ok((Some(std::fs::read(path)?), name))
            } else {
                Ok((None, name))
            }
        }
        None => Ok((default.map(<[u8]>::to_vec), GRAPH_PATH.to_string())),
    }
}

/// 스냅숏의 조각이에요: 코드, 설정 JSON, 문서 순서이고, 파일은 트리의 경로 바이트 순서(`git ls-files`)예요.
/// `graph`는 Graft 배선 그래프(없으면 스냅숏의 `graft/.graph/wiring.json`), `skip`은 그래프를 만든 뒤 바뀐 파일이에요.
/// `cached`(조각 캐시 파일)가 있으면 blob, 자르개, 이름표가 바뀐 파일만 다시 잘라요: 캐시는 파일마다 `cache_key`
/// 아래 조각을 둬요. 캐시에 맞는 파일은 `contents`에 없어도 돼요. 심볼릭 링크(120000)와 서브모듈은 트리에 있어도
/// 자르지 않아요(`contents`에 있어도 읽지 않아요).
pub fn chunks(
    snap: &Snapshot,
    graph: Option<&Path>,
    skip: &[String],
    cached: Option<&Path>,
) -> Result<Vec<RawChunk>, SourceError> {
    let files = snap.tree.files();
    let (graph, graph_name) = graph_bytes(graph, snap.graph.as_deref())?;
    let starts = match &graph {
        Some(bytes) => symbol_starts(bytes, &graph_name, skip)?,
        None => {
            eprintln!("no symbol graph ({graph_name}): chunks get no symbol labels");
            Starts::new()
        }
    };
    let Some(cache_path) = cached else {
        let mut by_kind: [Vec<RawChunk>; 3] = Default::default();
        for (slot, kind) in [FileKind::Code, FileKind::Extra, FileKind::Doc]
            .into_iter()
            .enumerate()
        {
            for path in files.iter().filter(|p| FileKind::of(p) == kind) {
                by_kind[slot].extend(cut(snap, path, kind, &starts)?);
            }
        }
        return Ok(by_kind.concat());
    };

    let blobs: HashMap<&str, &str> = snap
        .tree
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.sha.as_str()))
        .collect();
    let graph_id = graph_id(graph.as_deref());
    let skipped: HashSet<&str> = skip.iter().map(String::as_str).collect();
    let old = read_cache(cache_path)?;
    let mut kept = Map::new();
    let (mut code, mut extra, mut doc) = (Vec::new(), Vec::new(), Vec::new());
    for &path in &files {
        let kind = FileKind::of(path);
        let key = match blobs.get(path) {
            Some(blob) if !blob.is_empty() => {
                Some(cache_key(path, blob, label(path, &graph_id, &skipped)))
            }
            _ => None,
        };
        let hit = key.as_ref().and_then(|k| old.get(k));
        let (hit_kind, hit_chunks, entry) = match hit.and_then(|h| Some((valid_hit(h)?, h))) {
            Some(((k, c), h)) => (k, c, h.clone()),
            None => {
                let c = cut(snap, path, kind, &starts)?;
                let mut entry = Map::new();
                entry.insert("kind".into(), Value::from(kind.name()));
                entry.insert(
                    "chunks".into(),
                    Value::Array(c.iter().cloned().map(Value::Object).collect()),
                );
                (kind, c, Value::Object(entry))
            }
        };
        if let Some(k) = key {
            kept.insert(k, entry);
        }
        match hit_kind {
            FileKind::Code => code.extend(hit_chunks),
            FileKind::Extra => extra.extend(hit_chunks),
            FileKind::Doc => doc.extend(hit_chunks),
            FileKind::None => {}
        }
    }
    if kept != old {
        save(cache_path, &Value::Object(kept));
    }
    Ok([code, extra, doc].concat())
}
