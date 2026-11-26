//! The Google Drive connection (docs/19): Drive alone without a repo, with a repo, a first fetch cut into batches,
//! switching accounts, turning shared drives off, failures not breaking GitHub sync, `/index/check`,
//! `POST /gdrive/credentials`, `GET /gdrive/drives`, `gdrive` in `GET /index`.

mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::StatusCode;
use common::sandbox::{Sandbox, finish};
use serde_json::{Value, json};
use tacie_engine::refresh::{Target, seed_meta};
use tacie_sources::gdrive::GDriveFetch;

fn connect(s: &Sandbox, account_id: &str) {
    s.gdrive.set(
        Some("token".into()),
        Some("me@example.com".into()),
        Some(account_id.into()),
    );
}

/// 저장소 없는 씨앗 색인에 Drive만 연결한 사람이에요.
fn drive_alone() -> Sandbox {
    let s = Sandbox::new();
    s.set_meta(seed_meta());
    s.github.set_credentials("none", None);
    s.gdrive_api.doc(
        "d1",
        "배포 규칙",
        "# 배포\n금요일에는 배포하지 않아요.",
        None,
    );
    s.gdrive_api
        .doc("d2", "온보딩", "# 첫 주\n노트북을 받아요.", None);
    connect(&s, "p1");
    s
}

fn sources(s: &Sandbox) -> Value {
    Value::Object(s.meta())["sources"].clone()
}

fn post(s: &Sandbox, body: Value) -> (StatusCode, Value) {
    s.call("POST", "/gdrive/credentials", &body.to_string())
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
fn drive_alone_builds_an_index_without_a_repo() {
    let s = drive_alone();
    s.sync(Target::default());
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    let src = sources(&s);
    assert_eq!(
        src.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["gdrive"]
    );
    assert_eq!(src["gdrive"]["files"], 2);
    assert_eq!(src["gdrive"]["account"], "me@example.com");
    assert_eq!(src["gdrive"]["initial_done"], true);
    let raw = s.engine.index.load().raw.clone();
    assert_eq!(raw[0]["src"], "gdrive");
    assert!(
        raw[0]["text"]
            .as_str()
            .unwrap()
            .contains("meta: Google Docs · 내 드라이브")
    );
}

#[test]
fn drive_joins_the_repo() {
    let s = Sandbox::new();
    s.sync(Target::default());
    s.gdrive_api
        .doc("d1", "배포 규칙", "금요일에는 배포하지 않아요.", None);
    connect(&s, "p1");
    s.sync(Target::default());
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    let src = sources(&s);
    assert!(src.get("repo").is_some());
    assert_eq!(src["gdrive"]["files"], 1);
}

#[test]
fn a_large_first_fetch_is_split_and_carried_on_by_itself() {
    let s = drive_alone();
    for i in 3..8 {
        s.gdrive_api
            .doc(&format!("d{i}"), "문서", &format!("묶음 문서 {i}"), None);
    }
    s.gdrive.set_batch(2, Duration::from_secs(600));
    s.r.start(Target::default());
    settle(&s);
    let src = sources(&s);
    assert_eq!(src["gdrive"]["files"], 7, "{src}");
    assert_eq!(src["gdrive"]["initial_done"], true);
    assert!(s.texts().contains("묶음 문서 7"));
}

#[test]
fn edits_and_deletions_reach_the_index() {
    let s = drive_alone();
    s.sync(Target::default());
    s.gdrive_api
        .texts
        .lock()
        .unwrap()
        .insert("d1".into(), "목요일에도 배포하지 않아요.".into());
    s.gdrive_api.edit("d1", 0);
    s.gdrive_api.delete("d2");
    s.sync(Target::default());
    let texts = s.texts();
    assert!(texts.contains("목요일에도 배포하지 않아요."));
    assert!(!texts.contains("금요일에는"));
    assert!(!texts.contains("노트북을 받아요."));
}

#[test]
fn the_saved_files_stay_until_the_app_hands_the_connection_over() {
    let s = drive_alone();
    s.sync(Target::default());
    s.gdrive.set(None, None, None);
    s.gdrive_api
        .doc("d3", "새 문서", "아직 받지 않았어요.", None);
    s.sync(Target::default());
    let texts = s.texts();
    assert!(texts.contains("금요일에는 배포하지 않아요."));
    assert!(!texts.contains("아직 받지 않았어요."));
}

#[test]
fn a_new_account_wipes_the_old_one_first() {
    let s = drive_alone();
    s.sync(Target::default());
    s.gdrive_api.vanish("d1");
    s.gdrive_api.vanish("d2");
    s.gdrive_api
        .doc("d9", "다른 계정", "두 번째 계정이에요.", None);
    let (code, out) = post(
        &s,
        json!({"token": "token2", "account": "other@example.com", "account_id": "p2", "muted_drives": []}),
    );
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        out,
        json!({"connected": true, "account": "other@example.com"})
    );
    settle(&s);
    let texts = s.texts();
    assert!(texts.contains("두 번째 계정이에요."));
    assert!(!texts.contains("금요일에는 배포하지 않아요."));
}

