//! The Jira and Confluence connection (docs/31): Atlassian alone without a repo, with a repo, a first fetch cut by
//! the hourly budget, token refresh, turning projects off, disconnecting, a different site, failures not breaking
//! GitHub sync, `/index/check`, `POST /atlassian/credentials`, `GET /atlassian/scopes`, `atlassian` in
//! `GET /index`.

mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use common::sandbox::{Sandbox, finish};
use serde_json::{Value, json};
use tacie_engine::refresh::{Target, seed_meta};
use tacie_sources::atlassian::{self, AtlassianFetch};

const CLOUD: &str = "cloud-1";

/// Jira가 주는 모양의 시각이에요(밀리초, `+0000`).
fn jira_time(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.3f+0000").to_string()
}

fn adf(text: &str) -> Value {
    json!({"type": "doc", "version": 1, "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}]})
}

fn issue(id: &str, key: &str, project: &str, summary: &str, body: &str, ago_min: i64) -> Value {
    let updated = jira_time(Utc::now() - chrono::Duration::minutes(ago_min));
    json!({"id": id, "key": key, "fields": {
        "summary": summary, "updated": updated, "description": adf(body),
        "project": {"id": format!("p-{project}"), "key": project, "name": format!("{project} 팀")},
        "status": {"name": "진행 중"}, "issuetype": {"name": "버그"}}})
}

fn add(s: &Sandbox, v: Value) {
    s.atlassian_api.jira.lock().unwrap().push(v);
}

fn connect(s: &Sandbox, cloud: &str) {
    s.atlassian.set(
        Some("token".into()),
        Some(cloud.into()),
        Some("acme".into()),
        Some("https://acme.atlassian.net".into()),
    );
}

/// 저장소 없는 씨앗 색인에 Atlassian만 연결한 사람이에요.
fn atlassian_alone() -> Sandbox {
    let s = Sandbox::new();
    s.set_meta(seed_meta());
    s.github.set_credentials("none", None);
    add(
        &s,
        issue(
            "10",
            "ENG-1",
            "ENG",
            "배포 규칙",
            "금요일에는 배포하지 않아요.",
            60,
        ),
    );
    add(
        &s,
        issue(
            "11",
            "ENG-2",
            "ENG",
            "온보딩",
            "첫 주에 노트북을 받아요.",
            90,
        ),
    );
    connect(&s, CLOUD);
    s
}

fn sources(s: &Sandbox) -> Value {
    Value::Object(s.meta())["sources"].clone()
}

fn post(s: &Sandbox, body: Value) -> (StatusCode, Value) {
    s.call("POST", "/atlassian/credentials", &body.to_string())
}

fn credentials(cloud: &str, extra: Value) -> Value {
    let mut b = json!({"token": "token", "cloud": cloud, "site": "acme",
                       "site_url": "https://acme.atlassian.net"});
    for (k, v) in extra.as_object().unwrap() {
        b[k] = v.clone();
    }
    b
}

/// 갱신이 끝날 때까지, 그리고 이어서 걸린 갱신(`pending`)도 끝날 때까지 기다려요.
fn settle(s: &Sandbox) {
    for _ in 0..6 {
        finish(&s.r);
        std::thread::sleep(Duration::from_millis(150));
    }
    finish(&s.r);
}

fn state_path(s: &Sandbox) -> std::path::PathBuf {
    s.r.atlassian_dir().join("state.json")
}

#[test]
fn atlassian_alone_builds_an_index_without_a_repo() {
    let s = atlassian_alone();
    s.sync(Target::default());
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    let src = sources(&s);
    assert_eq!(
        src.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["atlassian"]
    );
    assert_eq!(src["atlassian"]["issues"], 2);
    assert_eq!(src["atlassian"]["site"], "acme");
    assert_eq!(src["atlassian"]["initial_done"], true);
    let raw = s.engine.index.load().raw.clone();
    assert!(raw.iter().all(|c| c["src"] == "jira"), "{raw:?}");
    assert!(raw.iter().any(|c| {
        c["text"]
            .as_str()
            .unwrap()
            .starts_with("jira: ENG-1 · 배포 규칙")
    }));
    assert_eq!(s.r.atlassian_resume(), None);
}

#[test]
fn atlassian_joins_the_repo() {
    let s = Sandbox::new();
    s.sync(Target::default());
    add(
        &s,
        issue(
            "10",
            "ENG-1",
            "ENG",
            "배포 규칙",
            "금요일에는 배포하지 않아요.",
            60,
        ),
    );
    connect(&s, CLOUD);
    s.sync(Target::default());
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    let src = sources(&s);
    assert!(src.get("repo").is_some());
    assert_eq!(src["atlassian"]["issues"], 1);
}

#[test]
fn a_first_fetch_over_the_hourly_budget_waits_for_the_next_hour() {
    let s = atlassian_alone();
    // 이 시간의 예산을 이미 다 쓴 저장본이에요(같은 사이트).
    let hour = Utc::now().timestamp() / 3600;
    std::fs::create_dir_all(s.r.atlassian_dir()).unwrap();
    std::fs::write(
        state_path(&s),
        json!({"version": 1, "cloud": CLOUD, "points": {"used": 5000, "hour": hour}}).to_string(),
    )
    .unwrap();
    s.sync(Target::default());
    settle(&s);
    let src = sources(&s);
    assert_eq!(src["atlassian"]["issues"], 0, "{src}");
    assert_eq!(src["atlassian"]["initial_done"], false);
    // 바로 다시 돌지 않고 다음 정각(UTC)을 기다려요.
    let resume = s.r.atlassian_resume().expect("a time to carry on");
    assert_eq!(resume.timestamp(), (hour + 1) * 3600);
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["atlassian"], false, "{c}");
    assert_eq!(c["changed"], false, "{c}");
    // 정각이 지났어요: 예산이 새로 생기고, 30분 확인이 이어 받기를 걸어요.
    let mut st: Value =
        serde_json::from_str(&std::fs::read_to_string(state_path(&s)).unwrap()).unwrap();
    st["points"] = json!({"used": 0, "hour": 0});
    std::fs::write(state_path(&s), st.to_string()).unwrap();
    s.r.set_atlassian_resume(Some(Utc::now() - chrono::Duration::seconds(1)));
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["atlassian"], true, "{c}");
    assert_eq!(c["changed"], true, "{c}");
    s.sync(Target::default());
    let src = sources(&s);
    assert_eq!(src["atlassian"]["issues"], 2, "{src}");
    assert_eq!(src["atlassian"]["initial_done"], true);
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    assert_eq!(s.r.atlassian_resume(), None);
}

