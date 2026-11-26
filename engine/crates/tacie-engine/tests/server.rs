//! Tests the server skeleton without a model: requests go straight into `router()` with the `mini` index (no port
//! is opened).
//! The embedder, reranker and answer model are fakes (`common/fakes.rs`). Meeting session routes are tested with
//! the `CHUNKS` index (`common/fixture.rs`) and a fake gate and transcriber.

mod common {
    pub mod fake_openrouter;
    pub mod fakes;
    pub mod fixture;
}

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::fakes::{
    FakeAnswerer, FakeEmbed, FakeGater, FakeNoteWriter, FakeReranker, FakeTranscriber, FakeVoice,
    fake_gate, services,
};
use common::fixture::{engine_with, index, no_github, refresher_for};
use http_body_util::BodyExt;
use serde_json::{Map, Value, json};
use tacie_engine::errors::{self, Kind};
use tacie_engine::jev::Jev;
use tacie_engine::keys::Keys;
use tacie_engine::pipeline::Engine;
use tacie_engine::refresh::describe;
use tacie_engine::routed_stt::{LoadModel, LocalModel, RoutedStt};
use tacie_engine::server::{AppState, GH_MODE_OFF, MAX_AUDIO_BYTES, SEND_AI_KEYS, router};
use tacie_engine::services::Services;
use tacie_engine::settings::{self, key_states};
use tacie_index::{Index, Mask, RawChunk};
use tacie_sources::github::GitHub;
use tacie_sources::testing::github::{FakeGhToken, FakeGitHub, PanicGhToken, Reply};
use tower::ServiceExt;

const MINI: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tacie-index/tests/fixtures/mini"
);
const TOKEN: &str = "test-token";

/// `jev_provider`의 키 파일만 둔 설정 폴더로 Jev 클라이언트를 만들어요(None이면 키 파일 없음).
fn state(env: &[(&str, &str)], jev_provider: Option<&str>) -> Arc<AppState> {
    state_with(env, jev_provider, &Arc::new(FakeAnswerer::ok("답 [1]")))
}

fn state_with(
    env: &[(&str, &str)],
    jev_provider: Option<&str>,
    answerer: &Arc<FakeAnswerer>,
) -> Arc<AppState> {
    state_from(
        env,
        jev_provider,
        services(Arc::new(FakeReranker::all(0.9)), answerer.clone()),
    )
}

/// 장부 모델만 가짜로 바꿔 끼운 서버예요.
fn notes_state(writer: &Arc<FakeNoteWriter>) -> Arc<AppState> {
    state_from(
        &[],
        None,
        Services {
            note_writer: writer.clone(),
            ..services(
                Arc::new(FakeReranker::all(0.9)),
                Arc::new(FakeAnswerer::ok("답 [1]")),
            )
        },
    )
}

fn state_from(
    env: &[(&str, &str)],
    jev_provider: Option<&str>,
    services: Services,
) -> Arc<AppState> {
    state_with_stt(env, jev_provider, services, None)
}

fn state_with_stt(
    env: &[(&str, &str)],
    jev_provider: Option<&str>,
    services: Services,
    stt: Option<Arc<RoutedStt>>,
) -> Arc<AppState> {
    let env: HashMap<String, String> = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let index = Index::open(Path::new(MINI), Mask::Plans, &Default::default()).unwrap();
    let settings = settings::resolve(&env);
    // Jev와 엔진(/config의 키 상태)이 같은 폴더를 봐요. 엔진이 사는 동안 읽으니 지우지 않고 남겨요.
    // 남는 폴더는 $TMPDIR가 아니라 target 아래(CARGO_TARGET_TMPDIR)에 쌓여요.
    let config = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap()
        .keep();
    if let Some(p) = jev_provider {
        std::fs::create_dir_all(config.join(p)).unwrap();
        std::fs::write(config.join(p).join("config.json"), r#"{"apiKey": "x"}"#).unwrap();
    }
    let last = Arc::new(Mutex::new(HashMap::new()));
    let jev = Jev::from_config(&config, &settings, &env, last.clone());
    let engine = Engine::new(
        index,
        Arc::new(FakeEmbed::default()),
        services,
        settings,
        Arc::new(jev),
        last,
    )
    .with_config_dir(config)
    .with_stt(stt);
    Arc::new(AppState {
        engine: Arc::new(engine),
        token: TOKEN.to_string(),
        github: no_github(),
        refresher: None,
        meeting_dir: None,
    })
}

/// (상태 코드, Content-Type, JSON 본문)
async fn get(state: Arc<AppState>, path: &str, auth: Option<&str>) -> (StatusCode, String, Value) {
    let mut req = Request::get(path);
    if let Some(a) = auth {
        req = req.header(header::AUTHORIZATION, a);
    }
    let res = router(state)
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ctype = res
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        ctype.unwrap_or_default(),
        serde_json::from_slice(&bytes).unwrap(),
    )
}

fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

#[tokio::test]
async fn missing_or_wrong_token_is_401_json() {
    for auth in [
        None,
        Some("Bearer wrong"),
        Some(TOKEN),
        Some("bearer test-token"),
        Some("Bearer test-token "),
    ] {
        let (status, ctype, body) = get(state(&[], None), "/health", auth).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "auth {auth:?}");
        assert_eq!(ctype, "application/json; charset=utf-8");
        assert_eq!(body, json!({"error": "unauthorized"}));
    }
    // 토큰을 먼저 봐요: 없는 경로도 토큰이 틀리면 401이에요.
    let (status, _, body) = get(state(&[], None), "/nope", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, json!({"error": "unauthorized"}));
}

#[tokio::test]
async fn unknown_path_is_404_not_found() {
    for path in ["/nope", "/health/", "/"] {
        let (status, ctype, body) = get(state(&[], None), path, Some(&bearer())).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "path {path}");
        assert_eq!(ctype, "application/json; charset=utf-8");
        assert_eq!(body, json!({"error": "not found"}));
    }
    // 메서드가 다르면 404예요(405가 아니에요).
    let req = Request::post("/health")
        .header(header::AUTHORIZATION, bearer())
        .body(Body::empty())
        .unwrap();
    let res = router(state(&[], None)).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        json!({"error": "not found"})
    );
}

#[tokio::test]
async fn health_lists_index_meta_and_mask() {
    let (status, ctype, body) = get(state(&[], None), "/health", Some(&bearer())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ctype, "application/json; charset=utf-8");
    assert_eq!(
        body,
        json!({"ready": true, "jev": null, "mask": "plans",
               "index": {"chunks": 6, "repo": "basejb/tacie", "commit": "abc1234",
                         "built_at": "2026-10-25T00:00:00+09:00"}})
    );
    // 필드 순서도 고정이에요.
    assert_eq!(keys(&body), ["ready", "jev", "mask", "index"]);
    assert_eq!(
        keys(&body["index"]),
        ["chunks", "repo", "commit", "built_at"]
    );

    let (_, _, body) = get(state(&[], Some("typesafe")), "/health", Some(&bearer())).await;
    assert_eq!(body["jev"], "typesafe");
}

