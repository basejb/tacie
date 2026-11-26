//! The Linear connection (docs/18): Linear alone without a repo, with Notion and a repo, saved copies, switching
//! workspaces, turning teams off, failures not breaking GitHub sync, `/index/check`, `POST /linear/credentials`,
//! `GET /linear/teams`.

mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use axum::http::StatusCode;
use common::sandbox::{Sandbox, finish};
use serde_json::{Value, json};
use tacie_engine::refresh::{Target, seed_meta};

const NEW: &str = "2026-10-30T00:00:00.000Z";
const NEWER: &str = "2026-10-31T00:00:00.000Z";

fn connect(s: &Sandbox, workspace_id: &str) {
    s.linear.set(
        Some("token".into()),
        Some("팀".into()),
        Some(workspace_id.into()),
    );
}

/// 저장소 없는 씨앗 색인에 Linear만 연결한 사람이에요.
fn linear_alone() -> Sandbox {
    let s = Sandbox::new();
    s.set_meta(seed_meta());
    s.github.set_credentials("none", None);
    s.linear_api
        .issue("1", "t1", "배포 순서", "금요일에는 배포하지 않아요.", NEW);
    s.linear_api
        .issue("2", "t2", "네비게이션", "웹 팀의 일이에요.", NEW);
    connect(&s, "w1");
    s
}

fn sources(s: &Sandbox) -> Value {
    Value::Object(s.meta())["sources"].clone()
}

fn post(s: &Sandbox, body: Value) -> (StatusCode, Value) {
    s.call("POST", "/linear/credentials", &body.to_string())
}

/// 갱신이 끝날 때까지, 그리고 이어서 걸린 갱신(`pending`)도 끝날 때까지 기다려요.
fn settle(s: &Sandbox) {
    for _ in 0..3 {
        finish(&s.r);
        std::thread::sleep(Duration::from_millis(150));
    }
    finish(&s.r);
}

#[test]
fn linear_alone_builds_an_index_without_a_repo() {
    let s = linear_alone();
    s.sync(Target::default());
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    let src = sources(&s);
    assert_eq!(
        src.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["linear"]
    );
    assert_eq!(src["linear"]["issues"], 2);
    assert_eq!(src["linear"]["workspace"], "팀");
    assert_eq!(s.engine.index.load().raw[0]["src"], "linear");
}

#[test]
fn linear_joins_notion_and_the_repo() {
    let s = Sandbox::new();
    s.sync(Target::default());
    s.notion_api.pages.lock().unwrap().push((
        "p-1".into(),
        "배포".into(),
        "노션의 배포 규칙".into(),
    ));
    s.notion.set(Some("token"), None);
    s.linear_api
        .issue("1", "t1", "배포 순서", "금요일에는 배포하지 않아요.", NEW);
    connect(&s, "w1");
    s.sync(Target::default());
    let texts = s.texts();
    assert!(texts.contains("노션의 배포 규칙"));
    assert!(texts.contains("금요일에는 배포하지 않아요."));
    let src = sources(&s);
    assert!(src.get("repo").is_some() && src.get("notion").is_some());
    assert_eq!(src["linear"]["issues"], 1);
}

#[test]
fn the_saved_items_stay_until_the_app_hands_the_connection_over() {
    let s = linear_alone();
    s.sync(Target::default());
    s.linear.set(None, None, None);
    s.linear_api
        .issue("3", "t1", "새 이슈", "아직 받지 않았어요.", NEWER);
    s.sync(Target::default());
    let texts = s.texts();
    assert!(texts.contains("금요일에는 배포하지 않아요."));
    assert!(!texts.contains("아직 받지 않았어요."));
}

#[test]
fn a_new_workspace_wipes_the_old_one_first() {
    let s = linear_alone();
    s.sync(Target::default());
    s.linear_api.issues.lock().unwrap().clear();
    s.linear_api
        .issue("9", "t1", "다른 회사", "두 번째 워크스페이스예요.", NEW);
    let (code, _) = post(
        &s,
        json!({"token": "token2", "workspace": "다른 팀", "workspace_id": "w2", "muted_teams": []}),
    );
    assert_eq!(code, StatusCode::OK);
    settle(&s);
    let texts = s.texts();
    assert!(texts.contains("두 번째 워크스페이스예요."));
    assert!(!texts.contains("금요일에는 배포하지 않아요."));
}

#[test]
fn muting_a_team_resyncs_and_drops_its_cards() {
    let s = linear_alone();
    s.sync(Target::default());
    let (code, _) = post(
        &s,
        json!({"token": "token", "workspace": "팀", "workspace_id": "w1", "muted_teams": ["t2"]}),
    );
    assert_eq!(code, StatusCode::OK);
    settle(&s);
    let texts = s.texts();
    assert!(!texts.contains("웹 팀의 일이에요."));
    assert!(texts.contains("금요일에는 배포하지 않아요."));
}

