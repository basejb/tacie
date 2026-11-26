//! The note folders source on this Mac (docs/34): notes alone, with a repo, notes changing, and removing a folder.

mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use std::fs;
use std::path::Path;
use std::time::Duration;

use axum::http::StatusCode;
use common::sandbox::{Sandbox, finish};
use serde_json::{Value, json};
use tacie_engine::refresh::{Target, seed_meta};
use tacie_sources::notes::{self, Folder};

const VAULT: &str = "0123456789abcdef";

fn alone() -> Sandbox {
    let s = Sandbox::new();
    s.set_meta(seed_meta());
    s.github.set_credentials("none", None);
    s
}

fn vault(dir: &Path) -> Folder {
    fs::create_dir_all(dir.join(".obsidian")).unwrap();
    Folder {
        path: dir.to_string_lossy().into_owned(),
        vault_id: Some(VAULT.into()),
        name: Some("일 노트".into()),
    }
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn paths(s: &Sandbox) -> Vec<String> {
    let mut out: Vec<String> = s
        .engine
        .index
        .load()
        .raw
        .iter()
        .filter(|c| c["src"] == "notes")
        .map(|c| c["path"].as_str().unwrap().to_string())
        .collect();
    out.dedup();
    out
}

fn post(s: &Sandbox, folders: &[Folder]) -> (StatusCode, Value) {
    s.call(
        "POST",
        "/notes/folders",
        &json!({ "folders": folders }).to_string(),
    )
}

fn settle(s: &Sandbox) {
    for _ in 0..3 {
        finish(&s.r);
        std::thread::sleep(Duration::from_millis(150));
    }
    finish(&s.r);
}

#[test]
fn notes_only_build_the_index() {
    let s = alone();
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "결정/캐시.md",
        "# 캐시\nTTL은 5분으로 정했어요.\n",
    );
    notes::save(&s.r.notes_dir(), &[vault(d.path())]).unwrap();
    s.sync(Target::default());
    assert!(s.texts().contains("TTL은 5분으로 정했어요."));
    assert_eq!(paths(&s), [format!("notes/{VAULT}/결정/캐시.md")]);
    assert_eq!(s.meta()["sources"]["notes"]["notes"], 1);
    assert!(s.meta().get("cleared_at").is_none());
}

#[test]
fn route_saves_puts_notes_in_and_index_reports_them() {
    let s = alone();
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "a.md", "하나");
    let (code, out) = post(&s, &[vault(d.path())]);
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"count": 1}));
    settle(&s);
    assert_eq!(paths(&s), [format!("notes/{VAULT}/a.md")]);
    let (code, info) = s.call("GET", "/index", "");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(info["notes"]["chosen"], 1);
    assert_eq!(info["notes"]["notes"], 1);
    assert_eq!(info["notes"]["folders"][0]["obsidian"], true);
}

#[test]
fn route_rejects_bad_body() {
    let s = alone();
    for bad in [
        "{}",
        "nope",
        r#"{"folders": "x"}"#,
        r#"{"folders": [{"path": "relative/dir"}]}"#,
        r#"{"folders": [{"path": "/a", "vault_id": "short"}]}"#,
    ] {
        let (code, _) = s.call("POST", "/notes/folders", bad);
        assert_eq!(code, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert!(!s.r.notes_dir().join(notes::FILE).exists());
}

#[test]
fn a_changed_note_goes_in_on_the_next_check_without_asking_github() {
    let s = Sandbox::new();
    s.sync(Target::default());
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "a.md", "처음 글");
    post(&s, &[vault(d.path())]);
    settle(&s);
    assert!(s.texts().contains("처음 글"));
    assert!(!s.r.notes_stale_quickly());
    let calls = s.calls().len();
    let before = s.r.status();
    write(d.path(), "a.md", "고친 글이에요");
    write(d.path(), "b.md", "새 노트");
    assert!(s.r.notes_stale_quickly());
    s.r.notes_changed();
    settle(&s);
    assert!(s.texts().contains("고친 글이에요"));
    assert!(!s.texts().contains("처음 글"));
    assert_eq!(paths(&s).len(), 2);
    assert_eq!(
        s.calls().len(),
        calls,
        "no GitHub request for a notes change"
    );
    assert_eq!(s.r.status(), before, "the last sync's status stays");
}

#[test]
fn removing_every_folder_drops_the_notes_and_leaves_the_seed() {
    let s = alone();
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "a.md", "하나");
    post(&s, &[vault(d.path())]);
    settle(&s);
    assert_eq!(paths(&s).len(), 1);
    post(&s, &[]);
    settle(&s);
    assert!(paths(&s).is_empty());
    assert!(s.meta().get("cleared_at").is_some());
}

#[test]
fn notes_stay_beside_a_repo_sync() {
    let s = Sandbox::new();
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "a.md", "노트 글");
    notes::save(&s.r.notes_dir(), &[vault(d.path())]).unwrap();
    s.sync(Target::default());
    assert_eq!(paths(&s).len(), 1);
    assert!(
        s.engine
            .index
            .load()
            .raw
            .iter()
            .any(|c| c["src"] != "notes")
    );
    // 다시 동기화해도 노트는 그대로예요
    s.sync(Target::default());
    assert_eq!(paths(&s).len(), 1);
}
