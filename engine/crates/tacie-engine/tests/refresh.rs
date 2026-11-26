//! Index refresh (`Refresher`, `rebuild_target`).
//!
//! Failure cases are made by changing `FakeGitHub`'s routes: a cut-off fetch is one list set to `Close`, a new
//! issue is added to a list, and a vanished repo is a 404 on `repos/o/old`. Code fetch failures are `code_offline`
//! cutting the head, tree, blob and tarball, the route of one blob or the head, and a panic at the token spot.
//! Status records come from `with_status_log`. Refresh does not launch git or create clone folders.

mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;

use chrono::{DateTime, Local, TimeDelta};
use common::sandbox::{
    Sandbox, age, code_of, files, finish, first_files, first_issue, issue, listing, snapshot_of,
    start_meta, target,
};
use serde_json::{Map, Value, json};
use tacie_engine::refresh::{Places, Target, rebuild_target, repo_key, seed_meta, under_raw};
use tacie_index::{EmbCache, pyjson};
use tacie_sources::github::Timeouts;
use tacie_sources::github::content::blob_sha;
use tacie_sources::github::docs::chunks as github_chunks;
use tacie_sources::github::items::{load_state, save_state};
use tacie_sources::github::probe::PROBES;
use tacie_sources::repo;
use tacie_sources::testing::github::Reply;

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

fn none() -> Target {
    Target::default()
}

fn on(repo: &str, branch: &str) -> Target {
    target(Some(repo), Some(branch), false)
}

fn fresh() -> Target {
    target(None, None, true)
}

fn cache(s: &Sandbox) -> EmbCache {
    EmbCache::new(&s.root.join("data/cache/emb"), "BAAI/bge-m3", 512)
}

fn chunk_with(s: &Sandbox, needle: &str) -> String {
    s.engine
        .index
        .load()
        .raw
        .iter()
        .map(|c| c["text"].as_str().unwrap().to_string())
        .find(|t| t.contains(needle))
        .unwrap_or_else(|| panic!("no chunk with {needle:?}"))
}

fn has_github(s: &Sandbox) -> bool {
    s.engine
        .index
        .load()
        .raw
        .iter()
        .any(|c| c["src"] == "github")
}

/// GitHub이 o/old에 닿지 못해요: 저장소 확인이 오프라인이에요.
fn github_offline(s: &Sandbox) {
    s.github_offline();
}

fn github_back(s: &Sandbox) {
    s.github_back();
}

/// 코드 받기만 호스트에 닿지 못해요.
fn code_offline(s: &Sandbox) {
    s.code_offline();
}

/// 조각 캐시 파일(경로 → 바이트)이에요. 폴더가 없으면 비어요.
fn chunk_caches(s: &Sandbox) -> std::collections::BTreeMap<String, Vec<u8>> {
    let dir = s.root.join("data/cache/chunks");
    if dir.exists() {
        files(&dir)
    } else {
        Default::default()
    }
}

fn ino(p: &Path) -> u64 {
    fs::metadata(p).unwrap().ino()
}

#[test]
fn refresher_refuses_without_connection() {
    let s = Sandbox::new();
    s.github.set_credentials("none", None);
    let saved = listing(&s.raw());
    assert!(s.raw().join("state.json").exists());
    s.gh.clear_calls();
    let st = s.run(on("o/new", "main"));
    assert_eq!(st["state"], "failed");
    assert_eq!(st["error_kind"], "key");
    assert_eq!(st["error"], "GitHub is not connected");
    assert!(s.gh.calls().is_empty()); // GitHub에도 묻지 않고 gh 토큰도 빌리지 않아요
    assert_eq!(s.gh_token.calls(), 0);
    assert_eq!(listing(&s.raw()), saved);
    assert_eq!(s.meta(), start_meta());
}

#[test]
fn missing_graph_commit_does_not_stop_refresh() {
    let s = Sandbox::new();
    fs::write(
        s.root.join("data/sources/old-graph/graph.json"),
        json!({"commit": "0".repeat(40)}).to_string(),
    )
    .unwrap();
    s.push("o/old", "app.py", "NAME = 'changed'\n");
    s.sync(none());
    assert!(!s.texts().contains("symbols:"));
    // README.md와 app.py: 이름표가 하나도 없어요
    assert_eq!(s.meta()["sources"]["repo"]["symbols_missing"], 2);
}

#[test]
fn chunks_match_a_cold_run() {
    let s = Sandbox::new();
    for i in [1, 2] {
        if i == 2 {
            s.push("o/old", "extra.py", "X = 1\n");
        }
        s.push("o/old", "app.py", &format!("NAME = 'v{i}'\n"));
        s.sync(none());
        let graph = s.root.join("data/sources/old-graph");
        // 그래프를 만든 커밋(처음 파일 둘) 뒤에 바뀐 파일이에요
        let made = first_files("o/old");
        let now = s.files_of("o/old", "main");
        let skip: Vec<String> = now
            .iter()
            .filter(|(p, b)| made.get(*p) != Some(*b))
            .map(|(p, _)| p.clone())
            .collect();
        let snap = snapshot_of(s.meta()["commit"].as_str().unwrap(), &now);
        let cold = repo::chunks(&snap, Some(&graph.join("wiring.json")), &skip, None).unwrap();
        assert_eq!(code_of(&s.engine.index.load().raw), cold);
    }
    assert!(s.root.join("data/cache/chunks/o__old.json").exists());
}

#[test]
fn refresher_switches_repo_and_reports_progress() {
    let s = Sandbox::new();
    let st = s.run(on("o/new", "main"));
    assert_eq!(st["state"], "done", "{st}");
    let ix = s.engine.index.load();
    let (meta, n) = (&ix.meta, ix.raw.len());
    assert_eq!(meta["repo"], "o/new");
    assert_eq!(keys(&meta["sources"]), ["repo", "github"]); // 다른 저장소의 자료에는 슬랙이 없어요
    assert_eq!(meta["sources"]["repo"]["path"], "data/sources/repos/o__new");
    assert_eq!(meta["sources"]["repo"]["branch"], "main");
    assert!(meta["sources"]["repo"].get("graph").is_none());
    assert_eq!(meta["sources"]["github"]["repo"], "o/new");
    assert_eq!(
        meta["sources"]["github"]["raw"],
        "data/sources/github-repos/o__new"
    );
    // 클론 폴더는 만들지 않아요: 그 자리 이름만 meta와 repos.json에 남아요(스펙 9절)
    assert!(!s.root.join("data/sources/repos/o__new").exists());
    assert!(!s.root.join("data/sources/repos").exists());
    assert!(s.code_calls().contains(&"git/ref/heads/main".to_string()));
    assert!(s.texts().contains("o/new의 이슈") && !s.texts().contains("o/old"));

    let log = s.r.status_log();
    let progress: Vec<(Value, Value)> = log
        .iter()
        .filter(|st| st.get("progress").is_some())
        .map(|st| (st["step"].clone(), st["progress"].clone()))
        .collect();
    assert!(!progress.is_empty());
    assert_eq!(progress[0].0, "embed");
    assert_eq!(progress.last().unwrap().1, json!({"done": n, "total": n}));
    // 상태는 늘 새 값이에요(고치지 않아요): 서버가 그동안 앞의 것을 JSON으로 바꾸고 있을 수 있어요
    for pair in log.windows(2) {
        assert!(!Arc::ptr_eq(&pair[0], &pair[1]));
    }
    assert_eq!(keys(&log[0]), ["state", "step", "started_at"]);
    assert_eq!(log[0]["step"], "repo");
    let steps: Vec<&Value> = log.iter().filter_map(|st| st.get("step")).collect();
    let mut seen: Vec<&Value> = Vec::new();
    for step in steps {
        if seen.last() != Some(&step) {
            seen.push(step);
        }
    }
    assert_eq!(
        seen,
        [
            &json!("repo"),
            &json!("sources"),
            &json!("embed"),
            &json!("swap")
        ]
    );
    let with_progress = log.iter().find(|st| st.get("progress").is_some()).unwrap();
    assert_eq!(
        keys(with_progress),
        ["state", "step", "started_at", "progress"]
    );
    assert_eq!(
        keys(log.last().unwrap()),
        [
            "state",
            "finished_at",
            "chunks",
            "embedded",
            "commit",
            "warnings"
        ]
    );

    let known = fs::read_to_string(s.root.join("data/sources/repos.json")).unwrap();
    let want = json!({"o/old": {"path": "data/sources/old", "raw": "data/sources/github",
                                "graph": "data/sources/old-graph", "branch": "main"},
                      "o/new": {"path": "data/sources/repos/o__new", "raw": "data/sources/github-repos/o__new",
                                "branch": "main"}});
    assert_eq!(known, pyjson::to_string(&want, Some(1)));
    // o/old의 원본은 그 자리에 그대로예요
    assert_eq!(load_state(&s.raw())["repo"], "o/old");
}

