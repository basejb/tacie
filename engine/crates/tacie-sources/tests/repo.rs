//! Repo chunks and the chunk cache. Chunks are cut from one commit's
//! `Snapshot` (tree + bytes + default graph): tests build the snapshot directly or fetch it from a `FakeGitHub` repo
//! (`content::snapshot`). The golden files (`*_v2.json`, `repo_cdc_cases.json`) are chunks cut from the same recipe
//! by the Python chunker used in research (eval/chunk_golden.py, docs/17 1-5). That generator also rebuilds the v1
//! golden files (`repo_chunks.json`, `repo_chunk_cache.json`) down to the byte. The v1 golden files are fixed values
//! made by the old Python engine (2026-10-26).
//! `graph_blobs` asks GitHub (`FakeGitHub`) for the tree of the commit the graph was built from. git is never launched.

mod common {
    pub mod github;
}

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use base64::Engine as _;
use common::github::connect;
use serde_json::{Map, Value, json};
use tacie_index::{RawChunk, pyjson};
use tacie_sources::error::Kind;
use tacie_sources::github::content::{Entry, Snapshot, Tree, blob_sha, snapshot};
use tacie_sources::repo::{
    CHUNKER_VERSION, GRAPH_PATH, cache_key, chunks, graph_blobs, graph_id, missing, wanted,
};
use tacie_sources::store::{self, Stored};
use tacie_sources::testing::github::{FakeGitHub, FakeRepo, Reply};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
const SKIP: &str = "src/skipped.ts";

/// 파일(경로, 모드, 바이트)로 만든 스냅숏이에요: 트리는 경로 바이트 순서, 보통 파일은 모두 `contents`에 있어요.
fn snap_of(files: &[(&str, &str, &[u8])]) -> Snapshot {
    let mut entries: Vec<Entry> = files
        .iter()
        .map(|(p, m, b)| Entry {
            path: p.to_string(),
            mode: m.to_string(),
            sha: blob_sha(b),
        })
        .collect();
    entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    let contents = files
        .iter()
        .map(|(p, _, b)| (p.to_string(), b.to_vec()))
        .collect();
    let graph = files
        .iter()
        .find(|(p, _, _)| *p == GRAPH_PATH)
        .map(|(_, _, b)| b.to_vec());
    Snapshot {
        tree: Tree {
            commit: "c".repeat(40),
            entries,
        },
        contents,
        graph,
    }
}

/// 파일 셋이에요.
fn three() -> Vec<(&'static str, &'static str, Vec<u8>)> {
    vec![
        ("a.py", "100644", "A = 1\n".repeat(3).into_bytes()),
        ("b.py", "100644", "B = 1\n".repeat(3).into_bytes()),
        ("README.md", "100644", b"# T\n\nhello\n".to_vec()),
    ]
}

fn snap(files: &[(&str, &str, Vec<u8>)]) -> Snapshot {
    let list: Vec<(&str, &str, &[u8])> = files
        .iter()
        .map(|(p, m, b)| (*p, *m, b.as_slice()))
        .collect();
    snap_of(&list)
}

/// golden을 만든 레시피예요: 경로마다 바이트예요.
fn recipe() -> Vec<(String, Vec<u8>)> {
    let recipe: Map<String, Value> = serde_json::from_str(
        &fs::read_to_string(Path::new(FIXTURES).join("repo_recipe.json")).unwrap(),
    )
    .unwrap();
    recipe
        .into_iter()
        .map(|(path, data)| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data.as_str().unwrap())
                .unwrap();
            (path, bytes)
        })
        .collect()
}

fn plain(snap: &Snapshot, graph: Option<&Path>, skip: &[String]) -> Vec<RawChunk> {
    chunks(snap, graph, skip, None).unwrap()
}

fn cached(snap: &Snapshot, graph: Option<&Path>, skip: &[String], cache: &Path) -> Vec<RawChunk> {
    chunks(snap, graph, skip, Some(cache)).unwrap()
}

