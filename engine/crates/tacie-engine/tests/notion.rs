//! The Notion connection (docs/13): with a repo, Notion alone without a repo, disconnecting, saved copies before
//! the app passes the connection, `/index/check`, `POST /notion/credentials`, `notion` in `GET /index`.

mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use axum::http::StatusCode;
use common::sandbox::Sandbox;
use serde_json::{Value, json};
use tacie_engine::refresh::{Target, seed_meta};
use tacie_sources::notion;

fn page(s: &Sandbox, id: &str, title: &str, body: &str) {
    s.notion_api
        .pages
        .lock()
        .unwrap()
        .push((id.into(), title.into(), body.into()));
}

/// 저장소 없는 씨앗 색인에 노션만 연결한 사람이에요.
fn notion_alone() -> Sandbox {
    let s = Sandbox::new();
    s.set_meta(seed_meta());
    s.github.set_credentials("none", None);
    page(&s, "p-1", "배포 순서", "금요일에는 배포하지 않아요.");
    s.notion.set(Some("token"), Some("팀"));
    s
}

fn notion_sources(s: &Sandbox) -> Value {
    s.meta()["sources"].clone()
}

#[test]
fn notion_alone_builds_an_index_without_a_repo() {
    let s = notion_alone();
    s.sync(Target::default());
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    let src = notion_sources(&s);
    assert_eq!(
        src.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["notion"]
    );
    assert_eq!(src["notion"]["pages"], 1);
    assert_eq!(src["notion"]["workspace"], "팀");
    assert!(s.meta().get("cleared_at").is_none());
    let raw = s.engine.index.load().raw.clone();
    assert_eq!(raw[0]["src"], "notion");
    assert_eq!(raw[0]["path"], "notion/p-1");
}

#[test]
fn the_saved_pages_stay_until_the_app_hands_the_connection_over() {
    let s = notion_alone();
    s.sync(Target::default());
    // 엔진이 새로 떴고 앱이 아직 연결을 넘기지 않았어요
    s.notion.set(None, None);
    page(&s, "p-2", "새 페이지", "아직 받지 않았어요.");
    s.sync(Target::default());
    let texts = s.texts();
    assert!(texts.contains("금요일에는 배포하지 않아요."));
    assert!(!texts.contains("아직 받지 않았어요."));
}

#[test]
fn disconnecting_notion_alone_goes_back_to_the_seed() {
    let s = notion_alone();
    s.sync(Target::default());
    s.notion.set(None, None);
    notion::forget(&s.r.notion_dir()).unwrap();
    s.sync(Target::default());
    assert!(s.engine.index.load().raw.is_empty());
    assert!(s.meta().get("cleared_at").is_some());
    assert!(s.meta().get("sources").is_none());
}

#[test]
fn notion_joins_the_repo_index_and_follows_a_repo_switch() {
    let s = Sandbox::new();
    s.sync(Target::default());
    assert!(notion_sources(&s).get("notion").is_none());
    page(&s, "p-1", "배포 순서", "금요일에는 배포하지 않아요.");
    s.notion.set(Some("token"), None);
    s.sync(Target::default());
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    assert!(notion_sources(&s).get("repo").is_some());
    assert_eq!(notion_sources(&s)["notion"]["pages"], 1);
}

#[test]
fn check_asks_for_notion_only_when_it_is_due() {
    let s = notion_alone();
    let st = s.r.check().unwrap();
    assert_eq!(st["notion"], true);
    assert_eq!(st["changed"], true);
    s.sync(Target::default());
    let st = s.r.check().unwrap();
    assert_eq!(st["changed"], false);
    // 연결이 없으면 칸도 없어요(앱과의 계약 그대로)
    s.notion.set(None, None);
    assert!(s.r.check().unwrap().get("notion").is_none());
}

#[test]
fn credentials_route_connects_and_reports_without_the_token() {
    let s = Sandbox::new();
    for bad in [
        "{}",
        r#"{"token": 3}"#,
        r#"{"token": "a b"}"#,
        r#"{"token": "t", "workspace": 1}"#,
    ] {
        let (code, _) = s.call("POST", "/notion/credentials", bad);
        assert_eq!(code, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (code, out) = s.call(
        "POST",
        "/notion/credentials",
        r#"{"token": "secret-token", "workspace": "팀"}"#,
    );
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"connected": true, "workspace": "팀"}));
    let (_, ix) = s.call("GET", "/index", "");
    assert_eq!(ix["notion"]["connected"], true);
    assert_eq!(ix["notion"]["workspace"], "팀");
    assert_eq!(ix["notion"]["refused"], false);
    assert!(!ix.to_string().contains("secret-token"));

    let (code, out) = s.call("POST", "/notion/credentials", r#"{"token": null}"#);
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"connected": false, "workspace": null}));
}

#[test]
fn notion_still_refreshes_when_github_fails() {
    let s = Sandbox::new();
    s.sync(Target::default());
    let before = s.engine.index.load().raw.len();
    page(&s, "p-1", "배포 순서", "금요일에는 배포하지 않아요.");
    s.notion.set(Some("token"), Some("팀"));
    s.github.set_credentials("none", None); // GitHub refuses: the sync fails as before
    let st = s.run(Target::default());
    assert_eq!(st["state"], "failed");
    assert_eq!(st["error_kind"], "key");
    // but the Notion pages are in, beside the code that was there
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    assert_eq!(s.engine.index.load().raw.len(), before + 1);
    assert_eq!(notion_sources(&s)["notion"]["pages"], 1);
    assert!(notion_sources(&s).get("repo").is_some());
}