#[test]
fn refresher_switches_back_to_a_known_repo() {
    let s = Sandbox::new();
    // 브랜치가 없으면 GitHub이 지금 말하는 저장소의 기본 브랜치예요(스펙 5-5)
    s.push_on("o/other", "trunk", "trunk.py", "TRUNK = 1\n");
    s.repo("o/other").default_branch("trunk");
    for t in [on("o/new", "main"), target(Some("o/other"), None, false)] {
        let repo = t.repo.clone().unwrap();
        s.sync(t);
        let texts = s.texts();
        assert!(texts.contains(&format!("{repo}의 이슈")) && !texts.contains("o/old"));
        assert_eq!(texts.contains("o/new"), repo == "o/new");
        let want = if repo == "o/new" { "main" } else { "trunk" };
        assert_eq!(s.meta()["sources"]["repo"]["branch"], want);
        assert_eq!(texts.contains("TRUNK = 1"), repo == "o/other");
    }
    let clones = |s: &Sandbox| s.root.join("data/sources/repos").exists();
    assert!(!clones(&s));
    s.sync(on("o/old", "main"));
    let meta = s.meta();
    assert_eq!(meta["repo"], "o/old");
    assert_eq!(meta["sources"]["github"]["raw"], "data/sources/github");
    assert_eq!(meta["sources"]["repo"]["path"], "data/sources/old");
    assert_eq!(meta["sources"]["repo"]["graph"], "data/sources/old-graph");
    assert!(!clones(&s));
    let texts = s.texts();
    assert!(
        texts.contains("o/old의 이슈") && !texts.contains("o/new") && !texts.contains("o/other")
    );
    let known: Map<String, Value> =
        serde_json::from_str(&fs::read_to_string(s.root.join("data/sources/repos.json")).unwrap())
            .unwrap();
    assert_eq!(
        known.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from(["o/old", "o/new", "o/other"])
    );
    // 같은 저장소의 다른 브랜치: 같은 자리 이름이에요
    s.push_on("o/old", "feat", "feat.py", "FEAT = 'only on feat'\n");
    s.sync(target(None, Some("feat"), false));
    let meta = s.meta();
    assert_eq!(meta["repo"], "o/old");
    assert_eq!(meta["sources"]["repo"]["branch"], "feat");
    assert!(s.texts().contains("only on feat"));
    assert_eq!(meta["sources"]["repo"]["path"], "data/sources/old");
    assert!(!clones(&s));
}

#[test]
fn refresher_repo_names_ignore_case() {
    let s = Sandbox::new();
    let clones = |s: &Sandbox| s.root.join("data/sources/repos").exists();
    s.sync(on("O/OLD", "main")); // 색인의 o/old 그대로예요
    let meta = s.meta();
    assert_eq!(meta["repo"], "o/old");
    assert_eq!(meta["sources"]["repo"]["path"], "data/sources/old");
    assert_eq!(meta["sources"]["repo"]["graph"], "data/sources/old-graph");
    assert!(!clones(&s));
    assert!(!s.root.join("data/sources/repos.json").exists()); // 바꾼 게 아니에요
    // 옛 파일이 다른 대소문자로 둔 repos.json 키도 그 자리를 찾아요
    s.sync(on("o/new", "main"));
    s.sync(on("o/old", "main"));
    let f = s.root.join("data/sources/repos.json");
    let known: Map<String, Value> = serde_json::from_str(&fs::read_to_string(&f).unwrap()).unwrap();
    let renamed: Map<String, Value> = known
        .into_iter()
        .map(|(k, v)| (if k == "o/new" { "O/NEW".to_string() } else { k }, v))
        .collect();
    fs::write(&f, Value::Object(renamed).to_string()).unwrap();
    // gh는 물은 이름 그대로 물어요
    s.lists("O/New", vec![first_issue("o/new")]);
    s.gh.clear_calls();
    s.sync(on("O/New", "main"));
    let meta = s.meta();
    assert_eq!(meta["repo"], "O/New");
    assert_eq!(meta["sources"]["repo"]["path"], "data/sources/repos/o__new");
    assert!(!clones(&s));
    // 코드는 물은 이름 그대로 물어요(GitHub은 대소문자를 가리지 않아요)
    assert!(
        s.calls()
            .iter()
            .any(|c| c.path == "repos/O/New/git/ref/heads/main")
    );
    let known: Map<String, Value> = serde_json::from_str(&fs::read_to_string(&f).unwrap()).unwrap();
    assert_eq!(
        known.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from(["o/old", "o/new"])
    );
}

#[test]
fn refresher_remembers_branch_and_slack_per_repo() {
    let s = Sandbox::new();
    s.push_on("o/old", "dev", "dev.py", "DEV = 'only on dev'\n");
    // o/old의 자료에는 슬랙이 있었어요
    let mut meta = start_meta();
    meta["sources"]
        .as_object_mut()
        .unwrap()
        .insert("slack".into(), json!({}));
    s.set_meta(meta);
    s.sync(target(None, Some("dev"), false));
    assert!(s.texts().contains("only on dev") && s.texts().contains("슬랙에서 받은 글"));
    s.sync(target(Some("o/new"), None, false));
    let meta = s.meta();
    assert_eq!(meta["sources"]["repo"]["branch"], "main");
    assert!(meta["sources"].get("slack").is_none());
    let known: Value =
        serde_json::from_str(&fs::read_to_string(s.root.join("data/sources/repos.json")).unwrap())
            .unwrap();
    assert_eq!(known["o/old"]["branch"], "dev");
    assert_eq!(known["o/old"]["slack"], true);
    assert!(known["o/new"].get("slack").is_none());
    s.sync(target(Some("o/old"), None, false)); // 브랜치가 없으면 마지막에 받은 것이에요
    let meta = s.meta();
    assert_eq!(meta["sources"]["repo"]["branch"], "dev");
    assert!(s.texts().contains("only on dev"));
    assert!(meta["sources"].get("slack").is_some() && s.texts().contains("슬랙에서 받은 글"));
}

#[test]
fn refresher_fresh_rebuild_keeps_old_raw_on_failure() {
    let s = Sandbox::new();
    let saved = files(&s.raw());
    // 목록 하나를 받다가 연결이 끊긴 받기예요
    s.gh.route("repos/o/old/issues?state=all", Reply::Close);
    let st = s.run(fresh());
    assert_eq!(st["state"], "failed", "{st}");
    assert_eq!(st["error_kind"], "network");
    assert_eq!(files(&s.raw()), saved);
    assert_eq!(s.meta(), start_meta());
    let names: Vec<String> = listing(&s.root.join("data/sources"))
        .unwrap()
        .into_keys()
        .collect();
    assert_eq!(names, ["github", "old", "old-graph"]);
    s.lists("o/old", vec![first_issue("o/old")]);
    let st = s.run(fresh());
    assert_eq!(st["state"], "done", "{st}");
    let n = s.engine.index.load().raw.len();
    assert!(n > 0);
    assert_eq!(st["embedded"], n);
    assert_eq!(st["chunks"], n);
    assert_eq!(s.meta()["repo"], "o/old");
    assert!(s.raw().join("state.json").exists());
}

#[test]
fn a_failed_fresh_keeps_the_chunk_cache() {
    let s = Sandbox::new();
    s.sync(none());
    let caches = chunk_caches(&s);
    assert!(!caches.is_empty());
    // 코드는 다 왔고 GitHub 원본을 다시 받다가 끊겼어요
    s.gh.route("repos/o/old/issues?state=all", Reply::Close);
    let st = s.run(fresh());
    assert_eq!(st["state"], "failed", "{st}");
    assert_eq!(chunk_caches(&s), caches);
}

#[test]
fn cache_survives_switch_and_fresh() {
    let s = Sandbox::new();
    s.push("o/old", "shared.py", "SHARED = 1\n");
    // shared.py만 새것이에요: 나머지는 캐시 없이 만든 색인에서 와요
    assert_eq!(s.sync(none()), 1);
    let shared = chunk_with(&s, "SHARED = 1");
    s.push("o/new", "shared.py", "SHARED = 1\n");
    let embedded = s.sync(on("o/new", "main"));
    let n_new = s.engine.index.load().raw.len() as u64;
    assert!(0 < embedded && embedded < n_new, "{embedded} of {n_new}"); // 같은 글은 다시 임베딩하지 않았어요
    assert!(!cache(&s).get(&[&shared]).unwrap().is_empty());
    assert_eq!(s.sync(on("o/old", "main")), 0);
    assert_eq!(s.sync(fresh()), 0);
}

#[test]
fn cache_forgets_replaced_texts() {
    let s = Sandbox::new();
    s.sync(none());
    let old_text = chunk_with(&s, "NAME = 'o/old'");
    let c = cache(&s);
    assert!(!c.get(&[&old_text]).unwrap().is_empty());
    s.push("o/old", "app.py", "NAME = 'renamed'\n");
    assert_eq!(s.sync(none()), 1);
    let new_text = chunk_with(&s, "NAME = 'renamed'");
    assert!(c.get(&[&old_text]).unwrap().is_empty());
    assert!(!c.get(&[&new_text]).unwrap().is_empty());
}