fn text_of<'a>(chunks: &'a [RawChunk], path: &str) -> &'a str {
    chunks
        .iter()
        .find(|c| c["path"] == path)
        .and_then(|c| c["text"].as_str())
        .unwrap_or_else(|| panic!("no chunk for {path}"))
}

fn all_text(chunks: &[RawChunk]) -> String {
    chunks.iter().map(|c| c["text"].as_str().unwrap()).collect()
}

/// 캐시 키의 경로들이에요(`"{version}:{path}:{blob}:{label}"`).
fn cache_paths(cache: &Path) -> Vec<String> {
    let Stored::Value(Value::Object(m)) = store::read_json(cache).unwrap() else {
        panic!("no cache at {}", cache.display());
    };
    let mut out: Vec<String> = m
        .keys()
        .map(|k| k.split(':').nth(1).unwrap().to_string())
        .collect();
    out.sort();
    out
}

fn assert_golden(name: &str, got: &[RawChunk], want: &[RawChunk], want_text: &str) {
    assert_eq!(got.len(), want.len(), "{name}: chunk count");
    for (g, w) in got.iter().zip(want) {
        assert_eq!(g, w, "{name}: {} line {}", w["path"], w["line"]);
    }
    // 키 순서와 글자까지: Python json.dumps(chunks, ensure_ascii=False, indent=1)와 같은 바이트예요
    assert_eq!(
        pyjson::objects_to_string(got, Some(1)),
        want_text,
        "{name}: bytes"
    );
}

#[test]
fn repo_chunks_match_python_golden() {
    let tmp = tempfile::tempdir().unwrap();
    let files = recipe();
    let list: Vec<(&str, &str, &[u8])> = files
        .iter()
        .map(|(p, b)| (p.as_str(), "100644", b.as_slice()))
        .collect();
    let snap = snap_of(&list);
    let graph = Path::new(FIXTURES).join("repo_graph.json");
    let skip = vec![SKIP.to_string()];
    let want_text = fs::read_to_string(Path::new(FIXTURES).join("repo_chunks_v2.json")).unwrap();
    let want: Vec<RawChunk> = serde_json::from_str(&want_text).unwrap();

    // 레시피에서 빠져야 하는 파일은 golden에도 없어요(골든이 바뀌어도 이 경우를 놓치지 않게요)
    let paths: BTreeSet<&str> = want.iter().map(|c| c["path"].as_str().unwrap()).collect();
    for left_out in [
        "src/__tests__/a.ts",
        "src/x.test.ts",
        "src/x.spec.js",
        "db/drizzle/0001.ts",
        "types/global.d.ts",
        "graft/tool.ts",
        "pnpm-lock.yaml",
        "apps/web/pnpm-lock.yaml",
        "package.json",
        "config/c.json",
        "apps/desktop/b.json",
        "infra.json",
        "CLAUDE.md",
        "docs/09_DB.md",
        "docs/09_DB_v2.md",
        "docs/api/y.md",
        "docs/superpowers/plans/assets/x.md",
        "X.MD",
        "graft/notes.md",
        "src/upper.TS",
        "src/empty.py",
        "src/blank.ts",
        "infra/empty.json",
        "docs/empty.md",
    ] {
        assert!(!paths.contains(left_out), "{left_out} is in the golden");
    }
    assert!(paths.contains("src/한글 폴더/모듈 이름.py") && paths.contains("infra/a.json"));

    assert_golden(
        "plain",
        &plain(&snap, Some(&graph), &skip),
        &want,
        &want_text,
    );

    // 캐시와 함께: 처음(모두 자름), 다음(모두 캐시에서)
    let cache = tmp.path().join("cache/chunks/r.json");
    assert_golden(
        "cold",
        &cached(&snap, Some(&graph), &skip, &cache),
        &want,
        &want_text,
    );
    // Python 자르개가 같은 저장소로 쓴 캐시와 같은 바이트예요(blob은 내용으로 정해져요)
    let python_cache =
        fs::read_to_string(Path::new(FIXTURES).join("repo_chunk_cache_v2.json")).unwrap();
    assert_eq!(
        fs::read_to_string(&cache).unwrap(),
        python_cache,
        "cache bytes"
    );
    assert_golden(
        "warm",
        &cached(&snap, Some(&graph), &skip, &cache),
        &want,
        &want_text,
    );

    // Python 자르개가 쓴 캐시를 그대로 이어 써요: 캐시에 맞는 파일은 바이트가 없어도 돼요(받지 않아요), 다시 쓰지 않아요
    let from_python = tmp.path().join("cache/chunks/python.json");
    fs::write(&from_python, &python_cache).unwrap();
    let digest = graph_id(Some(&fs::read(&graph).unwrap()));
    assert!(
        missing(&snap.tree, &digest, &skip, &from_python)
            .unwrap()
            .is_empty()
    );
    let bare = Snapshot {
        contents: Default::default(),
        ..snap.clone()
    };
    assert_golden(
        "python cache",
        &cached(&bare, Some(&graph), &skip, &from_python),
        &want,
        &want_text,
    );
    assert_eq!(fs::read_to_string(&from_python).unwrap(), python_cache);

    // GitHub(가짜)에 같은 레시피를 올려 받아도 같아요: 처음은 모두 받고, 캐시가 있으면 하나도 받지 않아요
    let fake = FakeGitHub::start();
    let repo = fake.repo("o/n", 7);
    let gh = connect(&fake);
    let commit = repo.push("main", &list);
    let got = snapshot(&gh, "o/n", &commit, Some(&graph), &skip, None).unwrap();
    assert_eq!(got.tree.files(), snap.tree.files());
    assert_eq!(got.contents.len(), wanted(&got.tree));
    assert_golden(
        "fetched",
        &plain(&got, Some(&graph), &skip),
        &want,
        &want_text,
    );
    fake.clear_calls();
    let warm = snapshot(&gh, "o/n", &commit, Some(&graph), &skip, Some(&from_python)).unwrap();
    assert!(warm.contents.is_empty());
    assert!(fake.calls().iter().all(|c| !c.path.contains("/git/blobs/")));
    assert_golden(
        "fetched warm",
        &cached(&warm, Some(&graph), &skip, &from_python),
        &want,
        &want_text,
    );
}

