//! The Dropbox connection (docs/33): Dropbox alone without a repo, with a repo, a first fetch cut into batches,
//! removing a shared folder, switching accounts, turning folders off, failures not breaking GitHub sync,
//! `/index/check`, `POST /dropbox/credentials`, `GET /dropbox/folders`, `dropbox` in `GET /index`, and deleting
//! search data.

mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use std::time::Duration;

use axum::http::StatusCode;
use common::sandbox::{Sandbox, finish};
use serde_json::{Value, json};
use tacie_engine::refresh::{Target, seed_meta};

fn connect(s: &Sandbox, account_id: &str) {
    s.dropbox.set(
        Some("token".into()),
        Some("me@example.com".into()),
        Some(account_id.into()),
    );
}

/// 폴더 `/a`(f1)와 `/b`(f2)에 글 파일 하나씩이에요.
fn two_files(s: &Sandbox) {
    s.dropbox_api.folder("f1", "/a");
    s.dropbox_api.folder("f2", "/b");
    s.dropbox_api.file(
        "x1",
        "/a/deploy.md",
        1,
        "# 배포\n금요일에는 배포하지 않아요.\n".as_bytes(),
    );
    s.dropbox_api.file(
        "x2",
        "/b/onboarding.md",
        2,
        "# 첫 주\n노트북을 받아요.\n".as_bytes(),
    );
}

/// 저장소 없는 씨앗 색인에 Dropbox만 연결한 사람이에요.
fn dropbox_alone() -> Sandbox {
    let s = Sandbox::new();
    s.set_meta(seed_meta());
    s.github.set_credentials("none", None);
    two_files(&s);
    connect(&s, "dbid:A");
    s
}

fn sources(s: &Sandbox) -> Value {
    Value::Object(s.meta())["sources"].clone()
}

fn post(s: &Sandbox, body: Value) -> (StatusCode, Value) {
    s.call("POST", "/dropbox/credentials", &body.to_string())
}

/// 갱신이 끝날 때까지, 그리고 이어서 걸린 갱신(`pending`)도 끝날 때까지 기다려요.
fn settle(s: &Sandbox) {
    for _ in 0..6 {
        finish(&s.r);
        std::thread::sleep(Duration::from_millis(150));
    }
    finish(&s.r);
}

#[test]
fn dropbox_alone_builds_an_index_without_a_repo() {
    let s = dropbox_alone();
    s.sync(Target::default());
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    let src = sources(&s);
    assert_eq!(
        src.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["dropbox"]
    );
    assert_eq!(src["dropbox"]["files"], 2);
    assert_eq!(src["dropbox"]["account"], "me@example.com");
    assert_eq!(src["dropbox"]["initial_done"], true);
    let raw = s.engine.index.load().raw.clone();
    assert_eq!(raw[0]["src"], "dropbox");
    let text = raw[0]["text"].as_str().unwrap();
    assert!(text.starts_with("dropbox: "), "{text}");
    assert!(text.contains("\nmeta: Dropbox · "), "{text}");
    assert!(
        raw[0]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://www.dropbox.com/home/")
    );
}

#[test]
fn dropbox_joins_the_repo() {
    let s = Sandbox::new();
    s.sync(Target::default());
    two_files(&s);
    connect(&s, "dbid:A");
    s.sync(Target::default());
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    let src = sources(&s);
    assert!(src.get("repo").is_some());
    assert_eq!(src["dropbox"]["files"], 2);
}

#[test]
fn a_large_first_fetch_is_split_and_carried_on_by_itself() {
    let s = dropbox_alone();
    for i in 3..8 {
        s.dropbox_api.file(
            &format!("x{i}"),
            &format!("/a/doc{i}.md"),
            1,
            format!("묶음 문서 {i}").as_bytes(),
        );
    }
    s.dropbox.set_batch(3, Duration::from_secs(600));
    s.r.start(Target::default());
    settle(&s);
    let src = sources(&s);
    assert_eq!(src["dropbox"]["files"], 7, "{src}");
    assert_eq!(src["dropbox"]["initial_done"], true);
    assert!(s.texts().contains("묶음 문서 7"));
}

#[test]
fn edits_and_deletions_reach_the_index() {
    let s = dropbox_alone();
    s.sync(Target::default());
    s.dropbox_api
        .edit("x1", "# 배포\n목요일에도 배포하지 않아요.\n".as_bytes());
    s.dropbox_api.delete("/b");
    s.sync(Target::default());
    let texts = s.texts();
    assert!(texts.contains("목요일에도 배포하지 않아요."));
    assert!(!texts.contains("금요일에는"));
    assert!(!texts.contains("노트북을 받아요."));
}