fn tree_times(dir: &Path) -> Vec<(PathBuf, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            out.push((p.clone(), fs::metadata(&p).unwrap().modified().unwrap()));
            if p.is_dir() {
                todo.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn unchanged_refresh_keeps_the_index_and_moves_built_at() {
    let s = Sandbox::new();
    s.sync(none());
    let ix = s.index_dir();
    // 하루 전에 만든 것처럼요: 나이 규칙대로라면 지금 갱신해요
    let day_ago = (Local::now() - TimeDelta::hours(21))
        .format("%Y-%m-%dT%H:%M:%S%z")
        .to_string();
    let mut meta = s.meta();
    meta.insert("built_at".into(), json!(day_ago));
    s.set_meta(meta.clone());
    let live = s.engine.index.load_full();
    for n in ["chunks.json", "emb.npy"] {
        age(&ix.join(n));
    }
    let kept: Vec<_> = ["chunks.json", "emb.npy"]
        .iter()
        .map(|n| fs::metadata(ix.join(n)).unwrap().modified().unwrap())
        .collect();
    let (folder, old) = (ino(&ix), listing(&s.root.join("index.old")));
    let cached = tree_times(&s.root.join("data/cache/emb"));
    assert!(!cached.is_empty());
    let calls = s.embed.calls();
    s.gh.clear_calls();

    let st = s.run(none());
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["unchanged"], true);
    assert_eq!(st["embedded"], 0);
    assert_eq!(st["warnings"], json!([]));
    // 바뀐 것이 없으면 아무것도 받지 않아요: 머리와 트리(그래프가 있으면 그 blob)만 물어요
    let code = s.code_calls();
    assert!(
        code.iter().any(|c| c.starts_with("git/ref/heads/")),
        "{code:?}"
    );
    assert!(code.iter().any(|c| c.starts_with("git/trees/")), "{code:?}");
    assert!(
        !code
            .iter()
            .any(|c| c.starts_with("git/blobs/") || c.starts_with("tarball/")),
        "{code:?}"
    );
    assert!(!live.raw.is_empty());
    assert_eq!(st["chunks"], live.raw.len());
    assert_eq!(st["commit"], meta["commit"].as_str().unwrap()[..8]);
    assert_eq!(
        keys(&st),
        [
            "state",
            "finished_at",
            "chunks",
            "embedded",
            "commit",
            "warnings",
            "unchanged"
        ]
    );
    let now = s.engine.index.load_full();
    assert!(Arc::ptr_eq(&now.raw, &live.raw) && Arc::ptr_eq(&now.chunks, &live.chunks));
    assert!(Arc::ptr_eq(&now.emb, &live.emb));
    assert_eq!(ino(&ix), folder);
    assert!(!s.root.join("index.new").exists());
    let after: Vec<_> = ["chunks.json", "emb.npy"]
        .iter()
        .map(|n| fs::metadata(ix.join(n)).unwrap().modified().unwrap())
        .collect();
    assert_eq!(after, kept);
    assert_eq!(listing(&s.root.join("index.old")), old);
    assert_eq!(tree_times(&s.root.join("data/cache/emb")), cached);
    assert_eq!(s.embed.calls(), calls);
    let on_disk: Map<String, Value> =
        serde_json::from_str(&fs::read_to_string(ix.join("meta.json")).unwrap()).unwrap();
    assert_eq!(on_disk, now.meta);
    let (mut a, mut b) = (on_disk.clone(), meta.clone());
    a.insert("built_at".into(), Value::Null);
    b.insert("built_at".into(), Value::Null);
    // GitHub info의 fetched_at은 이번 받기의 시각이에요
    a["sources"]["github"]["fetched_at"] = Value::Null;
    b["sources"]["github"]["fetched_at"] = Value::Null;
    assert_eq!(a, b);
    let built =
        DateTime::parse_from_str(on_disk["built_at"].as_str().unwrap(), "%Y-%m-%dT%H:%M:%S%z")
            .unwrap();
    assert!(built > DateTime::parse_from_str(&day_ago, "%Y-%m-%dT%H:%M:%S%z").unwrap());
    assert!(Local::now().fixed_offset() - built < TimeDelta::hours(20)); // 나이 규칙이 다시 기다려요
}

#[test]
fn unchanged_with_warnings_keeps_meta() {
    let s = Sandbox::new();
    s.sync(none());
    let ix = s.index_dir();
    for e in fs::read_dir(&ix).unwrap() {
        age(&e.unwrap().path());
    }
    let live = s.engine.index.load_full();
    let files_before = listing(&ix).unwrap();
    github_offline(&s); // GitHub에 한 번 닿지 못했어요: 저장본을 써요
    let st = s.run(none());
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["unchanged"], true);
    assert_eq!(st["warnings"], json!(["github"]));
    assert_eq!(st["embedded"], 0);
    // 모든 출처가 오지 않았으니 보여 주는 시각도 그대로예요: meta.json도 쓰지 않아요
    assert_eq!(listing(&ix).unwrap(), files_before);
    assert!(Arc::ptr_eq(&s.engine.index.load_full(), &live));
    github_back(&s);
    let st = s.run(none());
    assert_eq!(st["unchanged"], true);
    assert_eq!(st["warnings"], json!([]));
    assert!(Arc::ptr_eq(&s.engine.index.load().raw, &live.raw));
    let mut now = listing(&ix).unwrap();
    assert_ne!(now["meta.json"], files_before["meta.json"]);
    let mut was = files_before.clone();
    now.remove("meta.json");
    was.remove("meta.json");
    assert_eq!(now, was);
}

#[test]
fn code_fetch_failure_keeps_code_and_goes_on() {
    let s = Sandbox::new();
    s.sync(none());
    let live = s.engine.index.load_full();
    let old_code = code_of(&live.raw);
    assert!(!old_code.is_empty());
    // GitHub에는 새 이슈가 있어요
    s.lists(
        "o/old",
        vec![
            first_issue("o/old"),
            issue(
                "o/old",
                2,
                "o/old의 두 번째 이슈",
                "새로 연 이슈예요.",
                "2026-10-24T01:00:00Z",
            ),
        ],
    );
    s.gh.route("repos/o/old/issues/2/comments", Reply::Json(json!([])));
    code_offline(&s);
    let st = s.run(none());
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["warnings"], json!(["repo"]));
    assert_eq!(code_of(&s.engine.index.load().raw), old_code);
    assert!(s.texts().contains("이슈 #2"));
    let commit = live.meta["commit"].as_str().unwrap();
    assert_eq!(s.meta()["commit"], commit);
    assert_eq!(st["commit"], commit[..8]);
}

#[test]
fn a_gh_token_failure_keeps_the_indexed_code() {
    let s = Sandbox::new();
    // 슬랙이 있는 자료예요: GitHub이 모두 실패해도 슬랙은 와요
    let mut meta = start_meta();
    meta["sources"]
        .as_object_mut()
        .unwrap()
        .insert("slack".into(), json!({}));
    s.set_meta(meta);
    s.sync(none());
    let live = s.engine.index.load_full();
    let old_code = code_of(&live.raw);
    assert!(!old_code.is_empty());
    s.gh_token_fails(true);
    let st = s.run(none());
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["warnings"], json!(["repo", "github"]));
    assert_eq!(code_of(&s.engine.index.load().raw), old_code);
    assert_eq!(s.meta()["commit"], live.meta["commit"]);
    // 바꾸기는 받기 실패를 넘기지 않아요
    let st = s.run(on("o/new", "main"));
    assert_eq!(st["state"], "failed", "{st}");
    assert_eq!(st["error_kind"], "key");
    assert_eq!(st["error"], "gh is not logged in");
}

#[test]
fn everything_failing_still_fails() {
    let s = Sandbox::new();
    s.sync(none());
    let (ix, live) = (s.index_dir(), s.engine.index.load_full());
    let before: Vec<Vec<u8>> = ["chunks.json", "meta.json"]
        .iter()
        .map(|n| fs::read(ix.join(n)).unwrap())
        .collect();
    let listed = listing(&ix);
    github_offline(&s); // GitHub도 닿지 않고, 저장본은 있어요
    code_offline(&s);
    let st = s.run(none());
    assert_eq!(st["state"], "failed", "{st}");
    assert_eq!(st["error_kind"], "network");
    // 코드 받기의 오류예요: 고정 글이고 주소나 저장소 이름이 없어요(스펙 4-4)
    let error = st["error"].as_str().unwrap();
    assert!(error.starts_with("GitHub could not be reached"), "{error}");
    assert!(
        !error.contains("o/old") && !error.contains("127.0.0.1"),
        "{error}"
    );
    let after: Vec<Vec<u8>> = ["chunks.json", "meta.json"]
        .iter()
        .map(|n| fs::read(ix.join(n)).unwrap())
        .collect();
    assert_eq!(after, before);
    assert_eq!(listing(&ix), listed);
    assert!(Arc::ptr_eq(&s.engine.index.load_full(), &live));
    assert!(!s.root.join("index.new").exists());
}

/// GitHub이 o/old에 `status`로 답하고, 코드 받기는 실패해요.
fn repo_gone(s: &Sandbox, status: u16, body: Value, fail_fetch: bool) {
    s.gh.status("repos/o/old", status, &[], Some(body));
    if fail_fetch {
        s.code_offline();
    }
    s.gh.clear_calls();
}

/// 목록을 하나라도 물었는지예요(저장소 확인, login, 바뀜 확인 말고).
fn listed_anything(s: &Sandbox) -> bool {
    s.gh.calls().iter().any(|c| {
        let repo_only = c
            .path
            .strip_prefix("repos/")
            .is_some_and(|r| r.matches('/').count() == 1);
        !repo_only && c.path != "user" && !c.query.contains("sort=updated")
    })
}