/// 경계 규칙마다 걸리는 파일 하나씩이에요: 해시, 최대 줄, 글자 수(한글은 3바이트라 바이트로 세면 틀려요), 해시
/// 전에 떼는 공백, 다른 줄바꿈, 머리, 빈 파일. 기대값은 Python 자르개가 낸 조각이에요.
#[test]
fn cdc_cases_match_python() {
    let cases: Vec<Value> = serde_json::from_str(
        &fs::read_to_string(Path::new(FIXTURES).join("repo_cdc_cases.json")).unwrap(),
    )
    .unwrap();
    for case in &cases {
        let path = case["path"].as_str().unwrap();
        let text = case["text"].as_str().unwrap();
        let got = plain(
            &snap(&[(path, "100644", text.as_bytes().to_vec())]),
            None,
            &[],
        );
        let got: Vec<Value> = got.into_iter().map(Value::Object).collect();
        assert_eq!(&Value::Array(got), &case["chunks"], "{path}");
    }
}

/// 한 줄을 넣어도 그 근처 조각만 글이 바뀌어요(나머지는 임베딩 캐시가 맞아요). 40줄 창이었다면 넣은 줄 아래
/// 창이 모두 바뀌었어요.
#[test]
fn an_inserted_line_changes_only_nearby_pieces() {
    let cases: Vec<Value> = serde_json::from_str(
        &fs::read_to_string(Path::new(FIXTURES).join("repo_cdc_cases.json")).unwrap(),
    )
    .unwrap();
    let text = |p: &str| {
        cases.iter().find(|c| c["path"] == p).unwrap()["text"]
            .as_str()
            .unwrap()
            .as_bytes()
            .to_vec()
    };
    let texts = |bytes: Vec<u8>| -> BTreeSet<String> {
        plain(&snap(&[("src/a.ts", "100644", bytes)]), None, &[])
            .iter()
            .map(|c| c["text"].as_str().unwrap().to_string())
            .collect()
    };
    let before = texts(text("src/many.ts"));
    let after = texts(text("src/many_plus_one.ts"));
    assert!(before.len() >= 5);
    assert!(
        after.difference(&before).count() <= 3,
        "{}",
        after.difference(&before).count()
    );
}