#[test]
fn muted_teams_stay_out_after_an_engine_restart_before_credentials_arrive() {
    let s = linear_alone();
    s.sync(Target::default());
    post(
        &s,
        json!({"token": "token", "workspace": "팀", "workspace_id": "w1", "muted_teams": ["t2"]}),
    );
    settle(&s);
    s.linear.set(None, None, None); // the engine restarted
    s.sync(Target::default());
    assert!(!s.texts().contains("웹 팀의 일이에요."));
}

#[test]
fn disconnecting_linear_alone_goes_back_to_the_seed() {
    let s = linear_alone();
    s.sync(Target::default());
    let (_, out) = post(&s, json!({"token": null}));
    assert_eq!(out, json!({"connected": false, "workspace": null}));
    settle(&s);
    assert!(s.engine.index.load().raw.is_empty());
    assert!(s.meta().get("sources").is_none());
}

#[test]
fn linear_still_refreshes_when_github_fails() {
    let s = Sandbox::new();
    s.sync(Target::default());
    let before = s.engine.index.load().raw.len();
    s.linear_api
        .issue("1", "t1", "배포 순서", "금요일에는 배포하지 않아요.", NEW);
    connect(&s, "w1");
    s.github.set_credentials("none", None);
    let st = s.run(Target::default());
    assert_eq!(st["state"], "failed");
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    assert_eq!(s.engine.index.load().raw.len(), before + 1);
}

#[test]
fn a_busy_linear_keeps_saved_items_and_other_sources_refresh() {
    let s = Sandbox::new();
    s.linear_api
        .issue("1", "t1", "배포 순서", "금요일에는 배포하지 않아요.", NEW);
    connect(&s, "w1");
    s.sync(Target::default());
    s.linear_api.busy.store(true, Ordering::SeqCst);
    s.push("o/old", "NEW.md", "# 새 문서\n\n새로 올린 문서예요.\n");
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    assert!(
        st["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w == "linear")
    );
    let texts = s.texts();
    assert!(texts.contains("새로 올린 문서예요."));
    assert!(texts.contains("금요일에는 배포하지 않아요."));
}

#[test]
fn linear_first_fetch_refused_does_not_fail_the_github_sync() {
    let s = Sandbox::new();
    s.sync(Target::default());
    s.linear_api.refuse.store(true, Ordering::SeqCst);
    connect(&s, "w1");
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    assert!(st.get("error_kind").is_none());
    let (_, ix) = s.call("GET", "/index", "");
    assert_eq!(ix["linear"]["error_kind"], "key");
    assert_eq!(ix["linear"]["refused"], true);
    assert_eq!(
        ix["linear"]["issues"],
        Value::Null,
        "nothing was found, not zero issues"
    );
}

#[test]
fn notion_failing_does_not_block_a_linear_only_index() {
    let s = linear_alone();
    s.notion.set(Some("token"), None);
    s.notion_api.fail.store(true, Ordering::SeqCst);
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
}

/// 다음 이슈 받기에서 멈추는 문을 걸어요: (들어왔음 받기, 놓아주기 보내기).
fn gate(s: &Sandbox) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    *s.linear_api.gate.lock().unwrap() = Some((entered_tx, release_rx));
    (entered_rx, release_tx)
}

#[test]
fn a_workspace_switch_during_a_running_sync_leaves_no_old_files() {
    let s = linear_alone();
    let (entered, release) = gate(&s);
    assert!(s.r.start(Target::default()));
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    s.linear_api.issues.lock().unwrap().clear();
    s.linear_api
        .issue("9", "t1", "다른 회사", "두 번째 워크스페이스예요.", NEW);
    post(
        &s,
        json!({"token": "token2", "workspace": "다른 팀", "workspace_id": "w2", "muted_teams": []}),
    );
    release.send(()).unwrap();
    settle(&s);
    let texts = s.texts();
    assert!(texts.contains("두 번째 워크스페이스예요."), "{texts}");
    assert!(!texts.contains("금요일에는 배포하지 않아요."));
    let items = std::fs::read_dir(s.r.linear_dir().join("items"))
        .unwrap()
        .count();
    assert_eq!(items, 1);
}

#[test]
fn muting_during_a_running_sync_takes_effect_when_it_ends() {
    let s = linear_alone();
    let (entered, release) = gate(&s);
    assert!(s.r.start(Target::default()));
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    post(
        &s,
        json!({"token": "token", "workspace": "팀", "workspace_id": "w1", "muted_teams": ["t2"]}),
    );
    release.send(()).unwrap();
    settle(&s);
    let texts = s.texts();
    assert!(texts.contains("금요일에는 배포하지 않아요."));
    assert!(!texts.contains("웹 팀의 일이에요."));
}

