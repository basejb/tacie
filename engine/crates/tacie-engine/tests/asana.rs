//! The Asana connection (docs/32): Asana alone without a repo, a first fetch cut into shares together with a repo,
//! turning projects off, disconnecting, a different user, rejection and token refresh, `/index/check`,
//! `POST /asana/credentials`, `GET /asana/scopes`, `asana` in `GET /index`, and deleting search data.

mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use std::time::Duration;

use axum::http::StatusCode;
use chrono::Utc;
use common::sandbox::{Sandbox, finish};
use serde_json::{Value, json};
use tacie_engine::refresh::{Target, seed_meta};
use tacie_sources::asana;
use tacie_sources::testing::asana::Task;

const ME: &str = "me-1";

/// 워크스페이스 하나(`acme`)에 프로젝트 둘(`로드맵`, `운영`)과 task 하나씩이에요. task는 한 시간 전에 고쳤어요(받은
/// 뒤의 커서보다 앞이라 30분 확인이 바뀐 것으로 치지 않아요).
fn seed(s: &Sandbox) {
    let f = &s.asana_api;
    let hour_ago = f.now() - chrono::Duration::hours(1);
    f.workspace("w1", "acme");
    f.project("p1", "w1", "로드맵");
    f.project("p2", "w1", "운영");
    f.task(
        Task::new("1", "결제 웹훅")
            .notes("웹훅은 세 번까지 다시 보내요.")
            .in_project("p1")
            .modified(hour_ago),
    );
    f.task(
        Task::new("2", "장애 대응")
            .notes("새벽 장애는 운영팀이 받아요.")
            .in_project("p2")
            .modified(hour_ago),
    );
}

fn connect(s: &Sandbox, user: &str) {
    s.asana.set(Some("token".into()), Some(user.into()));
}

/// 저장소 없는 씨앗 색인에 Asana만 연결한 사람이에요.
fn asana_alone() -> Sandbox {
    let s = Sandbox::new();
    s.set_meta(seed_meta());
    s.github.set_credentials("none", None);
    seed(&s);
    connect(&s, ME);
    s
}

fn sources(s: &Sandbox) -> Value {
    Value::Object(s.meta())["sources"].clone()
}

fn post(s: &Sandbox, body: Value) -> (StatusCode, Value) {
    s.call("POST", "/asana/credentials", &body.to_string())
}

fn credentials(extra: Value) -> Value {
    let mut b = json!({"token": "token", "user_gid": ME});
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

fn index_asana(s: &Sandbox) -> Value {
    let (code, ix) = s.call("GET", "/index", "");
    assert_eq!(code, StatusCode::OK);
    ix["asana"].clone()
}

#[test]
fn asana_alone_builds_an_index() {
    let s = asana_alone();
    s.sync(Target::default());
    assert!(s.texts().contains("웹훅은 세 번까지 다시 보내요."));
    let src = sources(&s);
    assert_eq!(
        src.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["asana"]
    );
    assert_eq!(src["asana"]["tasks"], 2);
    assert_eq!(src["asana"]["workspaces"], 1);
    assert_eq!(src["asana"]["initial_done"], true);
    let raw = s.engine.index.load().raw.clone();
    assert!(raw.iter().all(|c| c["src"] == "asana"), "{raw:?}");
    assert!(
        raw.iter()
            .any(|c| c["text"].as_str().unwrap().starts_with("asana: 결제 웹훅")),
        "{raw:?}"
    );
    assert_eq!(s.r.asana_resume(), None);
}

#[test]
fn budget_more_resumes_and_github_is_not_blocked() {
    let s = Sandbox::new();
    s.sync(Target::default());
    let repo_chunks = s.engine.index.load().raw.len();
    assert!(repo_chunks > 0);
    seed(&s);
    connect(&s, ME);
    s.asana.set_budget(3);
    let started = Utc::now();
    s.sync(Target::default());
    // GitHub 쪽은 기다리지 않고 그대로 들어가요.
    let raw = s.engine.index.load().raw.clone();
    assert_eq!(
        raw.iter().filter(|c| c["src"] != "asana").count(),
        repo_chunks
    );
    assert_eq!(sources(&s)["asana"]["initial_done"], false);
    assert!(sources(&s).get("repo").is_some());
    // 바로 다시 돌지 않고 1분 뒤에 이어요(Asana 한도는 분 단위예요).
    let resume = s.r.asana_resume().expect("a time to carry on");
    assert!(resume > started, "{resume}");
    assert!(
        resume <= Utc::now() + chrono::Duration::seconds(61),
        "{resume}"
    );
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["asana"], false, "{c}");
    assert_eq!(c["changed"], false, "{c}");
    // 1분이 지날 때마다 30분 확인이 이어 받기를 걸어요.
    for _ in 0..60 {
        if s.r.asana_resume().is_none() {
            break;
        }
        s.r.set_asana_resume(Some(Utc::now() - chrono::Duration::seconds(1)));
        let (_, c) = s.call("POST", "/index/check", "");
        assert_eq!(c["asana"], true, "{c}");
        assert_eq!(c["changed"], true, "{c}");
        s.sync(Target::default());
    }
    assert_eq!(s.r.asana_resume(), None);
    let src = sources(&s);
    assert_eq!(src["asana"]["initial_done"], true, "{src}");
    assert_eq!(src["asana"]["tasks"], 2, "{src}");
    let texts = s.texts();
    assert!(texts.contains("웹훅은 세 번까지 다시 보내요."));
    assert!(texts.contains("새벽 장애는 운영팀이 받아요."));
    let raw = s.engine.index.load().raw.clone();
    assert_eq!(
        raw.iter().filter(|c| c["src"] != "asana").count(),
        repo_chunks
    );
}