/// 옛 자르개(v1)의 캐시 항목은 쓰지 않고 다시 잘라요. 다시 쓴 캐시에는 v2 키만 남아요.
#[test]
fn v1_cache_entries_are_recut() {
    let tmp = tempfile::tempdir().unwrap();
    let files = recipe();
    let list: Vec<(&str, &str, &[u8])> = files
        .iter()
        .map(|(p, b)| (p.as_str(), "100644", b.as_slice()))
        .collect();
    let snap = snap_of(&list);
    let graph = Path::new(FIXTURES).join("repo_graph.json");
    let skip = vec![SKIP.to_string()];
    let cache = tmp.path().join("chunks.json");
    fs::copy(Path::new(FIXTURES).join("repo_chunk_cache.json"), &cache).unwrap();
    let digest = graph_id(Some(&fs::read(&graph).unwrap()));
    assert_eq!(
        missing(&snap.tree, &digest, &skip, &cache).unwrap().len(),
        wanted(&snap.tree)
    );
    let got = cached(&snap, Some(&graph), &skip, &cache);
    assert_eq!(got, plain(&snap, Some(&graph), &skip));
    let Stored::Value(Value::Object(m)) = store::read_json(&cache).unwrap() else {
        panic!("no cache");
    };
    assert!(m.keys().all(|k| k.starts_with("2:")));
}

#[test]
fn cache_keys_and_graph_ids() {
    assert_eq!(CHUNKER_VERSION, 2);
    assert_eq!(cache_key("a/b.py", "abc", "-"), "2:a/b.py:abc:-");
    assert_eq!(graph_id(None), "-");
    // sha256("") 앞 16자
    assert_eq!(graph_id(Some(b"")), "e3b0c44298fc1c14");
}

#[test]
fn unchanged_blobs_are_not_recut() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("cache/chunks/r.json");
    let mut files = three();
    let first = cached(&snap(&files), None, &[], &cache);
    assert_eq!(cache_paths(&cache), ["README.md", "a.py", "b.py"]);

    // b.py만 바뀐 커밋이에요: a.py는 blob이 같아서 바이트 없이(받지 않고) 캐시에서 와요
    files[1].2 = "B = 2\n".repeat(3).into_bytes();
    let mut next = snap(&files);
    next.contents.remove("a.py");
    next.contents.remove("README.md");
    let second = cached(&next, None, &[], &cache);
    assert_eq!(text_of(&second, "a.py"), text_of(&first, "a.py"));
    assert!(text_of(&second, "a.py").contains("A = 1"));
    assert!(text_of(&second, "b.py").contains("B = 2"));
    assert_ne!(second, first);
    assert_eq!(second, plain(&snap(&files), None, &[])); // 캐시 없이 자른 것과 같아요

    // 캐시에도 바이트에도 없으면 down이에요(경로만)
    let other = tmp.path().join("cache/chunks/other.json");
    let e = chunks(&next, None, &[], Some(&other)).unwrap_err();
    assert_eq!(e.kind(), Kind::Down);
    assert_eq!(e.detail(), "the snapshot lacks README.md");
}