#[test]
fn a_rotated_token_does_not_start_a_sync() {
    let s = drive_alone();
    s.sync(Target::default());
    let before = s.meta();
    post(
        &s,
        json!({"token": "token-renewed", "account": "me@example.com", "account_id": "p1"}),
    );
    std::thread::sleep(Duration::from_millis(200));
    finish(&s.r);
    assert_eq!(s.meta(), before);
}

#[test]
fn muting_a_shared_drive_drops_its_cards_and_lists_the_drives() {
    let s = drive_alone();
    *s.gdrive_api.drives.lock().unwrap() = vec![json!({"id": "D1", "name": "기획팀"})];
    s.gdrive_api
        .doc("t1", "로드맵", "기획팀의 로드맵이에요.", Some("D1"));
    s.sync(Target::default());
    assert!(s.texts().contains("기획팀의 로드맵이에요."));
    let (_, d) = s.call("GET", "/gdrive/drives", "");
    assert_eq!(
        d,
        json!({"drives": [{"id": "D1", "name": "기획팀", "files": 1}]})
    );
    post(
        &s,
        json!({"token": "token", "account": "me@example.com", "account_id": "p1", "muted_drives": ["D1"]}),
    );
    settle(&s);
    assert!(!s.texts().contains("기획팀의 로드맵이에요."));
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
}

#[test]
fn disconnecting_drive_alone_goes_back_to_the_seed() {
    let s = drive_alone();
    s.sync(Target::default());
    let (_, out) = post(&s, json!({"token": null}));
    assert_eq!(out, json!({"connected": false, "account": null}));
    settle(&s);
    assert!(s.engine.index.load().raw.is_empty());
    assert!(s.meta().get("sources").is_none());
    assert!(!s.r.gdrive_dir().exists());
}

#[test]
fn drive_still_refreshes_when_github_fails() {
    let s = Sandbox::new();
    s.sync(Target::default());
    let before = s.engine.index.load().raw.len();
    s.gdrive_api
        .doc("d1", "배포 규칙", "금요일에는 배포하지 않아요.", None);
    connect(&s, "p1");
    s.github.set_credentials("none", None);
    let st = s.run(Target::default());
    assert_eq!(st["state"], "failed");
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    assert_eq!(s.engine.index.load().raw.len(), before + 1);
}

#[test]
fn a_busy_drive_keeps_saved_files_and_other_sources_refresh() {
    let s = Sandbox::new();
    s.gdrive_api
        .doc("d1", "배포 규칙", "금요일에는 배포하지 않아요.", None);
    connect(&s, "p1");
    s.sync(Target::default());
    s.gdrive_api.busy.store(true, Ordering::SeqCst);
    s.push("o/old", "NEW.md", "# 새 문서\n\n새로 올린 문서예요.\n");
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    assert!(
        st["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w == "gdrive")
    );
    let texts = s.texts();
    assert!(texts.contains("새로 올린 문서예요."));
    assert!(texts.contains("금요일에는 배포하지 않아요."));
}

#[test]
fn a_first_fetch_refused_shows_in_the_index_status() {
    let s = Sandbox::new();
    s.sync(Target::default());
    s.gdrive_api.refuse.store(true, Ordering::SeqCst);
    connect(&s, "p1");
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    let (_, ix) = s.call("GET", "/index", "");
    let g = &ix["gdrive"];
    assert_eq!(g["connected"], true);
    assert_eq!(g["account"], "me@example.com");
    assert_eq!(g["error_kind"], "key");
    assert_eq!(g["refused"], true);
    assert_eq!(g["files"], Value::Null, "nothing was found, not zero files");
    assert!(!s.gdrive.connected() || g.get("token").is_none());
}

#[test]
fn the_check_asks_drive_for_changes() {
    let s = drive_alone();
    s.sync(Target::default());
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["gdrive"], false, "{c}");
    s.gdrive_api.edit("d1", 0);
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["gdrive"], true, "{c}");
    assert_eq!(c["changed"], true);
}

#[test]
fn bad_credentials_bodies_are_refused() {
    let s = Sandbox::new();
    for body in [
        json!({}),
        json!({"token": 3}),
        json!({"token": "a b"}),
        json!({"token": "a", "muted_drives": [1]}),
        json!({"token": "a", "account": 1}),
    ] {
        let (code, _) = post(&s, body.clone());
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
    }
}

#[test]
fn clearing_the_search_data_removes_the_drive_files_too() {
    let s = Sandbox::new();
    s.sync(Target::default());
    s.gdrive_api
        .doc("d1", "배포 규칙", "금요일에는 배포하지 않아요.", None);
    connect(&s, "p1");
    s.sync(Target::default());
    assert!(s.r.gdrive_dir().join("items").exists());
    assert!(s.r.clear());
    assert!(!s.r.gdrive_dir().exists());
    assert!(!s.texts().contains("금요일에는 배포하지 않아요."));
}