#[test]
fn a_rotated_token_does_not_start_a_sync() {
    let s = atlassian_alone();
    s.sync(Target::default());
    let before = s.meta();
    let (code, out) = post(&s, credentials(CLOUD, json!({"token": "token-renewed"})));
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"connected": true, "site": "acme"}));
    std::thread::sleep(Duration::from_millis(200));
    finish(&s.r);
    assert_eq!(s.meta(), before);
    assert!(s.r.atlassian_dir().join("items").exists());
}

#[test]
fn muting_a_project_drops_its_cards_and_lists_the_scopes() {
    let s = atlassian_alone();
    add(
        &s,
        issue(
            "20",
            "OPS-1",
            "OPS",
            "장애 대응",
            "새벽 장애는 운영팀이 받아요.",
            30,
        ),
    );
    s.sync(Target::default());
    assert!(s.texts().contains("새벽 장애는 운영팀이 받아요."));
    let (code, sc) = s.call("GET", "/atlassian/scopes", "");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        sc,
        json!({"projects": [
            {"id": "p-ENG", "key": "ENG", "name": "ENG 팀", "issues": 2},
            {"id": "p-OPS", "key": "OPS", "name": "OPS 팀", "issues": 1}],
            "spaces": []})
    );
    post(&s, credentials(CLOUD, json!({"muted_projects": ["OPS"]})));
    settle(&s);
    assert!(!s.texts().contains("새벽 장애는 운영팀이 받아요."));
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
}

#[test]
fn disconnecting_atlassian_alone_goes_back_to_the_seed_and_deletes_the_files() {
    let s = atlassian_alone();
    s.sync(Target::default());
    assert!(s.r.atlassian_dir().exists());
    let (code, out) = post(&s, json!({"token": null}));
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"connected": false, "site": null}));
    // 사용자가 끊으면 받은 것을 바로 지워요(동기화를 기다리지 않아요).
    assert!(!s.r.atlassian_dir().exists());
    settle(&s);
    assert!(s.engine.index.load().raw.is_empty());
    assert!(s.meta().get("sources").is_none());
    assert!(!s.r.atlassian_dir().exists());
}