#[test]
fn a_rotated_token_does_not_start_a_sync() {
    let s = linear_alone();
    s.sync(Target::default());
    let before = s.linear_api.queries.load(Ordering::SeqCst);
    post(
        &s,
        json!({"token": "token-rotated", "workspace": "팀", "workspace_id": "w1", "muted_teams": []}),
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_ne!(s.r.status()["state"], "running");
    assert_eq!(s.linear_api.queries.load(Ordering::SeqCst), before);
}

#[test]
fn check_says_linear_changed_when_a_newer_item_exists() {
    let s = linear_alone();
    s.sync(Target::default());
    let st = s.r.check().unwrap();
    assert_eq!(st["linear"], false, "{st}");
    assert_eq!(st["changed"], false);
    s.linear_api.issue(
        "3",
        "t1",
        "새 이슈",
        "새로 생겼어요.",
        "2099-01-01T00:00:00.000Z",
    );
    let st = s.r.check().unwrap();
    assert_eq!(st["linear"], true);
    assert_eq!(st["changed"], true);
}

#[test]
fn check_covers_linear_without_github() {
    let s = linear_alone();
    let st = s.r.check().unwrap();
    assert_eq!(st["linear"], true, "never fetched: due");
    s.linear.set(None, None, None);
    assert!(s.r.check().unwrap().get("linear").is_none());
}

#[test]
fn credentials_route_connects_and_reports_without_the_token() {
    let s = Sandbox::new();
    for bad in [
        "{}",
        r#"{"token": 3}"#,
        r#"{"token": "a b"}"#,
        r#"{"token": "t", "workspace": 1}"#,
        r#"{"token": "t", "muted_teams": "t1"}"#,
    ] {
        let (code, _) = s.call("POST", "/linear/credentials", bad);
        assert_eq!(code, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (code, out) = post(
        &s,
        json!({"token": "secret-token", "workspace": "팀", "workspace_id": "w1", "muted_teams": []}),
    );
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"connected": true, "workspace": "팀"}));
    settle(&s);
    let (_, ix) = s.call("GET", "/index", "");
    assert_eq!(ix["linear"]["connected"], true);
    assert_eq!(ix["linear"]["workspace"], "팀");
    assert_eq!(ix["linear"]["refused"], false);
    assert!(!ix.to_string().contains("secret-token"));
    let (code, out) = post(&s, json!({"token": null}));
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"connected": false, "workspace": null}));
}

#[test]
fn teams_route_lists_visible_teams_with_issue_counts() {
    let s = linear_alone();
    let (_, out) = s.call("GET", "/linear/teams", "");
    assert_eq!(out, json!({"teams": []}));
    s.sync(Target::default());
    let (code, out) = s.call("GET", "/linear/teams", "");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        out["teams"][0],
        json!({"id": "t1", "name": "Core", "issues": 1})
    );
    assert_eq!(out["teams"].as_array().unwrap().len(), 2);
}

#[test]
fn a_switch_after_a_cut_first_fetch_wipes_it() {
    let s = linear_alone();
    s.linear_api.issues.lock().unwrap().clear();
    s.linear_api
        .issue("1", "t1", "첫 회사", "첫 번째 워크스페이스예요.", NEWER);
    s.linear_api.busy_after_issues.store(true, Ordering::SeqCst);
    s.run(Target::default()); // issues saved with a cursor, then cut
    s.linear_api
        .busy_after_issues
        .store(false, Ordering::SeqCst);
    s.linear_api.issues.lock().unwrap().clear();
    // older than the first workspace's cursor: only a wiped store asks for it
    s.linear_api
        .issue("9", "t1", "다른 회사", "두 번째 워크스페이스예요.", NEW);
    post(
        &s,
        json!({"token": "token2", "workspace": "다른 팀", "workspace_id": "w2", "muted_teams": []}),
    );
    settle(&s);
    let texts = s.texts();
    assert!(texts.contains("두 번째 워크스페이스예요."), "{texts}");
    assert!(!texts.contains("첫 번째 워크스페이스예요."));
}

#[test]
fn a_renewed_token_refused_again_does_not_resync_every_time() {
    let s = linear_alone();
    s.sync(Target::default());
    s.linear_api.refuse.store(true, Ordering::SeqCst);
    s.sync(Target::default()); // refused: the app renews
    post(
        &s,
        json!({"token": "token-2", "workspace": "팀", "workspace_id": "w1", "muted_teams": []}),
    );
    settle(&s);
    let before = s.linear_api.queries.load(Ordering::SeqCst);
    post(
        &s,
        json!({"token": "token-3", "workspace": "팀", "workspace_id": "w1", "muted_teams": []}),
    );
    std::thread::sleep(Duration::from_millis(200));
    settle(&s);
    assert_eq!(
        s.linear_api.queries.load(Ordering::SeqCst),
        before,
        "the next try waits for the 30-minute check"
    );
}
