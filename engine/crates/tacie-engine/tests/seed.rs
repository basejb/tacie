//! Starting without an index (plan 4 decision 6): an empty seed index is written only when there is nothing at
//! `--index-dir`, and the first repo is built with `/index/rebuild {repo, branch}`. If anything is already there
//! (even if broken), it is not overwritten.

#[allow(dead_code)] // 이 파일은 샌드박스의 일부만 써요
mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

use common::sandbox::{Sandbox, target};
use serde_json::Value;
use tacie_engine::refresh::{seed_index, seed_meta};
use tacie_index::{Index, Mask};

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

/// 폴더 아래 모든 것의 상대 경로 → (파일 바이트, 링크가 가리키는 곳)이에요. 링크는 따라가지 않아요.
fn tree(dir: &Path) -> BTreeMap<String, (Option<Vec<u8>>, Option<String>)> {
    fn walk(
        base: &Path,
        dir: &Path,
        out: &mut BTreeMap<String, (Option<Vec<u8>>, Option<String>)>,
    ) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            let m = fs::symlink_metadata(&p).unwrap();
            if m.is_symlink() {
                let to = fs::read_link(&p).unwrap().to_string_lossy().into_owned();
                out.insert(rel, (None, Some(to)));
            } else if m.is_dir() {
                out.insert(rel, (None, None));
                walk(base, &p, out);
            } else {
                out.insert(rel, (Some(fs::read(&p).unwrap()), None));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

#[test]
fn seed_writes_an_empty_index_that_opens() {
    let dir = tmp();
    let index = dir.path().join("index");
    assert!(seed_index(&index).unwrap());

    let ix = Index::open(&index, Mask::Plans, &HashSet::new()).unwrap();
    assert!(ix.chunks.is_empty() && ix.raw.is_empty() && ix.emb.is_empty());
    let npy = fs::read(index.join("emb.npy")).unwrap();
    let header = String::from_utf8_lossy(&npy[..npy.len().min(128)]).into_owned();
    assert!(header.contains("'shape': (0, 1024)"), "{header}");
    let meta: serde_json::Map<String, Value> =
        serde_json::from_str(&fs::read_to_string(index.join("meta.json")).unwrap()).unwrap();
    let keys: Vec<&str> = meta.keys().map(String::as_str).collect();
    assert_eq!(keys, ["repo", "commit", "built_at", "cleared_at"]);
    assert!(meta["repo"].is_null() && meta["commit"].is_null() && meta["built_at"].is_null());
    assert!(meta["cleared_at"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(ix.meta, meta);
    assert!(!dir.path().join("index.new").exists());
    assert_eq!(
        tree(dir.path()).into_keys().collect::<Vec<_>>(),
        [
            "index",
            "index/chunks.json",
            "index/emb.npy",
            "index/meta.json"
        ]
    );
}

#[test]
fn seed_makes_the_parent_and_replaces_a_half_written_new() {
    let dir = tmp();
    let index = dir.path().join("a/b/index");
    fs::create_dir_all(dir.path().join("a/b/index.new")).unwrap();
    fs::write(dir.path().join("a/b/index.new/chunks.json"), "[").unwrap();
    assert!(seed_index(&index).unwrap());
    assert!(Index::open(&index, Mask::Plans, &HashSet::new()).is_ok());
    assert!(!dir.path().join("a/b/index.new").exists());
}

#[test]
fn seed_never_replaces_an_existing_path() {
    type Make = fn(&Path);
    let cases: [(&str, Make); 4] = [
        ("empty folder", |p| fs::create_dir(p).unwrap()),
        ("broken chunks.json", |p| {
            fs::create_dir(p).unwrap();
            fs::write(p.join("chunks.json"), "{").unwrap();
        }),
        ("dangling symlink", |p| {
            std::os::unix::fs::symlink(p.with_file_name("nowhere"), p).unwrap()
        }),
        ("plain file", |p| fs::write(p, b"not an index").unwrap()),
    ];
    for (what, make) in cases {
        let dir = tmp();
        let index = dir.path().join("index");
        make(&index);
        let before = tree(dir.path());
        assert!(!seed_index(&index).unwrap(), "{what}");
        assert_eq!(tree(dir.path()), before, "{what}");
    }
}

#[test]
fn seed_fails_when_the_place_cannot_be_looked_at() {
    // 부모가 파일이면 NotFound가 아니라 NotADirectory예요: 무엇이 있는지 모르니 쓰지 않아요.
    let dir = tmp();
    fs::write(dir.path().join("file"), b"x").unwrap();
    assert!(seed_index(&dir.path().join("file/index")).is_err());
    assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"x");
}

#[test]
fn seed_reports_cleared_and_no_repo() {
    let s = Sandbox::new();
    s.set_meta(seed_meta());
    let (status, body) = s.call("GET", "/index", "");
    assert_eq!(status, 200, "{body}");
    assert!(body["repo"].is_null(), "{body}");
    assert!(body["branch"].is_null(), "{body}");
    assert_eq!(body["cleared"], true, "{body}");

    s.gh.clear_calls();
    let st = s.run(target(None, None, false));
    assert_eq!(st["state"], "failed", "{st}");
    assert_eq!(
        st["error"],
        "no repository yet: choose one with /index/rebuild {repo, branch}"
    );
    assert!(s.gh.calls().is_empty(), "{:?}", s.gh.calls());
}

#[test]
fn rebuild_from_a_seed_builds_the_chosen_repo() {
    let s = Sandbox::new();
    s.set_meta(seed_meta());
    let st = s.run(target(Some("o/new"), Some("main"), false));
    assert_eq!(st["state"], "done", "{st}");
    assert!(st["chunks"].as_u64().unwrap() > 0, "{st}");
    let meta = s.meta();
    assert_eq!(meta["repo"], "o/new");
    assert!(!meta.contains_key("cleared_at"), "{meta:?}");
    assert_eq!(meta["sources"]["repo"]["path"], "data/sources/repos/o__new");
    assert!(!s.root.join("data/sources/repos").exists()); // 클론 폴더는 만들지 않아요
    let on_disk: serde_json::Map<String, Value> =
        serde_json::from_str(&fs::read_to_string(s.index_dir().join("meta.json")).unwrap())
            .unwrap();
    assert!(!on_disk.contains_key("cleared_at"));
    assert!(!s.engine.index.load().raw.is_empty());
    assert!(s.texts().contains("o/new의 이슈"));
    let (_, body) = s.call("GET", "/index", "");
    assert_eq!(body["cleared"], false, "{body}");
    assert_eq!(body["repo"], "o/new", "{body}");
}