#[test]
fn missing_lists_only_uncached_wanted_files() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("cache/chunks/r.json");
    let mut files = three();
    files.push(("notes.txt", "100644", b"not cut".to_vec())); // 종류 없음: 받지 않아요
    files.push(("link.py", "120000", b"a.py".to_vec())); // 링크: 받지 않아요
    files.push(("run.py", "100755", b"run()\n".to_vec()));
    let s = snap(&files);
    assert_eq!(wanted(&s.tree), 4);
    let all = |v: Vec<(String, String)>| v.into_iter().map(|(p, _)| p).collect::<Vec<_>>();
    // 캐시가 없으면 자를 파일 모두예요(트리 순서)
    assert_eq!(
        all(missing(&s.tree, "-", &[], &cache).unwrap()),
        ["README.md", "a.py", "b.py", "run.py"]
    );
    cached(&s, None, &[], &cache);
    assert!(missing(&s.tree, "-", &[], &cache).unwrap().is_empty());
    // blob이 바뀐 파일만이에요
    files[0].2 = b"A = 5\n".to_vec();
    let s2 = snap(&files);
    let m = missing(&s2.tree, "-", &[], &cache).unwrap();
    assert_eq!(m, [("a.py".to_string(), blob_sha(b"A = 5\n"))]);
    // 새 그래프면 코드 파일의 이름표가 바뀌어요(문서는 그대로), skip의 파일은 `-` 그대로예요
    assert_eq!(
        all(missing(&s.tree, "0123456789abcdef", &[], &cache).unwrap()),
        ["a.py", "b.py", "run.py"]
    );
    assert_eq!(
        all(missing(&s.tree, "0123456789abcdef", &["b.py".to_string()], &cache).unwrap()),
        ["a.py", "run.py"]
    );
}

#[test]
fn deleted_file_leaves_the_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("cache/chunks/r.json");
    let mut files = three();
    cached(&snap(&files), None, &[], &cache);
    assert_eq!(cache_paths(&cache).len(), 3);
    files.remove(1);
    let out = cached(&snap(&files), None, &[], &cache);
    assert_eq!(cache_paths(&cache), ["README.md", "a.py"]);
    let paths: BTreeSet<&str> = out.iter().map(|c| c["path"].as_str().unwrap()).collect();
    assert_eq!(paths, BTreeSet::from(["a.py", "README.md"]));
}

#[test]
fn new_graph_relabels_unchanged_blobs() {
    let tmp = tempfile::tempdir().unwrap();
    let s = snap(&three());
    let cache = tmp.path().join("cache/chunks/r.json");
    let graph = tmp.path().join("wiring.json");
    fs::write(&graph, json!({"nodes": []}).to_string()).unwrap();
    let before = cached(&s, Some(&graph), &[], &cache);
    assert!(!all_text(&before).contains("symbols:"));

    let node = json!({"kind": "function", "path": "a.py", "span": "L1-L5", "name": "make_a"});
    fs::write(&graph, json!({"nodes": [node]}).to_string()).unwrap();
    let after = cached(&s, Some(&graph), &[], &cache); // 같은 blob, 새 그래프
    assert!(text_of(&after, "a.py").contains("symbols: make_a"));
    assert_eq!(after, plain(&s, Some(&graph), &[]));

    // skip에 있는 파일은 이름표가 없고, 이름표 붙은 캐시 항목을 쓰지 않아요
    let skipped = cached(&s, Some(&graph), &["a.py".to_string()], &cache);
    assert!(!text_of(&skipped, "a.py").contains("symbols:"));

    // 로컬 그래프 파일이 없으면 이름표가 없어요(스냅숏의 그래프로 가지 않아요)
    let gone = tmp.path().join("none.json");
    assert!(!all_text(&cached(&s, Some(&gone), &[], &cache)).contains("symbols:"));
}