#[tokio::test]
async fn config_keys_read_the_engine_config_dir() {
    // 설정 폴더에 openrouter 키만 있으면 keys도 그렇게 말해요(~/.config를 보지 않아요).
    let config = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(config.path().join("openrouter")).unwrap();
    std::fs::write(config.path().join("openrouter/config.json"), "{}").unwrap();
    let engine = Engine::new(
        Index::open(Path::new(MINI), Mask::Plans, &Default::default()).unwrap(),
        Arc::new(FakeEmbed::default()),
        services(
            Arc::new(FakeReranker::all(0.9)),
            Arc::new(FakeAnswerer::ok("답 [1]")),
        ),
        settings::resolve(&HashMap::new()),
        Arc::new(Jev::from_config(
            config.path(),
            &settings::resolve(&HashMap::new()),
            &HashMap::new(),
            Default::default(),
        )),
        Arc::new(Mutex::new(HashMap::new())),
    )
    .with_config_dir(config.path().to_path_buf());
    let state = Arc::new(AppState {
        engine: Arc::new(engine),
        token: TOKEN.to_string(),
        github: no_github(),
        refresher: None,
        meeting_dir: None,
    });
    let (_, _, body) = get(state, "/config", Some(&bearer())).await;
    let states: Vec<(&str, &str)> = body["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| (k["name"].as_str().unwrap(), k["state"].as_str().unwrap()))
        .collect();
    assert_eq!(
        states,
        [
            ("openrouter", "ok"),
            ("typesafe", "missing"),
            ("slack", "missing")
        ]
    );
}

#[tokio::test]
async fn config_applies_env_overrides_and_defaults() {
    let env = [
        ("TACIE_GATE_THR", "0.65"),
        ("TACIE_END_THR", "0.9"),
        ("TACIE_STT_MODEL", "  openai/whisper  "),
        ("TACIE_ANSWER_MODEL", "   "),
        ("TACIE_LOG_TEXT", "1"),
    ];
    let (status, ctype, body) = get(state(&env, None), "/config", Some(&bearer())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ctype, "application/json; charset=utf-8");
    assert_eq!(
        keys(&body),
        [
            "models",
            "model_defaults",
            "thresholds",
            "threshold_defaults",
            "keys",
            "log_text"
        ]
    );
    assert_eq!(
        body["models"],
        json!({"stt": "openai/whisper", "answer": "anthropic/claude-haiku-5.5", "jev": null})
    );
    assert_eq!(
        body["model_defaults"],
        json!({"stt": "elevenlabs/scribe-v2", "answer": "anthropic/claude-haiku-5.5", "jev": null})
    );
    assert_eq!(
        body["thresholds"],
        json!({"gate": 0.65, "show": 0.8, "end": 0.9})
    );
    assert_eq!(
        body["threshold_defaults"],
        json!({"gate": 0.7, "show": 0.8, "end": 0.8})
    );
    // 기준 순서도 고정이에요(Map의 ==는 순서를 보지 않아요).
    assert_eq!(keys(&body["thresholds"]), ["gate", "show", "end"]);
    assert_eq!(keys(&body["threshold_defaults"]), ["gate", "show", "end"]);
    assert_eq!(body["log_text"], true);
    let names: Vec<&str> = body["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["openrouter", "typesafe", "slack"]);

    // 숫자가 아니거나 0–1 밖이면 기본값이에요. Jev 모델은 공급자의 기본값, 덮어쓰면 그 값이에요.
    let env = [
        ("TACIE_GATE_THR", "abc"),
        ("TACIE_SHOW_THR", "1.5"),
        ("TACIE_END_THR", "2"),
    ];
    let (_, _, body) = get(state(&env, Some("openrouter")), "/config", Some(&bearer())).await;
    assert_eq!(
        body["thresholds"],
        json!({"gate": 0.7, "show": 0.8, "end": 0.8})
    );
    assert_eq!(body["models"]["jev"], "typesafe/jev-1.13");
    assert_eq!(body["model_defaults"]["jev"], "typesafe/jev-1.13");
    assert_eq!(body["log_text"], false);

    let env = [("TACIE_JEV_MODEL", "typesafe/jev-2")];
    let (_, _, body) = get(state(&env, Some("openrouter")), "/config", Some(&bearer())).await;
    assert_eq!(body["models"]["jev"], "typesafe/jev-2");
    assert_eq!(body["model_defaults"]["jev"], "typesafe/jev-1.13");
}

#[test]
fn key_states_report_missing_then_refused() {
    let dir = tempfile::tempdir().unwrap();
    let last = Mutex::new(HashMap::new());
    let keys = Keys::from_files(dir.path());
    let states = |provider| key_states(dir.path(), &keys, &last.lock().unwrap(), provider);
    let missing: Vec<Value> = ["openrouter", "typesafe", "slack"]
        .iter()
        .map(|n| json!({"name": n, "path": format!("~/.config/{n}/config.json"), "state": "missing", "in_app": false}))
        .collect();
    assert_eq!(states(None), missing);

    std::fs::create_dir_all(dir.path().join("openrouter")).unwrap();
    std::fs::write(dir.path().join("openrouter/config.json"), "{}").unwrap();
    assert_eq!(states(None)[0]["state"], "ok");

    // 답 모델이 키를 거절하면 그 키 파일이 refused예요. 다시 200이 오면(clear) ok로 돌아와요.
    let e = errors::from_status("answer", 401, "no", &last);
    assert_eq!(e.kind, Kind::Key);
    assert_eq!(e.message, "answer 401: no");
    assert_eq!(states(None)[0]["state"], "refused");
    assert_eq!(states(None)[1]["state"], "missing");
    errors::clear("answer", &last);
    assert_eq!(states(None)[0]["state"], "ok");

    // 크레딧은 credit, 키 거절이 함께 있으면 refused가 앞서요. 429는 기억하지 않아요.
    errors::from_status("stt", 402, "", &last);
    assert_eq!(states(None)[0]["state"], "credit");
    assert_eq!(
        errors::from_status("ledger", 429, "", &last).kind,
        Kind::Busy
    );
    assert_eq!(states(None)[0]["state"], "credit");
    errors::from_status("ledger", 403, "", &last);
    assert_eq!(states(None)[0]["state"], "refused");

    // jev 거절은 jev를 부르는 공급자의 키 파일로 가요.
    errors::clear("stt", &last);
    errors::clear("ledger", &last);
    errors::from_status("jev", 401, "", &last);
    assert_eq!(states(None)[0]["state"], "ok");
    assert_eq!(states(Some("openrouter"))[0]["state"], "refused");
    std::fs::create_dir_all(dir.path().join("typesafe")).unwrap();
    std::fs::write(dir.path().join("typesafe/config.json"), "{}").unwrap();
    let s = states(Some("typesafe"));
    assert_eq!(
        (s[0]["state"].as_str(), s[1]["state"].as_str()),
        (Some("ok"), Some("refused"))
    );
    assert_eq!(errors::from_status("jev", 500, "", &last).kind, Kind::Down);

    // 앱이 넘긴 키도 키예요: in_app이 true예요. 키 파일을 읽지 않는 엔진(제품)은 파일이 있어도 missing이에요.
    let product = Keys::new(None);
    let states = |keys: &Keys| key_states(dir.path(), keys, &HashMap::new(), None);
    assert_eq!(states(&product)[0]["state"], "missing");
    product.set("openrouter", Some("k"));
    let s = states(&product);
    assert_eq!(
        (&s[0]["state"], &s[0]["in_app"]),
        (&json!("ok"), &json!(true))
    );
    assert_eq!(
        (&s[1]["state"], &s[1]["in_app"]),
        (&json!("missing"), &json!(false))
    );
}

/// POST에 JSON 글을 실어 보내요. (상태 코드, JSON 본문)
async fn post(state: Arc<AppState>, path: &str, body: &str) -> (StatusCode, Value) {
    let req = Request::post(path)
        .header(header::AUTHORIZATION, bearer())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = router(state).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn ask_requires_text() {
    for body in [r#"{"text": "  "}"#, "{}", r#"{"text": null}"#, ""] {
        let (status, out) = post(state(&[], None), "/ask", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body {body:?}");
        assert_eq!(out, json!({"error": "text is required"}));
    }
    let (status, out) = post(state(&[], None), "/ask", "{not json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(out, json!({"error": "body is not JSON"}));
    // 묻기는 POST만이에요.
    let (status, _, out) = get(state(&[], None), "/ask", Some(&bearer())).await;
    assert_eq!(
        (status, out),
        (StatusCode::NOT_FOUND, json!({"error": "not found"}))
    );
}

#[tokio::test]
async fn answer_filters_ids_to_allowed_and_first_three() {
    // mini: 2는 plans 아래라 가렸고, 4는 비공개 슬랙 채널이라 이 사람(viewer 없음)에게 닫혔어요.
    let answerer = Arc::new(FakeAnswerer::ok("셋 다 봐요. [1][2][3]"));
    let body = json!({"text": " 색인은 언제 갱신해요? ", "context": ["a", "b", "c", "d"],
                      "chunk_ids": [2, 4, 99, -1, "0", 1.5, 0, 1, 3, 5]});
    let (status, out) = post(
        state_with(&[], None, &answerer),
        "/answer",
        &body.to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["answer"], "셋 다 봐요. [1][2][3]");
    assert_eq!(out["cited"], json!([0, 1, 3]));
    let (question, context, passages) = answerer.calls().remove(0);
    assert_eq!(question, "색인은 언제 갱신해요?");
    assert_eq!(context, ["b", "c", "d"]);
    let paths: Vec<&str> = passages
        .iter()
        .map(|p| p["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        [
            "engine/index.py",
            "docs/05_자료_연결.md",
            "PR #12 · 색인 갱신"
        ]
    );

    for body in [
        json!({"text": "x", "chunk_ids": [999]}),
        json!({"text": "x", "chunk_ids": [2, 4]}),
        json!({"text": "x", "chunk_ids": []}),
        json!({"text": "x"}),
        json!({"text": "  ", "chunk_ids": [0]}),
        json!({"chunk_ids": [0]}),
    ] {
        let (status, out) = post(state(&[], None), "/answer", &body.to_string()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body {body}");
        assert_eq!(out, json!({"error": "text and chunk_ids are required"}));
    }
}

#[tokio::test]
async fn notes_from_lines_route() {
    // 앱이 들고 있던 회의 글로 장부를 다시 만들어요(그사이 엔진이 다시 떴을 수 있어요).
    let writer = Arc::new(FakeNoteWriter::ok("{\"items\": []}"));
    let body = json!({"asked": [], "lines": [
        {"id": "s1u1", "speaker": "me", "text": "배포는 목요일로 해요"}, {"id": "s1u2", "speaker": "them", "text": "네"}]});
    let (status, out) = post(notes_state(&writer), "/ledger", &body.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["items"], json!([]));
    let seen = writer.calls();
    assert!(seen[0].contains("[s1u1] 나: 배포는 목요일로 해요") && !seen[0].contains("[s1u2]"));
    let (status, _) = post(notes_state(&writer), "/ledger", r#"{"lines": "x"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn ledger_route_reads_ids_speakers_and_asked() {
    // id는 글이 돼요(숫자 7은 "7", 없으면 "None"). me·them은 나·상대이고, 다른 화자는 앱이 보인
    // 이름 그대로예요. 빈 글, 글이 아닌 것, 맞장구는 빼요. asked는 글만 봐요.
    let raw = json!({"items": [
        {"kind": "decision", "text": "목요일에 배포해요.", "utterance_ids": ["7"]},
        {"kind": "commitment", "text": "제가 볼게요.", "utterance_ids": ["s1u3"], "owner": "x", "due": null},
        {"kind": "open_question", "text": "끝은 언제예요?", "utterance_ids": ["None"]}]})
    .to_string();
    let writer = Arc::new(FakeNoteWriter::ok(&raw));
    let body = json!({"asked": ["7", 7, null, "s1u9"], "lines": [
        {"id": 7, "speaker": "them", "text": "목요일에 배포해요"},
        {"id": "s1u2", "text": "  "},
        {"id": "s1u3", "speaker": "x", "text": "그건 제가 볼게요"},
        {"id": "s1u4", "text": 5},
        {"id": "s1u5", "speaker": "me", "text": "아 네네."},
        {"speaker": "me", "text": "끝\n이에요"},
        {"id": true, "speaker": null, "text": "참"}]});
    let (status, out) = post(notes_state(&writer), "/ledger", &body.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        writer.calls(),
        [
            "[7] 상대: 목요일에 배포해요\n[s1u3] x: 그건 제가 볼게요\n[None] 나: 끝\n이에요\n[True] 화자: 참"
        ]
    );
    // 참석자는 이름이 있는 화자예요(나온 순서, 한 번씩). 없는 화자의 "화자"는 이름이 아니에요.
    assert_eq!(writer.attendee_calls(), [vec!["상대", "x", "나"]]);
    assert_eq!(
        keys(&out),
        [
            "kind",
            "kind_source",
            "title",
            "summary",
            "topics",
            "items",
            "dropped",
            "ms",
            "cost"
        ]
    );
    let texts: Vec<&str> = out["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["제가 볼게요.", "끝은 언제예요?"]);
    assert_eq!(out["items"][0]["owner"], "x");
    assert_eq!(out["dropped"], 1);

    // 남는 줄이 없으면 모델을 부르지 않아요.
    let writer = Arc::new(FakeNoteWriter::ok(&raw));
    for body in [
        r#"{"lines": []}"#,
        r#"{"lines": [{"id": "s1u1", "speaker": "me", "text": "네네"}]}"#,
    ] {
        let (status, out) = post(notes_state(&writer), "/ledger", body).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            out,
            json!({"kind": "meeting", "kind_source": "auto", "title": null, "summary": [], "topics": [], "items": [],
                   "dropped": 0, "ms": 0, "cost": 0})
        );
    }
    assert!(writer.calls().is_empty());
}

#[tokio::test]
async fn ledger_route_rejects_what_is_not_a_list_of_lines() {
    let writer = Arc::new(FakeNoteWriter::ok("{\"items\": []}"));
    for body in [
        r#"{"lines": "x"}"#,
        "{}",
        "",
        r#"{"lines": null}"#,
        r#"{"lines": {"id": "s1u1", "text": "a"}}"#,
        r#"{"lines": [{"id": "s1u1", "text": "a"}, "b"]}"#,
    ] {
        let (status, out) = post(notes_state(&writer), "/ledger", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body {body:?}");
        assert_eq!(
            out,
            json!({"error": "lines must be a list of {id, speaker, text}"})
        );
    }
    let (status, out) = post(notes_state(&writer), "/ledger", "{not json").await;
    assert_eq!(
        (status, out),
        (
            StatusCode::BAD_REQUEST,
            json!({"error": "body is not JSON"})
        )
    );
    assert!(writer.calls().is_empty());
    // 장부는 POST만이에요.
    let (status, _, out) = get(notes_state(&writer), "/ledger", Some(&bearer())).await;
    assert_eq!(
        (status, out),
        (StatusCode::NOT_FOUND, json!({"error": "not found"}))
    );
}

/// 세션 서버의 받아쓰기가 늘 내는 글이에요.
const HEARD: &str = "세션은 어디서 만들어요?";

/// 세션 서버예요: 판단 0.9, 리랭크 [0.9, 0.1, 0.1, 0.1], 받아쓰기는 늘 `HEARD`,
/// 답은 "답 [1]"이에요. 장부 모델은 부르면 down이에요.
fn session_state() -> Arc<AppState> {
    session_state_with(
        Arc::new(FakeGater::ok(0.9)),
        Arc::new(FakeTranscriber::ok(HEARD)),
    )
}

fn session_state_with(gater: Arc<FakeGater>, transcriber: Arc<FakeTranscriber>) -> Arc<AppState> {
    let (engine, _) = engine_with(
        index(),
        Services {
            gater,
            transcriber,
            ..services(
                Arc::new(FakeReranker::ok(&[0.9, 0.1, 0.1, 0.1])),
                Arc::new(FakeAnswerer::ok("답 [1]")),
            )
        },
    );
    Arc::new(AppState {
        engine: Arc::new(engine),
        token: TOKEN.to_string(),
        github: no_github(),
        refresher: None,
        meeting_dir: None,
    })
}

/// 메서드, 경로, 본문, 헤더를 그대로 보내요(토큰은 붙여요). (상태 코드, JSON 본문)
async fn call(
    state: &Arc<AppState>,
    method: &str,
    path: &str,
    body: impl Into<Body>,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, bearer());
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let res = router(state.clone())
        .oneshot(req.body(body.into()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn new_session(state: &Arc<AppState>) -> String {
    let (status, out) = call(state, "POST", "/sessions", "{}", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys(&out), ["session_id"]);
    out["session_id"].as_str().unwrap().to_string()
}

/// "RIFF" 뒤에 0이 64바이트예요(WAV 자리).
fn wav() -> Vec<u8> {
    let mut wav = b"RIFF".to_vec();
    wav.extend([0u8; 64]);
    wav
}

#[tokio::test]
async fn ask_excludes_listed_paths() {
    let st = session_state();
    let body =
        json!({"text": "세션 어디서 만들어요?", "exclude_paths": ["apps/api/src/mail/session.ts"]});
    let (status, out) = call(&st, "POST", "/ask", body.to_string(), &[]).await;
    assert_eq!(status, StatusCode::OK);
    let cards = out["cards"].as_array().unwrap();
    assert!(!cards.is_empty());
    assert!(
        cards
            .iter()
            .all(|c| c["path"] != "apps/api/src/mail/session.ts")
    );
    let body = json!({"text": "세션 어디서 만들어요?", "exclude_paths": []});
    let (_, out) = call(&st, "POST", "/ask", body.to_string(), &[]).await;
    assert_eq!(out["cards"][0]["path"], "apps/api/src/mail/session.ts");
}

#[tokio::test]
async fn ask_rejects_bad_exclude() {
    let st = session_state();
    for bad in [
        json!({"text": "세션", "exclude_paths": "a"}),
        json!({"text": "세션", "exclude_paths": [1]}),
    ] {
        let (status, _) = call(&st, "POST", "/ask", bad.to_string(), &[]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
async fn server_ask_and_session_flow() {
    let st = session_state();
    let (status, out) = call(
        &st,
        "POST",
        "/ask",
        r#"{"text": "세션 어디서 만들어요?"}"#,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["cards"][0]["path"], "apps/api/src/mail/session.ts");
    let (status, _) = call(&st, "POST", "/ask", r#"{"text": "  "}"#, &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let sid = new_session(&st).await;
    assert_eq!(sid, "s1");
    let body = json!({"text": HEARD, "speaker": "지수"}).to_string();
    let (status, rec) = call(
        &st,
        "POST",
        &format!("/sessions/{sid}/utterances"),
        body,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((&rec["fired"], &rec["shown"]), (&json!(true), &json!(true)));
    assert_eq!(rec["utterance_id"], "s1u1");
    assert_eq!(rec["speaker"], "지수");

    let (status, summary) = call(&st, "GET", &format!("/sessions/{sid}"), Body::empty(), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["fired"], 1);
    assert_eq!(
        keys(&summary),
        [
            "session_id",
            "utterances",
            "fired",
            "answered",
            "unanswered"
        ]
    );
    assert_eq!(summary["session_id"], "s1");
    assert_eq!(summary["utterances"], 1);

    let (status, out) = call(
        &st,
        "DELETE",
        &format!("/sessions/{sid}"),
        Body::empty(),
        &[],
    )
    .await;
    assert_eq!((status, out), (StatusCode::OK, json!({"deleted": "s1"})));
    let (status, out) = call(&st, "GET", &format!("/sessions/{sid}"), Body::empty(), &[]).await;
    assert_eq!(
        (status, out),
        (StatusCode::NOT_FOUND, json!({"error": "not found"}))
    );
    // 지운 번호는 다시 쓰지 않아요.
    assert_eq!(new_session(&st).await, "s2");
}

#[tokio::test]
async fn server_audio_cards_answer() {
    let transcriber = Arc::new(FakeTranscriber::ok(HEARD));
    let st = session_state_with(Arc::new(FakeGater::ok(0.9)), transcriber.clone());
    let sid = new_session(&st).await;
    let headers = [("content-type", "audio/wav"), ("x-speaker", "them")];
    let (status, heard) = call(
        &st,
        "POST",
        &format!("/sessions/{sid}/audio"),
        wav(),
        &headers,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(heard["fired"], true);
    assert_eq!(heard["text"], HEARD);
    assert_eq!(heard["speaker"], "상대");
    // 받은 바이트를 그대로 받아쓰기에 넘겨요.
    assert_eq!(transcriber.calls(), [wav()]);

    let uid = heard["utterance_id"].as_str().unwrap();
    let path = format!("/sessions/{sid}/utterances/{uid}/cards");
    let (status, found) = call(&st, "POST", &path, "{}", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found["cards"][0]["chunk"], 0);

    let body = json!({"text": heard["text"], "chunk_ids": [0]}).to_string();
    let (status, ans) = call(&st, "POST", "/answer", body, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ans["answer"], "답 [1]");
    assert_eq!(ans["cited"], json!([0]));
    let (status, _) = call(
        &st,
        "POST",
        "/answer",
        r#"{"text": "x", "chunk_ids": [999]}"#,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn server_ledger_route() {
    let st = session_state();
    let sid = new_session(&st).await;
    let (status, out) = call(&st, "POST", &format!("/sessions/{sid}/ledger"), "{}", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["items"], json!([]));
    let (status, out) = call(&st, "POST", "/sessions/s999/ledger", "{}", &[]).await;
    assert_eq!(
        (status, out),
        (StatusCode::NOT_FOUND, json!({"error": "no such session"}))
    );
}

#[tokio::test]
async fn session_routes_reject_bad_ids_and_missing_sessions() {
    let transcriber = Arc::new(FakeTranscriber::ok(HEARD));
    let st = session_state_with(Arc::new(FakeGater::ok(0.9)), transcriber.clone());
    let sid = new_session(&st).await;
    assert_eq!(sid, "s1");
    let not_found = (StatusCode::NOT_FOUND, json!({"error": "not found"}));
    let no_session = (StatusCode::NOT_FOUND, json!({"error": "no such session"}));
    let no_utterance = (StatusCode::NOT_FOUND, json!({"error": "no such utterance"}));
    let not_json = (
        StatusCode::BAD_REQUEST,
        json!({"error": "body is not JSON"}),
    );
    let no_text = (
        StatusCode::BAD_REQUEST,
        json!({"error": "text is required"}),
    );
    let bad_wav = (
        StatusCode::BAD_REQUEST,
        json!({"error": "send one utterance as WAV, up to 30 s"}),
    );

    // `s\d+`·`s\d+u\d+` 꼴이 아닌 id는 없는 경로예요(본문을 보지 않아요). 날 경로를 봐요: s%31은 s1이 아니에요.
    // 경로 모양이나 메서드가 다른 것도 없는 경로예요.
    for (method, path) in [
        ("GET", "/sessions/x1"),
        ("GET", "/sessions/s"),
        ("GET", "/sessions/S1"),
        ("GET", "/sessions/s1x"),
        ("GET", "/sessions/s%31"),
        ("DELETE", "/sessions/x1"),
        ("DELETE", "/sessions/s%31"),
        ("POST", "/sessions/x1/utterances"),
        ("POST", "/sessions/s1a/audio"),
        ("POST", "/sessions/x1/ledger"),
        ("POST", "/sessions/s%31/audio"),
        ("POST", "/sessions/s1/utterances/s1x/cards"),
        ("POST", "/sessions/s1/utterances/u1/cards"),
        ("POST", "/sessions/s1/utterances/s1u1x/cards"),
        ("POST", "/sessions/x1/utterances/s1u1/cards"),
        ("GET", "/sessions"),
        ("GET", "/sessions/s1/"),
        ("GET", "/sessions/s1/utterances"),
        ("POST", "/sessions/s1"),
        ("POST", "/sessions/s1/cards"),
        ("POST", "/sessions/s1/utterances/s1u1"),
        ("DELETE", "/sessions/s1/ledger"),
        ("GET", "/sessions/s1/audio"),
    ] {
        let out = call(&st, method, path, "{not json", &[]).await;
        assert_eq!(out, not_found, "{method} {path}");
    }

    // 없는 세션: 받아쓰기와 장부는 no such session(본문 크기보다 먼저 봐요), 나머지는 not found예요.
    for body in [wav(), Vec::new()] {
        let out = call(&st, "POST", "/sessions/s99/audio", body, &[]).await;
        assert_eq!(out, no_session);
    }
    assert_eq!(
        call(&st, "POST", "/sessions/s99/ledger", "{}", &[]).await,
        no_session
    );
    assert_eq!(
        call(&st, "GET", "/sessions/s99", Body::empty(), &[]).await,
        not_found
    );
    assert_eq!(
        call(&st, "DELETE", "/sessions/s99", Body::empty(), &[]).await,
        not_found
    );
    let out = call(
        &st,
        "POST",
        "/sessions/s99/utterances",
        r#"{"text": "x"}"#,
        &[],
    )
    .await;
    assert_eq!(out, not_found);
    let out = call(
        &st,
        "POST",
        "/sessions/s99/utterances/s99u1/cards",
        "{}",
        &[],
    )
    .await;
    assert_eq!(out, not_found);

    // JSON 경로는 세션보다 본문을 먼저 봐요. 빈 본문은 {}예요.
    for path in [
        "/sessions",
        "/sessions/s1/utterances",
        "/sessions/s99/utterances",
        "/sessions/s1/utterances/s1u1/cards",
        "/sessions/s99/utterances/s99u1/cards",
    ] {
        assert_eq!(
            call(&st, "POST", path, "{not json", &[]).await,
            not_json,
            "{path}"
        );
    }
    for body in [
        r#"{"text": "  "}"#,
        "{}",
        r#"{"text": null}"#,
        r#"{"text": 5}"#,
        "",
    ] {
        let out = call(&st, "POST", "/sessions/s1/utterances", body, &[]).await;
        assert_eq!(out, no_text, "body {body:?}");
    }

    // 있는 세션의 없는 발화, 다른 세션 id의 발화, 맞장구(찾을 앞 대화가 없어요)는 no such utterance예요.
    let out = call(&st, "POST", "/sessions/s1/utterances/s1u9/cards", "", &[]).await;
    assert_eq!(out, no_utterance);
    let (status, rec) = call(
        &st,
        "POST",
        "/sessions/s1/utterances",
        r#"{"text": "네"}"#,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (&rec["utterance_id"], &rec["skipped"]),
        (&json!("s1u1"), &json!("backchannel"))
    );
    for uid in ["s1u1", "s2u1"] {
        let path = format!("/sessions/s1/utterances/{uid}/cards");
        assert_eq!(
            call(&st, "POST", &path, "{}", &[]).await,
            no_utterance,
            "{uid}"
        );
    }

    // WAV는 0바이트보다 크고 MAX_AUDIO_BYTES 이하예요. Content-Length가 있으면 그 값으로, 없으면 받은 길이로 봐요.
    // axum 기본 한도(2 MB)를 넘어도 413이 아니라 400이에요.
    let audio = "/sessions/s1/audio";
    let over = (MAX_AUDIO_BYTES + 1).to_string();
    for (body, headers) in [
        (Vec::new(), vec![]),
        (Vec::new(), vec![("content-length", "0")]),
        (vec![0u8; MAX_AUDIO_BYTES + 1], vec![]),
        (
            vec![0u8; MAX_AUDIO_BYTES + 1],
            vec![("content-length", over.as_str())],
        ),
        (vec![0u8; 3 * 1024 * 1024], vec![]),
    ] {
        let n = body.len();
        assert_eq!(
            call(&st, "POST", audio, body, &headers).await,
            bad_wav,
            "{n} bytes {headers:?}"
        );
    }
    assert!(transcriber.calls().is_empty());
    // 딱 MAX_AUDIO_BYTES까지는 받아요. 본문은 JSON으로 읽지 않아요: 무엇이든 WAV로 넘겨요.
    let max = MAX_AUDIO_BYTES.to_string();
    let (status, _) = call(
        &st,
        "POST",
        audio,
        vec![0u8; MAX_AUDIO_BYTES],
        &[("content-length", &max)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, rec) = call(&st, "POST", audio, "{not json", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rec["text"], HEARD);
    let sizes: Vec<usize> = transcriber.calls().iter().map(Vec::len).collect();
    assert_eq!(sizes, [MAX_AUDIO_BYTES, 9]);
    // 장부도 본문을 읽지 않아요.
    let (status, _) = call(&st, "POST", "/sessions/s1/ledger", "{not json", &[]).await;
    assert_eq!(status, StatusCode::OK);

    // 닫은 세션은 없는 세션이에요.
    let out = call(&st, "DELETE", "/sessions/s1", Body::empty(), &[]).await;
    assert_eq!(out, (StatusCode::OK, json!({"deleted": "s1"})));
    assert_eq!(
        call(&st, "DELETE", "/sessions/s1", Body::empty(), &[]).await,
        not_found
    );
    assert_eq!(call(&st, "POST", audio, wav(), &[]).await, no_session);
    assert_eq!(
        call(&st, "POST", "/sessions/s1/ledger", "", &[]).await,
        no_session
    );
}

#[tokio::test]
async fn audio_headers_map_speaker_and_gap() {
    // X-Speaker는 me·them만 나·상대예요(대소문자도 그대로). X-Gap-Ms는 ASCII 숫자만 읽어요.
    let gater = Arc::new(FakeGater::ok(0.9));
    let st = session_state_with(gater.clone(), Arc::new(FakeTranscriber::ok(HEARD)));
    let sid = new_session(&st).await;
    let audio = format!("/sessions/{sid}/audio");
    // (헤더, 기록의 speaker, 판단에 간 줄의 이름, 기록의 gap_ms)
    type Case = (
        &'static [(&'static str, &'static str)],
        Value,
        &'static str,
        Value,
    );
    let cases: [Case; 8] = [
        (
            &[("x-speaker", "them"), ("x-gap-ms", "800")],
            json!("상대"),
            "상대",
            json!(800),
        ),
        (
            &[("X-Speaker", "me"), ("X-Gap-Ms", "12a")],
            json!("나"),
            "나",
            Value::Null,
        ),
        (
            &[("x-speaker", "Them"), ("x-gap-ms", "-1")],
            Value::Null,
            "나",
            Value::Null,
        ),
        (
            &[("x-speaker", ""), ("x-gap-ms", "")],
            Value::Null,
            "나",
            Value::Null,
        ),
        (
            &[("x-speaker", "상대"), ("x-gap-ms", "1.5")],
            Value::Null,
            "나",
            Value::Null,
        ),
        (&[("x-gap-ms", "0012")], Value::Null, "나", json!(12)),
        (&[("x-gap-ms", "+5")], Value::Null, "나", Value::Null),
        (&[], Value::Null, "나", Value::Null),
    ];
    for (i, (headers, speaker, who, gap)) in cases.into_iter().enumerate() {
        let (status, rec) = call(&st, "POST", &audio, wav(), headers).await;
        assert_eq!(status, StatusCode::OK, "{headers:?}");
        assert_eq!(rec["utterance_id"], format!("s1u{}", i + 1));
        assert_eq!(rec["speaker"], speaker, "{headers:?}");
        assert_eq!(rec["gap_ms"], gap, "{headers:?}");
        let (_, line) = gater.calls().pop().unwrap();
        assert_eq!(line, format!("{who}: {HEARD}"), "{headers:?}");
    }
}

#[tokio::test]
async fn utterance_route_keeps_speaker_and_reads_thresholds() {
    let gater = Arc::new(FakeGater::ok(0.9));
    let st = session_state_with(gater.clone(), Arc::new(FakeTranscriber::ok(HEARD)));
    let sid = new_session(&st).await;
    let path = format!("/sessions/{sid}/utterances");
    let post = |body: Value| call(&st, "POST", &path, body.to_string(), &[]);

    // speaker는 받은 글 그대로예요(me·them을 이름으로 바꾸지 않아요). 글은 앞뒤 공백을 빼요.
    // 기준을 주지 않으면 설정값(gate 0.7, show 0.8)이에요: 판단 0.9, 리랭크 0.9라 찾고 보여요.
    let (status, rec) = post(json!({"text": format!("  {HEARD}\n"), "speaker": "them"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (&rec["speaker"], &rec["text"]),
        (&json!("them"), &json!(HEARD))
    );
    assert_eq!((&rec["fired"], &rec["shown"]), (&json!(true), &json!(true)));
    assert_eq!(gater.calls().pop().unwrap().1, format!("them: {HEARD}"));
    // speaker가 없거나 글이 아니면 이름이 없어요: 줄은 "나: …"예요.
    for speaker in [Value::Null, json!(7)] {
        let (_, rec) = post(json!({"text": HEARD, "speaker": speaker})).await;
        assert_eq!(rec["speaker"], Value::Null);
        assert_eq!(gater.calls().pop().unwrap().1, format!("나: {HEARD}"));
    }
    // 기준을 주면 그 값이에요: gate 0.95면 찾지 않고, show 0.95면 찾되 보이지 않아요.
    let (_, rec) = post(json!({"text": HEARD, "gate_threshold": 0.95})).await;
    assert_eq!((&rec["fired"], &rec["cards"]), (&json!(false), &json!([])));
    let (_, rec) = post(json!({"text": HEARD, "show_threshold": 0.95})).await;
    assert_eq!(
        (&rec["fired"], &rec["shown"]),
        (&json!(true), &json!(false))
    );
    let shown_at_095 = rec["utterance_id"].as_str().unwrap().to_string();

    // 카드 경로도 show_threshold를 읽고, 없으면 설정값이에요.
    let cards = format!("/sessions/{sid}/utterances/{shown_at_095}/cards");
    let (_, found) = call(&st, "POST", &cards, "{}", &[]).await;
    assert_eq!(found["shown"], true);
    let (_, found) = call(&st, "POST", &cards, r#"{"show_threshold": 0.95}"#, &[]).await;
    assert_eq!(found["shown"], false);
    assert_eq!(found["utterance_id"], shown_at_095.as_str());
}

#[tokio::test]
async fn dropped_audio_request_still_records_the_utterance() {
    // 앱이 기다리다 연결을 끊어도(요청 future가 버려져도) 받아쓰기와 판단은 끝까지 가서 발화가 기록돼요.
    let gater = Arc::new(FakeGater::ok(0.9));
    let transcriber = Arc::new(FakeTranscriber::ok(HEARD).with_delay(Duration::from_millis(300)));
    let st = session_state_with(gater.clone(), transcriber.clone());
    let sid = new_session(&st).await;
    let req = Request::post(format!("/sessions/{sid}/audio"))
        .header(header::AUTHORIZATION, bearer())
        .body(Body::from(wav()))
        .unwrap();
    let cut =
        tokio::time::timeout(Duration::from_millis(50), router(st.clone()).oneshot(req)).await;
    assert!(
        cut.is_err(),
        "the request should still be transcribing when the app hangs up"
    );
    assert_eq!(transcriber.calls().len(), 1);

    let summary_path = format!("/sessions/{sid}");
    let deadline = Instant::now() + Duration::from_secs(5);
    let summary = loop {
        let (_, summary) = call(&st, "GET", &summary_path, Body::empty(), &[]).await;
        if summary["utterances"] == 1 || Instant::now() > deadline {
            break summary;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(summary["utterances"], 1);
    assert_eq!(summary["fired"], 1);
    assert_eq!(gater.calls().len(), 1);
}

#[tokio::test]
async fn session_work_that_panics_is_500_internal() {
    // 세션 일이 패닉하면 500이에요. 세션 자물쇠는 독이 들지 않아 다음 발화는 그대로 받아요.
    let gater = Arc::new(FakeGater::from_fn(|_, line| {
        assert!(!line.contains("터져요"), "boom");
        Ok(fake_gate(0.9, None))
    }));
    let st = session_state_with(gater, Arc::new(FakeTranscriber::ok(HEARD)));
    let sid = new_session(&st).await;
    let path = format!("/sessions/{sid}/utterances");
    let out = call(&st, "POST", &path, r#"{"text": "터져요"}"#, &[]).await;
    assert_eq!(
        out,
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error": "internal"})
        )
    );
    let (status, rec) = call(&st, "POST", &path, json!({"text": HEARD}).to_string(), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (&rec["utterance_id"], &rec["fired"]),
        (&json!("s1u1"), &json!(true))
    );
}

/// 세션 경로와 `/ledger`도 토큰이 먼저예요: 토큰 없이는 401이고, 받아쓰기·장부·판단 모델은 한 번도 불리지 않아요.
#[tokio::test]
async fn session_routes_need_the_token() {
    let gater = Arc::new(FakeGater::ok(0.9));
    let transcriber = Arc::new(FakeTranscriber::ok(HEARD));
    let writer = Arc::new(FakeNoteWriter::ok("{}"));
    let (engine, _) = engine_with(
        index(),
        Services {
            gater: gater.clone(),
            transcriber: transcriber.clone(),
            note_writer: writer.clone(),
            ..services(
                Arc::new(FakeReranker::ok(&[0.9, 0.1, 0.1, 0.1])),
                Arc::new(FakeAnswerer::ok("답 [1]")),
            )
        },
    );
    let st = Arc::new(AppState {
        engine: Arc::new(engine),
        token: TOKEN.to_string(),
        github: no_github(),
        refresher: None,
        meeting_dir: None,
    });
    let unauthorized = (StatusCode::UNAUTHORIZED, json!({"error": "unauthorized"}));
    let asks: [(&str, &str, Vec<u8>); 5] = [
        ("POST", "/sessions", b"{}".to_vec()),
        ("POST", "/sessions/s1/audio", wav()),
        ("POST", "/sessions/s1/ledger", b"{}".to_vec()),
        ("POST", "/ledger", b"{}".to_vec()),
        ("GET", "/sessions/s1", Vec::new()),
    ];
    for (method, path, body) in asks {
        for auth in [None, Some("Bearer wrong")] {
            let mut req = Request::builder().method(method).uri(path);
            if let Some(a) = auth {
                req = req.header(header::AUTHORIZATION, a);
            }
            let res = router(st.clone())
                .oneshot(req.body(Body::from(body.clone())).unwrap())
                .await
                .unwrap();
            let status = res.status();
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            let out: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!((status, out), unauthorized, "{method} {path} {auth:?}");
        }
    }
    assert!(gater.calls().is_empty());
    assert!(transcriber.calls().is_empty());
    assert!(writer.calls().is_empty());
    // 거절된 요청은 세션도 만들지 않아요: 다음 세션이 s1이에요.
    assert_eq!(new_session(&st).await, "s1");
}

/// axum은 `nest` 안의 핸들러에 앞머리를 뗀 `Uri`를 줘요: 세션 경로는 `Uri`로 읽어야 `^/sessions/`에 맞아요
/// (`OriginalUri`는 `/v1/sessions/...`라서 맞지 않아요).
#[tokio::test]
async fn session_routes_work_under_a_nested_router() {
    let app = Router::new().nest("/v1", router(session_state()));
    let send = |method: &str, path: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, bearer())
            .body(Body::from("{}"))
            .unwrap()
    };
    for (method, path, want) in [
        ("POST", "/v1/sessions", json!({"session_id": "s1"})),
        ("GET", "/v1/sessions/s1", Value::Null),
        ("DELETE", "/v1/sessions/s1", json!({"deleted": "s1"})),
    ] {
        let res = app.clone().oneshot(send(method, path)).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let out: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::OK, "{method} {path} {out}");
        if !want.is_null() {
            assert_eq!(out, want, "{method} {path}");
        } else {
            assert_eq!(out["utterances"], 0, "{out}");
        }
    }
}

/// 테스트 도우미의 엔진은 Jev에 준 설정 폴더를 그대로 써요: 진짜 `~/.config`의 키를 읽지 않아요.
#[test]
fn test_helpers_use_their_own_config_dir() {
    let st = state(&[], Some("typesafe"));
    let dir = &st.engine.config_dir;
    assert_ne!(*dir, settings::config_dir());
    // 도우미가 끝나도 폴더가 남아 있어요: Jev가 읽은 키 파일이 엔진의 설정 폴더에 있어요.
    assert_eq!(st.engine.jev.provider_name(), Some("typesafe"));
    assert!(dir.join("typesafe/config.json").exists());
    assert_ne!(state(&[], None).engine.config_dir, *dir);

    let (engine, _) = engine_with(
        index(),
        services(
            Arc::new(FakeReranker::all(0.9)),
            Arc::new(FakeAnswerer::ok("")),
        ),
    );
    assert_eq!(engine.config_dir, Path::new("/nonexistent-tacie-config"));
}

// --- /index와 /github/*

/// `CHUNKS` 색인, 이 GitHub, `index_dir`을 보는 갱신기(상태 idle)로 띄운 서버예요.
fn index_state(github: Arc<GitHub>, index_dir: &Path) -> Arc<AppState> {
    let (engine, _) = engine_with(
        index(),
        services(
            Arc::new(FakeReranker::all(0.9)),
            Arc::new(FakeAnswerer::ok("답 [1]")),
        ),
    );
    let engine = Arc::new(engine);
    Arc::new(AppState {
        engine: engine.clone(),
        token: TOKEN.to_string(),
        github: github.clone(),
        refresher: Some(refresher_for(engine, index_dir, github)),
        meeting_dir: None,
    })
}

/// login이 `basejb`인 gh 모드 GitHub이에요: gh는 늘 가짜의 토큰을 빌려 줘요.
fn gh_github() -> (Arc<FakeGitHub>, Arc<GitHub>) {
    let (fake, github, _) = gh_github_counting();
    (fake, github)
}

/// `gh_github`에 gh 토큰을 부른 수까지예요.
fn gh_github_counting() -> (Arc<FakeGitHub>, Arc<GitHub>, Arc<FakeGhToken>) {
    let fake = FakeGitHub::start();
    fake.login(Reply::Json(json!({"login": "basejb"})));
    let gh_token = Arc::new(FakeGhToken::gives(FakeGitHub::TOKEN));
    let github = Arc::new(
        GitHub::new("gh", &fake.url(), gh_token.clone())
            .unwrap()
            .with_pause(|_| {}),
    );
    (fake, github, gh_token)
}

fn http_reply(code: u16, body: &str) -> Reply {
    Reply::Http {
        code,
        headers: vec![],
        body: body.as_bytes().to_vec(),
    }
}

fn temp() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

#[tokio::test]
async fn index_and_config_routes() {
    let dir = temp();
    let (_fake, github) = gh_github(); // 가짜 서버는 테스트 끝까지 살아 있어야 해요
    let st = index_state(github, dir.path());
    let (status, _, body) = get(st.clone(), "/index", Some(&bearer())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (body["files"].clone(), body["github_items"].clone()),
        (json!(3), json!(0))
    );
    assert_eq!(body["github_login"], "basejb");
    assert_eq!(body["branch"], Value::Null);
    assert_eq!(body["refresh"], json!({"state": "idle"}));
    let (status, _, body) = get(st, "/config", Some(&bearer())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&body),
        [
            "models",
            "model_defaults",
            "thresholds",
            "threshold_defaults",
            "keys",
            "log_text"
        ]
    );
    assert_eq!(
        body["thresholds"],
        json!({"gate": 0.7, "show": 0.8, "end": 0.8})
    );
    let names: Vec<&str> = body["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["openrouter", "typesafe", "slack"]);
}

/// `/index`의 키와 순서는 고정이에요(`describe`가 가운데에 들어가요).
#[tokio::test]
async fn index_route_key_order() {
    let dir = temp();
    std::fs::write(dir.path().join("chunks.json"), b"x".repeat(10)).unwrap();
    std::fs::write(dir.path().join("emb.npy"), b"y".repeat(5)).unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap(); // 폴더는 세지 않아요
    let (_fake, github) = gh_github(); // 가짜 서버는 테스트 끝까지 살아 있어야 해요
    let (status, _, body) = get(index_state(github, dir.path()), "/index", Some(&bearer())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        keys(&body),
        [
            "chunks",
            "commit",
            "built_at",
            "refresh",
            "files",
            "github_items",
            "bytes",
            "repo",
            "branch",
            "symbols_missing",
            "github_lost",
            "github_login",
            "connected",
            "cleared",
            "notion",
            "linear",
            "gdrive",
            "atlassian",
            "asana",
            "dropbox",
            "notes",
            "model_ready",
            "voice_ready",
            "sample"
        ]
    );
    assert_eq!(
        body,
        json!({"chunks": 4, "commit": "abc123", "built_at": null, "refresh": {"state": "idle"},
               "files": 3, "github_items": 0, "bytes": 15, "repo": "acme/widgets", "branch": null,
               "symbols_missing": null, "github_lost": false, "github_login": "basejb",
               "connected": true, "cleared": false,
               "notion": {"connected": false, "workspace": null, "refused": false, "pages": null,
                          "fetched_at": null},
               "linear": {"connected": false, "workspace": null, "refused": false, "issues": null,
                          "teams": null, "fetched_at": null, "error_kind": null, "lost_since": null},
               "gdrive": {"connected": false, "account": null, "refused": false, "files": null,
                          "drives": null, "fetched_at": null, "initial_done": null, "error_kind": null,
                          "lost_since": null},
               "atlassian": {"connected": false, "site": null, "refused": false, "issues": null,
                             "pages": null, "fetched_at": null, "initial_done": null,
                             "error_kind": null, "lost_since": null, "paused_until": null},
               "asana": {"connected": false, "refused": false, "workspaces": null, "tasks": null,
                         "fetched_at": null, "initial_done": null, "error_kind": null,
                         "lost_since": null, "blocked_workspaces": null},
               "dropbox": {"connected": false, "account": null, "files": null, "fetched_at": null,
                           "initial_done": null, "refused": false, "error_kind": null,
                           "lost_since": null},
               "notes": {"chosen": 0, "folders": [], "notes": null, "not_downloaded": null,
                         "read_at": null},
               "model_ready": true, "voice_ready": false,
               "sample": {"ready": false, "questions": []}})
    );
}

#[test]
fn describe_counts_files_items_and_bytes() {
    let dir = temp();
    let raw: Vec<RawChunk> = json!([
        {"src": "code", "path": "a.ts"}, {"src": "code", "path": "a.ts"}, {"src": "docs", "path": "b.md"},
        {"src": "github", "path": "github/pull/1"}, {"src": "github", "path": "github/pull/1"},
        {"src": "github", "path": "github/issues/2"}, {"src": "slack", "path": "slack/C1/1"}
    ])
    .as_array()
    .unwrap()
    .iter()
    .map(|c| c.as_object().unwrap().clone())
    .collect();
    std::fs::write(dir.path().join("chunks.json"), b"x".repeat(10)).unwrap();
    std::fs::write(dir.path().join("emb.npy"), b"y".repeat(5)).unwrap();
    let meta = json!({"repo": "acme/widgets", "sources": {"repo": {"branch": "dev", "symbols_missing": 478}}});
    let got = describe(&raw, meta.as_object().unwrap(), dir.path());
    assert_eq!(
        Value::Object(got.clone()),
        json!({"files": 2, "github_items": 2, "bytes": 15, "repo": "acme/widgets", "branch": "dev",
               "symbols_missing": 478, "github_lost": false})
    );
    assert_eq!(
        keys(&Value::Object(got)),
        [
            "files",
            "github_items",
            "bytes",
            "repo",
            "branch",
            "symbols_missing",
            "github_lost"
        ]
    );
    let none = dir.path().join("none");
    assert_eq!(
        Value::Object(describe(&[], &Map::new(), &none)),
        json!({"files": 0, "github_items": 0, "bytes": 0, "repo": null, "branch": null,
               "symbols_missing": null, "github_lost": false})
    );
    // test_repo_lost_twice_closes_github: 닫힌 GitHub은 github_lost예요
    let lost = json!({"sources": {"github": {"repo": "o/r", "fetched_at": "2026-10-24T00:00:00Z",
                                             "chunks": 0, "lost": true}}});
    assert_eq!(
        describe(&[], lost.as_object().unwrap(), &none)["github_lost"],
        true
    );
    let back = json!({"sources": {"github": {"repo": "o/r", "fetched_at": "2026-10-24T00:00:00Z", "chunks": 1}}});
    assert_eq!(
        describe(&[], back.as_object().unwrap(), &none)["github_lost"],
        false
    );
}

#[tokio::test]
async fn credentials_route() {
    let dir = temp();
    let (fake, github, gh_token) = gh_github_counting();
    let st = index_state(github.clone(), dir.path());
    let bodies = std::cell::RefCell::new(Vec::new());
    let post_creds = async |body: &str| {
        let got = post(st.clone(), "/github/credentials", body).await;
        bodies.borrow_mut().push(got.1.clone());
        got
    };
    fake.clear_calls();
    let (status, got) = post_creds(r#"{"mode": "token", "token": "gho_x"}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, json!({"connected": true, "login": "basejb"}));
    // 새 연결로 login을 다시 물어요(받은 토큰으로요: 가짜의 토큰이 아니라 has_token은 거짓이에요)
    let logins: Vec<(String, bool)> = fake
        .calls()
        .iter()
        .map(|c| (c.path.clone(), c.has_token))
        .collect();
    assert_eq!(logins, [("user".to_string(), false)]);
    assert_eq!(gh_token.calls(), 0); // token 모드는 gh를 묻지 않아요
    assert!(!format!("{github:?}").contains("gho_x")); // 토큰이 들어 있는 동안 Debug에 나오지 않아요
    let bad_mode = json!({"error": "mode must be token (with a token), gh or none"});
    for bad in [
        r#"{"mode": "token"}"#,
        r#"{"mode": "token", "token": ""}"#,
        r#"{"mode": "token", "token": 5}"#,
        r#"{"mode": "cloud"}"#,
        r#"{"mode": 1}"#,
        r#"{}"#,
        r#"[]"#,
        r#""gh""#,
    ] {
        assert_eq!(
            post_creds(bad).await,
            (StatusCode::BAD_REQUEST, bad_mode.clone()),
            "{bad}"
        );
    }
    assert_eq!(
        post_creds("{").await,
        (
            StatusCode::BAD_REQUEST,
            json!({"error": "body is not JSON"})
        )
    );
    assert!(github.connected()); // 틀린 요청은 연결을 바꾸지 않아요
    let (status, _, got) = get(st.clone(), "/index", Some(&bearer())).await;
    bodies.borrow_mut().push(got.clone());
    assert_eq!(
        (status, got["connected"].clone()),
        (StatusCode::OK, json!(true))
    );
    let (status, got) = post_creds(r#"{"mode": "none"}"#).await;
    assert_eq!(
        (status, got),
        (StatusCode::OK, json!({"connected": false, "login": null}))
    );
    let (status, _, got) = get(st.clone(), "/index", Some(&bearer())).await;
    bodies.borrow_mut().push(got.clone());
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (got["connected"].clone(), got["github_login"].clone()),
        (json!(false), Value::Null)
    );
    let (status, got) = post_creds(r#"{"mode": "gh", "token": "gho_x"}"#).await;
    assert_eq!(
        (status, got),
        (
            StatusCode::OK,
            json!({"connected": true, "login": "basejb"})
        )
    );
    // gh 모드는 gh에서 빌린 토큰으로 물어요
    assert!(fake.calls().last().unwrap().has_token);
    assert_eq!(gh_token.calls(), 1);
    assert!(
        !serde_json::to_string(&*bodies.borrow())
            .unwrap()
            .contains("gho_x")
    );
}

/// 거절된 토큰(401)은 연결이 아니에요(스펙 6-2): 앱이 자동 동기화를 시작하지 않고 연결 단추를 보여요.
#[tokio::test]
async fn credentials_with_a_refused_token_answer_not_connected() {
    let dir = temp();
    let (fake, github) = gh_github();
    fake.login(http_reply(
        401,
        r#"{"message": "Bad credentials", "status": "401"}"#,
    ));
    let st = index_state(github.clone(), dir.path());
    let (status, got) = post(
        st.clone(),
        "/github/credentials",
        r#"{"mode": "token", "token": "gho_refused"}"#,
    )
    .await;
    assert_eq!(
        (status, got),
        (StatusCode::OK, json!({"connected": false, "login": null}))
    );
    let (status, _, got) = get(st.clone(), "/index", Some(&bearer())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (got["connected"].clone(), got["github_login"].clone()),
        (json!(false), Value::Null)
    );
    assert!(github.connected()); // 모드는 token이에요(갱신은 key로 실패해요)
    assert!(!got.to_string().contains("gho_refused"));
    // 거절은 그 연결 동안 기억해요: /index가 다시 묻지 않아요
    assert_eq!(fake.calls().len(), 1);
}

/// 네트워크가 없으면 연결은 그대로이고 login만 모르는 거예요(로그인 때 켜진 앱은 Wi-Fi보다 먼저 돌아요).
#[tokio::test]
async fn credentials_offline_stay_connected_without_login() {
    let dir = temp();
    let (fake, github) = gh_github();
    fake.login(Reply::Close);
    let st = index_state(github, dir.path());
    let (status, got) = post(
        st.clone(),
        "/github/credentials",
        r#"{"mode": "token", "token": "gho_offline"}"#,
    )
    .await;
    assert_eq!(
        (status, got),
        (StatusCode::OK, json!({"connected": true, "login": null}))
    );
    let (status, _, got) = get(st.clone(), "/index", Some(&bearer())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (got["connected"].clone(), got["github_login"].clone()),
        (json!(true), Value::Null)
    );
    // 모르는 것은 기억하지 않아요: Wi-Fi가 돌아오면 다음 물음이 찾아요
    fake.login(Reply::Json(json!({"login": "basejb"})));
    let (_, _, got) = get(st, "/index", Some(&bearer())).await;
    assert_eq!(
        (got["connected"].clone(), got["github_login"].clone()),
        (json!(true), json!("basejb"))
    );
    assert_eq!(fake.calls().len(), 3);
}

/// gh 모드는 엔진을 `TACIE_GITHUB_MODE=gh`로 띄웠을 때만이에요(개발 앱). 제품 엔진(none으로 시작)은
/// `/github/credentials {"mode": "gh"}`를 400으로 거절하고, `gh`에 토큰을 묻지 않아요.
#[tokio::test]
async fn credentials_gh_only_when_started_in_gh_mode() {
    let dir = temp();
    let fake = FakeGitHub::start();
    let github = Arc::new(GitHub::new("none", &fake.url(), Arc::new(PanicGhToken)).unwrap());
    assert!(!github.gh_allowed());
    let st = index_state(github.clone(), dir.path());
    let (status, got) = post(st.clone(), "/github/credentials", r#"{"mode": "gh"}"#).await;
    assert_eq!(
        (status, got),
        (StatusCode::BAD_REQUEST, json!({"error": GH_MODE_OFF}))
    );
    let (_, _, got) = get(st.clone(), "/index", Some(&bearer())).await;
    assert_eq!(
        (got["connected"].clone(), got["github_login"].clone()),
        (json!(false), Value::Null)
    );
    // 안에서 바로 바꿔도 gh는 none이에요(PanicGhToken은 불리지 않아요)
    github.set_credentials("gh", None);
    assert!(!github.connected());
    // token으로 시작한 엔진도 연결이 없는 채 시작하니 gh는 쓸 수 없어요
    let token_start = GitHub::new("token", &fake.url(), Arc::new(PanicGhToken)).unwrap();
    assert!(!token_start.gh_allowed());
    assert!(fake.calls().is_empty());

    // gh로 시작한 엔진은 받아요
    let (fake, github, gh_token) = gh_github_counting();
    assert!(github.gh_allowed());
    let st = index_state(github.clone(), dir.path());
    let (status, got) = post(st.clone(), "/github/credentials", r#"{"mode": "none"}"#).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let (status, got) = post(st, "/github/credentials", r#"{"mode": "gh"}"#).await;
    assert_eq!(
        (status, got),
        (
            StatusCode::OK,
            json!({"connected": true, "login": "basejb"})
        )
    );
    assert_eq!(gh_token.calls(), 1);
    assert_eq!(fake.calls().len(), 1);
}

#[tokio::test]
async fn repo_routes() {
    let dir = temp();
    let (fake, github) = gh_github();
    let st = index_state(github.clone(), dir.path());
    let repos = "user/repos?affiliation=owner,collaborator,organization_member&sort=pushed";
    fake.route(repos, Reply::Json(json!([])));
    fake.route("user/orgs", Reply::Json(json!([])));
    fake.route("repos/o/n", Reply::Json(json!({"default_branch": "main"})));
    fake.route(
        "repos/o/n/branches",
        Reply::Json(json!([{"name": "dev"}, {"name": "main"}])),
    );
    let (status, _, got) = get(st.clone(), "/github/repos", Some(&bearer())).await;
    assert_eq!(
        (status, got),
        (
            StatusCode::OK,
            json!({"repos": [], "needs_approval": [], "needs_sso": []})
        )
    );
    let branches = json!({"branches": ["main", "dev"], "default": "main"});
    for ok in [
        "repo=o/n",
        "repo=o%2Fn",
        "repo=&repo=o/n",
        "x=1&repo=o/n&repo=nope",
    ] {
        let (status, _, got) = get(
            st.clone(),
            &format!("/github/branches?{ok}"),
            Some(&bearer()),
        )
        .await;
        assert_eq!((status, got), (StatusCode::OK, branches.clone()), "{ok}");
    }
    let bad = json!({"error": "repo must be owner/name"});
    // %0A: `$` 하나로는 통과하는 끝 줄바꿈이에요
    for q in [
        "?repo=nope",
        "?repo=o/..",
        "?repo=-x/y",
        "?repo=",
        "?repo=o/n%0A",
        "",
        "?other=o/n",
    ] {
        let (status, _, got) =
            get(st.clone(), &format!("/github/branches{q}"), Some(&bearer())).await;
        assert_eq!((status, got), (StatusCode::BAD_REQUEST, bad.clone()), "{q}");
    }

    // 실패는 고정 글과 종류의 502예요: gh가 찍은 글이나 토큰은 넣지 않아요
    let unavailable = |kind: &str| {
        (
            StatusCode::BAD_GATEWAY,
            json!({"error": "GitHub is not available", "error_kind": kind}),
        )
    };
    github.set_credentials("none", None);
    let (status, _, got) = get(st.clone(), "/github/repos", Some(&bearer())).await;
    assert_eq!((status, got), unavailable("key"));
    github.set_credentials("gh", None);
    let failures = [
        (
            http_reply(401, r#"{"message": "Bad credentials", "status": "401"}"#),
            "key",
        ),
        (http_reply(200, "[{\"full_name\": "), "down"), // 잘린 답
        (http_reply(502, "<html>api.github.com</html>"), "down"),
        (Reply::Close, "network"),
    ];
    for (reply, kind) in failures {
        fake.route(repos, reply.clone());
        let (status, _, got) = get(st.clone(), "/github/repos", Some(&bearer())).await;
        assert!(!got.to_string().contains("api.github.com"), "{got}");
        assert_eq!((status, got), unavailable(kind), "{reply:?}");
        fake.route("repos/o/n", reply.clone());
        let (status, _, got) = get(st.clone(), "/github/branches?repo=o/n", Some(&bearer())).await;
        assert_eq!((status, got), unavailable(kind), "{reply:?}");
    }
}

/// 갱신기가 없으면 `/index` 경로는 없는 경로예요.
#[tokio::test]
async fn index_routes_are_404_without_a_refresher() {
    let st = state(&[], None);
    let not_found = json!({"error": "not found"});
    let (status, _, got) = get(st.clone(), "/index", Some(&bearer())).await;
    assert_eq!((status, got), (StatusCode::NOT_FOUND, not_found.clone()));
    for path in [
        "/index/refresh",
        "/index/clear",
        "/index/check",
        "/index/rebuild",
    ] {
        for body in ["", "{}", r#"{"fresh": true}"#, "{"] {
            assert_eq!(
                post(st.clone(), path, body).await,
                (StatusCode::NOT_FOUND, not_found.clone()),
                "{path} {body}"
            );
        }
    }
    // GitHub 경로는 갱신기 없이도 있어요(연결이 없으면 key)
    let (status, _, got) = get(st, "/github/repos", Some(&bearer())).await;
    assert_eq!(
        (status, got),
        (
            StatusCode::BAD_GATEWAY,
            json!({"error": "GitHub is not available", "error_kind": "key"})
        )
    );
}

/// 새 경로도 토큰부터 봐요.
#[tokio::test]
async fn index_and_github_routes_need_the_token() {
    let dir = temp();
    let (fake, github, gh_token) = gh_github_counting();
    let st = index_state(github, dir.path());
    fake.clear_calls();
    let unauthorized = (StatusCode::UNAUTHORIZED, json!({"error": "unauthorized"}));
    for (method, path) in [
        ("GET", "/index"),
        ("POST", "/index/refresh"),
        ("POST", "/index/clear"),
        ("POST", "/index/check"),
        ("POST", "/index/rebuild"),
        ("POST", "/github/credentials"),
        ("GET", "/github/repos"),
        ("GET", "/github/branches?repo=o/n"),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, "Bearer wrong")
            .body(Body::from(r#"{"mode": "none"}"#))
            .unwrap();
        let res = router(st.clone()).oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let got: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!((status, got), unauthorized, "{method} {path}");
    }
    assert_eq!(fake.calls(), vec![]);
    assert_eq!(gh_token.calls(), 0);
    assert_eq!(
        *st.refresher.as_ref().unwrap().status(),
        json!({"state": "idle"})
    );
}

// 온보딩 예시 자료(docs/11 12절): 예시 요청은 예시 색인만, 내 자료 요청은 내 색인만 봐요. 모델이 없으면 임베딩이
// 필요한 곳만 `model`이에요.

/// 내 색인은 `mine.md` 조각 하나, 예시 색인은 `CHUNKS`예요.
fn sample_state(
    gater: Arc<FakeGater>,
    model_ready: bool,
    index_dir: Option<&Path>,
) -> Arc<AppState> {
    use tacie_engine::sample::{Question, Sample};
    let mine = common::fixture::index_of(
        &json!([{"src": "docs", "path": "mine.md", "line": 1, "text": "doc: mine.md\n내 자료"}]),
        &json!({"repo": "o/n"}),
    );
    let (engine, _) = engine_with(
        mine,
        Services {
            gater,
            ..services(
                Arc::new(FakeReranker::all(0.9)),
                Arc::new(FakeAnswerer::ok("답 [1]")),
            )
        },
    );
    let mut engine = engine
        .with_sample(Some(Sample {
            index: Arc::new(index()),
            questions: vec![Question {
                text: "세션 어디서 만들어요?".into(),
                purpose: Some("ask".into()),
            }],
        }))
        .with_model_ready(model_ready);
    if !model_ready {
        engine.embedder = Arc::new(tacie_engine::sample::MissingModel);
    }
    let engine = Arc::new(engine);
    Arc::new(AppState {
        engine: engine.clone(),
        token: TOKEN.to_string(),
        github: no_github(),
        refresher: index_dir.map(|d| refresher_for(engine, d, no_github())),
        meeting_dir: None,
    })
}

fn paths(cards: &Value) -> Vec<&str> {
    cards
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn sample_ask_with_exclude_keeps_sample_ids() {
    use tacie_engine::sample::SAMPLE_BASE;
    let st = sample_state(Arc::new(FakeGater::ok(0.9)), true, None);
    let body =
        json!({"text": "세션 어디서 만들어요?", "sample": true, "exclude_paths": ["nothing.md"]});
    let (status, out) = call(&st, "POST", "/ask", body.to_string(), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["sample"], true);
    let cards = out["cards"].as_array().unwrap();
    assert!(!cards.is_empty());
    assert!(
        cards
            .iter()
            .all(|c| c["chunk"].as_u64().unwrap() >= SAMPLE_BASE as u64)
    );
}

#[tokio::test]
async fn sample_ask_and_answer_use_the_sample_index_by_chunk_id() {
    use tacie_engine::sample::SAMPLE_BASE;
    let st = sample_state(Arc::new(FakeGater::ok(0.9)), true, None);
    let (status, out) = call(
        &st,
        "POST",
        "/ask",
        r#"{"text": "세션 어디서 만들어요?", "sample": true}"#,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["sample"], true);
    assert!(!paths(&out["cards"]).contains(&"mine.md"));
    let ids: Vec<u64> = out["cards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["chunk"].as_u64().unwrap())
        .collect();
    assert!(ids.iter().all(|&i| i >= SAMPLE_BASE as u64), "{ids:?}");

    // /answer는 플래그 없이 id로 예시 색인을 골라요. 근거 번호도 같은 꼴이에요.
    let body = json!({"text": "세션 어디서 만들어요?", "chunk_ids": [ids[0]]}).to_string();
    let (status, ans) = call(&st, "POST", "/answer", body, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ans["cited"], json!([ids[0]]));
    // 내 자료 id와 섞으면 받지 않아요.
    let body = json!({"text": "x", "chunk_ids": [ids[0], 0]}).to_string();
    let (status, _) = call(&st, "POST", "/answer", body, &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 내 자료 요청은 예시 색인을 보지 않아요.
    let (_, mine) = call(
        &st,
        "POST",
        "/ask",
        r#"{"text": "세션 어디서 만들어요?"}"#,
        &[],
    )
    .await;
    assert_eq!(paths(&mine["cards"]), ["mine.md"]);
    assert!(mine.get("sample").is_none());

    // 예시 자료가 없으면 /ask도 /answer처럼 400이에요.
    let (status, out) = call(
        &session_state(),
        "POST",
        "/ask",
        r#"{"text": "q", "sample": true}"#,
        &[],
    )
    .await;
    assert_eq!(
        (status, out),
        (StatusCode::BAD_REQUEST, json!({"error": "no sample data"}))
    );
}

#[tokio::test]
async fn a_sample_session_searches_the_sample_index() {
    let st = sample_state(Arc::new(FakeGater::ok(0.9)), true, None);
    let (status, out) = call(&st, "POST", "/sessions", r#"{"sample": true}"#, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let sid = out["session_id"].as_str().unwrap().to_string();
    let body = json!({"text": "음 세션은 어디서 만들었죠? 아는 분?"}).to_string();
    let (status, rec) = call(
        &st,
        "POST",
        &format!("/sessions/{sid}/utterances"),
        body,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rec["fired"], true);
    assert_eq!(rec["sample"], true);
    assert!(!paths(&rec["cards"]).is_empty());
    assert!(!paths(&rec["cards"]).contains(&"mine.md"));
}

#[tokio::test]
async fn without_the_model_only_the_sample_answers() {
    let gater = Arc::new(FakeGater::ok(0.9));
    let dir = temp();
    let st = sample_state(gater.clone(), false, Some(dir.path()));
    // 내 자료 검색은 `model`로 실패해요.
    let (status, out) = call(
        &st,
        "POST",
        "/ask",
        r#"{"text": "세션 어디서 만들어요?"}"#,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["error_kind"], "model");
    assert_eq!(out["cards"], json!([]));
    // 예시는 임베딩 없이 답해요.
    let (_, out) = call(
        &st,
        "POST",
        "/ask",
        r#"{"text": "아무 말이나", "sample": true}"#,
        &[],
    )
    .await;
    assert!(out.get("error_kind").is_none(), "{out}");
    assert!(!paths(&out["cards"]).is_empty());
    // 내 자료 회의는 판단(유료)을 부르지 않고 `model`이에요.
    let sid = new_session(&st).await;
    let body = json!({"text": "세션 어디서 만들어요?"}).to_string();
    let (_, rec) = call(
        &st,
        "POST",
        &format!("/sessions/{sid}/utterances"),
        body,
        &[],
    )
    .await;
    assert_eq!(
        (&rec["fired"], &rec["error_kind"]),
        (&json!(false), &json!("model"))
    );
    assert!(gater.calls.lock().unwrap().is_empty());
    // 동기화는 시작하지 않아요.
    for path in ["/index/refresh", "/index/rebuild"] {
        let (status, out) = call(&st, "POST", path, r#"{"repo": "o/n"}"#, &[]).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert_eq!(out["error_kind"], "model");
    }
    // /index가 알려 줘요.
    let (_, _, info) = get(st.clone(), "/index", Some(&bearer())).await;
    assert_eq!(info["model_ready"], false);
    assert_eq!(
        info["sample"],
        json!({"ready": true, "questions": [{"text": "세션 어디서 만들어요?", "use": "ask"}]})
    );
}

/// `POST /ai/credentials`: 앱이 넘긴 키를 메모리에 두고 있는지(불리언)만 답해요. 키 글자는 어디에도 나오지 않아요.
#[tokio::test]
async fn ai_credentials_hold_keys_in_memory_and_answer_booleans() {
    let st = state(&[], None);
    let (status, got) = post(st.clone(), "/ai/credentials", "{}").await;
    assert_eq!(
        (status, got),
        (
            StatusCode::OK,
            json!({"typesafe": false, "openrouter": false})
        )
    );

    // 지난 거절은 새 키를 받으면 잊어요.
    st.engine
        .last
        .lock()
        .unwrap()
        .insert("jev".into(), Kind::Key);
    let body = r#"{"typesafe": " sk-ts-secret ", "openrouter": "sk-or-secret"}"#;
    let (status, got) = post(st.clone(), "/ai/credentials", body).await;
    assert_eq!(
        (status, got),
        (
            StatusCode::OK,
            json!({"typesafe": true, "openrouter": true})
        )
    );
    let (_, text, health) = get(st.clone(), "/health", Some(&bearer())).await;
    assert_eq!(health["jev"], "typesafe");
    let (_, config_text, config) = get(st.clone(), "/config", Some(&bearer())).await;
    let keys: Vec<(&str, &str, bool)> = config["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| {
            (
                k["name"].as_str().unwrap(),
                k["state"].as_str().unwrap(),
                k["in_app"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        keys,
        [
            ("openrouter", "ok", true),
            ("typesafe", "ok", true),
            ("slack", "missing", false)
        ]
    );
    assert_eq!(config["models"]["jev"], "jev-latest");
    for shown in [&text, &config_text, &format!("{:?}", st.engine.jev.keys())] {
        assert!(!shown.contains("secret"), "{shown}");
    }

    // 같은 키를 다시 넘기면(엔진이 준비될 때마다) 지난 거절을 잊지 않아요. TypeSafe 키만 바뀌면 Jev의 거절만 잊어요.
    let remember = |pairs: &[(&str, Kind)]| {
        let mut last = st.engine.last.lock().unwrap();
        last.clear();
        last.extend(pairs.iter().map(|(s, k)| (s.to_string(), *k)));
    };
    let services = || {
        let mut s: Vec<String> = st.engine.last.lock().unwrap().keys().cloned().collect();
        s.sort();
        s
    };
    remember(&[("jev", Kind::Key), ("stt", Kind::Credit)]);
    post(st.clone(), "/ai/credentials", body).await;
    assert_eq!(services(), ["jev", "stt"]);
    post(
        st.clone(),
        "/ai/credentials",
        r#"{"typesafe": "sk-ts-secret-2"}"#,
    )
    .await;
    assert_eq!(services(), ["stt"]);
    remember(&[
        ("jev", Kind::Key),
        ("stt", Kind::Credit),
        ("answer", Kind::Key),
    ]);
    post(
        st.clone(),
        "/ai/credentials",
        r#"{"openrouter": "sk-or-secret-2"}"#,
    )
    .await;
    assert!(services().is_empty());
    post(
        st.clone(),
        "/ai/credentials",
        r#"{"typesafe": "sk-ts-secret"}"#,
    )
    .await;

    // 빠진 이름은 그대로, null은 지워요.
    let (_, got) = post(st.clone(), "/ai/credentials", r#"{"openrouter": null}"#).await;
    assert_eq!(got, json!({"typesafe": true, "openrouter": false}));

    // 틀린 본문은 아무것도 바꾸지 않고 400이에요. 받은 값은 오류 글에 넣지 않아요.
    for bad in [
        "[]",
        r#"{"typesafe": 5}"#,
        r#"{"openrouter": "sk-new", "typesafe": "has space-secret"}"#,
        r#"{"typesafe": "한글-secret"}"#,
    ] {
        let (status, got) = post(st.clone(), "/ai/credentials", bad).await;
        assert_eq!(
            (status, got),
            (StatusCode::BAD_REQUEST, json!({"error": SEND_AI_KEYS})),
            "{bad}"
        );
    }
    let (_, got) = post(st.clone(), "/ai/credentials", "{}").await;
    assert_eq!(got, json!({"typesafe": true, "openrouter": false}));
    assert_eq!(
        st.engine
            .jev
            .keys()
            .find("typesafe")
            .unwrap()
            .key
            .as_deref(),
        Some("sk-ts-secret")
    );

    // 토큰이 없으면 401이에요.
    let req = Request::post("/ai/credentials")
        .body(Body::from(body))
        .unwrap();
    let res = router(st).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// 기억한 사람(`people`)과 다시 넘기는 목소리(`restore`)는 512차원 벡터의 목록이에요. 틀린 값은 회의를 열지 않아요.
#[tokio::test]
async fn create_session_rejects_bad_people() {
    let st = session_state();
    let v = |n: usize| vec![0.1f32; n];
    let ok_restore = json!({"voice": "화자 2", "channel": "mic", "vector": v(512), "seconds": 4.5});
    let (status, out) = call(
        &st,
        "POST",
        "/sessions",
        json!({"people": [{"id": "p1", "vector": v(512)}], "restore": [ok_restore]}).to_string(),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = call(
        &st,
        "POST",
        "/sessions",
        json!({"people": null}).to_string(),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let bad = [
        json!({"people": {"id": "p1"}}),
        json!({"people": [{"id": "p1", "vector": v(511)}]}),
        json!({"people": [{"id": "", "vector": v(512)}]}),
        json!({"people": [{"vector": v(512)}]}),
        json!({"people": [{"id": "p1", "vector": [["x"]]}]}),
        json!({"restore": "화자 1"}),
        json!({"restore": [{"voice": "화자 1", "channel": "phone", "vector": v(512), "seconds": 1.0}]}),
        json!({"restore": [{"voice": "나", "channel": "room", "vector": v(512), "seconds": 1.0}]}),
        json!({"restore": [{"voice": "화자 0", "channel": "room", "vector": v(512), "seconds": 1.0}]}),
        json!({"restore": [{"voice": "화자 1", "channel": "room", "vector": v(3), "seconds": 1.0}]}),
        json!({"restore": [{"voice": "화자 1", "channel": "room", "vector": v(512)}]}),
        json!({"restore": [{"voice": "화자 1", "channel": "room", "vector": v(512), "seconds": 1.0},
                           {"voice": "화자 1", "channel": "call", "vector": v(512), "seconds": 1.0}]}),
    ];
    for body in bad {
        let (status, out) = call(&st, "POST", "/sessions", body.to_string(), &[]).await;
        assert_eq!(
            (status, out),
            (
                StatusCode::BAD_REQUEST,
                json!({"error": "people and restore must be lists of voices"})
            ),
            "{body}"
        );
    }
    // 무한대(1e39는 f32에 들지 않아요)도 틀린 값이에요.
    let mut inf = vec![json!(0.1); 512];
    inf[0] = serde_json::from_str("1e39").unwrap();
    let body = json!({"people": [{"id": "p1", "vector": inf}]}).to_string();
    let (status, _) = call(&st, "POST", "/sessions", body, &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn voices_route_returns_list() {
    let st = session_state();
    let sid = new_session(&st).await;
    let (status, out) = call(&st, "POST", &format!("/sessions/{sid}/voices"), "", &[]).await;
    assert_eq!((status, out), (StatusCode::OK, json!({"voices": []})));
    let (status, out) = call(
        &st,
        "POST",
        &format!("/sessions/{sid}/voices"),
        json!({"exclude": [format!("{sid}u1")]}).to_string(),
        &[],
    )
    .await;
    assert_eq!((status, out), (StatusCode::OK, json!({"voices": []})));
    let (status, _) = call(&st, "POST", &format!("/sessions/{sid}/voices"), "{", &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(&st, "POST", "/sessions/s999/voices", "{}", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// 앱이 화자 키("화자 2")에 붙인 이름 표예요. 통째로 바꿔요.
#[tokio::test]
async fn names_route_replaces_names() {
    let st = session_state();
    let sid = new_session(&st).await;
    let path = format!("/sessions/{sid}/names");
    let body = json!({"names": {"화자 2": "지수"}}).to_string();
    let (status, out) = call(&st, "POST", &path, body, &[]).await;
    assert_eq!((status, out), (StatusCode::OK, json!({})));
    let body = json!({"names": {}}).to_string();
    let (status, out) = call(&st, "POST", &path, body, &[]).await;
    assert_eq!((status, out), (StatusCode::OK, json!({})));
    for body in [
        json!({}),
        json!({"names": null}),
        json!({"names": ["화자 2", "지수"]}),
        json!({"names": {"화자 2": 3}}),
        json!({"names": {"화자 2": null}}),
        json!({"names": "지수"}),
    ] {
        let (status, out) = call(&st, "POST", &path, body.to_string(), &[]).await;
        assert_eq!(
            (status, out),
            (
                StatusCode::BAD_REQUEST,
                json!({"error": "names must map speakers to names"})
            ),
            "{body}"
        );
    }
    let (status, _) = call(&st, "POST", &path, "{", &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body = json!({"names": {}}).to_string();
    let (status, out) = call(&st, "POST", "/sessions/s999/names", body, &[]).await;
    assert_eq!(
        (status, out),
        (StatusCode::NOT_FOUND, json!({"error": "no such session"}))
    );
}

/// `after`는 앱이 벡터를 받기 전에 쓴 "화자 N"까지 번호를 비워 둬요. 번호가 너무 크거나 한 사람이 두 목소리에
/// 이어져 있으면 틀린 값이에요.
#[tokio::test]
async fn create_session_checks_after_numbers_and_people() {
    let st = session_state();
    let v = vec![0.1f32; 512];
    let restore = |voice: &str, person: Value| json!({"voice": voice, "channel": "room", "vector": v, "seconds": 1.0, "person": person});
    for body in [
        json!({"after": 3}),
        json!({"after": 0}),
        json!({"after": null}),
        json!({"after": 10000}),
        json!({"restore": [restore("화자 10000", json!(null))]}),
        json!({"restore": [restore("화자 1", json!("p1")), restore("화자 2", json!("p2")),
                           restore("화자 3", json!(null)), restore("화자 4", json!(null))]}),
    ] {
        let (status, out) = call(&st, "POST", "/sessions", body.to_string(), &[]).await;
        assert_eq!(status, StatusCode::OK, "{body} {out}");
    }
    for body in [
        json!({"after": "3"}),
        json!({"after": -1}),
        json!({"after": 1.5}),
        json!({"after": 10001}),
        json!({"restore": [restore("화자 10001", json!(null))]}),
        json!({"restore": [restore("화자 1", json!("p1")), restore("화자 2", json!("p1"))]}),
    ] {
        let (status, out) = call(&st, "POST", "/sessions", body.to_string(), &[]).await;
        assert_eq!(
            (status, out),
            (
                StatusCode::BAD_REQUEST,
                json!({"error": "people and restore must be lists of voices"})
            ),
            "{body}"
        );
    }
}

/// 모델 파일이 있으면 올라가는 가짜 로더로 만든 `/stt` 시험용 서버예요.
fn stt_state() -> Arc<AppState> {
    struct Never;
    impl LocalModel for Never {
        fn transcribe(
            &self,
            _: &[u8],
            _: &str,
        ) -> anyhow::Result<tacie_engine::services::Transcript> {
            anyhow::bail!("not used")
        }
    }
    let load: LoadModel = Arc::new(|path, _| {
        if path.exists() {
            Ok(Arc::new(Never) as Arc<dyn LocalModel>)
        } else {
            anyhow::bail!("no model file")
        }
    });
    let routed = RoutedStt::new(
        Arc::new(FakeTranscriber::ok("서버")),
        load,
        Arc::new(|| false),
        Arc::new(String::new),
    );
    state_with_stt(
        &[],
        None,
        services(
            Arc::new(FakeReranker::all(0.9)),
            Arc::new(FakeAnswerer::ok("답 [1]")),
        ),
        Some(routed),
    )
}

#[tokio::test]
async fn stt_route_sets_and_reports() {
    let state = stt_state();
    let (status, out) = post(
        state.clone(),
        "/stt",
        r#"{"on": true, "model": "/no/such/model.bin"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["on"], true);
    assert_eq!(out["ready"], false);
    assert_eq!(keys(&out), ["on", "ready", "loading", "broken", "device"]);
    // 없는 파일은 올라오지 못해서 곧 고장이에요.
    let model = tempfile::NamedTempFile::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let body = json!({"on": true, "model": model.path()}).to_string();
    let (status, _) = post(state.clone(), "/stt", &body).await;
    assert_eq!(status, StatusCode::OK);
    let mut out = Value::Null;
    for _ in 0..200 {
        out = post(state.clone(), "/stt", &body).await.1;
        if out["ready"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(out["ready"], true);
    assert_eq!(out["device"], "metal");
    let (_, out) = post(state, "/stt", r#"{"on": false, "model": null}"#).await;
    assert_eq!(out["on"], false);
    assert_eq!(out["ready"], false);
    assert!(out["device"].is_null());
}

#[tokio::test]
async fn stt_route_rejects_bad_body() {
    for body in [
        r#"{"on": "yes"}"#,
        r#"{"on": true, "model": 3}"#,
        r#"{"model": null}"#,
        "[]",
        "nope",
    ] {
        let (status, out) = post(stt_state(), "/stt", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body {body:?}");
        assert!(out["error"].is_string());
    }
    let (_, out) = post(stt_state(), "/stt", r#"{"on": "yes"}"#).await;
    assert_eq!(
        out["error"],
        "send {on: true or false, model: a file path or null}"
    );
}

fn voice_state(voice: FakeVoice) -> Arc<AppState> {
    let (engine, _) = engine_with(
        index(),
        Services {
            gater: Arc::new(FakeGater::ok(0.9)),
            transcriber: Arc::new(FakeTranscriber::ok(HEARD)),
            ..services(
                Arc::new(FakeReranker::ok(&[0.9, 0.1, 0.1, 0.1])),
                Arc::new(FakeAnswerer::ok("답 [1]")),
            )
        },
    );
    Arc::new(AppState {
        engine: Arc::new(engine.with_voice(Some(Arc::new(voice)))),
        token: TOKEN.to_string(),
        github: no_github(),
        refresher: None,
        meeting_dir: None,
    })
}

/// 회의 중에 사람이 말 하나에 다른 이름을 주면(이 말만) 그 말을 새 목소리로 떼어 내고, 다음 비슷한 말은 그쪽으로 가요.
#[tokio::test]
async fn split_route_moves_a_line_to_a_new_voice() {
    let wav_of = |tag: u8| {
        let mut w = b"RIFF".to_vec();
        w.extend([tag; 64]);
        w
    };
    let (a, c, c2) = (wav_of(1), wav_of(2), wav_of(3));
    // A와 C는 cos 0.6이라 한 목소리로 묶여요.
    let st = voice_state(FakeVoice::by_wav(&[
        (&a, &[1.0, 0.0, 0.0], 3.0),
        (&c, &[0.6, 0.8, 0.0], 3.0),
        (&c2, &[0.55, 0.83, 0.0], 3.0),
    ]));
    let sid = new_session(&st).await;
    let path = format!("/sessions/{sid}/audio");
    let hdr = [("content-type", "audio/wav")];
    let (_, h1) = call(&st, "POST", &path, a, &hdr).await;
    // 쉼 없이 이어 붙지 않게 2초 쉬어요.
    let gap = [("content-type", "audio/wav"), ("x-gap-ms", "5000")];
    let (_, h2) = call(&st, "POST", &path, c, &gap).await;
    assert_eq!(
        (h1["voice"].as_str(), h2["voice"].as_str()),
        (Some("화자 1"), Some("화자 1"))
    );
    let uid = h2["utterance_id"].as_str().unwrap().to_string();
    let split = format!("/sessions/{sid}/split");
    let (status, out) = call(
        &st,
        "POST",
        &split,
        json!({"utterances": [uid]}).to_string(),
        &[],
    )
    .await;
    assert_eq!((status, out), (StatusCode::OK, json!({"voice": "화자 2"})));
    let (_, h3) = call(&st, "POST", &path, c2, &gap).await;
    assert_eq!(h3["voice"], "화자 2");
    // 목소리가 없는 말, 모르는 세션이에요.
    let (status, _) = call(
        &st,
        "POST",
        &split,
        json!({"utterances": ["s9u9"]}).to_string(),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(&st, "POST", "/sessions/s99/split", "{}", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---- 녹음 유형(docs/23 3-4절) ----

#[tokio::test]
async fn ledger_takes_kind() {
    let writer = Arc::new(FakeNoteWriter::ok(r#"{"items": []}"#));
    let body = json!({"lines": [{"id": "s1u1", "speaker": "me", "text": "오늘은 임베딩을 설명할게요"}], "kind": "talk"});
    let (status, out) = post(notes_state(&writer), "/ledger", &body.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["kind"], "talk");
    assert_eq!(out["kind_source"], "user");
    let body =
        json!({"lines": [{"id": "s1u1", "speaker": "me", "text": "배포해요"}], "kind": null});
    let (status, out) = post(notes_state(&writer), "/ledger", &body.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["kind_source"], "auto");
}

#[tokio::test]
async fn ledger_rejects_unknown_kind() {
    let writer = Arc::new(FakeNoteWriter::ok(r#"{"items": []}"#));
    for kind in [json!("podcast"), json!(3), json!(["talk"])] {
        let body = json!({"lines": [{"id": "s1u1", "text": "배포해요"}], "kind": kind});
        let (status, out) = post(notes_state(&writer), "/ledger", &body.to_string()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{kind}");
        assert_eq!(
            out,
            json!({"error": "kind must be meeting, interview or talk"})
        );
    }
    let st = session_state();
    let sid = new_session(&st).await;
    let (status, _) = call(
        &st,
        "POST",
        &format!("/sessions/{sid}/ledger"),
        r#"{"kind": "podcast"}"#,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &st,
        "POST",
        &format!("/sessions/{sid}/ledger"),
        "{not json",
        &[],
    )
    .await;
    // JSON이 아닌 본문은 지금까지처럼 읽지 않아요.
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn session_ledger_reads_kind_body() {
    let st = session_state();
    let sid = new_session(&st).await;
    let (status, out) = call(
        &st,
        "POST",
        &format!("/sessions/{sid}/ledger"),
        r#"{"kind": "interview"}"#,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["kind"], "interview");
    assert_eq!(out["kind_source"], "user");
    // 빈 본문은 유형을 고르지 않은 거예요: 새 발화가 없으니 지난 결과예요.
    let (status, out) = call(&st, "POST", &format!("/sessions/{sid}/ledger"), "", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["kind"], "interview");
}

/// 기록만 하기: `{lookup: false}`로 연 회의와 회의 중 `/lookup`은 판단을 부르지 않고, 다시 켜면 그 뒤 발화부터 판단해요.
#[tokio::test]
async fn record_only_skips_the_gate_and_can_be_switched_mid_meeting() {
    let gater = Arc::new(FakeGater::ok(0.9));
    let st = session_state_with(gater.clone(), Arc::new(FakeTranscriber::ok(HEARD)));
    let (status, out) = call(&st, "POST", "/sessions", r#"{"lookup": false}"#, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let sid = out["session_id"].as_str().unwrap().to_string();
    let say = |text: &str| json!({"text": text, "speaker": "지수"}).to_string();
    let path = format!("/sessions/{sid}/utterances");

    let (_, rec) = call(&st, "POST", &path, say(HEARD), &[]).await;
    assert_eq!(
        (&rec["fired"], &rec["p_lookup"], &rec["ending"]),
        (&json!(false), &Value::Null, &json!(false))
    );
    assert_eq!(rec["text"], HEARD);
    assert!(gater.calls.lock().unwrap().is_empty());

    let lookup = format!("/sessions/{sid}/lookup");
    let (status, out) = call(&st, "POST", &lookup, r#"{"on": true}"#, &[]).await;
    assert_eq!((status, out), (StatusCode::OK, json!({"lookup": true})));
    let (_, rec) = call(&st, "POST", &path, say("그 설정 어디 있어요?"), &[]).await;
    assert_eq!(rec["fired"], json!(true));
    assert_eq!(gater.calls.lock().unwrap().len(), 1);

    let (status, out) = call(&st, "POST", &lookup, r#"{"on": false}"#, &[]).await;
    assert_eq!((status, out), (StatusCode::OK, json!({"lookup": false})));
    let (_, rec) = call(&st, "POST", &path, say("배포는 언제 해요?"), &[]).await;
    assert_eq!(rec["fired"], json!(false));
    assert_eq!(gater.calls.lock().unwrap().len(), 1);

    // 지금 앱처럼 lookup 없이 연 회의는 판단해요.
    let plain = new_session(&st).await;
    let (_, rec) = call(
        &st,
        "POST",
        &format!("/sessions/{plain}/utterances"),
        say(HEARD),
        &[],
    )
    .await;
    assert_eq!(rec["fired"], json!(true));

    let (status, _) = call(&st, "POST", &lookup, r#"{"on": "no"}"#, &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(&st, "POST", "/sessions/s99/lookup", r#"{"on": true}"#, &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn questions_review_route() {
    // docs/24 4절: 못 찾은 질문을 나온 줄과 함께 보내요. 줄은 장부와 같이 읽고(맞장구는 빠져요), 모르는 줄 id는
    // 질문 글만 보내요.
    let writer = Arc::new(FakeNoteWriter::ok(
        r#"{"questions": [{"id": "v1", "verdict": "answered", "question": "재시도는 몇 번인가요?", "answer": "[[나]]: 3회라고 답함.", "utterance_ids": ["s1u2"]}]}"#,
    ));
    let body = json!({"lines": [
        {"id": "s1u1", "speaker": "them", "text": "재시도 몇 번이었죠?"},
        {"id": "s1u2", "speaker": "me", "text": "세 번이요"},
        {"id": "s1u3", "speaker": "them", "text": "네"}],
        "questions": [{"id": "v1", "line": "s1u1", "text": "재시도 몇 번이었죠?"}, {"id": "v2", "line": "s9u9", "text": "그거 뭐죠?"}]});
    let (status, out) = post(notes_state(&writer), "/questions/review", &body.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        out["questions"],
        json!([{"id": "v1", "verdict": "answered", "question": "재시도는 몇 번인가요?",
                                         "answer": "[[나]]: 3회라고 답함.", "utterance_ids": ["s1u2"]}])
    );
    let (transcript, questions) = writer.reviews.lock().unwrap()[0].clone();
    assert!(transcript.contains("[s1u2] 나: 세 번이요") && !transcript.contains("[s1u3]"));
    assert_eq!(
        questions,
        // v2의 줄 s9u9는 대화 기록에 없어서 판정하지 않아요
        "- [v1] 나온 발화 [s1u1]: 재시도 몇 번이었죠?"
    );
    for bad in [
        r#"{"lines": []}"#,
        r#"{"lines": [], "questions": [{"id": "v1"}]}"#,
        "x",
    ] {
        let (status, _) = post(notes_state(&writer), "/questions/review", bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    // 질문 줄이 대화 기록에 없으면(긴 회의의 잘린 앞부분) 판정하지 않아요. 남는 질문이 없으면 모델을 부르지 않아요.
    let before = writer.reviews.lock().unwrap().len();
    let gone = json!({"lines": [{"id": "s1u2", "speaker": "me", "text": "세 번이요"}],
                      "questions": [{"id": "v1", "line": "s1u1", "text": "재시도 몇 번이었죠?"}]});
    let (status, out) = post(notes_state(&writer), "/questions/review", &gone.to_string()).await;
    assert_eq!(
        (status, out["questions"].clone()),
        (StatusCode::OK, json!([]))
    );
    assert_eq!(writer.reviews.lock().unwrap().len(), before);
    // 실패는 빈 목록과 이유예요(앱은 원문 질문을 그대로 둬요)
    let failing = Arc::new(FakeNoteWriter::fail(Kind::Credit, "no credit"));
    let (status, out) = post(
        notes_state(&failing),
        "/questions/review",
        &body.to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (out["questions"].clone(), out["error_kind"].clone()),
        (json!([]), json!("credit"))
    );
}

// ---------- 그림자 받아쓰기(docs/25): Soniox 임시 키와 회의 용어 ----------

#[tokio::test]
async fn stream_key_without_a_key_is_404_key() {
    // 키 파일 폴더가 없는 엔진이에요(`engine_with`): Soniox 키가 없어요.
    let st = session_state();
    let (status, out) = call(&st, "POST", "/stt/stream-key", "{}", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(out["error_kind"], "key");
}

#[tokio::test]
async fn session_terms_lists_the_index_terms() {
    let st = session_state();
    let sid = new_session(&st).await;
    let (status, out) = call(
        &st,
        "GET",
        &format!("/sessions/{sid}/terms"),
        Body::empty(),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out, json!({"terms": st.engine.term_fixer().list.clone()}));
}

#[tokio::test]
async fn sample_session_terms_are_empty() {
    let st = sample_state(Arc::new(FakeGater::ok(0.9)), true, None);
    let (_, out) = call(&st, "POST", "/sessions", r#"{"sample": true}"#, &[]).await;
    let sid = out["session_id"].as_str().unwrap().to_string();
    let (status, out) = call(
        &st,
        "GET",
        &format!("/sessions/{sid}/terms"),
        Body::empty(),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out, json!({"terms": []}));
}

#[tokio::test]
async fn unknown_session_terms_is_404() {
    let st = session_state();
    let (status, _) = call(&st, "GET", "/sessions/s999/terms", Body::empty(), &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------- 스트리밍 받아쓰기(docs/25): 앱이 Soniox에서 받은 글을 같이 보내면 엔진은 받아쓰지 않아요 ----------

fn transcript_header(text: &str) -> String {
    use base64::Engine as _;
    let body =
        json!({"text": text, "words": [{"text": "세션은", "start": 0.1, "end": 0.5}]}).to_string();
    base64::engine::general_purpose::STANDARD.encode(body)
}

#[tokio::test]
async fn audio_with_a_transcript_skips_the_transcriber() {
    let transcriber = Arc::new(FakeTranscriber::ok("엔진이 받아쓴 글"));
    let st = session_state_with(Arc::new(FakeGater::ok(0.9)), transcriber.clone());
    let sid = new_session(&st).await;
    let given = transcript_header(HEARD);
    let headers = [
        ("content-type", "audio/wav"),
        ("x-transcript", given.as_str()),
    ];
    let (status, heard) = call(
        &st,
        "POST",
        &format!("/sessions/{sid}/audio"),
        wav(),
        &headers,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(heard["text"], HEARD);
    assert_eq!(heard["stt"], "stream");
    assert_eq!(heard["fired"], true);
    assert!(transcriber.calls().is_empty());
}

#[tokio::test]
async fn audio_with_a_bad_transcript_is_400() {
    let st = session_state();
    let sid = new_session(&st).await;
    let headers = [
        ("content-type", "audio/wav"),
        ("x-transcript", "not base64 json"),
    ];
    let (status, _) = call(
        &st,
        "POST",
        &format!("/sessions/{sid}/audio"),
        wav(),
        &headers,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// --- /panel/*

async fn post_text(state: Arc<AppState>, path: &str, body: &str) -> (StatusCode, String, String) {
    let req = Request::post(path)
        .header(header::AUTHORIZATION, bearer())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = router(state).oneshot(req).await.unwrap();
    let status = res.status();
    let ctype = res
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, ctype, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn panel_prep_streams_steps_then_done() {
    use tacie_sources::meeting::{self, Entry, Meeting};
    let dir = tempfile::tempdir().unwrap();
    let st = index_state(no_github(), dir.path());
    let m = Meeting {
        id: "a".into(),
        title: "회의 a".into(),
        start: "2026-11-03T14:00:00+09:00".into(),
        end: "2026-11-03T15:00:00+09:00".into(),
        summary: vec!["세션 이야기".into()],
        topics: vec![],
        ledger: vec![Entry {
            kind: "open_question".into(),
            text: "세션을 어디에 둘까요".into(),
            owner: None,
            due: None,
            done: false,
            replaces: None,
        }],
    };
    meeting::save(&st.refresher.as_ref().unwrap().meeting_dir(), &[m]).unwrap();
    let (status, ctype, text) = post_text(st, "/panel/prep", r#"{"meeting_ids": ["a"]}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ctype, "application/x-ndjson");
    let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines[0], json!({"step": "meetings", "ids": ["a"]}));
    let last = lines.last().unwrap();
    assert!(last.get("done").is_some(), "{last}");
    assert_eq!(last["done"]["open"][0]["text"], "세션을 어디에 둘까요");
    assert!(lines[..lines.len() - 1].iter().all(|l| l.get("step").is_some()));
}

#[tokio::test]
async fn panel_prep_without_meetings_is_404() {
    let (status, out) = post(state(&[], None), "/panel/prep", r#"{"meeting_ids": []}"#).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(out, json!({"error": "not found"}));
}

#[tokio::test]
async fn panel_prep_rejects_bad_ids() {
    let dir = tempfile::tempdir().unwrap();
    let st = index_state(no_github(), dir.path());
    for body in [r#"{"meeting_ids": "a"}"#, r#"{"meeting_ids": [1]}"#] {
        let (status, out) = post(st.clone(), "/panel/prep", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(out["error"].is_string());
    }
}

#[tokio::test]
async fn panel_why_rejects_non_list_window() {
    let (status, out) = post(
        state(&[], None),
        "/panel/why",
        r#"{"decision": "세션은 쿠키", "window": "x"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(out["error"].is_string());
    let (status, _) = post(state(&[], None), "/panel/followup", r#"{"prepared": 1, "ledger": []}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn panel_ask_scoped_to_meeting() {
    // 회의 범위가 아니면 카드와 답이 나와요.
    let (status, out) = post(
        state(&[], None),
        "/panel/ask",
        r#"{"text": "세션은 어디서 만들어요?", "history": [{"q": "앞 질문", "a": "앞 답"}]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!out["cards"].as_array().unwrap().is_empty(), "{out}");
    assert!(out["answer"].is_string(), "{out}");
    // 없는 회의로 좁히면 카드가 없고 답 모델도 부르지 않아요.
    let answerer = Arc::new(FakeAnswerer::ok("답 [1]"));
    let st = state_with(&[], None, &answerer);
    let (status, out) = post(
        st,
        "/panel/ask",
        r#"{"text": "세션은 어디서 만들어요?", "meeting_ids": ["nope"]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["cards"], json!([]));
    assert_eq!(out["answer"], Value::Null);
    assert_eq!(out["cited"], json!([]));
    // 본문 검사예요.
    for body in [r#"{}"#, r#"{"text": "x", "meeting_ids": "a"}"#, r#"{"text": "x", "history": 1}"#] {
        let (status, out) = post(state(&[], None), "/panel/ask", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(out["error"].is_string());
    }
}

#[tokio::test]
async fn panel_ask_keeps_six_history_turns_whole() {
    let answerer = Arc::new(FakeAnswerer::ok("답 [1]"));
    let reranker = Arc::new(FakeReranker::all(0.9));
    let st = state_from(&[], None, services(reranker.clone(), answerer.clone()));
    let long = "가".repeat(300);
    let mut history: Vec<Value> = (0..7).map(|i| json!({"q": format!("q{i}"), "a": format!("a{i}")})).collect();
    history[3]["a"] = json!(long);
    // 여러 줄 답도 한 줄로 실어요.
    history[5]["a"] = json!("첫 줄\n둘째 줄");
    let body = json!({"text": "세션은 어디서 만들어요?", "history": history});
    let (status, out) = post(st, "/panel/ask", &body.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(out["answer"].is_string(), "{out}");
    let (_, context, _) = answerer.calls().remove(0);
    assert_eq!(context.len(), 6, "{context:?}");
    assert_eq!(context[0], "나: q1 / Tacie: a1");
    assert_eq!(context[2], format!("나: q3 / Tacie: {long}"));
    assert_eq!(context[4], "나: q5 / Tacie: 첫 줄 둘째 줄");
    assert_eq!(context[5], "나: q6 / Tacie: a6");
    // 찾기(리랭커)도 같은 6줄이고 답은 앞 200자만이에요.
    let (rerank_context, line, _) = reranker.calls().remove(0);
    assert_eq!(line, "나: 세션은 어디서 만들어요?");
    assert_eq!(rerank_context.len(), 6, "{rerank_context:?}");
    assert_eq!(rerank_context[0], "나: q1 / Tacie: a1");
    assert_eq!(rerank_context[2], format!("나: q3 / Tacie: {}", "가".repeat(200)));
    assert_eq!(rerank_context[4], "나: q5 / Tacie: 첫 줄 둘째 줄");
}

#[tokio::test]
async fn panel_ask_paraphrased_not_found_drops_cards() {
    let answerer = Arc::new(FakeAnswerer::ok("악성 임대인 명단 공개 얘기는 찾은 자료에 없어요. [1][2][3]"));
    let (status, out) = post(
        state_with(&[], None, &answerer),
        "/panel/ask",
        r#"{"text": "세션은 어디서 만들어요?"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["answer"], "찾은 자료에는 답이 없어요.", "{out}");
    assert_eq!(out["cited"], json!([]));
    assert_eq!(out["cards"], json!([]));
}

#[tokio::test]
async fn panel_ask_null_answer_is_not_found_without_cards() {
    // 인용이 없는 답은 걸러져 answer가 null이 돼요.
    let answerer = Arc::new(FakeAnswerer::ok("세션은 서버가 만들어요"));
    let (status, out) = post(
        state_with(&[], None, &answerer),
        "/panel/ask",
        r#"{"text": "세션은 어디서 만들어요?"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["answer"], "찾은 자료에는 답이 없어요.", "{out}");
    assert_eq!(out["cited"], json!([]));
    assert_eq!(out["cards"], json!([]));
    // 답 모델이 실패하면 오류 그대로예요.
    let failing = Arc::new(FakeAnswerer::fail(tacie_sources::Kind::Network, "x"));
    let (_, out) = post(state_with(&[], None, &failing), "/panel/ask", r#"{"text": "세션은 어디서 만들어요?"}"#).await;
    assert_eq!(out["error_kind"], "network", "{out}");
    assert_eq!(out["answer"], Value::Null, "{out}");
}

async fn panel_ask_keeps_cards_for(answer: &str) {
    let answerer = Arc::new(FakeAnswerer::ok(answer));
    let (status, out) = post(
        state_with(&[], None, &answerer),
        "/panel/ask",
        r#"{"text": "세션은 어디서 만들어요?"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!out["cards"].as_array().unwrap().is_empty(), "{answer}: {out}");
    assert_eq!(out["answer"], answer, "{out}");
}

#[tokio::test]
async fn panel_ask_mixed_or_positive_answers_keep_cards() {
    // 일부만 없는 답: 앞 문장이 내용을 말해요.
    panel_ask_keeps_cards_for("세션은 서버가 만들어요 [1]. 명단 공개는 찾은 자료에 없어요.").await;
    panel_ask_keeps_cards_for("세션은 서버가 만들어요 [1]. 찾은 자료에 없어요.").await;
    // 한 문장에 둘이 섞인 답.
    panel_ask_keeps_cards_for("세션은 서버가 만들고 [1], 명단은 찾은 자료에 없어요.").await;
    // 자료를 근거로 말하는 긍정 답.
    panel_ask_keeps_cards_for("찾은 자료에 따르면 문제가 없어요 [1].").await;
    panel_ask_keeps_cards_for("찾은 자료에서 확인해 보니 이상이 없어요 [1].").await;
}

#[tokio::test]
async fn panel_ask_english_meeting_no_answer_lands_on_not_found() {
    // 영어 회의의 "답 없음"은 엔진이 한국어 문장으로 바꿔 넘겨요(openrouter.rs). 그 문장이 오면 카드를 비워요.
    for answer in ["찾은 자료에는 답이 없어요.", "찾은 자료에는 답이 없어요. [1]"] {
        let answerer = Arc::new(FakeAnswerer::ok(answer));
        let (_, out) = post(
            state_with(&[], None, &answerer),
            "/panel/ask",
            r#"{"text": "Where is the session created?"}"#,
        )
        .await;
        assert_eq!(out["answer"], "찾은 자료에는 답이 없어요.", "{out}");
        assert_eq!(out["cards"], json!([]));
        assert_eq!(out["cited"], json!([]));
    }
}

#[tokio::test]
async fn panel_ask_real_answer_mentioning_found_material_keeps_cards() {
    let answerer = Arc::new(FakeAnswerer::ok("찾은 자료에서 세션은 서버가 만들어요. [1]"));
    let (status, out) = post(
        state_with(&[], None, &answerer),
        "/panel/ask",
        r#"{"text": "세션은 어디서 만들어요?"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!out["cards"].as_array().unwrap().is_empty(), "{out}");
    assert_eq!(out["answer"], "찾은 자료에서 세션은 서버가 만들어요. [1]");
}

#[test]
fn panel_ask_drops_card_ids_that_no_longer_match_the_index() {
    let ix = index();
    let n = ix.chunks.len();
    let good = (0..n).find(|&i| ix.allowed[i]).unwrap();
    let hidden = (0..n).find(|&i| !ix.allowed[i]);
    let path = ix.chunks[good].path.clone();
    let mut cards = vec![
        json!({"chunk": good, "path": path}),
        json!({"chunk": n + 5, "path": path}),
        json!({"chunk": good, "path": "other/path.md"}),
    ];
    if let Some(h) = hidden {
        cards.push(json!({"chunk": h, "path": ix.chunks[h].path}));
    }
    assert_eq!(
        tacie_engine::server::card_chunk_ids(&ix, &json!(cards)),
        vec![good]
    );
}

/// 오픈소스판의 AI 주소(2-A): 키와 같은 요청으로 받고, 응답 모양은 그대로이고, `/config`에 주소와 모델만 보여요.
#[cfg(feature = "local-prompts")]
#[tokio::test]
async fn ai_credentials_take_an_ai_address() {
    let st = state(&[], None);
    let (_, _, before) = get(st.clone(), "/config", Some(&bearer())).await;
    let today_model = before["models"]["answer"].clone();
    assert_eq!(get(st.clone(), "/ai/address", Some(&bearer())).await.2, Value::Null);

    let body = r#"{"chat": {"url": "https://api.openai.com/v1", "model": "gpt-5-mini", "key": "sk-chat-secret"}}"#;
    let (status, got) = post(st.clone(), "/ai/credentials", body).await;
    assert_eq!((status, got), (StatusCode::OK, json!({"typesafe": false, "openrouter": false})));
    let (_, _, addr) = get(st.clone(), "/ai/address", Some(&bearer())).await;
    assert_eq!(addr, json!({"host": "api.openai.com", "model": "gpt-5-mini", "state": "ok"}));
    let (_, _, cfg) = get(st.clone(), "/config", Some(&bearer())).await;
    assert_eq!(cfg["models"]["answer"], "gpt-5-mini");
    assert!(!cfg.to_string().contains("sk-chat-secret") && !addr.to_string().contains("sk-chat-secret"));

    let (status, _) = post(st.clone(), "/ai/credentials", r#"{"chat": {"url": "ftp://x", "model": "m"}}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = post(st.clone(), "/ai/credentials", r#"{"chat": null}"#).await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, after) = get(st.clone(), "/config", Some(&bearer())).await;
    assert_eq!(after["models"]["answer"], today_model);
    assert_eq!(get(st.clone(), "/ai/address", Some(&bearer())).await.2, Value::Null);
}

/// 프롬프트 글이 없는 엔진(공식 배포, iPhone)은 AI 주소를 받지 않아요.
#[cfg(not(feature = "local-prompts"))]
#[tokio::test]
async fn no_local_prompts_rejects_chat() {
    let st = state(&[], None);
    let body = r#"{"chat": {"url": "https://api.openai.com/v1", "model": "gpt-5-mini"}}"#;
    let (status, _) = post(st.clone(), "/ai/credentials", body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// 사용자 주소의 실패는 서비스 `chat`이라 OpenRouter 키를 거절됐다고 보이지 않아요.
#[test]
fn chat_failure_is_its_own_service() {
    let dir = tempfile::tempdir().unwrap();
    let keys = tacie_engine::keys::Keys::new(None);
    keys.set("openrouter", Some("sk-or"));
    let last = std::collections::HashMap::from([("chat".to_string(), Kind::Key)]);
    let states = settings::key_states(dir.path(), &keys, &last, None);
    let or = states.iter().find(|s| s["name"] == "openrouter").unwrap();
    assert_ne!(or["state"], "refused", "{or}");
}

/// 사용자 주소의 실패는 `/ai/address`의 `state`로 보여요(설정 화면이 읽어요, 2-D).
#[cfg(feature = "local-prompts")]
#[tokio::test]
async fn ai_address_shows_its_failure() {
    let st = state(&[], None);
    post(st.clone(), "/ai/credentials", r#"{"chat": {"url": "https://api.openai.com/v1", "model": "gpt-5-mini"}}"#).await;
    assert_eq!(get(st.clone(), "/ai/address", Some(&bearer())).await.2["state"], "ok");
    st.engine.last.lock().unwrap().insert("chat".into(), Kind::Key);
    assert_eq!(get(st.clone(), "/ai/address", Some(&bearer())).await.2["state"], "key");
}

/// 받아쓰기 업체(2-B): 키와 같은 요청으로 받고, 응답 모양은 그대로이고, `/stt/provider`에 업체만 보여요.
#[tokio::test]
async fn ai_credentials_take_stt() {
    let st = state(&[], None);
    assert_eq!(get(st.clone(), "/stt/provider", Some(&bearer())).await.2, Value::Null);
    let (status, got) = post(st.clone(), "/ai/credentials", r#"{"stt": {"provider": "soniox", "key": "sk-stt-secret", "region": "jp"}}"#).await;
    assert_eq!((status, got), (StatusCode::OK, json!({"typesafe": false, "openrouter": false})));
    let (_, _, p) = get(st.clone(), "/stt/provider", Some(&bearer())).await;
    assert_eq!(p, json!({"provider": "soniox", "region": "jp", "state": "ok"}));
    let (status, _) = post(st.clone(), "/ai/credentials", r#"{"stt": {"provider": "nope", "key": "k"}}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    post(st.clone(), "/ai/credentials", r#"{"stt": null}"#).await;
    assert_eq!(get(st.clone(), "/stt/provider", Some(&bearer())).await.2, Value::Null);
}

/// 업체를 바꾸면 지난 받아쓰기 거절을 잊어요.
#[tokio::test]
async fn stt_change_clears_last() {
    let st = state(&[], None);
    post(st.clone(), "/ai/credentials", r#"{"stt": {"provider": "elevenlabs", "key": "k1"}}"#).await;
    st.engine.last.lock().unwrap().insert("stt_provider".into(), Kind::Key);
    assert_eq!(get(st.clone(), "/stt/provider", Some(&bearer())).await.2["state"], "key");
    post(st.clone(), "/ai/credentials", r#"{"stt": {"provider": "soniox", "key": "k2"}}"#).await;
    assert_eq!(get(st.clone(), "/stt/provider", Some(&bearer())).await.2["state"], "ok");
}

/// ElevenLabs가 업체면 `/config`의 받아쓰기 모델 값이 그걸 따라요(칸은 그대로).
#[tokio::test]
async fn config_models_stt_follows_elevenlabs() {
    let st = state(&[], None);
    let today = get(st.clone(), "/config", Some(&bearer())).await.2["models"]["stt"].clone();
    post(st.clone(), "/ai/credentials", r#"{"stt": {"provider": "elevenlabs", "key": "k"}}"#).await;
    assert_eq!(get(st.clone(), "/config", Some(&bearer())).await.2["models"]["stt"], "elevenlabs/scribe_v2");
    post(st.clone(), "/ai/credentials", r#"{"stt": null}"#).await;
    assert_eq!(get(st.clone(), "/config", Some(&bearer())).await.2["models"]["stt"], today);
}

/// 다른 리전의 키처럼 사용자 Soniox 키가 거절되면 502 키 오류이고, `/stt/provider`에 남아요(2-B).
#[tokio::test]
async fn wrong_soniox_key_is_recorded() {
    let fake = common::fake_openrouter::FakeOpenRouter::start(vec![(401, json!("bad key"))]).await;
    // 이 시험만 이 변수를 써요(업체 키 길의 시험용 주소, 개발 빌드만 받아요).
    unsafe { std::env::set_var("TACIE_SONIOX_API", &fake.url_base) };
    let st = session_state();
    post(st.clone(), "/ai/credentials", r#"{"stt": {"provider": "soniox", "key": "sk-user"}}"#).await;
    let (status, out) = call(&st, "POST", "/stt/stream-key", "{}", &[]).await;
    unsafe { std::env::remove_var("TACIE_SONIOX_API") };
    assert_eq!((status, out["error_kind"].clone()), (StatusCode::BAD_GATEWAY, json!("key")));
    assert_eq!(get(st.clone(), "/stt/provider", Some(&bearer())).await.2["state"], "key");
}

/// 업체 실패는 따로 기록돼요: OpenRouter 키를 거절됐다고 보이지 않고, OpenRouter 키가 바뀌어도 업체 상태가 남아요.
#[tokio::test]
async fn provider_failure_is_its_own_record() {
    let st = state(&[], None);
    post(st.clone(), "/ai/credentials", r#"{"stt": {"provider": "elevenlabs", "key": "k"}}"#).await;
    st.engine.last.lock().unwrap().insert("stt_provider".into(), Kind::Key);
    post(st.clone(), "/ai/credentials", r#"{"openrouter": "sk-or-new"}"#).await;
    assert_eq!(get(st.clone(), "/stt/provider", Some(&bearer())).await.2["state"], "key");
    // OpenRouter 받아쓰기의 거절은 업체 상태가 아니에요
    st.engine.last.lock().unwrap().insert("stt".into(), Kind::Key);
    st.engine.last.lock().unwrap().remove("stt_provider");
    assert_eq!(get(st.clone(), "/stt/provider", Some(&bearer())).await.2["state"], "ok");
}

#[test]
fn provider_failure_does_not_blame_openrouter() {
    let dir = tempfile::tempdir().unwrap();
    let keys = tacie_engine::keys::Keys::new(None);
    keys.set("openrouter", Some("sk-or"));
    let last = std::collections::HashMap::from([("stt_provider".to_string(), Kind::Key)]);
    let states = settings::key_states(dir.path(), &keys, &last, None);
    let or = states.iter().find(|s| s["name"] == "openrouter").unwrap();
    assert_ne!(or["state"], "refused", "{or}");
}