#[test]
fn lost_repo_closes_even_when_code_download_fails() {
    let s = Sandbox::new();
    s.sync(none());
    let old_code = code_of(&s.engine.index.load().raw);
    repo_gone(&s, 404, json!({"message": "Not Found"}), true);
    let st = s.run(none());
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["warnings"], json!(["repo", "github"])); // 404 한 번: 아직은 저장본이에요
    assert!(has_github(&s));
    assert_eq!(load_state(&s.raw())["lost"], 1);
    let st = s.run(none());
    assert_eq!(st["state"], "done", "{st}");
    assert!(
        st["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("github-lost"))
    );
    assert_eq!(code_of(&s.engine.index.load().raw), old_code);
    assert!(!has_github(&s));
    assert_eq!(s.meta()["sources"]["github"]["lost"], true);
    assert_eq!(load_state(&s.raw())["lost"], 2);
    assert_eq!(s.code_calls(), Vec::<String>::new()); // 두 번 다 GitHub의 404가 먼저였어요
    assert!(!listed_anything(&s)); // 보이지 않는 저장소는 나열하지 않아요
}

#[test]
fn repo_id_change_does_not_fetch_new_code() {
    let s = Sandbox::new();
    s.sync(none());
    let mut state = load_state(&s.raw());
    state.insert("repo_id".into(), json!(1));
    save_state(&s.raw(), &state).unwrap();
    let old_code = code_of(&s.engine.index.load().raw);
    s.push("o/old", "app.py", "NAME = 'not yours'\n"); // 같은 이름 아래 다른 주인의 코드예요
    repo_gone(&s, 200, json!({"id": 2, "full_name": "o/old"}), false);
    let st = s.run(none());
    assert_eq!(st["state"], "done", "{st}");
    assert!(
        st["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("github-lost"))
    );
    assert_eq!(s.code_calls(), Vec::<String>::new()); // 새 주인의 코드는 묻지도 않아요
    assert_eq!(code_of(&s.engine.index.load().raw), old_code);
    assert!(!has_github(&s));
    assert!(!s.texts().contains("not yours"));
    assert_eq!(load_state(&s.raw())["repo_id"], 1);
    assert!(!listed_anything(&s));
}

#[test]
fn first_sync_of_an_invisible_repo_fails() {
    let s = Sandbox::new();
    s.gh.status(
        "repos/o/new",
        404,
        &[],
        Some(json!({"message": "Not Found"})),
    );
    let st = s.run(on("o/new", "main"));
    assert_eq!(st["state"], "failed", "{st}");
    assert_eq!(st["error_kind"], "key"); // 이어 갈 저장본이 없어요
    assert_eq!(st["error"], "GitHub repo not visible");
    assert_eq!(keys(&st), ["state", "finished_at", "error", "error_kind"]);
    assert_eq!(s.meta(), start_meta());
}

#[test]
fn embed_change_is_not_unchanged() {
    let s = Sandbox::new();
    s.sync(none());
    let meta = s.meta();
    assert_eq!(
        meta["embed"],
        json!({"model": "BAAI/bge-m3", "tokens": 512})
    );
    // meta가 "embed"를 갖기 전의 색인은 bge-m3, 512로 만들었어요: 할 일이 없어요
    let mut without = meta.clone();
    without.shift_remove("embed");
    s.set_meta(without);
    assert_eq!(s.run(none())["unchanged"], true);
    // 다른 글 길이의 벡터예요: 조각이 그대로여도 다시 만들어요
    let folder = ino(&s.index_dir());
    let mut other = meta.clone();
    other.insert(
        "embed".into(),
        json!({"model": "BAAI/bge-m3", "tokens": 8192}),
    );
    s.set_meta(other);
    let st = s.run(none());
    assert_eq!(st["state"], "done", "{st}");
    assert!(st.get("unchanged").is_none());
    assert_ne!(ino(&s.index_dir()), folder);
    assert_eq!(
        s.meta()["embed"],
        json!({"model": "BAAI/bge-m3", "tokens": 512})
    );
}

/// test_repo_lost_twice_closes_github의 끝이에요: 처음부터 받을 때의 404 한 번도 저장본을 바꾸지 않아요(닫지도
/// 않아요).
#[test]
fn fresh_raw_keeps_saved_data_on_a_404() {
    let s = Sandbox::new();
    s.sync(none());
    let paths = |raw: &Path| -> Vec<Value> {
        github_chunks(raw)
            .unwrap()
            .iter()
            .map(|c| c["path"].clone())
            .collect()
    };
    let before = paths(&s.raw());
    assert_eq!(before, [json!("github/issue/1")]);
    let meta = s.meta();
    s.gh.status(
        "repos/o/old",
        404,
        &[],
        Some(json!({"message": "Not Found"})),
    );
    let st = s.run(fresh());
    assert_eq!(st["state"], "failed", "{st}");
    assert_eq!(st["error_kind"], "key");
    assert_eq!(st["error"], "GitHub cannot see this repository now");
    assert!(!s.root.join("data/sources/github+new").exists());
    assert_eq!(paths(&s.raw()), before);
    assert_eq!(load_state(&s.raw())["lost"], 0);
    assert_eq!(s.meta(), meta);
}

/// 갱신 스레드의 패닉은 `failed`/`down`으로 끝나고, 지난 색인이 계속 답하고, 다음 갱신을 막지 않아요.
#[test]
fn a_panic_in_refresh_ends_failed() {
    let s = Sandbox::new();
    let live = s.engine.index.load_full();
    let files_before = listing(&s.index_dir());
    s.panic_on_token(true); // 갱신 스레드가 GitHub에 묻는 자리에서 패닉해요
    let st = s.run(none());
    assert_eq!(keys(&st), ["state", "finished_at", "error", "error_kind"]);
    assert_eq!(st["state"], "failed");
    assert_eq!(st["error"], "internal error");
    assert_eq!(st["error_kind"], "down");
    assert!(Arc::ptr_eq(&s.engine.index.load_full(), &live));
    assert_eq!(listing(&s.index_dir()), files_before);
    assert!(!s.root.join("index.new").exists());
    // 다음 갱신도 시작해요(같은 자리에서 다시 패닉해도 다시 끝나요)
    assert!(s.r.start(none()));
    assert_eq!(finish(&s.r)["state"], "failed");
    assert!(Arc::ptr_eq(&s.engine.index.load_full(), &live));
}

/// 다시 쓴 meta.json의 키 순서는 바꾸지 않아요(색인 파일 형식이에요).
#[test]
fn meta_key_order_matches_python() {
    let s = Sandbox::new();
    s.sync(none());
    let text = fs::read_to_string(s.index_dir().join("meta.json")).unwrap();
    let meta: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        keys(&meta),
        ["repo", "sources", "commit", "built_at", "embed"]
    );
    assert_eq!(
        keys(&meta["sources"]["repo"]),
        ["path", "branch", "graph", "symbols_missing"]
    );
    assert_eq!(
        keys(&meta["sources"]["github"]),
        ["repo", "fetched_at", "chunks", "raw"]
    );
    assert_eq!(text, pyjson::to_string(&meta, Some(1)));
    // 지운 뒤의 meta(cleared_at이 가운데)여도 나머지 키는 제자리예요: `remove`라면 맨 뒤 키가 그 자리로 와요
    let mut cleared = Map::new();
    for (k, v) in meta.as_object().unwrap() {
        cleared.insert(k.clone(), v.clone());
        if k == "repo" {
            cleared.insert("cleared_at".into(), json!("2026-10-26T10:00:00+0900"));
        }
    }
    cleared.insert("commit".into(), Value::Null);
    cleared.insert("built_at".into(), Value::Null);
    s.set_meta(cleared);
    s.sync(none());
    let text = fs::read_to_string(s.index_dir().join("meta.json")).unwrap();
    let meta: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        keys(&meta),
        ["repo", "sources", "commit", "built_at", "embed"]
    );
    assert_eq!(text, pyjson::to_string(&meta, Some(1)));
}

#[test]
fn places_resolve_and_rel() {
    let p = Places::new(Path::new("/x/y/data"));
    assert_eq!(p.root, Path::new("/x/y"));
    assert_eq!(p.data, Path::new("/x/y/data"));
    assert_eq!(p.raw(), Path::new("/x/y/data/sources"));
    assert_eq!(
        p.resolve("data/sources/old"),
        Path::new("/x/y/data/sources/old")
    );
    assert_eq!(p.resolve("data"), Path::new("/x/y/data"));
    assert_eq!(p.resolve("database/x"), Path::new("/x/y/database/x"));
    assert_eq!(p.resolve("other/z"), Path::new("/x/y/other/z"));
    assert_eq!(p.resolve("/abs/q"), Path::new("/abs/q"));
    assert_eq!(
        p.rel(Path::new("/x/y/data/sources/github")),
        "data/sources/github"
    );
    assert_eq!(p.rel(Path::new("/x/y/data")), "data");
    assert_eq!(p.rel(Path::new("/x/y/other")), "other");
    assert_eq!(p.rel(Path::new("/elsewhere/q")), "/elsewhere/q");
    assert_eq!(
        p.chunk_cache("O/New"),
        Path::new("/x/y/data/cache/chunks/o__new.json")
    );
    assert_eq!(p.emb_cache_root(), Path::new("/x/y/data/cache/emb"));
    // --data-dir의 이름이 data가 아니어도 meta의 data/… 자리는 그 아래예요
    let p = Places::new(Path::new("/x/store"));
    assert_eq!(
        (p.root.as_path(), p.data.as_path()),
        (Path::new("/x"), Path::new("/x/store"))
    );
    assert_eq!(
        p.resolve("data/sources/repos/o__n"),
        Path::new("/x/store/sources/repos/o__n")
    );
    assert_eq!(
        p.rel(Path::new("/x/store/sources/github")),
        "data/sources/github"
    );
    assert_eq!(p.rel(Path::new("/x/other")), "other");
    // 상대 경로는 절대 경로가 돼요
    let p = Places::new(Path::new("rel-data"));
    assert!(p.data.is_absolute() && p.data.ends_with("rel-data"));
    assert_eq!(repo_key("O/New"), "o/new");
}