#[test]
fn default_graph_comes_from_the_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let node = json!({"kind": "function", "path": "a.py", "span": "L1-L5", "name": "make_a"});
    let wiring = json!({"nodes": [node]}).to_string();
    let mut files = three();
    files.push((GRAPH_PATH, "100644", wiring.clone().into_bytes()));
    let fake = FakeGitHub::start();
    let repo = fake.repo("o/n", 7);
    let gh = connect(&fake);
    let list: Vec<(&str, &str, &[u8])> = files
        .iter()
        .map(|(p, m, b)| (*p, *m, b.as_slice()))
        .collect();
    let commit = repo.push("main", &list);

    let cache = tmp.path().join("cache/chunks/r.json");
    let got = snapshot(&gh, "o/n", &commit, None, &[], Some(&cache)).unwrap();
    assert_eq!(got.graph.as_deref(), Some(wiring.as_bytes()));
    // 그래프는 조각이 아니에요(graft/): 그래프 자리 말고는 자를 파일만 받아요
    let mut got_paths: Vec<&str> = got.contents.keys().map(String::as_str).collect();
    got_paths.sort();
    assert_eq!(got_paths, ["README.md", "a.py", "b.py"]);
    let out = cached(&got, None, &[], &cache);
    assert!(text_of(&out, "a.py").contains("symbols: make_a"));
    assert_eq!(out, plain(&snap(&files), None, &[]));
    // 캐시 키의 이름표는 그 그래프 바이트예요
    let id = graph_id(Some(wiring.as_bytes()));
    let key = cache_key("a.py", &blob_sha(&files[0].2), &id);
    let Stored::Value(Value::Object(kept)) = store::read_json(&cache).unwrap() else {
        panic!("no cache");
    };
    assert!(kept.contains_key(&key), "{key}");

    // 다시 받으면 바뀐 것이 없어 파일은 받지 않아요(그래프 blob 하나만)
    fake.clear_calls();
    let again = snapshot(&gh, "o/n", &commit, None, &[], Some(&cache)).unwrap();
    assert!(again.contents.is_empty());
    let blobs = fake
        .calls()
        .iter()
        .filter(|c| c.path.contains("/git/blobs/"))
        .count();
    assert_eq!(blobs, 1);

    // 트리에 그래프가 없으면 이름표가 없어요
    let s = snap(&three());
    assert!(s.graph.is_none());
    assert!(!all_text(&plain(&s, None, &[])).contains("symbols:"));

    // 없는 커밋은 down이에요
    let e = snapshot(&gh, "o/n", &"0".repeat(40), None, &[], None).unwrap_err();
    assert_eq!(e.kind(), Kind::Down);
}

#[test]
fn symlinks_are_never_read() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("cache/chunks/r.json");
    let mut files = three();
    // 링크 blob은 대상 경로예요. 바이트가 `contents`에 있어도(잘못 넣었어도) 읽지 않아요
    files.push((
        "leak.md",
        "120000",
        b"# key\n\nSECRET-OUTSIDE-THE-CLONE\n".to_vec(),
    ));
    files.push(("link.py", "120000", b"a.py".to_vec()));
    files.push(("gone.md", "120000", b"nowhere.md".to_vec()));
    files.push(("sub", "160000", b"".to_vec()));
    let s = snap(&files);
    assert_eq!(
        s.tree
            .links()
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["gone.md", "leak.md", "link.py"])
    );
    let first = cached(&s, None, &[], &cache);
    for c in &first {
        assert!(!["leak.md", "link.py", "gone.md", "sub"].contains(&c["path"].as_str().unwrap()));
    }
    assert!(!all_text(&first).contains("SECRET-OUTSIDE-THE-CLONE"));
    assert_eq!(cache_paths(&cache), ["README.md", "a.py", "b.py"]);
    assert_eq!(first, plain(&s, None, &[]));
    assert!(!all_text(&plain(&s, None, &[])).contains("SECRET-OUTSIDE-THE-CLONE"));

    // 캐시를 못 쓰면 다음에 다시 자를 뿐이에요: 임시 파일 자리가 폴더라 쓰기가 실패해요
    let other = tmp.path().join("cache/chunks/other.json");
    fs::create_dir_all(tmp.path().join("cache/chunks/other.json+tmp/inside")).unwrap();
    assert_eq!(cached(&s, None, &[], &other), first);
    assert!(!other.exists());
}

// ----- graph_blobs: 그래프를 만든 커밋의 트리를 GitHub에 물어요 -----