#[test]
fn an_unmounted_shared_folder_leaves_the_index() {
    let s = dropbox_alone();
    s.sync(Target::default());
    s.dropbox_api.join("/shared", "s1", &[("j1", "notes.md")]);
    s.sync(Target::default());
    assert!(s.texts().contains("공유 폴더의 파일이에요"));
    s.dropbox_api.unmount("/shared");
    s.sync(Target::default());
    let texts = s.texts();
    assert!(!texts.contains("공유 폴더의 파일이에요"));
    assert!(texts.contains("금요일에는 배포하지 않아요."));
}

#[test]
fn a_new_account_wipes_the_old_one_first() {
    let s = dropbox_alone();
    s.sync(Target::default());
    assert!(s.r.dropbox_dir().join("items").exists());
    // 가짜의 계정은 dbid:A라서 dbid:B로는 받지 못해요: 남는 것은 지운 결과뿐이에요
    let (code, out) = post(
        &s,
        json!({"token": "token2", "account": "other@example.com", "account_id": "dbid:B", "muted_folders": []}),
    );
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        out,
        json!({"connected": true, "account": "other@example.com"})
    );
    settle(&s);
    assert!(!s.r.dropbox_dir().join("items").exists());
    assert!(!s.texts().contains("금요일에는 배포하지 않아요."));
}

#[test]
fn the_refresh_wipes_files_saved_for_another_account() {
    let s = dropbox_alone();
    s.sync(Target::default());
    connect(&s, "dbid:B");
    s.sync(Target::default());
    assert!(!s.r.dropbox_dir().join("items").exists());
    assert!(!s.texts().contains("금요일에는 배포하지 않아요."));
}

#[test]
fn a_rotated_token_does_not_start_a_sync() {
    let s = dropbox_alone();
    s.sync(Target::default());
    let before = s.meta();
    post(
        &s,
        json!({"token": "token-renewed", "account": "me@example.com", "account_id": "dbid:A"}),
    );
    std::thread::sleep(Duration::from_millis(200));
    finish(&s.r);
    assert_eq!(s.meta(), before);
}

#[test]
fn muting_a_top_folder_drops_its_cards_and_lists_the_folders() {
    let s = dropbox_alone();
    s.sync(Target::default());
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    let (code, d) = s.call("GET", "/dropbox/folders", "");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        d,
        json!({"folders": [
            {"id": "f1", "name": "a", "path": "/a", "files": 1, "team_folder": false},
            {"id": "f2", "name": "b", "path": "/b", "files": 1, "team_folder": false},
        ]})
    );
    post(
        &s,
        json!({"token": "token", "account": "me@example.com", "account_id": "dbid:A", "muted_folders": ["id:f1"]}),
    );
    settle(&s);
    let texts = s.texts();
    assert!(!texts.contains("금요일에는 배포하지 않아요."));
    assert!(texts.contains("노트북을 받아요."));
    assert_eq!(
        tacie_sources::dropbox::muted(&s.r.dropbox_dir()),
        ["f1".to_string()]
    );
}

#[test]
fn disconnecting_dropbox_alone_goes_back_to_the_seed() {
    let s = dropbox_alone();
    s.sync(Target::default());
    let (_, out) = post(&s, json!({"token": null}));
    assert_eq!(out, json!({"connected": false, "account": null}));
    settle(&s);
    assert!(s.engine.index.load().raw.is_empty());
    assert!(s.meta().get("sources").is_none());
    assert!(!s.r.dropbox_dir().exists());
}

#[test]
fn disconnecting_after_a_restart_still_removes_the_saved_files() {
    let s = dropbox_alone();
    s.sync(Target::default());
    // 엔진을 다시 띄운 것처럼 메모리의 연결만 없어요
    s.dropbox.set(None, None, None);
    let (code, _) = post(&s, json!({"token": null}));
    assert_eq!(code, StatusCode::OK);
    settle(&s);
    assert!(!s.r.dropbox_dir().exists());
    assert!(!s.texts().contains("금요일에는 배포하지 않아요."));
}

#[test]
fn dropbox_still_refreshes_when_github_fails() {
    let s = Sandbox::new();
    s.sync(Target::default());
    let before = s.engine.index.load().raw.len();
    s.dropbox_api.file(
        "x1",
        "/a/deploy.md",
        1,
        "# 배포\n금요일에는 배포하지 않아요.\n".as_bytes(),
    );
    connect(&s, "dbid:A");
    s.github.set_credentials("none", None);
    let st = s.run(Target::default());
    assert_eq!(st["state"], "failed");
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    assert_eq!(s.engine.index.load().raw.len(), before + 1);
}