#[test]
fn rebuild_target_rules() {
    for bad in [
        json!({"repo": "o/n\n"}),
        json!({"repo": "nope"}),
        json!({"branch": "-x"}),
        json!({"branch": "@{-1}"}),
        json!({"fresh": "yes"}),
        json!({"repo": 1}),
        json!([]),
        json!("o/n"),
        json!(null),
    ] {
        assert_eq!(rebuild_target(&bad), None, "{bad}");
    }
    assert_eq!(
        rebuild_target(&json!({"repo": null})),
        Some(Target::default())
    );
    assert_eq!(rebuild_target(&json!({})), Some(Target::default()));
    assert_eq!(
        rebuild_target(&json!({"repo": "o/n", "branch": "feat/x", "fresh": true})),
        Some(target(Some("o/n"), Some("feat/x"), true))
    );
    assert_eq!(
        rebuild_target(&json!({"fresh": false, "branch": null})),
        Some(Target::default())
    );
}

#[test]
fn status_starts_idle_and_start_refuses_while_busy() {
    let s = Sandbox::new();
    assert_eq!(*s.r.status(), json!({"state": "idle"}));
    for busy in [json!({"state": "running"}), json!({"state": "clearing"})] {
        s.r.set_status(busy.clone());
        assert!(!s.r.start(none()));
        assert_eq!(*s.r.status(), busy);
    }
    s.r.set_status(json!({"state": "idle"}));
    assert!(s.r.start(target(None, Some("-x"), false)));
    let st = finish(&s.r);
    assert_eq!(st["state"], "failed");
    assert_eq!(st["error"], "not a branch name");
    assert_eq!(st["error_kind"], "down");
    assert_eq!(s.r.index_dir(), s.index_dir());
    assert!(s.r.sources().github.connected());
}

/// 갱신 자물쇠를 잡은 채 패닉한 쪽이 있어도 다음 갱신은 시작해요. 잡고 있는 동안은 시작하지 않아요(지우기 중).
#[test]
fn a_poisoned_lock_does_not_block_the_next_start() {
    let s = Sandbox::new();
    {
        let _held = s.r.hold_lock();
        assert!(!s.r.start(none()));
    }
    assert_eq!(*s.r.status(), json!({"state": "idle"}));
    let r = s.r.clone();
    let poisoned = std::thread::spawn(move || {
        let _held = r.hold_lock();
        panic!("poison the refresh lock");
    })
    .join();
    assert!(poisoned.is_err());
    assert!(s.r.start(none()));
    assert_eq!(finish(&s.r)["state"], "done");
}

/// 씨앗 색인에서 다른 저장소로 바꾸다 첫 받기(tarball)가 끊기면 `failed`(network)이고, 색인·조각 캐시·repos.json은
/// 그대로이고 클론 폴더도 남기지 않아요(옛 `a_failed_clone_leaves_nothing`).
#[test]
fn a_failed_first_download_keeps_the_old_index() {
    let s = Sandbox::new();
    s.sync(none());
    s.set_meta(seed_meta());
    let live = s.engine.index.load_full();
    let (index_files, caches) = (files(&s.index_dir()), chunk_caches(&s));
    assert!(caches.contains_key("o__old.json"));
    // 원하는 파일이 많아 tarball 한 번으로 받아요. codeload가 몸통 절반에서 끊어요
    for i in 0..45 {
        s.push("o/new", &format!("m{i:02}.py"), &format!("M = {i}\n"));
    }
    s.repo("o/new").codeload_cut(true);
    let st = s.run(on("o/new", "main"));
    assert_eq!(st["state"], "failed", "{st}");
    assert_eq!(st["error_kind"], "network");
    assert!(s.gh.codeload_served() > 0);
    assert!(Arc::ptr_eq(&s.engine.index.load_full(), &live));
    assert_eq!(files(&s.index_dir()), index_files);
    assert_eq!(chunk_caches(&s), caches);
    assert!(!s.root.join("data/sources/repos.json").exists());
    assert!(!s.root.join("data/sources/repos").exists());
    assert!(!s.root.join("index.new").exists());
}

/// 보통 갱신에서 트리는 왔는데 blob 하나가 끊기면: 지난 코드와 커밋으로 끝나고(`warnings: ["repo"]`), 반쯤 받은
/// 것은 조각 캐시에도 색인에도 없어요.
#[test]
fn download_failure_midway_keeps_the_indexed_code() {
    let s = Sandbox::new();
    s.sync(none());
    let live = s.engine.index.load_full();
    let old_code = code_of(&live.raw);
    let commit = live.meta["commit"].as_str().unwrap().to_string();
    let cache_before = chunk_caches(&s);
    s.push("o/old", "fine.py", "FINE = 'arrived'\n");
    s.push("o/old", "app.py", "NAME = 'cut midway'\n");
    let cut = blob_sha(b"NAME = 'cut midway'\n");
    s.gh.route(&format!("repos/o/old/git/blobs/{cut}"), Reply::Close);
    s.gh.clear_calls();
    let st = s.run(none());
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["warnings"], json!(["repo"]));
    assert_eq!(st["commit"], commit[..8]);
    assert_eq!(s.meta()["commit"], commit.as_str());
    assert_eq!(code_of(&s.engine.index.load().raw), old_code);
    assert!(!s.texts().contains("arrived") && !s.texts().contains("cut midway"));
    let calls = s.code_calls();
    assert!(
        calls.iter().any(|c| c.starts_with("git/trees/")),
        "{calls:?}"
    );
    assert!(calls.contains(&format!("git/blobs/{cut}")), "{calls:?}");
    // 조각 캐시는 그대로예요: 새 blob의 키가 없어요
    assert_eq!(chunk_caches(&s), cache_before);
    let cache = String::from_utf8(cache_before["o__old.json"].clone()).unwrap();
    assert!(!cache.contains(&cut) && !cache.contains(&blob_sha(b"FINE = 'arrived'\n")));

    // 머리는 답했는데 그 트리가 없으면(404) down이고, 받기 실패라 같은 길이에요
    let head = s.push("o/old", "app.py", "NAME = 'no tree'\n");
    s.gh.status(
        &format!("repos/o/old/git/trees/{head}"),
        404,
        &[],
        Some(json!({"message": "Not Found"})),
    );
    let st = s.run(none());
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["warnings"], json!(["repo"]));
    assert_eq!(s.meta()["commit"], commit.as_str());
    assert_eq!(chunk_caches(&s), cache_before);
    // 이어 갈 코드가 없는 바꾸기라면 같은 404가 실패예요(down)
    let head = s.push("o/new", "app.py", "NAME = 'no tree'\n");
    s.gh.status(&format!("repos/o/new/git/trees/{head}"), 404, &[], None);
    let st = s.run(on("o/new", "main"));
    assert_eq!(
        (
            st["state"].clone(),
            st["error_kind"].clone(),
            st["error"].clone()
        ),
        (
            json!("failed"),
            json!("down"),
            json!("GitHub has no tree for the branch head")
        )
    );
}

/// 다른 저장소로 바꾸는 중의 받기 실패는 갱신 실패예요: 이어 갈 코드가 없어요.
#[test]
fn download_failure_while_switching_fails() {
    let s = Sandbox::new();
    s.sync(none());
    let live = s.engine.index.load_full();
    let before = listing(&s.index_dir());
    s.repo("o/new").offline(true);
    let st = s.run(on("o/new", "main"));
    assert_eq!(st["state"], "failed", "{st}");
    assert_eq!(st["error_kind"], "network");
    assert!(Arc::ptr_eq(&s.engine.index.load_full(), &live));
    assert_eq!(listing(&s.index_dir()), before);
    assert_eq!(s.meta()["repo"], "o/old");
    assert!(!s.root.join("data/sources/repos.json").exists());
    assert!(!s.root.join("data/cache/chunks/o__new.json").exists());
}

/// 브랜치를 지웠으면(`git/ref/heads/…`가 404인데 저장소는 보여요) down이에요(옛 `git fetch`의 실패와 같아요): 보통
/// 갱신은 지난 코드로 가고, 이어 갈 코드가 없는 바꾸기는 실패해요. 다시 연결하라는 key가 아니에요.
#[test]
fn a_deleted_branch_is_down_and_keeps_the_last_code() {
    let s = Sandbox::new();
    s.sync(none());
    let commit = s.meta()["commit"].as_str().unwrap().to_string();
    let cache_before = chunk_caches(&s);
    s.gh.status(
        "repos/o/old/git/ref/heads/main",
        404,
        &[],
        Some(json!({"message": "Not Found"})),
    );
    let st = s.run(none());
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["warnings"], json!(["repo"]));
    assert_eq!(s.meta()["commit"], commit.as_str());
    assert_eq!(chunk_caches(&s), cache_before);
    // 바꾸기는 이어 갈 코드가 없어 실패해요(down)
    let live = s.engine.index.load_full();
    s.push("o/new", "app.py", "NAME = 'new'\n");
    s.gh.status(
        "repos/o/new/git/ref/heads/gone",
        404,
        &[],
        Some(json!({"message": "Not Found"})),
    );
    let st = s.run(on("o/new", "gone"));
    assert_eq!(
        (
            st["state"].clone(),
            st["error_kind"].clone(),
            st["error"].clone()
        ),
        (json!("failed"), json!("down"), json!("the branch is gone"))
    );
    assert!(Arc::ptr_eq(&s.engine.index.load_full(), &live));
    assert_eq!(s.meta()["repo"], "o/old");
}