#[test]
fn a_new_site_wipes_the_old_one_first() {
    let s = atlassian_alone();
    s.sync(Target::default());
    s.atlassian_api.resources.lock().unwrap().push(json!({
        "id": "cloud-2", "url": "https://other.atlassian.net", "name": "other",
        "scopes": ["read:jira-work"]}));
    *s.atlassian_api.jira.lock().unwrap() = vec![issue(
        "90",
        "NEW-1",
        "NEW",
        "다른 사이트",
        "두 번째 사이트예요.",
        10,
    )];
    let (code, out) = post(
        &s,
        json!({"token": "token2", "cloud": "cloud-2", "site": "other",
               "site_url": "https://other.atlassian.net"}),
    );
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"connected": true, "site": "other"}));
    settle(&s);
    let texts = s.texts();
    assert!(texts.contains("두 번째 사이트예요."));
    assert!(!texts.contains("금요일에는 배포하지 않아요."));
    let raw = s.engine.index.load().raw.clone();
    assert!(
        raw.iter()
            .all(|c| c["path"].as_str().unwrap().starts_with("jira/cloud-2/"))
    );
}

#[test]
fn a_saved_copy_of_another_site_is_wiped_when_the_app_hands_the_connection_back() {
    let s = atlassian_alone();
    s.sync(Target::default());
    // 엔진이 새로 떠서 연결이 없어요. 앱이 다른 사이트를 넘겨요.
    s.atlassian.set(None, None, None, None);
    let (code, _) = post(&s, credentials("cloud-9", json!({})));
    assert_eq!(code, StatusCode::OK);
    assert!(
        !s.r.atlassian_dir().join("items").exists(),
        "the old site's files go before any fetch"
    );
    settle(&s);
}

#[test]
fn atlassian_still_refreshes_when_github_fails() {
    let s = Sandbox::new();
    s.sync(Target::default());
    let before = s.engine.index.load().raw.len();
    add(
        &s,
        issue(
            "10",
            "ENG-1",
            "ENG",
            "배포 규칙",
            "금요일에는 배포하지 않아요.",
            60,
        ),
    );
    connect(&s, CLOUD);
    s.github.set_credentials("none", None);
    let st = s.run(Target::default());
    assert_eq!(st["state"], "failed");
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    assert_eq!(s.engine.index.load().raw.len(), before + 1);
    // 한 번 더 실패해도 Jira 조각이 겹치지 않아요.
    let st = s.run(Target::default());
    assert_eq!(st["state"], "failed");
    assert_eq!(s.engine.index.load().raw.len(), before + 1);
}

#[test]
fn a_first_fetch_refused_shows_in_the_index_status() {
    let s = Sandbox::new();
    s.sync(Target::default());
    s.atlassian_api.refuse.store(true, Ordering::SeqCst);
    connect(&s, CLOUD);
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    let (_, ix) = s.call("GET", "/index", "");
    let a = &ix["atlassian"];
    assert_eq!(a["connected"], true);
    assert_eq!(a["site"], "acme");
    assert_eq!(a["error_kind"], "key");
    assert_eq!(a["refused"], true);
    assert_eq!(
        a["issues"],
        Value::Null,
        "nothing was found, not zero issues"
    );
    assert!(a.get("token").is_none());
    assert!(s.atlassian.connected());
}

#[test]
fn a_quota_pause_shows_paused_until_and_keeps_the_saved_issues() {
    let s = atlassian_alone();
    s.sync(Target::default());
    let reset = Utc::now() + chrono::Duration::hours(1);
    *s.atlassian_api.quota.lock().unwrap() =
        Some(reset.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    add(
        &s,
        issue("12", "ENG-3", "ENG", "새 이슈", "아직 받지 못했어요.", 1),
    );
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    let (_, ix) = s.call("GET", "/index", "");
    let a = &ix["atlassian"];
    assert_eq!(a["error_kind"], "busy", "{a}");
    let until = DateTime::parse_from_rfc3339(a["paused_until"].as_str().expect("paused_until"))
        .unwrap()
        .with_timezone(&Utc);
    assert!(
        until >= reset - chrono::Duration::seconds(1),
        "{until} {reset}"
    );
    assert!(
        until <= reset + chrono::Duration::minutes(16),
        "{until} {reset}"
    );
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    assert!(!s.texts().contains("아직 받지 못했어요."));
    // 멈춰 있는 동안은 30분 확인이 Atlassian을 바뀜으로 치지 않아요.
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["atlassian"], false, "{c}");
}

#[test]
fn the_check_asks_atlassian_for_changes() {
    let s = atlassian_alone();
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["atlassian"], true, "never fetched: {c}");
    s.sync(Target::default());
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["atlassian"], false, "{c}");
    assert_eq!(c["changed"], false, "{c}");
    add(
        &s,
        issue("12", "ENG-3", "ENG", "새 이슈", "방금 고쳤어요.", 0),
    );
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["atlassian"], true, "{c}");
    assert_eq!(c["changed"], true);
}

#[test]
fn the_check_has_no_atlassian_key_when_not_connected() {
    let s = Sandbox::new();
    s.sync(Target::default());
    let (_, c) = s.call("POST", "/index/check", "");
    assert!(c.get("atlassian").is_none(), "{c}");
}