#[test]
fn muting_hides_and_scopes_lists() {
    let s = asana_alone();
    s.sync(Target::default());
    assert!(s.texts().contains("새벽 장애는 운영팀이 받아요."));
    let (code, sc) = s.call("GET", "/asana/scopes", "");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        sc,
        json!({"workspaces": [{"gid": "w1", "name": "acme", "tasks": 2, "blocked": false}],
               "projects": [{"gid": "p1", "workspace": "w1", "name": "로드맵", "tasks": 1},
                            {"gid": "p2", "workspace": "w1", "name": "운영", "tasks": 1}]})
    );
    let (code, out) = post(&s, credentials(json!({"muted_projects": ["p2"]})));
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"connected": true}));
    settle(&s);
    assert!(!s.texts().contains("새벽 장애는 운영팀이 받아요."));
    assert!(s.texts().contains("웹훅은 세 번까지 다시 보내요."));
    // 끈 워크스페이스는 조각에서 모두 빠져요.
    post(&s, credentials(json!({"muted_workspaces": ["w1"]})));
    settle(&s);
    assert!(!s.texts().contains("웹훅은 세 번까지 다시 보내요."));
}

#[test]
fn disconnect_wipes_raw() {
    let s = asana_alone();
    s.sync(Target::default());
    assert!(s.r.asana_dir().exists());
    let (code, out) = post(&s, json!({"token": null}));
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"connected": false}));
    // 사용자가 끊으면 받은 것을 바로 지워요(동기화를 기다리지 않아요).
    assert!(!s.r.asana_dir().exists());
    settle(&s);
    assert!(s.engine.index.load().raw.is_empty());
    assert!(s.meta().get("sources").is_none());
    assert!(!s.r.asana_dir().exists());
}

#[test]
fn other_user_wipes_raw() {
    let s = asana_alone();
    s.sync(Target::default());
    let marker = s.r.asana_dir().join("marker");
    std::fs::write(&marker, "old user").unwrap();
    // 연결된 채로 다른 사용자가 와요.
    let (code, out) = post(&s, json!({"token": "token-b", "user_gid": "me-2"}));
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"connected": true}));
    settle(&s);
    assert!(!marker.exists(), "the old user's files go");
    assert_eq!(
        asana::stored_user(&s.r.asana_dir()).as_deref(),
        Some("me-2")
    );
    // 엔진이 새로 떠서 연결이 없어요. 앱이 저장본과 다른 사용자를 넘겨요.
    std::fs::write(&marker, "me-2's").unwrap();
    s.asana.set(None, None);
    let (code, _) = post(&s, credentials(json!({})));
    assert_eq!(code, StatusCode::OK);
    settle(&s);
    assert!(!marker.exists(), "the saved copy of another user goes");
    assert_eq!(asana::stored_user(&s.r.asana_dir()).as_deref(), Some(ME));
    // 같은 사용자를 다시 넘기면 지우지 않아요.
    std::fs::write(&marker, "kept").unwrap();
    s.asana.set(None, None);
    post(&s, credentials(json!({})));
    settle(&s);
    assert!(marker.exists());
}