/// 저장소도 볼 수 없으면(브랜치와 저장소 둘 다 404) key로 실패해요: 권한을 잃은 거예요.
#[test]
fn a_hidden_repo_branch_is_a_key_failure() {
    let s = Sandbox::new();
    s.sync(none());
    let live = s.engine.index.load_full();
    s.gh.status(
        "repos/o/new/git/ref/heads/main",
        404,
        &[],
        Some(json!({"message": "Not Found"})),
    );
    s.gh.status(
        "repos/o/new",
        404,
        &[],
        Some(json!({"message": "Not Found"})),
    );
    let st = s.run(on("o/new", "main"));
    assert_eq!(
        (
            st["state"].clone(),
            st["error_kind"].clone(),
            st["error"].clone()
        ),
        (
            json!("failed"),
            json!("key"),
            json!("GitHub answered HTTP 404")
        )
    );
    assert!(Arc::ptr_eq(&s.engine.index.load_full(), &live));
    assert_eq!(s.meta()["repo"], "o/old");
}

// --- 지우기와 싼 바뀜 확인(`Refresher::clear`, `Refresher::check`, `under_raw`, `/index*`)

fn places(s: &Sandbox) -> Places {
    Places::new(&s.root.join("data"))
}

#[test]
fn clear_keeps_connection_and_empties_index() {
    let s = Sandbox::new();
    s.sync(none());
    assert!(s.root.join("index.old").is_dir());
    // 지우기는 저장소의 GitHub 원본만 지울 수 있어요. 자리가 있어야 참이에요(없는 것은 지울 것도 없어요)
    let sources = s.root.join("data/sources");
    for d in ["github-repos/o__new/x", "repos/o__new"] {
        fs::create_dir_all(sources.join(d)).unwrap();
    }
    let p = places(&s);
    assert!(under_raw(&p, &sources.join("github")));
    assert!(under_raw(&p, &sources.join("github-repos/o__new")));
    for no in [
        sources.clone(),
        sources.join("github-repos"),
        sources.join("repos/o__new"),
        sources.join("old"),
        sources.join("github-repos/o__new/x"),
        s.root.clone(),
        sources.join("github-repos/o__gone"), // 없어요
    ] {
        assert!(!under_raw(&p, &no), "{}", no.display());
    }
    let before = s.meta();

    let send =
        json!({"error": "send {repo?: owner/name, branch?: a branch name, fresh?: true or false}"});
    for bad in [
        r#"{"repo": "o/n\n"}"#,
        r#"{"repo": "nope"}"#,
        r#"{"branch": "-x"}"#,
        r#"{"branch": "@{-1}"}"#,
        r#"{"fresh": "yes"}"#,
        r#"{"repo": 1}"#,
        "[]",
    ] {
        assert_eq!(
            s.call("POST", "/index/rebuild", bad),
            (StatusCode::BAD_REQUEST, send.clone()),
            "{bad}"
        );
    }
    assert_eq!(
        s.call("POST", "/index/rebuild", "{"),
        (
            StatusCode::BAD_REQUEST,
            json!({"error": "body is not JSON"})
        )
    );

    s.r.set_status(json!({"state": "running"}));
    assert!(!s.r.clear() && !s.r.start(none()));
    assert_eq!(
        s.call("POST", "/index/clear", ""),
        (StatusCode::CONFLICT, json!({"cleared": false}))
    );
    let (status, got) = s.call(
        "POST",
        "/index/rebuild",
        r#"{"repo": "o/new", "branch": "main"}"#,
    );
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        got,
        json!({"started": false, "refresh": {"state": "running"}})
    );
    let (status, got) = s.call("POST", "/index/refresh", "not even JSON"); // 본문은 읽지 않아요
    assert_eq!(
        (status, got["started"].clone()),
        (StatusCode::OK, json!(false))
    );
    s.r.set_status(json!({"state": "idle"}));

    // 지우는 동안의 상태와 start: 색인을 여는 자리에서 봐요
    let during = Arc::new(Mutex::new(Vec::new()));
    {
        let (r, during) = (s.r.clone(), during.clone());
        s.on_open(Some(Box::new(move || {
            let st = r.status()["state"].as_str().unwrap().to_string();
            during.lock().unwrap().push((st, r.start(none())));
        })));
    }
    assert_eq!(
        s.call("POST", "/index/clear", "{"),
        (StatusCode::OK, json!({"cleared": true}))
    );
    s.on_open(None);
    assert_eq!(*during.lock().unwrap(), [("clearing".to_string(), false)]);
    let st = s.r.status();
    assert_eq!(keys(&st), ["state", "cleared_at"]);
    assert_eq!(st["state"], "idle");

    let ix = s.engine.index.load_full();
    assert!(ix.raw.is_empty() && ix.chunks.is_empty() && ix.emb.is_empty());
    assert_eq!(ix.meta["sources"], before["sources"]);
    assert_eq!(ix.meta["repo"], "o/old");
    assert_eq!(
        (ix.meta["built_at"].clone(), ix.meta["commit"].clone()),
        (Value::Null, Value::Null)
    );
    assert_eq!(ix.meta["cleared_at"], st["cleared_at"]);
    // 키는 제자리, cleared_at은 뒤에 붙어요
    let mut want: Vec<&str> = before.keys().map(String::as_str).collect();
    want.push("cleared_at");
    assert_eq!(ix.meta.keys().map(String::as_str).collect::<Vec<_>>(), want);
    let on_disk: Value =
        serde_json::from_slice(&fs::read(s.index_dir().join("meta.json")).unwrap()).unwrap();
    assert_eq!(on_disk, Value::Object(ix.meta.clone()));
    assert!(!s.raw().exists());
    assert!(s.root.join("data/sources/old/.git").is_dir());
    for gone in ["index.old", "index.new", "index+old"] {
        assert!(!s.root.join(gone).exists(), "{gone}");
    }
    let (status, got) = s.call("GET", "/index", "");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (
            got["cleared"].clone(),
            got["chunks"].clone(),
            got["refresh"]["state"].clone()
        ),
        (json!(true), json!(0), json!("idle"))
    );

    // 다시 받으면 돌아와요
    let (status, got) = s.call("POST", "/index/rebuild", r#"{"fresh": true}"#);
    assert_eq!(
        (status, got["started"].clone()),
        (StatusCode::OK, json!(true))
    );
    assert_eq!(finish(&s.r)["state"], "done");
    let (_, got) = s.call("GET", "/index", "");
    assert_eq!(got["cleared"], false);
    assert!(got["chunks"].as_u64().unwrap() > 0);
    assert!(!s.meta().contains_key("cleared_at"));
    let (status, got) = s.call("POST", "/index/refresh", "");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&got), ["started", "refresh"]);
    assert_eq!(got["started"], true);
    assert_eq!(finish(&s.r)["state"], "done");
}

/// 지우기 안의 패닉도 `failed`로 끝나요: 상태가 `clearing`에 남으면 갱신을 다시 시작할 수 없어요.
#[test]
fn a_panic_in_clear_ends_failed() {
    let s = Sandbox::new();
    s.on_open(Some(Box::new(|| {
        panic!("open_index panics as the test asked")
    })));
    assert_eq!(
        s.call("POST", "/index/clear", ""),
        (StatusCode::CONFLICT, json!({"cleared": false}))
    );
    s.on_open(None);
    let st = s.r.status();
    assert_eq!(keys(&st), ["state", "finished_at", "error", "error_kind"]);
    assert_eq!(
        (
            st["state"].clone(),
            st["error"].clone(),
            st["error_kind"].clone()
        ),
        (json!("failed"), json!("internal error"), json!("down"))
    );
    assert!(s.r.start(none()));
    assert_eq!(finish(&s.r)["state"], "done");
}

#[test]
fn clear_drops_only_that_repos_vectors() {
    let s = Sandbox::new();
    s.push("o/old", "shared.py", "SHARED = 1\n");
    s.push("o/new", "shared.py", "SHARED = 1\n");
    s.sync(none());
    s.sync(on("o/new", "main"));
    s.sync(on("o/old", "main"));
    let shared = chunk_with(&s, "SHARED = 1");
    let only_old = chunk_with(&s, "NAME = 'o/old'");
    let c = cache(&s);
    assert!(!c.get(&[&shared]).unwrap().is_empty() && !c.get(&[&only_old]).unwrap().is_empty());
    let chunk_cache = s.root.join("data/cache/chunks/o__old.json");
    assert!(chunk_cache.exists());
    assert!(s.r.clear());
    assert!(!c.get(&[&shared]).unwrap().is_empty());
    assert!(c.get(&[&only_old]).unwrap().is_empty());
    assert!(!chunk_cache.exists() && s.root.join("data/cache/chunks/o__new.json").exists());
}