/// 파일 셋을 올린 가짜 저장소 `o/n`이에요. 첫 커밋이에요.
fn pushed(repo: &FakeRepo, files: &[(&str, &str, Vec<u8>)]) -> String {
    let list: Vec<(&str, &str, &[u8])> = files
        .iter()
        .map(|(p, m, b)| (*p, *m, b.as_slice()))
        .collect();
    repo.push("main", &list)
}

/// 파일들의 `{경로: blob sha}`예요(링크도 blob이에요).
fn blob_map_of(files: &[(&str, &str, Vec<u8>)]) -> BTreeMap<String, String> {
    files
        .iter()
        .filter(|(_, m, _)| *m != "160000")
        .map(|(p, _, b)| (p.to_string(), blob_sha(b)))
        .collect()
}

#[test]
fn graph_blobs_keeps_the_graphs_blob_map() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start();
    let repo = fake.repo("o/n", 7);
    let gh = connect(&fake);
    let mut files = three();
    files.push(("link.md", "120000", b"a.py".to_vec())); // 링크도 지도에 있어요(skip 계산)
    files.push(("sub", "160000", Vec::new())); // 서브모듈은 blob이 아니에요
    let made = pushed(&repo, &files);
    let graph = tmp.path().join("graph");
    fs::create_dir_all(&graph).unwrap();
    assert_eq!(graph_blobs(&gh, "o/n", &graph).unwrap(), None); // graph.json이 없어요
    assert!(fake.calls().is_empty());

    fs::write(
        graph.join("graph.json"),
        json!({"commit": made}).to_string(),
    )
    .unwrap();
    let at_made = blob_map_of(&files);
    assert_eq!(at_made.len(), 4);
    assert_eq!(
        graph_blobs(&gh, "o/n", &graph).unwrap(),
        Some(at_made.clone())
    );
    assert_eq!(
        fake.calls()
            .iter()
            .map(|c| c.path.as_str())
            .collect::<Vec<_>>(),
        [format!("repos/o/n/git/trees/{made}")]
    );
    let Stored::Value(kept) = store::read_json(&graph.join("blobs.json")).unwrap() else {
        panic!("blobs.json was not written");
    };
    assert_eq!(kept, json!({"commit": made, "blobs": at_made}));

    // 그 커밋을 다시 묻지 않고 blobs.json으로 답해요(GitHub이 그 커밋을 잃어도요)
    fake.clear_calls();
    fs::write(
        graph.join("blobs.json"),
        json!({"commit": made, "blobs": {"a.py": "kept"}}).to_string(),
    )
    .unwrap();
    let only: BTreeMap<String, String> = [("a.py".to_string(), "kept".to_string())].into();
    assert_eq!(graph_blobs(&gh, "o/n", &graph).unwrap(), Some(only));
    assert!(fake.calls().is_empty());

    // GitHub이 모르는 커밋(422)이고 blobs.json도 다른 커밋 것이면 이름표를 붙이지 않아요. SHA가 아닌 값은 묻지도 않아요
    fs::write(
        graph.join("graph.json"),
        json!({"commit": "0".repeat(40)}).to_string(),
    )
    .unwrap();
    assert_eq!(graph_blobs(&gh, "o/n", &graph).unwrap(), None);
    fake.status(
        &format!("repos/o/n/git/trees/{}", "1".repeat(40)),
        404,
        &[],
        Some(json!({"message": "Not Found"})),
    );
    fs::write(
        graph.join("graph.json"),
        json!({"commit": "1".repeat(40)}).to_string(),
    )
    .unwrap();
    assert_eq!(graph_blobs(&gh, "o/n", &graph).unwrap(), None);
    fake.clear_calls();
    for bad in [
        json!({"commit": ""}),
        json!({"commit": 7}),
        json!([made]),
        json!({"commit": "../../user"}),
        json!({"commit": "main"}),
    ] {
        fs::write(graph.join("graph.json"), bad.to_string()).unwrap();
        assert_eq!(graph_blobs(&gh, "o/n", &graph).unwrap(), None, "{bad}");
    }
    fs::write(graph.join("graph.json"), "{").unwrap(); // 깨진 JSON은 그래프가 없는 것과 같아요
    assert_eq!(graph_blobs(&gh, "o/n", &graph).unwrap(), None);
    assert!(fake.calls().is_empty());

    // 잘린 트리도 끝까지 내려가요
    repo.truncate_trees(true);
    let other = tmp.path().join("graph-truncated");
    fs::create_dir_all(&other).unwrap();
    fs::write(
        other.join("graph.json"),
        json!({"commit": made}).to_string(),
    )
    .unwrap();
    assert_eq!(
        graph_blobs(&gh, "o/n", &other).unwrap(),
        Some(at_made.clone())
    );
    repo.truncate_trees(false);

    // blobs.json을 못 써도 실패가 아니에요(다음에 다시 만들어요): 임시 파일 자리가 폴더예요
    let other = tmp.path().join("graph2");
    fs::create_dir_all(other.join("blobs.json+tmp/inside")).unwrap();
    fs::write(
        other.join("graph.json"),
        json!({"commit": made}).to_string(),
    )
    .unwrap();
    assert_eq!(graph_blobs(&gh, "o/n", &other).unwrap(), Some(at_made));
    assert!(!other.join("blobs.json").exists());
}