#[test]
fn refused_sets_flag_and_rotation_restarts() {
    let s = asana_alone();
    s.sync(Target::default());
    // 토큰만 바뀐 것은 동기화를 걸지 않아요.
    let before = s.meta();
    let (code, out) = post(&s, credentials(json!({"token": "token-renewed"})));
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"connected": true}));
    std::thread::sleep(Duration::from_millis(200));
    finish(&s.r);
    assert_eq!(s.meta(), before);
    // 받는 도중 401이에요: 앱에 갱신을 부탁하고, 저장본은 그대로 찾아요.
    s.asana_api.fail_next(401);
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    let a = index_asana(&s);
    assert_eq!(a["refused"], true, "{a}");
    assert_eq!(a["error_kind"], "key", "{a}");
    // 경고가 난 갱신은 meta를 그대로 둬서 `lost_since`는 저장본에만 적혀요(다음에 조각이 바뀌면 meta로 가요).
    let st: Value =
        serde_json::from_str(&std::fs::read_to_string(s.r.asana_dir().join("state.json")).unwrap())
            .unwrap();
    assert!(st["lost_since"].is_string(), "{st}");
    assert!(s.texts().contains("웹훅은 세 번까지 다시 보내요."));
    // 거절 뒤 새 토큰이 오면 다시 받아요.
    post(&s, credentials(json!({"token": "token-3"})));
    let a = index_asana(&s);
    assert_eq!(a["lost_since"], Value::Null, "untried token: {a}");
    settle(&s);
    let st: Value =
        serde_json::from_str(&std::fs::read_to_string(s.r.asana_dir().join("state.json")).unwrap())
            .unwrap();
    assert!(st["lost_since"].is_null(), "{st}");
    let a = index_asana(&s);
    assert_eq!(a["refused"], false, "{a}");
    assert_eq!(a["error_kind"], Value::Null, "{a}");
    assert_eq!(a["lost_since"], Value::Null, "{a}");
}

#[test]
fn check_reports_asana_change() {
    let s = Sandbox::new();
    s.sync(Target::default());
    let (_, c) = s.call("POST", "/index/check", "");
    assert!(c.get("asana").is_none(), "not connected: {c}");
    s.set_meta(seed_meta());
    s.github.set_credentials("none", None);
    seed(&s);
    connect(&s, ME);
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["asana"], true, "never fetched: {c}");
    s.sync(Target::default());
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["asana"], false, "{c}");
    assert_eq!(c["changed"], false, "{c}");
    s.asana_api.task(
        Task::new("3", "새 task")
            .notes("방금 고쳤어요.")
            .in_project("p1"),
    );
    let (_, c) = s.call("POST", "/index/check", "");
    assert_eq!(c["asana"], true, "{c}");
    assert_eq!(c["changed"], true, "{c}");
}

#[test]
fn bad_bodies_are_400() {
    let s = Sandbox::new();
    for body in [
        json!({}),
        json!({"token": 3}),
        json!({"token": "a b"}),
        json!({"token": "café"}),
        json!({"token": "a", "user_gid": 1}),
        json!({"token": "a", "user_gid": "café"}),
        json!({"token": "a", "muted_workspaces": [1]}),
        json!({"token": "a", "muted_projects": "café"}),
    ] {
        let (code, out) = post(&s, body.clone());
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
        assert!(!out.to_string().contains("café"), "{out}");
    }
    let (code, _) = s.call("POST", "/asana/credentials", "not json");
    assert_eq!(code, StatusCode::BAD_REQUEST);
}

#[test]
fn index_status_block() {
    let s = Sandbox::new();
    assert_eq!(
        index_asana(&s),
        json!({"connected": false, "refused": false, "workspaces": null, "tasks": null,
               "fetched_at": null, "initial_done": null, "error_kind": null, "lost_since": null,
               "blocked_workspaces": null})
    );
    let (code, sc) = s.call("GET", "/asana/scopes", "");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(sc, json!({"workspaces": [], "projects": []}));

    let s = asana_alone();
    s.asana_api.workspace("w2", "beta");
    s.asana_api.block_workspace("w2");
    s.sync(Target::default());
    let a = index_asana(&s);
    assert_eq!(
        a.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "connected",
            "refused",
            "workspaces",
            "tasks",
            "fetched_at",
            "initial_done",
            "error_kind",
            "lost_since",
            "blocked_workspaces"
        ]
    );
    assert_eq!(a["connected"], true);
    assert_eq!(a["refused"], false);
    assert_eq!(a["workspaces"], 1);
    assert_eq!(a["tasks"], 2);
    assert!(a["fetched_at"].is_string());
    assert_eq!(a["initial_done"], true);
    assert_eq!(a["error_kind"], Value::Null);
    assert_eq!(a["lost_since"], Value::Null);
    assert_eq!(a["blocked_workspaces"], json!(["beta"]));
    assert!(a.get("token").is_none());
    // 고르기 시트는 막힌 워크스페이스를 `blocked`로 알아요.
    let (_, sc) = s.call("GET", "/asana/scopes", "");
    assert_eq!(
        sc["workspaces"],
        json!([{"gid": "w1", "name": "acme", "tasks": 2, "blocked": false},
               {"gid": "w2", "name": "beta", "tasks": 0, "blocked": true}])
    );
}