#[test]
fn code_failure_after_clear_fails() {
    let s = Sandbox::new();
    s.sync(none());
    assert!(s.r.clear());
    let live = s.engine.index.load_full();
    let before = listing(&s.index_dir());
    code_offline(&s);
    let st = s.run(none());
    assert_eq!(
        (st["state"].clone(), st["error_kind"].clone()),
        (json!("failed"), json!("network")),
        "{st}"
    );
    assert!(Arc::ptr_eq(&live, &s.engine.index.load_full()));
    assert!(live.raw.is_empty() && py_truthy(&live.meta["cleared_at"]));
    assert_eq!(listing(&s.index_dir()), before);
}

fn py_truthy(v: &Value) -> bool {
    tacie_sources::py::truthy(v)
}

#[test]
fn corrupt_emb_cache_starts_over() {
    let s = Sandbox::new();
    let db = cache(&s).path;
    let bad = common::sandbox::with_suffix(&db, "+bad");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    for (i, t) in [none(), fresh()].into_iter().enumerate() {
        let junk = format!("not a database {i} ").repeat(100);
        fs::write(&db, &junk).unwrap();
        s.sync(t);
        assert_eq!(fs::read_to_string(&bad).unwrap(), junk); // 옆에 두고, 앞의 것은 바꿔요
        let first = s.engine.index.load().raw[0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!cache(&s).get(&[&first]).unwrap().is_empty()); // 새 캐시를 써요
    }
    fs::write(&db, "junk".repeat(100)).unwrap();
    assert!(s.r.clear());
    assert_eq!(fs::read_to_string(&bad).unwrap(), "junk".repeat(100));
}

/// 마지막 받기가 모든 목록을 `cursor`까지 나열했어요.
fn caught_up(s: &Sandbox, cursor: &str) -> PathBuf {
    let raw = s.raw();
    let mut st = load_state(&raw);
    let cursors: Map<String, Value> = ["issues", "comments", "review_comments"]
        .iter()
        .map(|k| (k.to_string(), json!(cursor)))
        .collect();
    st.insert("cursors".into(), Value::Object(cursors));
    save_state(&raw, &st).unwrap();
    raw
}