#[test]
fn a_graph_that_cannot_be_read_fails() {
    // 읽을 수 없는 graph.json·blobs.json은 Err라 갱신이 실패해요(옛 색인이 계속 답해요). GitHub에
    // 닿지 못한 것도 Err예요(갱신은 그것을 코드 받기 실패로 봐요)
    let tmp = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start();
    let repo = fake.repo("o/n", 7);
    let gh = connect(&fake);
    let made = pushed(&repo, &three());
    let graph = tmp.path().join("graph");
    fs::create_dir_all(graph.join("graph.json/inside")).unwrap();
    assert!(graph_blobs(&gh, "o/n", &graph).is_err());

    fs::remove_dir_all(graph.join("graph.json")).unwrap();
    fs::write(
        graph.join("graph.json"),
        json!({"commit": made}).to_string(),
    )
    .unwrap();
    fs::create_dir_all(graph.join("blobs.json/inside")).unwrap();
    assert!(graph_blobs(&gh, "o/n", &graph).is_err());

    fs::remove_dir_all(graph.join("blobs.json")).unwrap();
    fake.route(&format!("repos/o/n/git/trees/{made}"), Reply::Close);
    let err = graph_blobs(&gh, "o/n", &graph).unwrap_err();
    assert_eq!(err.kind(), Kind::Network, "{err:?}");
    assert!(!graph.join("blobs.json").exists());
}

/// 트리에 있는 기본 그래프(`graft/.graph/wiring.json`)가 깨졌으면 그 경로를 말하는 Json 오류예요(폴더 경로가
/// 아니에요). 그 자리가 심볼릭 링크면 따라가지 않아서 그래프가 없는 것이에요.
#[test]
fn a_broken_default_graph_names_its_path() {
    let mut files = three();
    files.push((GRAPH_PATH, "100644", b"{not json".to_vec()));
    let err = chunks(&snap(&files), None, &[], None).unwrap_err();
    assert!(
        matches!(&err, tacie_sources::SourceError::Json(m) if m.contains(GRAPH_PATH)),
        "{err:?}"
    );
    assert_eq!(err.kind(), Kind::Down);

    let fake = FakeGitHub::start();
    let repo = fake.repo("o/n", 7);
    let gh = connect(&fake);
    let mut files = three();
    files.push(("graph.json", "100644", b"{\"nodes\": []}".to_vec()));
    files.push((GRAPH_PATH, "120000", b"../../graph.json".to_vec()));
    let commit = pushed(&repo, &files);
    let got = snapshot(&gh, "o/n", &commit, None, &[], None).unwrap();
    assert_eq!(got.graph, None);
    assert!(!got.contents.contains_key(GRAPH_PATH));
}