#[test]
fn a_first_fetch_refused_shows_in_the_index_status() {
    let s = Sandbox::new();
    s.sync(Target::default());
    seed(&s);
    s.asana_api.fail_next(401);
    connect(&s, ME);
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    let a = index_asana(&s);
    assert_eq!(a["connected"], true);
    assert_eq!(a["error_kind"], "key", "{a}");
    assert_eq!(a["refused"], true);
    assert_eq!(a["tasks"], Value::Null, "nothing was found, not zero tasks");
}

#[test]
fn asana_still_refreshes_when_github_fails() {
    let s = Sandbox::new();
    s.sync(Target::default());
    let before = s.engine.index.load().raw.len();
    seed(&s);
    connect(&s, ME);
    s.github.set_credentials("none", None);
    let st = s.run(Target::default());
    assert_eq!(st["state"], "failed");
    assert!(s.texts().contains("웹훅은 세 번까지 다시 보내요."));
    assert_eq!(s.engine.index.load().raw.len(), before + 2);
    // 한 번 더 실패해도 Asana 조각이 겹치지 않아요.
    let st = s.run(Target::default());
    assert_eq!(st["state"], "failed");
    assert_eq!(s.engine.index.load().raw.len(), before + 2);
}

#[test]
fn clear_forgets_asana() {
    let s = Sandbox::new();
    s.sync(Target::default());
    seed(&s);
    connect(&s, ME);
    s.sync(Target::default());
    assert!(s.r.asana_dir().join("state.json").exists());
    assert!(s.r.clear());
    assert!(!s.r.asana_dir().exists());
    assert!(!s.texts().contains("웹훅은 세 번까지 다시 보내요."));
}

/// 검색 데이터 지우기는 끈 워크스페이스·프로젝트를 잊지 않아요: 다음 동기화가 끈 워크스페이스를 묻지 않고, 끈
/// 프로젝트의 task는 조각이 되지 않아요(끈 것은 AI로 가면 안 돼요).
#[test]
fn clear_keeps_the_mutes() {
    let s = Sandbox::new();
    s.sync(Target::default());
    seed(&s);
    s.asana_api.workspace("w2", "beta");
    s.asana_api.project("p3", "w2", "비밀");
    s.asana_api.task(
        Task::new("9", "인사 평가")
            .notes("평가 기준은 비공개예요.")
            .in_project("p3")
            .workspace("w2"),
    );
    connect(&s, ME);
    s.sync(Target::default());
    assert!(s.texts().contains("평가 기준은 비공개예요."));
    post(
        &s,
        credentials(json!({"muted_workspaces": ["w2"], "muted_projects": ["p2"]})),
    );
    settle(&s);
    assert!(!s.texts().contains("새벽 장애는 운영팀이 받아요."));
    assert!(s.r.clear());
    assert_eq!(
        asana::muted(&s.r.asana_dir()),
        (vec!["w2".to_string()], vec!["p2".to_string()])
    );
    s.asana_api.clear_calls();
    s.sync(Target::default());
    let texts = s.texts();
    assert!(texts.contains("웹훅은 세 번까지 다시 보내요."), "{texts}");
    assert!(!texts.contains("새벽 장애는 운영팀이 받아요."), "{texts}");
    assert!(!texts.contains("평가 기준은 비공개예요."), "{texts}");
    let calls = s.asana_api.calls();
    assert!(!calls.is_empty());
    assert!(
        calls.iter().all(|c| !c.contains("w2") && !c.contains("p3")),
        "{calls:?}"
    );
}

fn lost_since(s: &Sandbox) -> Value {
    let st: Value =
        serde_json::from_str(&std::fs::read_to_string(s.r.asana_dir().join("state.json")).unwrap())
            .unwrap();
    st["lost_since"].clone()
}

/// 끊긴 연결에 새 토큰이 오면(다시 연결) 거절 뒤 30분 안이라도 바로 받아요.
#[test]
fn reconnect_after_lost_syncs_inside_the_retry_window() {
    let s = asana_alone();
    s.sync(Target::default());
    s.asana_api.fail_next(401);
    s.run(Target::default());
    assert!(lost_since(&s).is_string());
    // 첫 새 토큰은 거절 뒤 다시 받기(30분에 한 번)를 써요.
    post(&s, credentials(json!({"token": "token-3"})));
    settle(&s);
    assert!(lost_since(&s).is_null());
    // 또 끊겼어요. 다시 연결하면 30분 안이라도 받아요.
    s.asana_api.fail_next(401);
    s.run(Target::default());
    assert!(lost_since(&s).is_string());
    post(&s, credentials(json!({"token": "token-4"})));
    settle(&s);
    assert!(lost_since(&s).is_null(), "{}", lost_since(&s));
}