/// GitHub 목록의 지금 모습: 목록 → (ETag, 가장 새 updated_at 또는 빈 목록이면 None).
type Remote = Arc<Mutex<Vec<(&'static str, String, Option<String>)>>>;
/// 확인이 물은 (목록, 상태 코드)예요.
type Asked = Arc<Mutex<Vec<(String, u16)>>>;

/// 확인의 세 물음에 `remote`대로 답해요: 맞는 If-None-Match는 304예요. 저장소 자체는 있어요
/// (id 1). `meanwhile`은 답하기 전에 불러요.
fn fake_probes(
    s: &Sandbox,
    remote: &Remote,
    meanwhile: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Asked {
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    // 저장소 자체(id 1)는 놀이터가 답해요. 세 물음은 목록의 길보다 앞서요(쿼리 값이 더 많아요)
    for (stream, probe) in PROBES {
        let (remote, record, meanwhile) = (remote.clone(), asked.clone(), meanwhile.clone());
        let want = format!("repos/o/old/{probe}");
        s.gh.route_with(&want.clone(), move |path, etag| {
            assert_eq!(path, want);
            if let Some(f) = &meanwhile {
                f();
            }
            let remote = remote.lock().unwrap();
            let (_, tag, top) = remote.iter().find(|(s, _, _)| *s == stream).unwrap();
            let code = if etag == Some(tag.as_str()) { 304 } else { 200 };
            record.lock().unwrap().push((stream.to_string(), code));
            let body = match (code, top) {
                (304, _) => String::new(),
                (_, Some(top)) => json!([{"updated_at": top}]).to_string(),
                (_, None) => "[]".to_string(),
            };
            Reply::Http {
                code,
                headers: vec![("ETag".into(), tag.clone())],
                body: body.into_bytes(),
            }
        });
    }
    asked
}

fn streams_answered(code: u16) -> Vec<(String, u16)> {
    ["issues", "comments", "review_comments"]
        .iter()
        .map(|s| (s.to_string(), code))
        .collect()
}

#[test]
fn changed_stays_true_until_a_refresh_moves_the_cursor() {
    let s = Sandbox::new();
    s.sync(none());
    let (t0, t1) = ("2026-10-24T10:00:00Z", "2026-10-24T11:00:00Z");
    let raw = caught_up(&s, t0);
    let remote: Remote = Arc::new(Mutex::new(vec![
        ("issues", r#"W/"i1""#.to_string(), Some(t1.to_string())),
        ("comments", r#"W/"c0""#.to_string(), Some(t0.to_string())),
        ("review_comments", r#"W/"r0""#.to_string(), None),
    ]));
    let asked = fake_probes(&s, &remote, None);
    let changed = json!({"code": false, "github": true, "changed": true});
    s.gh.clear_calls();
    assert_eq!(s.r.check(), Some(changed.clone())); // 지난 받기 뒤에 이슈가 바뀌었어요
    assert_eq!(s.code_calls(), ["git/ref/heads/main"]); // 코드는 브랜치 머리 하나만 물어요
    assert_eq!(*asked.lock().unwrap(), streams_answered(200));
    let saved: Map<String, Value> = remote
        .lock()
        .unwrap()
        .iter()
        .map(|(st, e, top)| (st.to_string(), json!({"etag": e, "top": top})))
        .collect();
    assert_eq!(load_state(&raw)["etags"], Value::Object(saved));
    // 그다음 받기는 GitHub에 닿지 못했어요: 커서는 그대로예요
    github_offline(&s);
    assert_eq!(s.run(none())["warnings"], json!(["github"]));
    let cursors: Map<String, Value> = ["issues", "comments", "review_comments"]
        .iter()
        .map(|k| (k.to_string(), json!(t0)))
        .collect();
    assert_eq!(load_state(&raw)["cursors"], Value::Object(cursors));
    asked.lock().unwrap().clear();
    assert_eq!(s.r.check(), Some(changed)); // GitHub에 새것은 없지만 아직 받지 않았어요
    assert_eq!(*asked.lock().unwrap(), streams_answered(304));

    // 커서를 t1로 옮기는 받기예요: 목록의 이슈 #1이 t1에 바뀌었어요
    github_back(&s);
    s.lists(
        "o/old",
        vec![issue(
            "o/old",
            1,
            "o/old의 이슈",
            "o/old에서 받은 이슈예요.",
            t1,
        )],
    );
    s.gh.route("repos/o/old/issues/1/comments", Reply::Json(json!([])));
    s.sync(none());
    assert_eq!(load_state(&raw)["cursors"]["issues"], t1);
    asked.lock().unwrap().clear();
    assert_eq!(
        s.r.check(),
        Some(json!({"code": false, "github": false, "changed": false}))
    );
    assert_eq!(*asked.lock().unwrap(), streams_answered(304));
}

#[test]
fn check_route() {
    let s = Sandbox::new();
    s.sync(none());
    let t0 = "2026-10-24T10:00:00Z";
    let raw = caught_up(&s, t0);
    let remote: Remote = Arc::new(Mutex::new(
        ["issues", "comments", "review_comments"]
            .iter()
            .map(|st| (*st, format!(r#"W/"{st}""#), Some(t0.to_string())))
            .collect(),
    ));
    let asked = fake_probes(&s, &remote, None);
    assert_eq!(
        s.call_with("POST", "/index/check", "", None),
        (StatusCode::UNAUTHORIZED, json!({"error": "unauthorized"}))
    );
    let same = json!({"code": false, "github": false, "changed": false});
    assert_eq!(
        s.call("POST", "/index/check", ""),
        (StatusCode::OK, same.clone())
    ); // 브랜치 머리가 색인의 커밋이에요
    assert_eq!(
        s.call("POST", "/index/check", r#"{"anything": 1}"#),
        (StatusCode::OK, same.clone())
    );
    assert_eq!(
        s.call("POST", "/index/check", "not JSON"),
        (StatusCode::OK, same)
    ); // 본문은 읽지 않아요
    assert_eq!(asked.lock().unwrap()[6..], streams_answered(304));
    s.push("o/old", "new.py", "NEW = 1\n");
    assert_eq!(
        s.call("POST", "/index/check", ""),
        (
            StatusCode::OK,
            json!({"code": true, "github": false, "changed": true})
        )
    );
    let conflict = (StatusCode::CONFLICT, json!({"changed": false}));
    for busy in [
        json!({"state": "running", "step": "embed", "started_at": "2026-10-24T10:00:00+0900"}),
        json!({"state": "clearing"}),
    ] {
        s.r.set_status(busy);
        assert_eq!(s.call("POST", "/index/check", ""), conflict);
        assert_eq!(s.r.check(), None);
    }
    s.r.set_status(json!({"state": "idle"}));
    {
        let _held = s.r.hold_lock(); // 지우기가 잡고 있어요
        assert_eq!(s.call("POST", "/index/check", ""), conflict);
        assert!(!s.r.start(none())); // 잡혀 있으면 갱신도 시작하지 않아요
        assert!(!s.r.clear());
    }
    assert_eq!(*s.r.status(), json!({"state": "idle"}));

    // 물음과 저장 사이에 갱신이 끝났거나 지웠으면: 물어 온 것은 버리고 폴더도 만들지 않아요
    let saved = load_state(&raw)["etags"].clone();
    assert!(py_truthy(&saved));
    for (_, tag, _) in remote.lock().unwrap().iter_mut() {
        *tag = r#"W/"other""#.to_string();
    }
    let r = s.r.clone();
    let finished: Arc<dyn Fn() + Send + Sync> =
        Arc::new(move || r.set_status(json!({"state": "done", "finished_at": "x"})));
    fake_probes(&s, &remote, Some(finished));
    assert_eq!(s.r.check().unwrap()["changed"], true);
    assert_eq!(load_state(&raw)["etags"], saved);
    let gone = raw.clone();
    let cleared: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = fs::remove_dir_all(&gone);
    });
    s.r.set_status(json!({"state": "idle"}));
    fake_probes(&s, &remote, Some(cleared));
    assert!(s.r.check().is_some());
    assert!(!raw.exists());

    // 연결이 없으면 아무것도 띄우지 않고 묻지도 않아요: git도 GitHub도요
    s.github.set_credentials("none", None);
    s.gh.clear_calls();
    assert_eq!(
        s.call("POST", "/index/check", ""),
        (
            StatusCode::OK,
            json!({"code": null, "github": null, "changed": false})
        )
    );
    assert!(s.gh.calls().is_empty());
}

/// 코드 확인은 `meta.repo`의 브랜치 머리(`git/ref/heads/…`) 하나를 확인의 시간 제한으로 물어요: 늦으면 알 수 없어요
/// (`code: null`). 클론 자리는 보지 않아요.
#[test]
fn check_asks_the_branch_ref_with_the_check_timeout() {
    let s = Sandbox::with_timeouts(Some(Timeouts {
        check: Duration::from_millis(300),
        ..Timeouts::default()
    }));
    s.sync(none());
    assert!(!s.root.join("data/sources/old/app.py").exists()); // 클론 자리에는 아무것도 없어요
    s.gh.clear_calls();
    assert_eq!(s.r.check().unwrap()["code"], false);
    assert_eq!(s.code_calls(), ["git/ref/heads/main"]);
    let head = s.meta()["commit"].as_str().unwrap().to_string();
    // 답은 테스트가 놓아 줄 때까지 기다려요: 확인이 먼저 돌아오면 check 시간 제한이 끊은 거예요(시계 여유에 기대지
    // 않아요). 제한이 없으면 30 s 뒤 답이 와서 `code`가 false가 돼요
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let wait = std::sync::Mutex::new(wait);
    let (asked, asked_rx) = std::sync::mpsc::channel::<()>();
    let asked = std::sync::Mutex::new(asked);
    s.gh.route_with("repos/o/old/git/ref/heads/main", move |_, _| {
        let _ = asked.lock().unwrap().send(());
        let _ = wait.lock().unwrap().recv_timeout(Duration::from_secs(30));
        Reply::Json(json!({"ref": "refs/heads/main", "object": {"sha": head, "type": "commit"}}))
    });
    let got = s.r.check().unwrap();
    let _ = release.send(());
    asked_rx.try_recv().expect("the branch ref was asked");
    assert_eq!(got["code"], Value::Null);
    assert_eq!(got["changed"], false);
}

/// 지운 색인이나 `sources.repo`가 없는 색인의 확인은 아무것도 띄우지 않아요(물을 것이 없어요).
#[test]
fn check_answers_nothing_without_a_repo_or_after_clear() {
    let s = Sandbox::new();
    s.sync(none());
    let nothing = Some(json!({"code": null, "github": null, "changed": false}));
    s.gh.clear_calls();
    let meta = s.meta();
    let mut no_repo = meta.clone();
    no_repo["sources"]
        .as_object_mut()
        .unwrap()
        .shift_remove("repo");
    s.set_meta(no_repo);
    assert_eq!(s.r.check(), nothing);
    let mut cleared = meta.clone();
    cleared.insert("cleared_at".into(), json!("2026-10-24T10:00:00+0900"));
    s.set_meta(cleared);
    assert_eq!(s.r.check(), nothing);
    assert!(s.gh.calls().is_empty());
}

/// ETag를 저장하지 못해도(여기서는 state.json 자리가 폴더) 확인은 답해요: ETag는 아끼는 것일 뿐이에요.
#[test]
fn check_answers_even_when_etags_cannot_be_saved() {
    let s = Sandbox::new();
    s.sync(none());
    let raw = s.raw();
    fs::remove_file(raw.join("state.json")).unwrap();
    fs::create_dir(raw.join("state.json")).unwrap();
    let remote: Remote = Arc::new(Mutex::new(
        ["issues", "comments", "review_comments"]
            .iter()
            .map(|st| {
                (
                    *st,
                    format!(r#"W/"{st}""#),
                    Some("2026-10-24T10:00:00Z".to_string()),
                )
            })
            .collect(),
    ));
    let asked = fake_probes(&s, &remote, None);
    // 읽지 못한 상태는 빈 상태라 커서가 없어요: 무엇이든 새것이에요
    assert_eq!(
        s.r.check(),
        Some(json!({"code": false, "github": true, "changed": true}))
    );
    assert_eq!(*asked.lock().unwrap(), streams_answered(200));
    assert!(raw.join("state.json").is_dir());
    assert!(!raw.join("state.json+tmp").exists());
}

/// meta.json의 GitHub 원본 자리에 `..`나 바깥을 가리키는 링크가 있어도 지우기는
/// `data/sources/github`나 `data/sources/github-repos/<하나>` 밖을 지우지 않아요.
#[test]
fn clear_never_deletes_outside_raw() {
    let s = Sandbox::new();
    s.sync(none());
    let sources = s.root.join("data/sources");
    let outside = s.root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep.txt"), "keep").unwrap();
    fs::create_dir_all(sources.join("github-repos")).unwrap();
    std::os::unix::fs::symlink(&outside, sources.join("github-repos/evil")).unwrap();
    let p = places(&s);
    assert!(!under_raw(&p, &sources.join("github-repos/evil")));
    assert!(!under_raw(&p, &sources.join("github-repos/..")));
    assert!(!under_raw(&p, &sources.join("github-repos/evil/..")));
    assert!(under_raw(&p, &sources.join("github-repos/../github"))); // 풀면 data/sources/github예요

    let before = files(&sources);
    for raw in [
        "data/sources/github-repos/..",
        "data/sources/github-repos/evil",
        "outside",
    ] {
        let mut meta = s.meta();
        meta["sources"]["github"]
            .as_object_mut()
            .unwrap()
            .insert("raw".into(), json!(raw));
        s.set_meta(meta);
        assert!(s.r.clear(), "{raw}");
        assert_eq!(
            fs::read_to_string(outside.join("keep.txt")).unwrap(),
            "keep",
            "{raw}"
        );
        assert!(
            fs::symlink_metadata(sources.join("github-repos/evil"))
                .unwrap()
                .is_symlink()
        );
        assert!(
            sources.join("old/.git").is_dir() && sources.join("old-graph").is_dir(),
            "{raw}"
        );
        assert!(s.raw().join("state.json").exists(), "{raw}"); // 이 meta의 원본 자리가 아니에요
    }
    assert_eq!(
        files(&sources).keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>()
    );

    // `data/sources/github`이 바깥을 가리키는 링크여도 그 바깥은 그대로예요
    fs::remove_dir_all(s.raw()).unwrap();
    std::os::unix::fs::symlink(&outside, s.raw()).unwrap();
    assert!(!under_raw(&p, &s.raw()));
    let mut meta = s.meta();
    meta["sources"]["github"]
        .as_object_mut()
        .unwrap()
        .shift_remove("raw");
    s.set_meta(meta);
    assert!(s.r.clear());
    assert_eq!(
        fs::read_to_string(outside.join("keep.txt")).unwrap(),
        "keep"
    );

    // `github-repos/` 안의 링크가 다른 저장소의 원본을 가리켜도 그 원본은 그대로예요(링크면
    // 아무것도 지우지 않아요). 푼 자리는 `github-repos` 바로 아래라 `under_raw`는 참이에요
    let other = sources.join("github-repos/a__b");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("state.json"), "{}").unwrap();
    std::os::unix::fs::symlink(&other, sources.join("github-repos/link")).unwrap();
    assert!(under_raw(&p, &sources.join("github-repos/link")));
    let mut meta = s.meta();
    meta["sources"]["github"]
        .as_object_mut()
        .unwrap()
        .insert("raw".into(), json!("data/sources/github-repos/link"));
    s.set_meta(meta);
    assert!(s.r.clear());
    assert_eq!(fs::read_to_string(other.join("state.json")).unwrap(), "{}");
    assert!(
        fs::symlink_metadata(sources.join("github-repos/link"))
            .unwrap()
            .is_symlink()
    );
    // 끝에 `/`나 `.`가 붙어도 같은 링크예요(경로를 풀 때 떼요). 절대 경로는 그대로 읽어요
    let link = sources.join("github-repos/link");
    for raw in [
        "data/sources/github-repos/link/".to_string(),
        "data/sources/github-repos/./link/.".to_string(),
        format!("{}/", link.display()),
        format!("{}/.", link.display()),
    ] {
        let mut meta = s.meta();
        meta["sources"]["github"]
            .as_object_mut()
            .unwrap()
            .insert("raw".into(), json!(raw));
        s.set_meta(meta);
        assert!(s.r.clear(), "{raw}");
        assert_eq!(
            fs::read_to_string(other.join("state.json")).unwrap(),
            "{}",
            "{raw}"
        );
    }
}