#[test]
fn a_busy_dropbox_keeps_saved_files_and_other_sources_refresh() {
    let s = Sandbox::new();
    s.dropbox_api.file(
        "x1",
        "/a/deploy.md",
        1,
        "# 배포\n금요일에는 배포하지 않아요.\n".as_bytes(),
    );
    connect(&s, "dbid:A");
    s.sync(Target::default());
    s.dropbox_api.busy(true);
    s.push("o/old", "NEW.md", "# 새 문서\n\n새로 올린 문서예요.\n");
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    assert!(
        st["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w == "dropbox"),
        "{st}"
    );
    let texts = s.texts();
    assert!(texts.contains("새로 올린 문서예요."));
    assert!(texts.contains("금요일에는 배포하지 않아요."));
}

#[test]
fn a_first_fetch_refused_shows_in_the_index_status() {
    let s = Sandbox::new();
    s.sync(Target::default());
    s.dropbox_api.refuse(true);
    connect(&s, "dbid:A");
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    let (_, ix) = s.call("GET", "/index", "");
    assert_eq!(
        ix["dropbox"],
        json!({"connected": true, "account": "me@example.com", "files": null, "fetched_at": null,
               "initial_done": false, "refused": true, "error_kind": "key", "lost_since": null})
    );
    assert!(!ix.to_string().contains("\"token\""));
}

#[test]
fn the_check_asks_dropbox_for_changes() {
    let s = dropbox_alone();
    s.sync(Target::default());
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["dropbox"], false, "{c}");
    s.dropbox_api.file(
        "x9",
        "/a/new.md",
        0,
        "# 새 파일\n새로 쓴 글이에요.\n".as_bytes(),
    );
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["dropbox"], true, "{c}");
    assert_eq!(c["changed"], true);
}

#[test]
fn bad_credentials_bodies_are_refused() {
    let s = Sandbox::new();
    for body in [
        json!({}),
        json!({"token": 3}),
        json!({"token": "a b"}),
        json!({"token": "t", "account": 1}),
        json!({"token": "t", "muted_folders": "f1"}),
        json!({"token": "t", "muted_folders": [1]}),
        json!({"token": "café"}),
        json!({"token": "t"}),
        json!({"token": "t", "account": "me@example.com"}),
        json!({"token": "t", "account_id": null}),
        json!({"token": "t", "account_id": " "}),
    ] {
        let (code, out) = post(&s, body.clone());
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
        assert!(!out.to_string().contains("café"), "{out}");
    }
}

#[test]
fn a_token_without_an_account_id_is_refused_and_keeps_the_saved_files() {
    let s = dropbox_alone();
    s.sync(Target::default());
    assert!(s.r.dropbox_dir().join("items").exists());
    let (code, out) = post(&s, json!({"token": "token2", "account": "me@example.com"}));
    assert_eq!(code, StatusCode::BAD_REQUEST, "{out}");
    assert!(out.to_string().contains("account_id"), "{out}");
    settle(&s);
    assert!(s.r.dropbox_dir().join("items").exists());
    assert_eq!(s.dropbox.account_id().as_deref(), Some("dbid:A"));
    assert!(s.texts().contains("노트북을 받아요."));
}

#[test]
fn clearing_the_search_data_removes_the_dropbox_files_but_keeps_muted_folders() {
    let s = dropbox_alone();
    s.sync(Target::default());
    post(
        &s,
        json!({"token": "token", "account": "me@example.com", "account_id": "dbid:A", "muted_folders": ["f1"]}),
    );
    settle(&s);
    assert!(s.r.dropbox_dir().join("items").exists());
    let (code, _) = s.call("POST", "/index/clear", "");
    assert_eq!(code, StatusCode::OK);
    finish(&s.r);
    assert!(!s.r.dropbox_dir().join("items").exists());
    assert_eq!(
        tacie_sources::dropbox::muted(&s.r.dropbox_dir()),
        ["f1".to_string()]
    );
    assert!(!s.texts().contains("노트북을 받아요."));
}

#[test]
fn the_token_never_shows_in_answers_or_status() {
    let s = Sandbox::new();
    s.sync(Target::default());
    two_files(&s);
    s.dropbox_api.refuse(true);
    let secret = "sl.SECRET-dropbox-token-42";
    let (_, out) = post(
        &s,
        json!({"token": secret, "account": "me@example.com", "account_id": "dbid:A"}),
    );
    settle(&s);
    let (_, ix) = s.call("GET", "/index", "");
    let (_, folders) = s.call("GET", "/dropbox/folders", "");
    let st = finish(&s.r);
    assert_eq!(ix["dropbox"]["error_kind"], "key", "{ix}");
    for v in [out, ix, folders, st, Value::Object(s.meta())] {
        assert!(!v.to_string().contains("SECRET"), "{v}");
    }
}