#[test]
fn an_email_makes_it_an_api_token() {
    let s = Sandbox::new();
    let (code, _) = post(&s, credentials(CLOUD, json!({"email": "a@b.c"})));
    assert_eq!(code, StatusCode::OK);
    assert!(s.r.sources().atlassian.uses_api_token());
    let (code, _) = post(&s, credentials(CLOUD, json!({})));
    assert_eq!(code, StatusCode::OK);
    assert!(!s.r.sources().atlassian.uses_api_token());
}

#[test]
fn bad_credentials_bodies_are_refused_without_echoing_them() {
    let s = Sandbox::new();
    for body in [
        json!({}),
        json!({"token": 3}),
        json!({"token": "a b"}),
        json!({"token": "café"}),
        json!({"token": "a", "cloud": 1}),
        json!({"token": "a", "site": ["café"]}),
        json!({"token": "a", "site_url": false}),
        json!({"token": "a", "muted_projects": [1]}),
        json!({"token": "a", "muted_spaces": "café"}),
        json!({"token": "a", "email": 5}),
        json!({"token": "a", "email": "café@x"}),
        json!({"token": "a", "email": "a@b.c"}),
    ] {
        let (code, out) = post(&s, body.clone());
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
        assert!(!out.to_string().contains("café"), "{out}");
    }
    let (code, _) = s.call("POST", "/atlassian/credentials", "not json");
    assert_eq!(code, StatusCode::BAD_REQUEST);
}

#[test]
fn the_index_status_before_any_connection() {
    let s = Sandbox::new();
    let (_, ix) = s.call("GET", "/index", "");
    assert_eq!(
        ix["atlassian"],
        json!({"connected": false, "site": null, "refused": false, "issues": null, "pages": null,
               "fetched_at": null, "initial_done": null, "error_kind": null, "lost_since": null,
               "paused_until": null})
    );
    let (code, sc) = s.call("GET", "/atlassian/scopes", "");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(sc, json!({"projects": [], "spaces": []}));
}

#[test]
fn the_index_status_after_a_fetch() {
    let s = atlassian_alone();
    s.sync(Target::default());
    let (_, ix) = s.call("GET", "/index", "");
    let a = &ix["atlassian"];
    assert_eq!(
        a.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "connected",
            "site",
            "refused",
            "issues",
            "pages",
            "fetched_at",
            "initial_done",
            "error_kind",
            "lost_since",
            "paused_until"
        ]
    );
    assert_eq!(a["connected"], true);
    assert_eq!(a["issues"], 2);
    assert_eq!(a["pages"], 0);
    assert_eq!(a["initial_done"], true);
    assert!(a["fetched_at"].is_string());
    assert_eq!(a["error_kind"], Value::Null);
    assert_eq!(a["paused_until"], Value::Null);
}

#[test]
fn clearing_the_search_data_removes_the_atlassian_files_too() {
    let s = Sandbox::new();
    s.sync(Target::default());
    add(
        &s,
        issue(
            "10",
            "ENG-1",
            "ENG",
            "배포 규칙",
            "금요일에는 배포하지 않아요.",
            60,
        ),
    );
    connect(&s, CLOUD);
    s.sync(Target::default());
    assert!(s.r.atlassian_dir().join("items").exists());
    assert!(s.r.clear());
    assert!(!s.r.atlassian_dir().exists());
    assert!(!s.texts().contains("금요일에는 배포하지 않아요."));
}

/// 검색 데이터 지우기는 끈 프로젝트·스페이스를 잊지 않아요: 다음 동기화에서도 끈 프로젝트의 이슈는 조각이 되지
/// 않아요(끈 것은 AI로 가면 안 돼요).
#[test]
fn clearing_the_search_data_keeps_the_mutes() {
    let s = atlassian_alone();
    add(
        &s,
        issue(
            "20",
            "OPS-1",
            "OPS",
            "장애 대응",
            "새벽 장애는 운영팀이 받아요.",
            30,
        ),
    );
    s.sync(Target::default());
    post(
        &s,
        credentials(
            CLOUD,
            json!({"muted_projects": ["OPS"], "muted_spaces": ["HR"]}),
        ),
    );
    settle(&s);
    assert!(!s.texts().contains("새벽 장애는 운영팀이 받아요."));
    assert!(s.r.clear());
    assert_eq!(
        atlassian::muted(&s.r.atlassian_dir()),
        (vec!["OPS".to_string()], vec!["HR".to_string()])
    );
    s.sync(Target::default());
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
    assert!(!s.texts().contains("새벽 장애는 운영팀이 받아요."));
}
