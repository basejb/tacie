//! The local HTTP API the desktop app calls (127.0.0.1 only, token in `TACIE_ENGINE_TOKEN`).
//!
//! The app launches this process and, once the index is loaded, reads the first stdout line
//! `{"port": N, "load_ms": M}`.
//! Every request carries the token, so other local programs or browser pages cannot pull company code out.
//!
//! Index refresh (`/index*`) and GitHub (`/github/*`) routes are here too. Blocking work (deleting, checking, gh
//! and git calls) runs in `spawn_blocking`, so it finishes even if the app drops the connection.
//! - Without a refresher (`refresher: None`), `/index*` is 404 regardless of the body (the same as every missing
//!   route: this server does not read the body of a missing route).
//! - `/index*` and `/github/*` match on the path without the query (`/index?x=1` is `/index` too).
//! - If the body of `POST /github/credentials` is not an object, it is 400 `mode must be …`.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, LazyLock, PoisonError};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use regex::Regex;
use serde_json::{Map, Value, json};
use subtle::ConstantTimeEq;
use tacie_embed::voice::VOICE_DIM;
use tacie_sources::asana::{self, AsanaFetch};
use tacie_sources::atlassian::{self, AtlassianFetch};
use tacie_sources::dropbox::{self, DropboxFetch};
use tacie_sources::gdrive::{self, GDriveFetch};
use tacie_sources::github::{GitHub, LoginState, valid_repo_name};
use tacie_sources::linear::{self, Change, LinearFetch};
use tacie_sources::meeting::{self, Meeting};
use tacie_sources::notes::{self, Folder};
use tacie_sources::notion::{self, NotionFetch};
use tacie_sources::py;

use crate::errors::Kind;
use crate::keys::{AI_KEYS, MAX_KEY_LEN};
use crate::notes::{is_backchannel, one_line};
use crate::notes_kind::NotesKind;
use crate::pipeline::Engine;
use crate::refresh::{Refresher, Target, describe, rebuild_target};
use crate::sample::{MODEL_MISSING, SAMPLE_BASE};
use crate::settings::{self, DEFAULTS};
use crate::voices::{Channel, MAX_NUMBER, Person, Restored, Voices, number_of};

/// 앱이 보내는 화자 이름이에요. 그 밖의 값은 이름이 없어요.
pub const SPEAKERS: [(&str, &str); 2] = [("me", "나"), ("them", "상대")];
/// 발화 하나의 WAV 크기 한도예요: 16 kHz 모노 16비트로 30초에 헤더 몫을 더했어요.
pub const MAX_AUDIO_BYTES: usize = 16000 * 2 * 30 + 1024;

/// 회의 세션 경로예요. 1: 세션 id, 2: 뒤 경로, 3: 카드를 찾는 발화 id.
/// 퍼센트 인코딩을 풀지 않은 날 경로에 대요(`/sessions/s%31`은 s1이 아니에요).
static SESSION_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^/sessions/(s[0-9]+)(/utterances|/audio|/ledger|/voices|/names|/split|/lookup|/terms|/utterances/(s[0-9]+u[0-9]+)/cards)?$",
    )
    .expect("session path regex")
});

pub struct AppState {
    /// 색인, 임베더, 리랭커, 답 모델, 설정, Jev 공급자, 마지막 거절이 모두 여기 있어요.
    pub engine: Arc<Engine>,
    pub token: String,
    /// GitHub 연결이에요. 갱신기(`refresher`)의 출처와 같은 것이에요.
    pub github: Arc<GitHub>,
    /// 색인 갱신이에요. 없으면 `/index*` 경로가 없어요.
    pub refresher: Option<Arc<Refresher>>,
    /// 갱신기가 없는 엔진(iPhone)이 회의 파일을 두는 폴더예요. 갱신기가 있으면 그 폴더를 먼저 써요.
    pub meeting_dir: Option<std::path::PathBuf>,
}

/// 회의 파일 폴더예요: 갱신기의 폴더가 먼저고, 없으면 `AppState.meeting_dir`이에요.
pub fn meeting_dir(state: &AppState) -> Option<std::path::PathBuf> {
    state
        .refresher
        .as_ref()
        .map(|r| r.meeting_dir())
        .or_else(|| state.meeting_dir.clone())
}

/// GitHub 목록이 실패했을 때의 고정 글이에요. gh가 찍은 글이나 토큰은 응답에 넣지 않아요.
const GITHUB_UNAVAILABLE: &str = "GitHub is not available";
/// `POST /sessions`의 `people`·`restore`가 틀렸을 때예요. 받은 값은 글에 넣지 않아요.
pub const SEND_VOICES: &str = "people and restore must be lists of voices";
/// `POST /sessions/{sid}/names`의 본문이 틀렸을 때예요.
pub const SEND_NAMES: &str = "names must map speakers to names";
/// `POST /index/rebuild`의 본문이 틀렸을 때예요.
const SEND_TARGET: &str = "send {repo?: owner/name, branch?: a branch name, fresh?: true or false}";
/// `POST /ai/credentials`의 본문이 틀렸을 때예요. 받은 값은 글에 넣지 않아요.
pub const SEND_AI_KEYS: &str =
    "send {typesafe?: key or null, openrouter?: key or null}: printable ASCII keys only";
/// `POST /stt`의 본문이 틀렸을 때예요.
pub const SEND_STT: &str = "send {on: true or false, model: a file path or null}";
/// `POST /github/credentials`의 본문이 틀렸을 때예요.
const MODE_MUST_BE: &str = "mode must be token (with a token), gh or none";
/// gh 모드로 시작하지 않은 엔진(`TACIE_GITHUB_MODE=gh`가 아님)에 gh를 보냈을 때예요: 제품 엔진은 `gh`를 띄우지 않아요.
pub const GH_MODE_OFF: &str =
    "gh mode is off: the engine was not started with TACIE_GITHUB_MODE=gh";

/// 토큰부터 봐요: 없는 경로라도 토큰이 틀리면 404가 아니라 401이에요.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/config", get(config))
        .route("/ai/address", get(ai_address))
        .route("/stt/provider", get(stt_provider))
        .route("/index", get(index_info))
        .route("/index/refresh", post(index_refresh))
        .route("/index/clear", post(index_clear))
        .route("/index/check", post(index_check))
        .route("/index/rebuild", post(index_rebuild))
        .route("/github/credentials", post(github_credentials))
        .route("/ai/credentials", post(ai_credentials))
        .route("/stt", post(stt_settings))
        .route("/stt/stream-key", post(stt_stream_key))
        .route("/notion/credentials", post(notion_credentials))
        .route("/linear/credentials", post(linear_credentials))
        .route("/linear/teams", get(linear_teams))
        .route("/gdrive/credentials", post(gdrive_credentials))
        .route("/gdrive/drives", get(gdrive_drives))
        .route("/atlassian/credentials", post(atlassian_credentials))
        .route("/atlassian/scopes", get(atlassian_scopes))
        .route("/asana/credentials", post(asana_credentials))
        .route("/asana/scopes", get(asana_scopes))
        .route("/dropbox/credentials", post(dropbox_credentials))
        .route("/dropbox/folders", get(dropbox_folders))
        .route("/meetings", post(meetings))
        .route("/notes/folders", post(notes_folders))
        .route("/github/repos", get(github_repos))
        .route("/github/branches", get(github_branches))
        .route("/ask", post(ask))
        .route("/answer", post(answer))
        .route("/ledger", post(ledger))
        .route("/tools/meetings", post(tools_meetings))
        .route("/tools/ledger", post(tools_ledger))
        .route("/tools/search", post(tools_search))
        .route("/panel/prep", post(panel_prep))
        .route("/panel/why", post(panel_why))
        .route("/panel/ask", post(panel_ask))
        .route("/panel/chat", post(panel_chat))
        .route("/panel/followup", post(panel_followup))
        .route("/decisions/changes", post(decision_changes))
        .route("/questions/review", post(review_questions))
        .route("/sessions", post(create_session))
        .route(
            "/sessions/{sid}",
            get(session_summary).delete(close_session),
        )
        .route("/sessions/{sid}/utterances", post(session_utterance))
        .route("/sessions/{sid}/audio", post(session_audio))
        .route("/sessions/{sid}/ledger", post(session_ledger))
        .route("/sessions/{sid}/voices", post(session_voices))
        .route("/sessions/{sid}/names", post(session_names))
        .route("/sessions/{sid}/split", post(session_split))
        .route("/sessions/{sid}/lookup", post(session_lookup))
        .route("/sessions/{sid}/terms", get(session_terms))
        .route(
            "/sessions/{sid}/utterances/{uid}/cards",
            post(utterance_cards),
        )
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), authorize))
        .with_state(state)
}

/// 127.0.0.1:port에 묶어요(0이면 아무 빈 포트). 묶인 주소와, 기다리면 서버가 도는 future를 돌려줘요.
pub async fn serve(
    state: Arc<AppState>,
    port: u16,
) -> std::io::Result<(SocketAddr, impl Future<Output = std::io::Result<()>>)> {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    let addr = listener.local_addr()?;
    let app = router(state);
    Ok((addr, async move { axum::serve(listener, app).await }))
}

async fn authorize(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let got = req
        .headers()
        .get(header::AUTHORIZATION)
        .map_or(&b""[..], |v| v.as_bytes());
    // 상수 시간 비교예요.
    let ok = bool::from(got.ct_eq(format!("Bearer {}", state.token).as_bytes()));
    if ok {
        next.run(req).await
    } else {
        send(StatusCode::UNAUTHORIZED, json!({"error": "unauthorized"}))
    }
}

async fn health(State(state): State<Arc<AppState>>) -> Response {
    let ix = state.engine.index.load();
    let meta = |key: &str| ix.meta.get(key).cloned().unwrap_or(Value::Null);
    send(
        StatusCode::OK,
        json!({
            "ready": true,
            "jev": state.engine.jev.provider_name(),
            "mask": ix.mask.to_string(),
            "index": {"chunks": ix.chunks.len(), "repo": meta("repo"), "commit": meta("commit"),
                      "built_at": meta("built_at")},
        }),
    )
}

async fn config(State(state): State<Arc<AppState>>) -> Response {
    let s = &state.engine.settings;
    let jev = &state.engine.jev;
    let address = jev.keys().address();
    // 키가 있는지는 요청마다 봐요(설정 화면이 새로 넣은 키를 바로 보여요).
    let keys = {
        let last = state
            .engine
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        settings::key_states(
            &state.engine.config_dir,
            jev.keys(),
            &last,
            jev.provider_name(),
        )
    };
    send(
        StatusCode::OK,
        json!({
            "models": {"stt": if matches!(jev.keys().stt(), Some(crate::stt_provider::SttProvider::ElevenLabs { .. })) { "elevenlabs/scribe_v2" } else { s.stt_model.as_str() }, "answer": address.as_ref().map_or(s.answer_model.as_str(), |a| a.model.as_str()), "jev": jev.model()},
            "model_defaults": {"stt": DEFAULTS.0, "answer": DEFAULTS.1, "jev": jev.model_default()},
            "thresholds": {"gate": s.gate, "show": s.show, "end": s.end},
            "threshold_defaults": {"gate": DEFAULTS.2, "show": DEFAULTS.3, "end": DEFAULTS.4},
            "keys": keys,
            "log_text": s.log_text,
        }),
    )
}

/// 지금 AI 주소예요(2-A): `{host, model}` 또는 `null`. 키와 전체 주소는 보여 주지 않아요. `/config`의 칸은 계약이라
/// 따로 둬요(engine/contract/test_core.py).
async fn ai_address(State(state): State<Arc<AppState>>) -> Response {
    let a = state.engine.jev.keys().address();
    // 마지막 실패의 종류예요(key, credit, network, busy, down …), 없으면 "ok"
    let failed = state.engine.last.lock().unwrap_or_else(PoisonError::into_inner).get("chat").copied();
    let st = failed.map_or("ok", |k| k.as_str());
    send(
        StatusCode::OK,
        a.map_or(Value::Null, |a| json!({"host": a.host(), "model": a.model, "state": st})),
    )
}

/// 지금 받아쓰기 업체예요(2-B): `{provider, region?, state}` 또는 `null`. 키는 보여 주지 않아요.
async fn stt_provider(State(state): State<Arc<AppState>>) -> Response {
    let p = state.engine.jev.keys().stt();
    let failed = state.engine.last.lock().unwrap_or_else(PoisonError::into_inner).get("stt_provider").copied();
    let st = failed.map_or("ok", |k| k.as_str());
    send(
        StatusCode::OK,
        p.map_or(Value::Null, |p| match p {
            crate::stt_provider::SttProvider::Soniox { region, .. } => json!({"provider": "soniox", "region": region, "state": st}),
            crate::stt_provider::SttProvider::ElevenLabs { .. } => json!({"provider": "elevenlabs", "state": st}),
        }),
    )
}

/// 색인과 갱신 상태, 연결이에요(`GET /index`): 키 순서는 `chunks, commit, built_at, refresh`, 그다음
/// `describe`, 그다음 `github_login, connected, cleared`예요. login은 gh를 띄울 수 있어서(5 s) 따로 돌려요.
async fn index_info(State(state): State<Arc<AppState>>) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let (engine, github) = (state.engine.clone(), state.github.clone());
    blocking(move || {
        let ix = engine.index.load_full();
        let meta = |key: &str| ix.meta.get(key).cloned().unwrap_or(Value::Null);
        let mut out = Map::new();
        out.insert("chunks".into(), Value::from(ix.chunks.len()));
        out.insert("commit".into(), meta("commit"));
        out.insert("built_at".into(), meta("built_at"));
        out.insert("refresh".into(), (*r.status()).clone());
        for (k, v) in describe(&ix.raw, &ix.meta, r.index_dir()) {
            out.insert(k, v);
        }
        // 로그인을 한 번만 물어요(모르면 기억하지 않아서 두 번 물으면 두 번 기다려요)
        let (connected, login) = github_presence(&github);
        out.insert("github_login".into(), Value::from(login));
        out.insert("connected".into(), Value::Bool(connected));
        out.insert(
            "cleared".into(),
            Value::Bool(ix.meta.get("cleared_at").is_some_and(py::truthy)),
        );
        // 노션 연결과 받은 것이에요(docs/13): 토큰은 넣지 않아요.
        let ns = &r.sources().notion;
        let got = ix
            .meta
            .get("sources")
            .and_then(|s| s.get("notion"))
            .cloned()
            .unwrap_or(Value::Null);
        out.insert(
            "notion".into(),
            json!({"connected": ns.connected(), "workspace": ns.workspace(), "refused": ns.refused(),
                   "pages": got.get("pages").cloned().unwrap_or(Value::Null),
                   "fetched_at": got.get("fetched_at").cloned().unwrap_or(Value::Null)}),
        );
        // Linear 연결과 받은 것이에요(docs/18): 토큰은 넣지 않아요.
        let ls = &r.sources().linear;
        let got = ix
            .meta
            .get("sources")
            .and_then(|s| s.get("linear"))
            .cloned()
            .unwrap_or(Value::Null);
        let pick = |k: &str| got.get(k).cloned().unwrap_or(Value::Null);
        out.insert(
            "linear".into(),
            json!({"connected": ls.connected(), "workspace": ls.workspace(), "refused": ls.refused(),
                   "issues": pick("issues"), "teams": pick("teams"), "fetched_at": pick("fetched_at"),
                   "error_kind": ls.error_kind().map_or_else(|| pick("error_kind"), Value::from),
                   "lost_since": pick("lost_since")}),
        );
        // Google Drive 연결과 받은 것이에요(docs/19): 토큰은 넣지 않아요. `refused`가 참이면 앱이 토큰을 갱신해요.
        let gs = &r.sources().gdrive;
        let got = ix
            .meta
            .get("sources")
            .and_then(|s| s.get("gdrive"))
            .cloned()
            .unwrap_or(Value::Null);
        let pick = |k: &str| got.get(k).cloned().unwrap_or(Value::Null);
        out.insert(
            "gdrive".into(),
            json!({"connected": gs.connected(), "account": gs.account(), "refused": gs.refused(),
                   "files": pick("files"), "drives": pick("drives"), "fetched_at": pick("fetched_at"),
                   "initial_done": pick("initial_done"),
                   "error_kind": gs.error_kind().map_or_else(|| pick("error_kind"), Value::from),
                   "lost_since": pick("lost_since")}),
        );
        // Jira·Confluence 연결과 받은 것이에요(docs/31): 토큰은 넣지 않아요. `refused`가 참이면 앱이 토큰을 갱신해요.
        // `paused_until`은 시간당 한도로 멈춘 때까지이고 지나면 null이에요.
        let at = &r.sources().atlassian;
        let got = ix
            .meta
            .get("sources")
            .and_then(|s| s.get("atlassian"))
            .cloned()
            .unwrap_or(Value::Null);
        let pick = |k: &str| got.get(k).cloned().unwrap_or(Value::Null);
        // 지금 값이 지난 멈춤이면 meta 값으로 가요(지난 값이 meta의 아직 앞인 멈춤을 가리지 않게 먼저 걸러요).
        let ahead = |t: &String| {
            chrono::DateTime::parse_from_rfc3339(t)
                .is_ok_and(|t| t.with_timezone(&chrono::Utc) > chrono::Utc::now())
        };
        let paused = at.paused_until().filter(ahead).or_else(|| {
            pick("paused_until")
                .as_str()
                .map(str::to_string)
                .filter(ahead)
        });
        // 다시 연결해 새 토큰이 왔으면 그 토큰으로 받기 전까지 지난 끊김을 보이지 않아요
        let lost_since = if at.token_untried() {
            Value::Null
        } else {
            pick("lost_since")
        };
        out.insert(
            "atlassian".into(),
            json!({"connected": at.connected(), "site": at.site(), "refused": at.refused(),
                   "issues": pick("issues"), "pages": pick("pages"), "fetched_at": pick("fetched_at"),
                   "initial_done": pick("initial_done"),
                   "error_kind": at.error_kind().map_or_else(|| pick("error_kind"), Value::from),
                   "lost_since": lost_since, "paused_until": paused}),
        );
        // Asana 연결과 받은 것이에요(docs/32): 토큰은 넣지 않아요. `refused`가 참이면 앱이 토큰을 갱신해요.
        let asn = &r.sources().asana;
        let got = ix
            .meta
            .get("sources")
            .and_then(|s| s.get("asana"))
            .cloned()
            .unwrap_or(Value::Null);
        let pick = |k: &str| got.get(k).cloned().unwrap_or(Value::Null);
        // 다시 연결해 새 토큰이 왔으면 그 토큰으로 받기 전까지 지난 끊김을 보이지 않아요
        let lost_since = if asn.token_untried() {
            Value::Null
        } else {
            pick("lost_since")
        };
        out.insert(
            "asana".into(),
            json!({"connected": asn.connected(), "refused": asn.refused(),
                   "workspaces": pick("workspaces"), "tasks": pick("tasks"),
                   "fetched_at": pick("fetched_at"), "initial_done": pick("initial_done"),
                   "error_kind": asn.error_kind().map_or_else(|| pick("error_kind"), Value::from),
                   "lost_since": lost_since, "blocked_workspaces": pick("blocked_workspaces")}),
        );
        // Dropbox 연결과 받은 것이에요(docs/33): 토큰은 넣지 않아요. `refused`가 참이면 앱이 토큰을 갱신해요.
        let db = &r.sources().dropbox;
        let got = ix
            .meta
            .get("sources")
            .and_then(|s| s.get("dropbox"))
            .cloned()
            .unwrap_or(Value::Null);
        let pick = |k: &str| got.get(k).cloned().unwrap_or(Value::Null);
        // 연결했는데 받은 것이 meta에 없으면(처음 받기가 실패해 색인을 그대로 둔 갱신도요) 처음 받기는 아직 끝나지
        // 않았어요
        let initial_done = match got.get("initial_done") {
            Some(v) => v.clone(),
            None if db.connected() => Value::Bool(false),
            None => Value::Null,
        };
        out.insert(
            "dropbox".into(),
            json!({"connected": db.connected(), "account": db.account(),
                   "files": pick("files"), "fetched_at": pick("fetched_at"),
                   "initial_done": initial_done, "refused": db.refused(),
                   "error_kind": db.error_kind().map_or_else(|| pick("error_kind"), Value::from),
                   "lost_since": pick("lost_since")}),
        );
        // 이 Mac의 노트 폴더예요(docs/34): 고른 폴더 수와 지난번에 읽은 결과예요.
        let read = ix
            .meta
            .get("sources")
            .and_then(|s| s.get("notes"))
            .cloned()
            .unwrap_or(Value::Null);
        let field = |k: &str| read.get(k).cloned().unwrap_or(Value::Null);
        // 고른 폴더마다 지난번에 읽은 결과를 붙여요(아직 읽지 않았으면 `notes`가 null이에요)
        let chosen = notes::folders(&r.notes_dir());
        let folders: Vec<Value> = chosen
            .iter()
            .map(|f| {
                let id = f.id();
                let got = read
                    .get("folders")
                    .and_then(Value::as_array)
                    .and_then(|a| a.iter().find(|x| x["id"] == id.as_str()));
                json!({"id": id, "path": f.path, "name": f.label(), "vault_id": f.vault_id,
                       "obsidian": f.obsidian(),
                       "notes": got.map_or(Value::Null, |g| g["notes"].clone()),
                       "missing": !std::path::Path::new(&f.path).is_dir()})
            })
            .collect();
        out.insert(
            "notes".into(),
            json!({"chosen": chosen.len(), "folders": folders, "notes": field("notes"),
                   "not_downloaded": field("not_downloaded"), "read_at": field("read_at")}),
        );
        // 검색 모델이 있는지, 온보딩 연습의 예시 자료(docs/11 12절)예요.
        out.insert("model_ready".into(), Value::Bool(engine.model_ready));
        // 화자 임베딩 모델이 있는지예요(없으면 들은 발화의 `voice`가 늘 null이에요).
        out.insert("voice_ready".into(), Value::Bool(engine.voice.is_some()));
        out.insert(
            "sample".into(),
            match &engine.sample {
                Some(s) => json!({"ready": s.index.searchable(), "questions": s.questions}),
                None => json!({"ready": false, "questions": []}),
            },
        );
        send(StatusCode::OK, Value::Object(out))
    })
    .await
}

/// 색인의 저장소와 브랜치로 뒤에서 갱신해요: `{started, refresh}`. 본문은 읽지 않아요.
async fn index_refresh(State(state): State<Arc<AppState>>) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    if let Some(no) = no_model(&state) {
        return no;
    }
    started(&r, Target::default())
}

/// 색인을 비워요: 200 `{cleared: true}`, 갱신이나 지우기가 도는 중이면 409 `{cleared: false}`. 본문은 읽지 않아요.
async fn index_clear(State(state): State<Arc<AppState>>) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    blocking(move || {
        let cleared = r.clear();
        let status = if cleared {
            StatusCode::OK
        } else {
            StatusCode::CONFLICT
        };
        send(status, json!({"cleared": cleared}))
    })
    .await
}

/// 갱신이 새것을 찾을지예요: 200 `{code, github, changed}`, 바쁘면 409 `{changed: false}`. 본문은 읽지 않아요.
async fn index_check(State(state): State<Arc<AppState>>) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let ready = state.engine.model_ready;
    blocking(move || {
        // 노트는 이 Mac의 파일이라 묻는 비용이 없어요: 바뀌었으면 노트 몫만 바로 다시 넣어요(docs/34 5-5)
        if ready && r.notes_stale_quickly() {
            r.notes_changed();
        }
        match r.check() {
            Some(found) => send(StatusCode::OK, found),
            None => send(StatusCode::CONFLICT, json!({"changed": false})),
        }
    })
    .await
}

/// 다른 저장소나 브랜치, 또는 처음부터: `{"repo"?, "branch"?, "fresh"?}` → `{started, refresh}`. 브랜치 이름은
/// 순수 Rust로 검사해요.
async fn index_rebuild(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    if let Some(no) = no_model(&state) {
        return no;
    }
    blocking(move || match rebuild_target(&body) {
        Some(target) => started(&r, target),
        None => error(StatusCode::BAD_REQUEST, SEND_TARGET),
    })
    .await
}

/// 검색 모델을 아직 받지 않았으면 동기화를 시작하지 않아요: 코드와 GitHub 받기를 다 하고 임베딩에서 실패하는
/// 대신 바로 503 `model`이에요(앱이 "검색 모델을 받고 있어요"를 보여 줘요).
fn no_model(state: &AppState) -> Option<Response> {
    (!state.engine.model_ready).then(|| {
        send(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": MODEL_MISSING, "error_kind": Kind::Model}),
        )
    })
}

/// `start`의 답이에요(스레드를 띄우기만 해서 막히지 않아요).
fn started(r: &Arc<Refresher>, target: Target) -> Response {
    let started = r.start(target);
    send(
        StatusCode::OK,
        json!({"started": started, "refresh": *r.status()}),
    )
}

/// GitHub에 어떻게 닿을지예요: `{"mode": "token", "token": …}`, `{"mode": "gh"}`, `{"mode": "none"}` →
/// `{connected, login}`. 토큰은 연결에만 두고 응답·로그에 넣지 않아요.
async fn github_credentials(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let mode = body.get("mode").and_then(Value::as_str).unwrap_or_default();
    let token = body
        .get("token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty());
    let ok = match mode {
        "gh" | "none" => true,
        "token" => token.is_some(),
        _ => false,
    };
    if !ok {
        return error(StatusCode::BAD_REQUEST, MODE_MUST_BE);
    }
    if mode == "gh" && !state.github.gh_allowed() {
        return error(StatusCode::BAD_REQUEST, GH_MODE_OFF);
    }
    let (mode, token) = (mode.to_string(), token.map(str::to_string));
    let github = state.github.clone();
    blocking(move || {
        github.set_credentials(&mode, token.as_deref());
        let (connected, login) = github_presence(&github);
        send(
            StatusCode::OK,
            json!({"connected": connected, "login": login}),
        )
    })
    .await
}

/// 이 Mac에서 받아쓸지 정해요: `{"on": bool, "model": 파일 경로|null}` → 지금 상태예요(`RoutedStt::status`).
/// 모델은 뒤에서 올라와서 `ready`가 곧 바뀔 수 있어요. 앱은 같은 값을 다시 보내 상태를 물어봐도 돼요.
/// `X-Transcript`: base64로 싼 JSON `{"text", "words": [{"text", "start", "end"}]}`(초, WAV 처음 기준)예요.
fn given_transcript(header: &str) -> Option<crate::services::Transcript> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(header.trim())
        .ok()?;
    let v: Value = serde_json::from_slice(&raw).ok()?;
    let text = v.get("text")?.as_str()?.to_string();
    let words = v
        .get("words")
        .and_then(Value::as_array)
        .map(|ws| {
            ws.iter()
                .filter_map(|w| {
                    Some(crate::services::SpokenWord {
                        text: w.get("text")?.as_str()?.to_string(),
                        start: w.get("start")?.as_f64()?,
                        end: w.get("end")?.as_f64()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(crate::services::Transcript {
        text,
        ms: 0.0,
        cost: 0.0,
        confidence: None,
        words,
        via: crate::services::SttVia::Stream,
    })
}

/// 개발 빌드의 그림자 받아쓰기(docs/25): 키 파일의 Soniox 키(`~/.config/soniox/config.json`의 `apiKey`, 개발 엔진만
/// 읽어요)로 실시간 연결용 임시 키를 만들어 줘요. 앱은 키 파일을 읽지 않고 이 임시 키만 다뤄요. 리전은
/// `TACIE_SONIOX_REGION`(`jp`면 일본, 키 파일 `~/.config/soniox-jp`)이에요.
async fn stt_stream_key(State(state): State<Arc<AppState>>) -> Response {
    let region =
        crate::soniox::Region::from_env(std::env::var("TACIE_SONIOX_REGION").ok().as_deref());
    let keys = state.engine.jev.keys();
    // 사용자가 고른 Soniox 키(2-B)가 먼저예요. 시험용 주소 덮어쓰기는 개발 빌드에서만 받아요.
    if let Some(crate::stt_provider::SttProvider::Soniox { key, region }) = keys.stt() {
        let over = |name: &str| std::env::var(name).ok().filter(|_| cfg!(debug_assertions));
        let (api, ws) = (over("TACIE_SONIOX_API"), over("TACIE_SONIOX_WS"));
        return match crate::soniox::provider_key(region, &key, api.as_deref(), ws.as_deref()).await {
            Ok((k, ws)) => {
                state.engine.last.lock().unwrap_or_else(PoisonError::into_inner).remove("stt_provider");
                send(StatusCode::OK, json!({"api_key": k.api_key, "expires_at": k.expires_at, "ws_url": ws}))
            }
            Err(e) => {
                // 업체 실패는 따로 남겨요(OpenRouter 받아쓰기의 "stt"와 섞이지 않게). 거절(키·한도)만이에요.
                if matches!(e.kind, Kind::Key | Kind::Credit) {
                    state.engine.last.lock().unwrap_or_else(PoisonError::into_inner).insert("stt_provider".into(), e.kind);
                }
                send(StatusCode::BAD_GATEWAY, json!({"error": "stream key failed", "error_kind": e.kind}))
            }
        };
    }
    let Some(key) = keys.find(region.key_name).and_then(|f| f.key) else {
        // 배포 앱: 키 파일이 없고 OpenRouter 주소가 Tacie 중계예요. 중계에서 받아요(로그인 세션으로).
        let base = std::env::var("TACIE_OPENROUTER_URL").unwrap_or_default();
        let (Some(url), Some(token)) = (
            crate::soniox::relay_key_url(&base),
            keys.find("openrouter").and_then(|f| f.key),
        ) else {
            return send(
                StatusCode::NOT_FOUND,
                json!({"error": "no Soniox key", "error_kind": Kind::Key}),
            );
        };
        let http = settings::http_client(&url, HeaderMap::new());
        return match crate::soniox::relay_key(&http, &url, &token, region.name).await {
            Ok(k) => send(
                StatusCode::OK,
                json!({"api_key": k.api_key, "expires_at": k.expires_at, "ws_url": k.ws_url}),
            ),
            Err(e) => send(
                StatusCode::BAD_GATEWAY,
                json!({"error": "stream key failed", "error_kind": e.kind}),
            ),
        };
    };
    // 시험용 가짜 서버를 가리킬 때만 주소를 바꿔요(테스트·eval).
    let api = std::env::var("TACIE_SONIOX_API").unwrap_or_else(|_| region.api.to_string());
    let ws = std::env::var("TACIE_SONIOX_WS").unwrap_or_else(|_| region.ws.to_string());
    let http = settings::http_client(&api, HeaderMap::new());
    match crate::soniox::mint_key(&http, &api, &key).await {
        Ok(k) => send(
            StatusCode::OK,
            json!({"api_key": k.api_key, "expires_at": k.expires_at, "ws_url": ws}),
        ),
        Err(e) => {
            eprintln!("soniox: {e}");
            send(
                StatusCode::BAD_GATEWAY,
                json!({"error": "Soniox key failed", "error_kind": e.kind}),
            )
        }
    }
}

/// 이 회의의 팀 용어예요(`terms.rs`, 회의 시작 때 뽑는 것과 같아요). 그림자 받아쓰기가 Soniox `context.terms`로 보내요.
/// 예시 자료 회의(온보딩 연습)는 내 팀 용어를 쓰지 않아서 빈 목록이에요.
async fn session_terms(State(state): State<Arc<AppState>>, uri: Uri) -> Response {
    let Some((sid, _)) = session_path(&uri) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(session) = state.engine.session(sid) else {
        return error(StatusCode::NOT_FOUND, "no such session");
    };
    let terms = if session.sample {
        Vec::new()
    } else {
        state.engine.term_fixer().list.clone()
    };
    send(StatusCode::OK, json!({"terms": terms}))
}

async fn stt_settings(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(stt) = state.engine.stt.as_ref() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let on = body.get("on").and_then(Value::as_bool);
    let model = match body.get("model") {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(p)) if !p.is_empty() => Some(Some(std::path::PathBuf::from(p))),
        Some(_) => None,
    };
    let (Some(on), Some(model)) = (on, model) else {
        return error(StatusCode::BAD_REQUEST, SEND_STT);
    };
    stt.set(on, model);
    let st = stt.status();
    send(
        StatusCode::OK,
        json!({"on": st.on, "ready": st.ready, "loading": st.loading, "broken": st.broken,
        "device": st.device.map(|d| match d {
            crate::local_stt::Device::Metal => "metal",
            crate::local_stt::Device::Cpu => "cpu",
        })}),
    )
}

/// AI 키를 엔진 메모리에 둬요(#7 5단계): `{"typesafe"?: 키 | null, "openrouter"?: 키 | null}` → 키마다 지금 쓸 수
/// 있는 키가 있는지(`{"typesafe": bool, "openrouter": bool}`, 개발 엔진은 키 파일도 쳐요)예요. 키 글자는 응답·로그·
/// 파일에 남기지 않아요. 빠진 이름은 그대로 두고, null이나 빈 글은 앱이 넘긴 키를 지워요. 하나라도 틀리면 아무것도
/// 바꾸지 않고 400이에요. 키가 바뀌면 그 키를 쓰는 서비스의 지난 거절(`last`)을 잊어요: 새 키는 다음 호출이 판단해요.
async fn ai_credentials(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let Some(body) = body.as_object() else {
        return error(StatusCode::BAD_REQUEST, SEND_AI_KEYS);
    };
    // AI 주소(2-A): 프롬프트 글이 있는 엔진만 받아요. 글이 없는 엔진은 프롬프트 이름만 보내서 사용자 주소로 보낼
    // 요청을 만들 수 없어요(공식 배포, iPhone).
    let address = match body.get("chat") {
        None => None,
        Some(v) => {
            if !cfg!(feature = "local-prompts") && !v.is_null() {
                return error(
                    StatusCode::BAD_REQUEST,
                    "this engine names its prompts and cannot send them to an AI address",
                );
            }
            match crate::ai_address::AiAddress::parse(v) {
                Ok(a) => Some(a),
                Err(why) => return error(StatusCode::BAD_REQUEST, &why),
            }
        }
    };
    // 받아쓰기 업체(2-B): 키 칸과 같은 규칙(없으면 그대로, null은 끄기)이에요.
    let stt = match body.get("stt") {
        None => None,
        Some(v) => match crate::stt_provider::SttProvider::parse(v) {
            Ok(p) => Some(p),
            Err(why) => return error(StatusCode::BAD_REQUEST, &why),
        },
    };
    let mut changes = Vec::new();
    for name in AI_KEYS {
        let key = match body.get(name) {
            None => continue,
            Some(Value::Null) => None,
            Some(Value::String(k)) => Some(k.trim()),
            Some(_) => return error(StatusCode::BAD_REQUEST, SEND_AI_KEYS),
        };
        // 헤더에 실을 수 있는 글자(보이는 ASCII)만 받아요. 아니면 서버에 닿기 전에 알려 줘요.
        if key.is_some_and(|k| k.len() > MAX_KEY_LEN || !k.bytes().all(|b| b.is_ascii_graphic())) {
            return error(StatusCode::BAD_REQUEST, SEND_AI_KEYS);
        }
        changes.push((name, key));
    }
    let keys = state.engine.jev.keys();
    let address_changed = address.is_some_and(|a| keys.set_address(a));
    if stt.is_some_and(|p| keys.set_stt(p)) {
        state.engine.last.lock().unwrap_or_else(PoisonError::into_inner).remove("stt_provider");
    }
    let changed: Vec<&str> = changes
        .into_iter()
        .filter(|&(name, key)| keys.set(name, key))
        .map(|(name, _)| name)
        .collect();
    // 바뀐 키를 쓰는 서비스의 지난 거절만 잊어요: OpenRouter 키는 받아쓰기·답·장부, 둘 다 Jev(공급자가 바뀔 수
    // 있어요)예요. 같은 키를 다시 넘기면(엔진이 준비될 때마다) 아무것도 잊지 않아요.
    if address_changed {
        state.engine.last.lock().unwrap_or_else(PoisonError::into_inner).remove("chat");
    }
    if !changed.is_empty() {
        let mut last = state
            .engine
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        last.retain(|service, _| {
            let uses_changed = service == "jev"
                || (changed.contains(&"openrouter")
                    && matches!(service.as_str(), "stt" | "answer" | "ledger" | "classify"));
            !uses_changed
        });
    }
    let out: Map<String, Value> = AI_KEYS
        .iter()
        .map(|&n| (n.to_string(), Value::Bool(keys.has(n))))
        .collect();
    send(StatusCode::OK, Value::Object(out))
}

/// `POST /notion/credentials`의 본문이 틀렸을 때예요.
const SEND_NOTION: &str = "send {token: key or null, workspace?: name}: printable ASCII token only";

/// 노션 연결을 엔진 메모리에 둬요(docs/13): `{"token": 키 | null, "workspace"?: 이름}` → `{connected, workspace}`.
/// 토큰은 응답·로그·파일에 남기지 않아요. 연결이 바뀌면 갱신을 걸어요(검색 모델이 있을 때). null은 연결 해제예요:
/// 받은 노션 자료를 지우고 갱신이 노션 조각을 빼요.
async fn notion_credentials(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let token = match body.get("token") {
        Some(Value::Null) => None,
        Some(Value::String(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
        _ => return error(StatusCode::BAD_REQUEST, SEND_NOTION),
    };
    if token
        .as_deref()
        .is_some_and(|k| k.len() > MAX_KEY_LEN || !k.bytes().all(|b| b.is_ascii_graphic()))
    {
        return error(StatusCode::BAD_REQUEST, SEND_NOTION);
    }
    let workspace = match body.get("workspace") {
        None | Some(Value::Null) => None,
        Some(Value::String(w)) => Some(w.clone()),
        Some(_) => return error(StatusCode::BAD_REQUEST, SEND_NOTION),
    };
    let ready = state.engine.model_ready;
    blocking(move || {
        let ns = &r.sources().notion;
        // 엔진이 새로 떠서 앱이 저장한 연결을 다시 넘긴 것(받아 둔 페이지가 있어요)은 새 연결이 아니에요: 동기화는
        // 늘 하던 대로(`/index/check`) 해요
        let handed_back = !ns.connected() && r.notion_dir().join(notion::FILE).exists();
        let changed = ns.set(token.as_deref(), workspace.as_deref());
        if token.is_none()
            && let Err(e) = notion::forget(&r.notion_dir())
        {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("could not forget the notion pages ({})", e.kind()),
            );
        }
        // 갱신 중이면 시작하지 않아요: 앱이 다음 확인(`/index/check`)에서 다시 걸어요
        if changed && ready && !(handed_back && token.is_some()) {
            r.start(Target::default());
        }
        send(
            StatusCode::OK,
            json!({"connected": ns.connected(), "workspace": ns.workspace()}),
        )
    })
    .await
}

/// `POST /meetings`의 본문이 틀렸을 때예요.
const SEND_MEETINGS: &str =
    "send {meetings: [{id, title, start, end, summary?, topics?, ledger?}]}";

/// 앱이 끝난 회의 전부를 보내요(docs/30): `{"meetings": [...]}` → `{count}`. 앱에 있는 것이 엔진에 있는 것이 되게
/// 늘 전부를 받아서 바꿔 넣어요(지운 회의는 목록에서 빠져서 와요). 저장된 목록과 같으면 갱신을 걸지 않아요. 갱신
/// 중이면 그 갱신이 끝난 뒤 한 번 더 돌려요. 회의 글은 로그에 남기지 않아요.
async fn meetings(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(dir) = meeting_dir(&state) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let Some(list) = body
        .get("meetings")
        .cloned()
        .and_then(|v| serde_json::from_value::<Vec<Meeting>>(v).ok())
    else {
        return error(StatusCode::BAD_REQUEST, SEND_MEETINGS);
    };
    let ready = state.engine.model_ready;
    let r = state.refresher.clone();
    blocking(move || {
        let changed = match meeting::save(&dir, &list) {
            Ok(changed) => changed,
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("could not save the meetings ({})", e.kind()),
                );
            }
        };
        eprintln!(
            "meetings: {} {}",
            list.len(),
            if changed { "saved" } else { "unchanged" }
        );
        // 저장한 목록과 같아도 검색 데이터에 아직 없을 수 있어요(모델을 받는 중에 저장했을 때): 엔진이 색인과 비교해요
        // 갱신기가 없는 엔진(iPhone)은 색인을 만들지 않아요: 저장만 해요
        if ready && let Some(r) = r {
            r.meetings_changed();
        }
        send(StatusCode::OK, json!({"count": list.len()}))
    })
    .await
}

/// `POST /notes/folders`의 본문이 틀렸을 때예요.
const SEND_NOTES: &str = "send {folders: [{path: absolute path, vault_id?: 16 characters, name?}]}";

/// 앱이 사용자가 고른 노트 폴더 전부를 보내요(docs/34): `{"folders": [...]}` → `{count}`. 빈 목록이면 노트를 다
/// 빼요. 저장된 목록과 같아도 엔진이 색인과 비교해서 필요할 때만 다시 넣어요. 경로는 로그에 남기지 않아요.
async fn notes_folders(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let Some(list) = body
        .get("folders")
        .cloned()
        .and_then(|v| serde_json::from_value::<Vec<Folder>>(v).ok())
    else {
        return error(StatusCode::BAD_REQUEST, SEND_NOTES);
    };
    let fine = |f: &Folder| {
        f.path.starts_with('/')
            && !f.path.contains('\0')
            && f.vault_id
                .as_deref()
                .is_none_or(|v| v.len() == 16 && v.chars().all(|c| c.is_ascii_alphanumeric()))
    };
    if !list.iter().all(fine) {
        return error(StatusCode::BAD_REQUEST, SEND_NOTES);
    }
    let ready = state.engine.model_ready;
    blocking(move || {
        let changed = match notes::save(&r.notes_dir(), &list) {
            Ok(changed) => changed,
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("could not save the note folders ({})", e.kind()),
                );
            }
        };
        eprintln!(
            "notes: {} folders {}",
            list.len(),
            if changed { "saved" } else { "unchanged" }
        );
        if ready {
            r.notes_changed();
        }
        send(StatusCode::OK, json!({"count": list.len()}))
    })
    .await
}

/// `POST /linear/credentials`의 본문이 틀렸을 때예요.
const SEND_LINEAR: &str = "send {token: key or null, workspace?: name, workspace_id?: id, muted_teams?: [team id]}: printable ASCII token only";

fn opt_text(body: &Value, key: &str) -> Result<Option<String>, ()> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(v)) => Ok(Some(v.clone())),
        Some(_) => Err(()),
    }
}

/// Linear 연결을 엔진 메모리에 둬요(docs/18): `{"token": 키 | null, "workspace"?, "workspace_id"?, "muted_teams"?}` →
/// `{connected, workspace}`. 토큰은 응답·로그·파일에 남기지 않아요. 끈 팀은 `raw/linear/state.json`에 적어서 엔진이
/// 다시 떠도 지켜요. 토큰만 바뀐 것(하루마다 갱신)은 동기화를 걸지 않아요. 다른 워크스페이스나 연결 해제면 받은 것을
/// 지우고, 갱신 중이면 그 갱신이 끝난 뒤 한 번 더 돌려요.
async fn linear_credentials(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let token = match body.get("token") {
        Some(Value::Null) => None,
        Some(Value::String(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
        _ => return error(StatusCode::BAD_REQUEST, SEND_LINEAR),
    };
    if token
        .as_deref()
        .is_some_and(|k| k.len() > MAX_KEY_LEN || !k.bytes().all(|b| b.is_ascii_graphic()))
    {
        return error(StatusCode::BAD_REQUEST, SEND_LINEAR);
    }
    let (Ok(workspace), Ok(workspace_id)) = (
        opt_text(&body, "workspace"),
        opt_text(&body, "workspace_id"),
    ) else {
        return error(StatusCode::BAD_REQUEST, SEND_LINEAR);
    };
    let muted: Vec<String> = match body.get("muted_teams") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => return error(StatusCode::BAD_REQUEST, SEND_LINEAR),
    };
    let ready = state.engine.model_ready;
    blocking(move || {
        let ls = &r.sources().linear;
        let dir = r.linear_dir();
        let was_refused = ls.refused();
        let saved_id = linear::stored_workspace(&dir);
        let connecting = token.is_some();
        let change = ls.set(token, workspace, workspace_id.clone());
        // 다른 워크스페이스: 저장본이 어느 워크스페이스 것인지 몰라도(처음 받기가 끊김) 지워요
        let wipe = matches!(change, Change::Disconnected | Change::Switched)
            || (connecting && saved_id.is_some() && saved_id != workspace_id);
        if wipe && let Err(e) = r.forget_linear() {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("could not forget the linear items ({})", e.kind()),
            );
        }
        let muted_changed = connecting && linear::set_muted(&dir, &muted).unwrap_or(false);
        // 엔진이 새로 떠서 앱이 저장한 연결을 다시 넘긴 것(같은 워크스페이스를 받아 둠)은 새 연결이 아니에요
        let handed_back = change == Change::Connected && !wipe && saved_id.is_some();
        let wants = match change {
            Change::Connected => !handed_back,
            Change::Switched | Change::Disconnected => true,
            // 거절 뒤 갱신한 토큰이에요: 다시 받기는 30분에 한 번만(또 거절되면 30분 확인에 맡겨요)
            Change::Rotated => was_refused && ls.retry_after_refusal(),
            Change::Same => false,
        } || muted_changed;
        if wants && ready && !r.start(Target::default()) {
            r.queue_personal();
        }
        send(
            StatusCode::OK,
            json!({"connected": ls.connected(), "workspace": ls.workspace()}),
        )
    })
    .await
}

/// 팀 고르기 화면의 목록이에요(docs/18 10절): `{"teams": [{id, name, issues}]}`. 받은 것이 없으면 비어요.
async fn linear_teams(State(state): State<Arc<AppState>>) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    blocking(move || {
        send(
            StatusCode::OK,
            json!({"teams": linear::teams(&r.linear_dir())}),
        )
    })
    .await
}

/// `POST /gdrive/credentials`의 본문이 틀렸을 때예요.
const SEND_GDRIVE: &str = "send {token: key or null, account?: email, account_id?: id, muted_drives?: [drive id]}: printable ASCII token only";

/// Google Drive 연결을 엔진 메모리에 둬요(docs/19 10절): `{"token": 키 | null, "account"?, "account_id"?,
/// "muted_drives"?}` → `{connected, account}`. Linear와 같은 규칙이에요: 토큰은 응답·로그·파일에 남기지 않고, 끈 공유
/// 드라이브는 `raw/gdrive/state.json`에 적고, 토큰만 바뀐 것(한 시간마다 갱신)은 동기화를 걸지 않아요. 다른 계정이나
/// 연결 해제면 받은 것을 지워요.
async fn gdrive_credentials(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let token = match body.get("token") {
        Some(Value::Null) => None,
        Some(Value::String(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
        _ => return error(StatusCode::BAD_REQUEST, SEND_GDRIVE),
    };
    if token
        .as_deref()
        .is_some_and(|k| k.len() > MAX_KEY_LEN || !k.bytes().all(|b| b.is_ascii_graphic()))
    {
        return error(StatusCode::BAD_REQUEST, SEND_GDRIVE);
    }
    let (Ok(account), Ok(account_id)) = (opt_text(&body, "account"), opt_text(&body, "account_id"))
    else {
        return error(StatusCode::BAD_REQUEST, SEND_GDRIVE);
    };
    let muted: Vec<String> = match body.get("muted_drives") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => return error(StatusCode::BAD_REQUEST, SEND_GDRIVE),
    };
    let ready = state.engine.model_ready;
    blocking(move || {
        let gs = &r.sources().gdrive;
        let dir = r.gdrive_dir();
        let was_refused = gs.refused();
        let saved_id = gdrive::stored_account(&dir);
        let connecting = token.is_some();
        let change = gs.set(token, account, account_id.clone());
        let wipe = matches!(change, Change::Disconnected | Change::Switched)
            || (connecting && saved_id.is_some() && saved_id != account_id);
        if wipe && let Err(e) = r.forget_gdrive() {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("could not forget the drive files ({})", e.kind()),
            );
        }
        let muted_changed = connecting && gdrive::set_muted(&dir, &muted).unwrap_or(false);
        let handed_back = change == Change::Connected && !wipe && saved_id.is_some();
        let wants = match change {
            Change::Connected => !handed_back,
            Change::Switched | Change::Disconnected => true,
            // 받는 도중 401로 기다리던 받기는 새 토큰으로 이어 가요. 받기가 돌지 않을 때 거절된 것만 다시 걸어요
            Change::Rotated => was_refused && !r.busy() && gs.retry_after_refusal(),
            Change::Same => false,
        } || muted_changed;
        if wants && ready && !r.start(Target::default()) {
            r.queue_personal();
        }
        send(
            StatusCode::OK,
            json!({"connected": gs.connected(), "account": gs.account()}),
        )
    })
    .await
}

/// 공유 드라이브 고르기 화면의 목록이에요(docs/19 11절): `{"drives": [{id, name, files}]}`. 받은 것이 없으면 비어요.
async fn gdrive_drives(State(state): State<Arc<AppState>>) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    blocking(move || {
        send(
            StatusCode::OK,
            json!({"drives": gdrive::drives(&r.gdrive_dir())}),
        )
    })
    .await
}

/// `POST /atlassian/credentials`의 본문이 틀렸을 때예요.
const SEND_ATLASSIAN: &str = "send {token: key or null, cloud?: site id, site?: name, site_url?: url, email?: login email with an API token, muted_projects?: [project id or key], muted_spaces?: [space id or key]}: printable ASCII token only";

/// 글자 목록 칸이에요: 없거나 null이면 빈 목록, 글자만 든 배열이 아니면 Err예요.
fn text_list(body: &Value, key: &str) -> Result<Vec<String>, ()> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => Ok(a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()),
        Some(_) => Err(()),
    }
}

/// Jira·Confluence 연결을 엔진 메모리에 둬요(docs/31 6절): `{"token": 키 | null, "cloud"?, "site"?, "site_url"?,
/// "muted_projects"?, "muted_spaces"?}` → `{connected, site}`. Drive와 같은 규칙이에요: 토큰은 응답·로그·파일에 남기지
/// 않고, 끈 프로젝트·스페이스는 `raw/atlassian/state.json`에 적고, 토큰만 바뀐 것(한 시간마다 갱신)은 동기화를 걸지
/// 않아요. Drive와 달리 연결 해제(`token: null`)면 받은 것을 그 자리에서 지워요. 다른 사이트(cloud)도 받기 전에 지워요.
async fn atlassian_credentials(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let token = match body.get("token") {
        Some(Value::Null) => None,
        Some(Value::String(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
        _ => return error(StatusCode::BAD_REQUEST, SEND_ATLASSIAN),
    };
    if token
        .as_deref()
        .is_some_and(|k| k.len() > MAX_KEY_LEN || !k.bytes().all(|b| b.is_ascii_graphic()))
    {
        return error(StatusCode::BAD_REQUEST, SEND_ATLASSIAN);
    }
    let (Ok(cloud), Ok(site), Ok(site_url), Ok(email)) = (
        opt_text(&body, "cloud"),
        opt_text(&body, "site"),
        opt_text(&body, "site_url"),
        opt_text(&body, "email"),
    ) else {
        return error(StatusCode::BAD_REQUEST, SEND_ATLASSIAN);
    };
    // an API token's login email goes into the Basic header with the token: printable ASCII only
    if email
        .as_deref()
        .is_some_and(|e| e.is_empty() || e.len() > 320 || !e.bytes().all(|b| b.is_ascii_graphic()))
        // an API token calls the site itself: without its address it would go to the gateway and be refused
        || (email.is_some() && site_url.is_none())
    {
        return error(StatusCode::BAD_REQUEST, SEND_ATLASSIAN);
    }
    let (Ok(projects), Ok(spaces)) = (
        text_list(&body, "muted_projects"),
        text_list(&body, "muted_spaces"),
    ) else {
        return error(StatusCode::BAD_REQUEST, SEND_ATLASSIAN);
    };
    let ready = state.engine.model_ready;
    blocking(move || {
        let at = &r.sources().atlassian;
        let dir = r.atlassian_dir();
        let was_refused = at.refused();
        let saved_cloud = atlassian::stored_cloud(&dir);
        let had_files = dir.exists();
        let connecting = token.is_some();
        let change = at.set_with_email(token, cloud.clone(), site, site_url, email);
        // 사용자가 끊었거나(엔진이 새로 떠서 연결이 없던 때도요) 다른 사이트예요: 받기 전에 지워요
        let wipe = !connecting
            || matches!(change, Change::Disconnected | Change::Switched)
            || (saved_cloud.is_some() && saved_cloud != cloud);
        if wipe && let Err(e) = r.forget_atlassian() {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("could not forget the atlassian items ({})", e.kind()),
            );
        }
        let muted_changed =
            connecting && atlassian::set_muted(&dir, &projects, &spaces).unwrap_or(false);
        let handed_back = change == Change::Connected && !wipe && saved_cloud.is_some();
        let wants = match change {
            Change::Connected => !handed_back,
            Change::Switched | Change::Disconnected => true,
            // 받는 도중 401로 기다리던 받기는 새 토큰으로 이어 가요. 받기가 돌지 않을 때 거절된 것만 다시 걸어요
            Change::Rotated => was_refused && !r.busy() && at.retry_after_refusal(),
            Change::Same => false,
        } || muted_changed
            || (wipe && had_files);
        if wants && ready && !r.start(Target::default()) {
            r.queue_personal();
        }
        send(
            StatusCode::OK,
            json!({"connected": at.connected(), "site": at.site()}),
        )
    })
    .await
}

/// 프로젝트·스페이스 고르기 시트의 목록이에요(docs/31 8절): `{"projects": [{id, key, name, issues}], "spaces": [{id,
/// key, name, pages}]}`. 받은 것이 없으면 비어요.
async fn atlassian_scopes(State(state): State<Arc<AppState>>) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    blocking(move || send(StatusCode::OK, atlassian::scopes(&r.atlassian_dir()))).await
}

/// `POST /asana/credentials`의 본문이 틀렸을 때예요.
const SEND_ASANA: &str = "send {token: key or null, user_gid?: user id, muted_workspaces?: [workspace gid], muted_projects?: [project gid]}: printable ASCII token and user id only";

/// Asana 사용자 gid의 가장 긴 길이예요(숫자 열여섯 자 남짓이에요).
const MAX_GID_LEN: usize = 64;

/// Asana 연결을 엔진 메모리에 둬요(docs/32 6절): `{"token": 키 | null, "user_gid"?, "muted_workspaces"?,
/// "muted_projects"?}` → `{connected}`. Atlassian과 같은 규칙이에요: 토큰은 응답·로그·파일에 남기지 않고, 끈
/// 워크스페이스·프로젝트는 `raw/asana/state.json`에 적고, 토큰만 바뀐 것은 동기화를 걸지 않아요(거절 뒤 새 토큰이면
/// 다시 받아요). 연결 해제(`token: null`)면 받은 것을 그 자리에서 지워요. 저장본과 다른 사용자도 받기 전에 지워요.
async fn asana_credentials(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let token = match body.get("token") {
        Some(Value::Null) => None,
        Some(Value::String(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
        _ => return error(StatusCode::BAD_REQUEST, SEND_ASANA),
    };
    if token
        .as_deref()
        .is_some_and(|k| k.len() > MAX_KEY_LEN || !k.bytes().all(|b| b.is_ascii_graphic()))
    {
        return error(StatusCode::BAD_REQUEST, SEND_ASANA);
    }
    let Ok(user_gid) = opt_text(&body, "user_gid") else {
        return error(StatusCode::BAD_REQUEST, SEND_ASANA);
    };
    if user_gid.as_deref().is_some_and(|u| {
        u.is_empty() || u.len() > MAX_GID_LEN || !u.bytes().all(|b| b.is_ascii_graphic())
    }) {
        return error(StatusCode::BAD_REQUEST, SEND_ASANA);
    }
    let (Ok(workspaces), Ok(projects)) = (
        text_list(&body, "muted_workspaces"),
        text_list(&body, "muted_projects"),
    ) else {
        return error(StatusCode::BAD_REQUEST, SEND_ASANA);
    };
    let ready = state.engine.model_ready;
    blocking(move || {
        let asn = &r.sources().asana;
        let dir = r.asana_dir();
        let was_refused = asn.refused();
        let was_lost = asana::stored_lost(&dir);
        let saved_user = asana::stored_user(&dir);
        let had_files = dir.exists();
        let connecting = token.is_some();
        let change = asn.set(token, user_gid.clone());
        // 사용자가 끊었거나(엔진이 새로 떠서 연결이 없던 때도요) 다른 사용자예요: 받기 전에 지워요
        let wipe = !connecting
            || matches!(change, Change::Disconnected | Change::Switched)
            || (saved_user.is_some() && saved_user != user_gid);
        if wipe && let Err(e) = r.forget_asana() {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("could not forget the asana items ({})", e.kind()),
            );
        }
        let muted_changed =
            connecting && asana::set_muted(&dir, &workspaces, &projects).unwrap_or(false);
        let handed_back = change == Change::Connected && !wipe && saved_user.is_some();
        let wants = match change {
            Change::Connected => !handed_back,
            Change::Switched | Change::Disconnected => true,
            // 받는 도중 401로 기다리던 받기는 새 토큰으로 이어 가요. 받기가 돌지 않을 때 거절된 것만 다시 걸어요.
            // 끊긴 연결(`lost_since`)에 새 토큰이 오면(다시 연결) 30분을 기다리지 않고 바로 받아요
            Change::Rotated => {
                !r.busy() && (was_lost || (was_refused && asn.retry_after_refusal()))
            }
            Change::Same => false,
        } || muted_changed
            || (wipe && had_files);
        if wants && ready && !r.start(Target::default()) {
            r.queue_personal();
        }
        send(StatusCode::OK, json!({"connected": asn.connected()}))
    })
    .await
}

/// 워크스페이스·프로젝트 고르기 시트의 목록이에요(docs/32 6절): `{"workspaces": [{gid, name, tasks, blocked}], "projects":
/// [{gid, workspace, name, tasks}]}`. 받은 것이 없으면 비어요.
async fn asana_scopes(State(state): State<Arc<AppState>>) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    blocking(move || send(StatusCode::OK, asana::scopes(&r.asana_dir()))).await
}

/// `POST /dropbox/credentials`의 본문이 틀렸을 때예요.
const SEND_DROPBOX: &str = "send {token: key or null, account?: email, account_id: id (with a token), muted_folders?: [folder id]}: printable ASCII token only";

/// Dropbox 연결을 엔진 메모리에 둬요(docs/33): `{"token": 키 | null, "account"?, "account_id", "muted_folders"?}`
/// → `{connected, account}`. Drive와 같은 규칙이에요: 토큰은 응답·로그·파일에 남기지 않고, 끈 폴더는
/// `raw/dropbox/state.json`에 적고(`id:` 접두는 떼요), 토큰만 바뀐 것(4시간마다 갱신)은 동기화를 걸지 않아요. 연결
/// 해제(`token: null`, 엔진을 다시 띄워 연결이 없던 때도요)나 다른 계정이면 받은 것을 지워요. 토큰에는 `account_id`가
/// 꼭 있어야 해요(없으면 400).
async fn dropbox_credentials(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let token = match body.get("token") {
        Some(Value::Null) => None,
        Some(Value::String(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
        _ => return error(StatusCode::BAD_REQUEST, SEND_DROPBOX),
    };
    if token
        .as_deref()
        .is_some_and(|k| k.len() > MAX_KEY_LEN || !k.bytes().all(|b| b.is_ascii_graphic()))
    {
        return error(StatusCode::BAD_REQUEST, SEND_DROPBOX);
    }
    let (Ok(account), Ok(account_id)) = (opt_text(&body, "account"), opt_text(&body, "account_id"))
    else {
        return error(StatusCode::BAD_REQUEST, SEND_DROPBOX);
    };
    // 계정 id 없는 토큰은 다른 계정으로 바뀐 것처럼 보여 저장본을 지우게 돼요
    if token.is_some() && account_id.as_deref().is_none_or(|a| a.trim().is_empty()) {
        return error(StatusCode::BAD_REQUEST, SEND_DROPBOX);
    }
    let muted: Vec<String> = match body.get("muted_folders") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => return error(StatusCode::BAD_REQUEST, SEND_DROPBOX),
    };
    let ready = state.engine.model_ready;
    blocking(move || {
        let db = &r.sources().dropbox;
        let dir = r.dropbox_dir();
        let was_refused = db.refused();
        let saved_id = dropbox::stored_account(&dir);
        let had_files = dir.exists();
        let connecting = token.is_some();
        let change = db.set(token, account, account_id.clone());
        let wipe = !connecting
            || matches!(change, Change::Disconnected | Change::Switched)
            || (saved_id.is_some() && saved_id != account_id);
        if wipe && let Err(e) = r.forget_dropbox() {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("could not forget the dropbox files ({})", e.kind()),
            );
        }
        // `set_muted`가 `id:` 접두를 떼요
        let muted_changed = connecting && dropbox::set_muted(&dir, &muted).unwrap_or(false);
        let handed_back = change == Change::Connected && !wipe && saved_id.is_some();
        let wants = match change {
            Change::Connected => !handed_back,
            Change::Switched | Change::Disconnected => true,
            // 받는 도중 401로 기다리던 받기는 새 토큰으로 이어 가요. 받기가 돌지 않을 때 거절된 것만 다시 걸어요
            Change::Rotated => was_refused && !r.busy() && db.retry_after_refusal(),
            Change::Same => false,
        } || muted_changed
            || (wipe && had_files);
        if wants && ready && !r.start(Target::default()) {
            r.queue_personal();
        }
        send(
            StatusCode::OK,
            json!({"connected": db.connected(), "account": db.account()}),
        )
    })
    .await
}

/// 폴더 고르기 화면의 목록이에요(docs/33): `{"folders": [{id, name, path, files, team_folder}]}`. 받은 것이 없으면
/// 비어요.
async fn dropbox_folders(State(state): State<Arc<AppState>>) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    blocking(move || {
        send(
            StatusCode::OK,
            json!({"folders": dropbox::folders(&r.dropbox_dir())}),
        )
    })
    .await
}

/// 응답의 `(connected, login)`이에요(스펙 6-2): 모드가 none이 아니고 토큰이 거절되지 않았으면 연결이에요
/// (`GitHub::connected_now`). login은 GitHub이 계정을 알려 줬을 때만이에요. 오프라인이면 연결은 그대로, login은 null이에요.
fn github_presence(github: &GitHub) -> (bool, Option<String>) {
    let state = github.login_state();
    (
        github.connected() && state != LoginState::Refused,
        state.name(),
    )
}

/// 사용자의 저장소와 승인·SSO가 필요한 조직이에요. 실패는 502예요.
async fn github_repos(State(state): State<Arc<AppState>>) -> Response {
    let github = state.github.clone();
    blocking(move || github_answer(github.repos())).await
}

/// 저장소의 브랜치예요: `?repo=owner/name`(빈 값은 버린 첫 `repo`). 이름이 아니면 400이에요.
async fn github_branches(State(state): State<Arc<AppState>>, uri: Uri) -> Response {
    // 첫 `repo` 값을 써요: 빈 값은 버려요
    let repo = uri
        .query()
        .and_then(|q| {
            form_urlencoded::parse(q.as_bytes())
                .find(|(k, v)| k == "repo" && !v.is_empty())
                .map(|(_, v)| v.into_owned())
        })
        .unwrap_or_default();
    // 전체가 맞아야 해요: `$` 하나로는 끝 줄바꿈이 지나가요
    if !valid_repo_name(&repo) {
        return error(StatusCode::BAD_REQUEST, "repo must be owner/name");
    }
    let github = state.github.clone();
    blocking(move || github_answer(github.branches(&repo))).await
}

/// GitHub 목록의 답이에요: 실패는 고정 글과 그 종류의 502예요(gh가 찍은 글이나 토큰은 넣지 않아요). 종류는
/// 연결 없음·거절은 key, gh·git 실패는 찍은 글로, 시간 초과는 network, 잘린 답·gh가 없음은 down이에요.
fn github_answer(got: Result<Value, tacie_sources::SourceError>) -> Response {
    match got {
        Ok(v) => send(StatusCode::OK, v),
        Err(e) => send(
            StatusCode::BAD_GATEWAY,
            json!({"error": GITHUB_UNAVAILABLE, "error_kind": e.kind().as_str()}),
        ),
    }
}

/// 막히는 일을 따로 띄우고 기다려요: 요청이 버려져도 일은 끝까지 가요(계획 2 판정). 패닉하면 500이에요.
async fn blocking(work: impl FnOnce() -> Response + Send + 'static) -> Response {
    match tokio::task::spawn_blocking(work).await {
        Ok(res) => res,
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    }
}

/// 묻기 창이에요: `{"text", "context"?: [...], "show_threshold"?: 0.8}` → 카드.
async fn ask(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(body) = read_json(&body) else {
        return send(
            StatusCode::BAD_REQUEST,
            json!({"error": "body is not JSON"}),
        );
    };
    let text = body["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        return send(
            StatusCode::BAD_REQUEST,
            json!({"error": "text is required"}),
        );
    }
    let engine = &state.engine;
    let show = body["show_threshold"]
        .as_f64()
        .unwrap_or(engine.settings.show);
    // `sample: true`: 온보딩 연습이에요. 예시 색인에서만 찾아요(docs/11 12절). 예시 자료가 없으면 `/answer`처럼 400이에요.
    let sample = body["sample"].as_bool().unwrap_or(false);
    if sample && engine.sample.is_none() {
        return send(StatusCode::BAD_REQUEST, json!({"error": "no sample data"}));
    }
    // 후보에서 뺄 문서예요(docs/30): 지난 질문을 다시 찾을 때 그 질문이 나온 회의(`meeting/<id>`)를 빼요.
    let exclude: Vec<String> = match &body["exclude_paths"] {
        Value::Null => Vec::new(),
        Value::Array(a) if a.iter().all(Value::is_string) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => {
            return send(
                StatusCode::BAD_REQUEST,
                json!({"error": "exclude_paths must be a list of paths"}),
            );
        }
    };
    send(
        StatusCode::OK,
        engine
            .ask_except(sample, text, &strings(&body["context"]), show, &exclude)
            .await,
    )
}

/// 보여 준 카드로 한 줄 답이에요: `{"text", "context"?: [...], "chunk_ids": [...]}`.
async fn answer(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(body) = read_json(&body) else {
        return send(
            StatusCode::BAD_REQUEST,
            json!({"error": "body is not JSON"}),
        );
    };
    // 이 사람이 찾아도 되는 조각만요: 비공개 채널 글이 답 모델로 가지 않아요. 거른 색인으로 답까지 써요
    // (그사이 갱신이 색인을 바꿔도 id가 다른 조각을 가리키지 않아요).
    // 예시 조각은 id에 SAMPLE_BASE가 더해져 있어요: 어느 색인인지는 플래그가 아니라 id로 알아요. 섞여 있으면 400이에요.
    let raw: Vec<usize> = body["chunk_ids"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_u64)
                .filter_map(|i| usize::try_from(i).ok())
                .collect()
        })
        .unwrap_or_default();
    let sample = raw.iter().any(|&i| i >= SAMPLE_BASE);
    if sample && raw.iter().any(|&i| i < SAMPLE_BASE) {
        return send(
            StatusCode::BAD_REQUEST,
            json!({"error": "chunk_ids mix the sample and the index"}),
        );
    }
    let ix = match (sample, &state.engine.sample) {
        (true, Some(s)) => s.index.clone(),
        (true, None) => return send(StatusCode::BAD_REQUEST, json!({"error": "no sample data"})),
        (false, _) => state.engine.index.load_full(),
    };
    let ids: Vec<usize> = raw
        .into_iter()
        .map(|i| if sample { i - SAMPLE_BASE } else { i })
        .filter(|&i| i < ix.chunks.len() && ix.allowed.get(i) == Some(&true))
        .collect();
    let text = body["text"].as_str().unwrap_or("").trim();
    if text.is_empty() || ids.is_empty() {
        return send(
            StatusCode::BAD_REQUEST,
            json!({"error": "text and chunk_ids are required"}),
        );
    }
    let ids = &ids[..ids.len().min(3)];
    let mut out = state
        .engine
        .compose_answer_from(&ix, text, &strings(&body["context"]), ids)
        .await;
    // 근거 번호도 앱이 받은 카드 id와 같은 꼴로 돌려줘요.
    if sample && let Some(cited) = out["cited"].as_array_mut() {
        for c in cited {
            if let Some(id) = c.as_u64() {
                *c = json!(id + SAMPLE_BASE as u64);
            }
        }
    }
    send(StatusCode::OK, out)
}

/// `/tools/*`의 본문이 틀렸을 때예요.
const SEND_TOOLS: &str = "send strings, string lists or a number in from, to, text, ids, kind, owner, done, meeting_ids, limit";

/// 글이어야 하는 필드예요. 없으면 None이고, 글이 아니면 Err예요.
fn text_field(body: &Value, key: &str) -> Result<Option<String>, ()> {
    match &body[key] {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        _ => Err(()),
    }
}

/// 개수 필드예요. 없으면 0(기본 상한)이고, 0 이상 정수가 아니면 Err예요.
fn limit_field(body: &Value) -> Result<usize, ()> {
    match &body["limit"] {
        Value::Null => Ok(0),
        v => v.as_u64().map(|n| n as usize).ok_or(()),
    }
}

/// 로컬 MCP의 회의 목록이에요(docs/38): `{from?, to?, text?, ids?, limit?}` → `{meetings, more}`.
async fn tools_meetings(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let q = (|| {
        Some(crate::tools::MeetingQuery {
            from: text_field(&body, "from").ok()?,
            to: text_field(&body, "to").ok()?,
            text: text_field(&body, "text").ok()?,
            ids: string_list_field(&body, "ids")?,
            limit: limit_field(&body).ok()?,
        })
    })();
    let Some(q) = q else {
        return error(StatusCode::BAD_REQUEST, SEND_TOOLS);
    };
    blocking(move || {
        let all = meeting::all(&r.meeting_dir());
        send(StatusCode::OK, crate::tools::list_meetings(&all, &q))
    })
    .await
}

/// 로컬 MCP의 회의 장부 목록이에요(docs/38): `{kind?, owner?, done?, from?, to?, meeting_ids?, limit?}` → `{entries, more}`.
async fn tools_ledger(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(r) = state.refresher.clone() else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let q = (|| {
        Some(crate::tools::LedgerQuery {
            kind: text_field(&body, "kind").ok()?,
            owner: text_field(&body, "owner").ok()?,
            done: match &body["done"] {
                Value::Null => None,
                Value::Bool(b) => Some(*b),
                _ => return None,
            },
            from: text_field(&body, "from").ok()?,
            to: text_field(&body, "to").ok()?,
            meeting_ids: string_list_field(&body, "meeting_ids")?,
            limit: limit_field(&body).ok()?,
        })
    })();
    let Some(q) = q else {
        return error(StatusCode::BAD_REQUEST, SEND_TOOLS);
    };
    blocking(move || {
        let all = meeting::all(&r.meeting_dir());
        send(StatusCode::OK, crate::tools::list_ledger(&all, &q))
    })
    .await
}

/// 로컬 MCP의 검색이에요(docs/38): `{text, meeting_ids?}` → `{cards, confident, ranked, reason?}`.
/// 답 모델은 부르지 않고, 카드 8장을 점수와 함께 돌려줘요. 기준을 못 넘어도 카드는 그대로 줘요.
async fn tools_search(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let text = body["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        return error(StatusCode::BAD_REQUEST, "text is required");
    }
    let Some(ids) = string_list_field(&body, "meeting_ids") else {
        return error(StatusCode::BAD_REQUEST, "meeting_ids must be a list of strings");
    };
    let scope = if ids.is_empty() {
        crate::pipeline::Scope::All
    } else {
        crate::pipeline::Scope::Meetings(ids)
    };
    let engine = &state.engine;
    let found = engine
        .ask_scoped_turns(text, &[], &scope, engine.settings.show, crate::tools::SEARCH_CARDS, 0)
        .await;
    send(StatusCode::OK, crate::tools::mcp_cards(&found))
}

/// `/panel/*`의 본문이 틀렸을 때예요.
const SEND_PANEL_LIST: &str = "meeting_ids, window, prepared and ledger must be lists";

/// 목록이어야 하는 필드예요. 없으면 빈 목록이고, 목록이 아니면 None이에요.
fn list_field(body: &Value, key: &str) -> Option<Vec<Value>> {
    match &body[key] {
        Value::Null => Some(Vec::new()),
        Value::Array(a) => Some(a.clone()),
        _ => None,
    }
}

/// 글 목록이어야 하는 필드예요. 없으면 빈 목록이고, 목록이 아니거나 글이 아닌 줄이 있으면 None이에요.
fn string_list_field(body: &Value, key: &str) -> Option<Vec<String>> {
    list_field(body, key)?
        .iter()
        .map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// 다음 회의 준비예요: `{"meeting_ids": [...]}` → ndjson. 줄마다 `{"step": ...}`, 마지막 줄은 `{"done": ...}`.
async fn panel_prep(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(dir) = meeting_dir(&state) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let Some(ids) = string_list_field(&body, "meeting_ids") else {
        return error(StatusCode::BAD_REQUEST, SEND_PANEL_LIST);
    };
    let engine = state.engine.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let steps = tx.clone();
        let done = crate::panel::prep(&engine, &dir, &ids, move |step| {
            let line = match step {
                crate::panel::Step::Meetings { ids } => json!({"step": "meetings", "ids": ids}),
                crate::panel::Step::Related { ids } => json!({"step": "related", "ids": ids}),
                crate::panel::Step::Sources { count } => json!({"step": "sources", "count": count}),
                crate::panel::Step::Writing => json!({"step": "writing"}),
            };
            let _ = steps.send(format!("{line}\n"));
        })
        .await;
        let _ = tx.send(format!("{}\n", json!({"done": done})));
    });
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        let line = rx.recv().await?;
        Some((Ok::<_, std::convert::Infallible>(Bytes::from(line)), rx))
    });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from_stream(stream),
    )
        .into_response()
}

/// Tacie AI 패널 하네스예요(docs/38 6절): `{"text", "meeting_ids", "history": [{q, a}], "today", "transcripts":
/// {회의 id: [발화]}, "refs": {회의 id: [[발화 key] 장부 항목 순서대로]}}` → ndjson. 줄마다 `{"step": …}`, 마지막 줄은 `{"done": …}`.
async fn panel_chat(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(dir) = meeting_dir(&state) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let text = body["text"].as_str().unwrap_or("").trim().to_string();
    if text.is_empty() {
        return error(StatusCode::BAD_REQUEST, "text is required");
    }
    let Some(meeting_ids) = string_list_field(&body, "meeting_ids") else {
        return error(StatusCode::BAD_REQUEST, SEND_PANEL_LIST);
    };
    let Some(history) = list_field(&body, "history") else {
        return error(StatusCode::BAD_REQUEST, "history must be a list");
    };
    let history = history
        .iter()
        .map(|t| (t["q"].as_str().unwrap_or("").to_string(), t["a"].as_str().unwrap_or("").to_string()))
        .collect();
    let transcripts = body["transcripts"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_array()?.clone())))
                .collect()
        })
        .unwrap_or_default();
    // {meeting_id: [[발화 key, …] 장부 항목 순서대로]}
    let refs = body["refs"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| {
                    let per: Vec<Vec<String>> = v
                        .as_array()?
                        .iter()
                        .map(|keys| {
                            keys.as_array()
                                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                                .unwrap_or_default()
                        })
                        .collect();
                    Some((k.clone(), per))
                })
                .collect()
        })
        .unwrap_or_default();
    let req = crate::harness::Request {
        text,
        history,
        meeting_ids,
        today: body["today"].as_str().unwrap_or("").to_string(),
        transcripts,
        refs,
    };
    let engine = state.engine.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let steps = tx.clone();
        let done = crate::harness::run(&engine, &dir, &req, move |step| {
            let _ = steps.send(format!("{step}\n"));
        })
        .await;
        let _ = tx.send(format!("{}\n", json!({"done": done})));
    });
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        let line = rx.recv().await?;
        Some((Ok::<_, std::convert::Infallible>(Bytes::from(line)), rx))
    });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from_stream(stream),
    )
        .into_response()
}

/// 결정의 이유예요: `{"decision", "window": [...]}`.
async fn panel_why(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let decision = body["decision"].as_str().unwrap_or("").trim();
    if decision.is_empty() {
        return error(StatusCode::BAD_REQUEST, "decision is required");
    }
    let Some(window) = list_field(&body, "window") else {
        return error(StatusCode::BAD_REQUEST, SEND_PANEL_LIST);
    };
    send(
        StatusCode::OK,
        crate::panel::why(&state.engine, decision, &window).await,
    )
}

/// 준비한 것이 어떻게 됐는지예요: `{"prepared": [...], "ledger": [...]}`.
async fn panel_followup(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let (Some(prepared), Some(ledger)) = (
        list_field(&body, "prepared"),
        list_field(&body, "ledger"),
    ) else {
        return error(StatusCode::BAD_REQUEST, SEND_PANEL_LIST);
    };
    send(
        StatusCode::OK,
        crate::panel::followup(&state.engine, &prepared, &ledger).await,
    )
}

/// 카드가 가리키는 조각 id 중 `ix`에서 그대로 쓸 수 있는 것만이에요: 범위 안, 찾아도 되는 조각, 카드의 `path`와 같은 조각.
pub fn card_chunk_ids(ix: &tacie_index::Index, cards: &Value) -> Vec<usize> {
    cards
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let i = usize::try_from(c["chunk"].as_u64()?).ok()?;
                    let chunk = ix.chunks.get(i)?;
                    (ix.allowed.get(i) == Some(&true) && c["path"].as_str() == Some(&chunk.path))
                        .then_some(i)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 메인 창 묻기예요: `{"text", "meeting_ids"?: [...]|null, "history"?: [{q, a}]}` → `{answer, cited, cards, cost}`.
/// 찾고(`ask_scoped`) 나온 카드로 한 줄 답을 써요. 찾기가 실패했거나 카드가 없으면 답 모델은 부르지 않아요.
async fn panel_ask(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let text = body["text"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        return error(StatusCode::BAD_REQUEST, "text is required");
    }
    let Some(ids) = string_list_field(&body, "meeting_ids") else {
        return error(StatusCode::BAD_REQUEST, SEND_PANEL_LIST);
    };
    let scope = if ids.is_empty() {
        crate::pipeline::Scope::All
    } else {
        crate::pipeline::Scope::Meetings(ids)
    };
    let Some(history) = list_field(&body, "history") else {
        return error(StatusCode::BAD_REQUEST, "history must be a list");
    };
    // 한 턴이 한 줄이에요. 앞 대화는 마지막 6줄, 답은 자르지 않고 실어요(`panel_history`, 시험은 바꿔 봐요).
    let (turns, chars) = panel_history();
    let recent: Vec<&Value> = history.iter().rev().take(turns).rev().collect();
    let lines = |cut: usize| -> Vec<String> {
        recent
            .iter()
            .map(|t| {
                let a: String = t["a"].as_str().unwrap_or("").chars().take(cut).collect();
                crate::notes::one_line(&format!("나: {} / Tacie: {a}", t["q"].as_str().unwrap_or("")))
            })
            .collect()
    };
    let context = lines(chars);
    // 찾기(리랭커)에도 같은 줄 수를 싣되, 답은 앞 200자만 실어요: 리랭크 요청이 길어지지 않게요.
    let search_context = lines(chars.min(PANEL_SEARCH_ANSWER_CHARS));
    let engine = &state.engine;
    // 찾은 색인 그대로 답을 써요: 그사이 갱신이 색인을 바꿔도 id가 다른 조각을 가리키지 않아요(`/answer`와 같아요).
    let ix = engine.index.load_full();
    let found = engine
        .ask_scoped_turns(text, &search_context, &scope, engine.settings.show, 3, turns)
        .await;
    let cards = found["cards"].clone();
    let chunk_ids = card_chunk_ids(&ix, &cards);
    let mut out = json!({"answer": null, "cited": [], "cards": cards});
    if found.get("error").is_some() || chunk_ids.is_empty() {
        out["cards"] = json!([]);
        if let Some(k) = found.get("error_kind") {
            out["error_kind"] = k.clone();
        }
        return send(StatusCode::OK, out);
    }
    let said = engine
        .compose_answer_keeping(&ix, text, &context, &chunk_ids, turns)
        .await;
    out["answer"] = said["answer"].clone();
    out["cited"] = said["cited"].clone();
    if let Some(c) = said.get("cost") {
        out["cost"] = c.clone();
    }
    if let Some(k) = said.get("error_kind") {
        out["error_kind"] = k.clone();
    }
    // 모델이 "답 없음" 문장을 바꿔 쓰면(예: "~는 찾은 자료에 없어요. [1]") 카드를 근거처럼 보이지 않게 비워요.
    // 답이 비었는데(쓸 수 없는 답이라 걸러졌어요) 오류도 아니면 못 찾은 거예요.
    let dropped = said["answer"].is_null() && out.get("error_kind").is_none();
    if dropped || said["answer"].as_str().is_some_and(is_paraphrased_no_answer) {
        out["answer"] = json!(format!("{}.", crate::pipeline::NO_ANSWER));
        out["cited"] = json!([]);
        out["cards"] = json!([]);
    }
    send(StatusCode::OK, out)
}

/// Each earlier answer is cut to this many characters in the panel's search (rerank) context.
const PANEL_SEARCH_ANSWER_CHARS: usize = 200;

/// How much of the panel's conversation goes with a question: the last `turns` turns, each answer cut at `chars`
/// characters. 6 and the whole answer unless `TACIE_PANEL_HISTORY="<turns>,<chars>"` (0 chars: the whole answer) says otherwise,
/// for the follow-up test (eval/panel_followup.py).
fn panel_history() -> (usize, usize) {
    let parsed = std::env::var("TACIE_PANEL_HISTORY").ok().and_then(|v| {
        let (t, c) = v.split_once(',')?;
        Some((t.trim().parse().ok()?, c.trim().parse::<usize>().ok()?))
    });
    match parsed {
        Some((t, 0)) => (t, usize::MAX),
        Some(p) => p,
        None => (6, usize::MAX),
    }
}

/// 답 전체가 "찾은 자료에(는) ... 없어요/없습니다/없네요" 한 문장이면 답을 못 찾은 거예요(인용 표시 `[1]`은 무시해요).
/// 다른 문장이 더 있거나("…는 정했고 …는 찾은 자료에 없어요"처럼 일부만 없는 답), "찾은 자료에서/따르면 … 없어요"처럼
/// 자료를 근거로 말하는 문장은 아니에요.
fn is_paraphrased_no_answer(answer: &str) -> bool {
    let mut plain = String::new();
    let mut chars = answer.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '[' {
            let mut digits = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    digits.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            if !digits.is_empty() && chars.peek() == Some(&']') {
                chars.next();
                continue;
            }
            plain.push('[');
            plain.push_str(&digits);
        } else {
            plain.push(c);
        }
    }
    let mut sentences = plain
        .split(['.', '!', '?', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (Some(one), None) = (sentences.next(), sentences.next()) else {
        return false;
    };
    if !["없어요", "없습니다", "없네요"].iter().any(|n| one.ends_with(n)) {
        return false;
    }
    let Some(at) = one.find("찾은 자료에") else {
        return false;
    };
    // 앞에 쉼표로 이어진 다른 절이 있으면 일부만 없는 답이에요.
    if one[..at].contains([',', '，']) {
        return false;
    }
    let rest = &one[at + "찾은 자료에".len()..];
    if rest.starts_with('서') {
        return false;
    }
    let rest = rest.strip_prefix('는').unwrap_or(rest).trim_start();
    !["따르면", "의하면", "따라"].iter().any(|w| rest.starts_with(w))
}

/// 앱이 들고 있던 회의 글로 장부를 다시 만들어요(그사이 엔진이 다시 떴을 수 있어요):
/// `{"lines": [{id, speaker, text}], "asked"?: [...]}` → 제목, 요약, 장부.
/// 이번 회의의 결정이 지난 결정을 바꿨는지예요(docs/30 8절): `{decisions: [글], past: [{at, title, text}]}` →
/// `{changes: [{decision, past}], ms, cost}`(번호는 보낸 목록의 0부터 자리). 지난 결정은 앱이 최근 것부터 보내요.
async fn decision_changes(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(body) = read_json(&body) else {
        return send(
            StatusCode::BAD_REQUEST,
            json!({"error": "body is not JSON"}),
        );
    };
    let decisions = strings(&body["decisions"]);
    let Some(past) = body["past"].as_array().map(|p| {
        p.iter()
            .map(|e| {
                let s = |k: &str| e[k].as_str().unwrap_or("").to_string();
                (s("at"), s("title"), s("text"))
            })
            .filter(|(_, _, t)| !t.trim().is_empty())
            .collect::<Vec<_>>()
    }) else {
        return send(
            StatusCode::BAD_REQUEST,
            json!({"error": "past must be a list of {at, title, text}"}),
        );
    };
    send(
        StatusCode::OK,
        state.engine.decision_changes(&decisions, &past).await,
    )
}

async fn ledger(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(body) = read_json(&body) else {
        return send(
            StatusCode::BAD_REQUEST,
            json!({"error": "body is not JSON"}),
        );
    };
    let Some(lines) = body["lines"]
        .as_array()
        .filter(|lines| lines.iter().all(Value::is_object))
    else {
        return send(
            StatusCode::BAD_REQUEST,
            json!({"error": "lines must be a list of {id, speaker, text}"}),
        );
    };
    let (kept, attendees) = transcript_lines(lines);
    let asked: HashSet<String> = strings(&body["asked"]).into_iter().collect();
    let Ok(kind) = notes_kind_of(&body["kind"]) else {
        return send(StatusCode::BAD_REQUEST, json!({"error": KIND_ERROR}));
    };
    // 패널의 "요청대로 다시 쓰기"(docs/38 12절): 사용자가 한 말이에요. 2000자까지만 실어요.
    let request: Option<String> = body["request"].as_str().map(|r| r.chars().take(2000).collect());
    let request = request.as_deref().filter(|r| !r.trim().is_empty());
    // 설정 → 회의 → "회의 정리 언어"가 "앱 언어로"면 앱 언어(`ko`·`en`)예요. 없으면 회의 언어대로예요.
    let lang = crate::lang::notes_target(body["language"].as_str());
    send(
        StatusCode::OK,
        state
            .engine
            .notes_in(&kept, &attendees, &asked, kind, request, lang)
            .await,
    )
}

/// 앱이 보낸 회의 글의 줄들(`[id] 말한 사람: 글`)과 참석자예요. 빈 글, 글이 아닌 것, 맞장구는 빼요. 글은 받은 그대로
/// 실어요. me·them은 나·상대이고, 다른 화자는 앱이 보인 이름 그대로예요. 참석자는 이름이 있는 화자이고, 키가 따로
/// 없어서 owner도 그 이름이에요.
#[allow(clippy::type_complexity)]
fn transcript_lines(lines: &[Value]) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let mut attendees: Vec<(String, String)> = Vec::new();
    let kept: Vec<(String, String)> = lines
        .iter()
        .filter_map(|ln| {
            let text = ln["text"].as_str()?;
            if text.trim().is_empty() || is_backchannel(text) {
                return None;
            }
            let id = py_str(&ln["id"]);
            let speaker = ln["speaker"]
                .as_str()
                .map(|s| speaker_name(s).map_or_else(|| one_line(s), str::to_string))
                .filter(|s| !s.trim().is_empty());
            if let Some(s) = &speaker
                && !attendees.iter().any(|(name, _)| name == s)
            {
                attendees.push((s.clone(), s.clone()));
            }
            let speaker = speaker.as_deref().unwrap_or("화자");
            let line = format!("[{id}] {speaker}: {text}");
            Some((id, line))
        })
        .collect();
    (kept, attendees)
}

/// 회의에서 답을 찾지 못한 질문을 판정하고 다시 써요(docs/24 4절):
/// `{"lines": [{id, speaker, text}], "questions": [{id, line?, text}]}` → `{"questions": [{id, verdict, question,
/// answer, utterance_ids}], "ms", "cost"}`. `line`은 질문이 나온 줄의 id예요. 질문은 20개까지만 봐요.
async fn review_questions(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    const BAD: &str = "send {lines: [{id, speaker, text}], questions: [{id, line?, text}]}";
    let Some(body) = read_json(&body) else {
        return send(
            StatusCode::BAD_REQUEST,
            json!({"error": "body is not JSON"}),
        );
    };
    let (Some(lines), Some(questions)) = (
        body["lines"]
            .as_array()
            .filter(|l| l.iter().all(Value::is_object)),
        body["questions"].as_array(),
    ) else {
        return send(StatusCode::BAD_REQUEST, json!({"error": BAD}));
    };
    let Some(questions) = questions
        .iter()
        .map(|q| {
            let id = q["id"].as_str().map(str::trim).filter(|s| !s.is_empty())?;
            let text = q["text"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())?;
            Some((
                id.to_string(),
                q["line"].as_str().map(str::to_string),
                text.to_string(),
            ))
        })
        .collect::<Option<Vec<_>>>()
    else {
        return send(StatusCode::BAD_REQUEST, json!({"error": BAD}));
    };
    let questions: Vec<_> = questions.into_iter().take(20).collect();
    let (kept, attendees) = transcript_lines(lines);
    send(
        StatusCode::OK,
        state
            .engine
            .review_questions(&kept, &attendees, &questions)
            .await,
    )
}

const KIND_ERROR: &str = "kind must be meeting, interview or talk";

/// 정리 요청의 `kind`예요(docs/23 3-4절): 없거나 null이면 엔진이 정하고, 세 값 말고는 잘못된 요청이에요.
fn notes_kind_of(v: &Value) -> Result<Option<NotesKind>, ()> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => NotesKind::parse(s).map(Some).ok_or(()),
        _ => Err(()),
    }
}

/// 회의를 열어요: → `{"session_id": "s1"}`. 본문이 JSON이 아니면 400이에요.
async fn create_session(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    // `{sample: true}`: 온보딩 회의 연습이에요. 이 회의는 예시 자료에서만 찾아요.
    // `{gate_empty: true}`: 찾을 자료가 아직 없어도 질문은 골라요(앱이 적어 뒀다가 동기화 뒤에 다시 찾아요).
    let sample = body["sample"].as_bool().unwrap_or(false);
    let gate_empty = body["gate_empty"].as_bool().unwrap_or(false);
    // `{lookup: false}`: 기록만 해요. 질문을 판단하지 않아요(회의 중에 `/lookup`으로 바꿔요).
    let lookup = body["lookup"].as_bool().unwrap_or(true);
    // `{people: [{id, vector}]}`: 기억한 사람들이에요. 새 목소리를 먼저 이들과 견줘요.
    // `{restore: [{voice, channel, vector, diar_vector?, seconds, person?}]}`: 회의 중에 엔진이 다시 떴을 때 앱이 다시 넘긴
    // 목소리예요. `diar_vector`는 `/voices`가 준 화자 나누기 벡터예요(그 모델로 묶던 회의만).
    // `{after: N}`: 앱이 벡터를 받기 전에 쓴 "화자 N"까지예요. 새 목소리는 그 다음 번호부터예요.
    let after = match &body["after"] {
        Value::Null => Some(0),
        v => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|&n| n <= MAX_NUMBER),
    };
    let (Some(people), Some(restore), Some(after)) = (
        people_of(&body["people"]),
        restored_of(&body["restore"]),
        after,
    ) else {
        return error(StatusCode::BAD_REQUEST, SEND_VOICES);
    };
    let voices = Voices::tuned(people, restore, state.engine.voice_tuning()).after(after);
    let session = state.engine.new_session_with(sample, gate_empty, voices);
    session.set_lookup(lookup);
    send(StatusCode::OK, json!({"session_id": session.id}))
}

/// 회의 중에 기록만 하기를 켜고 꺼요: `{"on": bool}` → `{"lookup": bool}`. `on`이 false면 기록만 하고, 그 뒤 발화는
/// 판단하지 않아요. 다시 켜면 그 뒤 발화부터 판단해요(지난 발화는 거슬러 찾지 않아요).
async fn session_lookup(State(state): State<Arc<AppState>>, uri: Uri, body: Bytes) -> Response {
    let Some((sid, _)) = session_path(&uri) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(on) = read_json(&body).and_then(|b| b["on"].as_bool()) else {
        return error(StatusCode::BAD_REQUEST, "send {\"on\": true|false}");
    };
    let Some(session) = state.engine.session(sid) else {
        return error(StatusCode::NOT_FOUND, "no such session");
    };
    session.set_lookup(on);
    send(StatusCode::OK, json!({"lookup": session.lookup()}))
}

/// 기억한 사람들이에요. 없거나 null이면 빈 목록, 틀린 값이 하나라도 있으면 None이에요.
fn people_of(v: &Value) -> Option<Vec<Person>> {
    list_of(v, |p| {
        let id = p["id"].as_str().filter(|id| !id.is_empty())?;
        Some(Person {
            id: id.to_string(),
            vector: voice_vector(&p["vector"])?,
        })
    })
}

/// 다시 넘겨받은 목소리들이에요. 번호("화자 N")가 겹치거나 `MAX_NUMBER`보다 크거나, 한 사람이 두 목소리에
/// 이어져 있으면 틀린 값이에요.
fn restored_of(v: &Value) -> Option<Vec<Restored>> {
    let out = list_of(v, |r| {
        let channel = match r["channel"].as_str()? {
            "room" => Channel::Room,
            "mic" => Channel::Mic,
            "call" => Channel::Call,
            _ => return None,
        };
        let seconds = r["seconds"].as_f64().map(|s| s as f32)?;
        Some(Restored {
            number: number_of(r["voice"].as_str()?).filter(|&n| n <= MAX_NUMBER)?,
            channel,
            vector: voice_vector(&r["vector"])?,
            diar: match &r["diar_vector"] {
                Value::Null => None,
                d => Some(diar_vector(d)?),
            },
            seconds: (seconds.is_finite() && seconds >= 0.0).then_some(seconds)?,
            person: match &r["person"] {
                Value::Null => None,
                p => Some(p.as_str().filter(|p| !p.is_empty())?.to_string()),
            },
        })
    })?;
    let numbers: HashSet<usize> = out.iter().map(|r| r.number).collect();
    let linked: Vec<&String> = out.iter().filter_map(|r| r.person.as_ref()).collect();
    let people: HashSet<&String> = linked.iter().copied().collect();
    (numbers.len() == out.len() && people.len() == linked.len()).then_some(out)
}

/// 없거나 null이면 빈 목록이에요. 목록이 아니거나 하나라도 못 읽으면 None이에요.
fn list_of<T>(v: &Value, one: impl Fn(&Value) -> Option<T>) -> Option<Vec<T>> {
    match v {
        Value::Null => Some(Vec::new()),
        Value::Array(items) => items.iter().map(one).collect(),
        _ => None,
    }
}

/// 보이스 모델의 벡터예요: 유한한 수 `VOICE_DIM`개예요.
fn voice_vector(v: &Value) -> Option<Vec<f32>> {
    let items = v.as_array().filter(|a| a.len() == VOICE_DIM)?;
    items
        .iter()
        .map(|x| x.as_f64().map(|x| x as f32).filter(|x| x.is_finite()))
        .collect()
}

/// 화자 나누기 모델의 벡터예요: 유한한 수 1–`MAX_DIAR_DIM`개예요. 길이가 지금 모델과 다르면 `Voices`가 쓰지 않아요.
fn diar_vector(v: &Value) -> Option<Vec<f32>> {
    let items = v
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= MAX_DIAR_DIM)?;
    items
        .iter()
        .map(|x| x.as_f64().map(|x| x as f32).filter(|x| x.is_finite()))
        .collect()
}

/// `diar_vector` 길이의 한도예요.
const MAX_DIAR_DIM: usize = 4096;

/// 들은 발화 수, 판단을 넘은 수, 답한 것, 답하지 못한 것이에요.
async fn session_summary(State(state): State<Arc<AppState>>, uri: Uri) -> Response {
    let Some(session) = session_path(&uri).and_then(|(sid, _)| state.engine.session(sid)) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    send(StatusCode::OK, session.summary().await)
}

/// 회의를 닫아요: → `{"deleted": "s1"}`.
async fn close_session(State(state): State<Arc<AppState>>, uri: Uri) -> Response {
    match session_path(&uri) {
        Some((sid, _)) if state.engine.drop_session(sid) => {
            send(StatusCode::OK, json!({"deleted": sid}))
        }
        _ => error(StatusCode::NOT_FOUND, "not found"),
    }
}

/// 이미 받아쓴 발화예요: `{"text", "speaker"?, "gate_threshold"?, "show_threshold"?}` → 판단하고, 넘으면 카드까지.
/// `speaker`는 받은 글 그대로예요(me·them을 이름으로 바꾸지 않아요).
async fn session_utterance(State(state): State<Arc<AppState>>, uri: Uri, body: Bytes) -> Response {
    let Some((sid, _)) = session_path(&uri) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let Some(session) = state.engine.session(sid) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let text = body["text"].as_str().unwrap_or("").trim().to_string();
    if text.is_empty() {
        return error(StatusCode::BAD_REQUEST, "text is required");
    }
    let engine = state.engine.clone();
    let speaker = body["speaker"].as_str().map(str::to_string);
    let gate = body["gate_threshold"]
        .as_f64()
        .unwrap_or(engine.settings.gate);
    let show = body["show_threshold"]
        .as_f64()
        .unwrap_or(engine.settings.show);
    let work = async move {
        session
            .utterance(&engine, &text, speaker.as_deref(), gate, show, None)
            .await
    };
    detached(work, |rec| send(StatusCode::OK, rec)).await
}

/// 발화 하나의 WAV예요(본문 그대로, JSON이 아니에요). 헤더 `X-Speaker`(me|them), `X-Gap-Ms`(앞 발화와의 쉼).
async fn session_audio(
    State(state): State<Arc<AppState>>,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let Some((sid, _)) = session_path(&uri) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(session) = state.engine.session(sid) else {
        return error(StatusCode::NOT_FOUND, "no such session");
    };
    let Some(wav) = read_wav(&headers, body).await else {
        return error(
            StatusCode::BAD_REQUEST,
            "send one utterance as WAV, up to 30 s",
        );
    };
    let speaker = headers
        .get("x-speaker")
        .and_then(|v| v.to_str().ok())
        .and_then(speaker_name);
    let gap_ms = gap_ms(&headers);
    // docs/25: the app sends what Soniox heard; the engine then does not transcribe.
    let given = match headers.get("x-transcript") {
        None => None,
        Some(v) => match v.to_str().ok().and_then(given_transcript) {
            Some(t) => Some(t),
            None => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "X-Transcript is base64 JSON {text, words}",
                );
            }
        },
    };
    let engine = state.engine.clone();
    let work = async move {
        session
            .hear_audio_with(&engine, &wav, speaker, gap_ms, given)
            .await
    };
    detached(work, |rec| send(StatusCode::OK, rec)).await
}

/// 지금까지의 결정, 할 일, 남은 질문이에요. 본문은 `{"kind"?}`예요(docs/23 3-4절). 비었거나 JSON이 아니면 지금까지처럼
/// 엔진이 유형을 정해요. JSON인데 `kind`가 세 값 밖이면 잘못된 요청이에요.
async fn session_ledger(State(state): State<Arc<AppState>>, uri: Uri, body: Bytes) -> Response {
    let Some((sid, _)) = session_path(&uri) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(session) = state.engine.session(sid) else {
        return error(StatusCode::NOT_FOUND, "no such session");
    };
    let body = read_json(&body).unwrap_or(Value::Null);
    let Ok(kind) = notes_kind_of(&body["kind"]) else {
        return error(StatusCode::BAD_REQUEST, KIND_ERROR);
    };
    let lang = crate::lang::notes_target(body["language"].as_str());
    let engine = state.engine.clone();
    let work = async move { session.ledger_in(&engine, kind, lang).await };
    detached(work, |out| send(StatusCode::OK, out)).await
}

/// 이 회의의 목소리들이에요: `{"exclude"?: [발화 id]}` → `{"voices": [{"voice", "channel", "vector", "seconds",
/// "person", "near"}]}`. 앱이 이름을 붙일 때 대표 벡터를 받아 가요. 뺀 줄은 벡터와 초에 넣지 않아요.
async fn session_voices(State(state): State<Arc<AppState>>, uri: Uri, body: Bytes) -> Response {
    let Some((sid, _)) = session_path(&uri) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let Some(session) = state.engine.session(sid) else {
        return error(StatusCode::NOT_FOUND, "no such session");
    };
    let exclude = strings(&body["exclude"]);
    send(StatusCode::OK, session.voices(&exclude).await)
}

/// 사람이 회의 중에 말 몇 개에 그 화자와 다른 이름을 줬어요: `{"utterances": ["s1u3"], "into": "화자 7" 또는 null}` →
/// `{"voice": "화자 7"}`. 그 말들을 원래 목소리에서 떼어 `into`(없으면 새 목소리)로 옮기고, 다음 발화부터 그쪽과도
/// 견줘요(docs/04 4절 "회의 중 화자 바로잡기").
async fn session_split(State(state): State<Arc<AppState>>, uri: Uri, body: Bytes) -> Response {
    let Some((sid, _)) = session_path(&uri) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let Some(session) = state.engine.session(sid) else {
        return error(StatusCode::NOT_FOUND, "no such session");
    };
    let utterances = strings(&body["utterances"]);
    let into = body["into"].as_str();
    match session.split(&utterances, into).await {
        Ok(voice) => send(StatusCode::OK, json!({"voice": voice})),
        Err(why) => error(StatusCode::BAD_REQUEST, why),
    }
}

/// 앱이 화자 키에 붙인 이름 표예요: `{"names": {"화자 2": "지수"}}` → `{}`. 표를 통째로 바꿔요. 다음 발화의
/// 앞 대화 줄과 다음 장부부터 그 이름을 써요.
async fn session_names(State(state): State<Arc<AppState>>, uri: Uri, body: Bytes) -> Response {
    let Some((sid, _)) = session_path(&uri) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let Some(names) = body["names"].as_object().and_then(|m| {
        m.iter()
            .map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
            .collect::<Option<HashMap<String, String>>>()
    }) else {
        return error(StatusCode::BAD_REQUEST, SEND_NAMES);
    };
    let Some(session) = state.engine.session(sid) else {
        return error(StatusCode::NOT_FOUND, "no such session");
    };
    session.set_names(names).await;
    send(StatusCode::OK, json!({}))
}

/// 판단을 넘은 발화의 카드예요: `{"show_threshold"?}`. 받아쓰기 경로는 판단까지만 해서, 앱이 이어서 불러요.
async fn utterance_cards(State(state): State<Arc<AppState>>, uri: Uri, body: Bytes) -> Response {
    let Some((sid, Some(uid))) = session_path(&uri) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let Some(body) = read_json(&body) else {
        return error(StatusCode::BAD_REQUEST, "body is not JSON");
    };
    let Some(session) = state.engine.session(sid) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let engine = state.engine.clone();
    let show = body["show_threshold"]
        .as_f64()
        .unwrap_or(engine.settings.show);
    let uid = uid.to_string();
    let work = async move { session.find(&engine, &uid, show).await };
    detached(work, |found| match found {
        Some(found) => send(StatusCode::OK, found),
        None => error(StatusCode::NOT_FOUND, "no such utterance"),
    })
    .await
}

/// 경로가 `SESSION_PATH`에 맞으면 (세션 id, 발화 id)예요. 발화 id는 카드 경로에만 있어요.
/// 라우터를 `nest`해도 axum은 핸들러에 앞머리를 뗀 `Uri`를 줘서 그대로 맞아요(`OriginalUri`는 앞머리가 붙어 맞지 않아요).
fn session_path(uri: &Uri) -> Option<(&str, Option<&str>)> {
    let m = SESSION_PATH.captures(uri.path())?;
    Some((m.get(1)?.as_str(), m.get(3).map(|g| g.as_str())))
}

/// 세션 일은 따로 띄우고 기다렸다가 `reply`로 답해요: 앱이 연결을 끊어 이 요청이 버려져도 일은 끝까지 가서
/// 발화가 기록돼요. 일이 패닉하면 500 `{"error": "internal"}`이에요.
async fn detached<T: Send + 'static>(
    work: impl Future<Output = T> + Send + 'static,
    reply: impl FnOnce(T) -> Response,
) -> Response {
    match tokio::spawn(work).await {
        Ok(out) => reply(out),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    }
}

/// 발화 하나의 WAV예요. 0바이트이거나 `MAX_AUDIO_BYTES`를 넘으면 None이에요.
/// `Content-Length`가 있으면 그 값으로 보고, 넘으면 본문을 읽지 않아요. 없으면(나눠 보낸 본문) 받은 길이로 봐요.
/// 한도까지만 읽어서 axum 기본 한도(2 MB)에 걸리지 않아요: 큰 본문도 413이 아니라 400이에요.
async fn read_wav(headers: &HeaderMap, body: Body) -> Option<Bytes> {
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n == 0 || n > MAX_AUDIO_BYTES) {
        return None;
    }
    let wav = axum::body::to_bytes(body, MAX_AUDIO_BYTES).await.ok()?;
    (!wav.is_empty()).then_some(wav)
}

/// `X-Gap-Ms`는 ASCII 숫자만 읽어요: 빈 값, 부호, 소수점, 다른 글자가 섞이면 None이에요.
/// u64를 넘는 수도 None이에요(그만큼 쉬었으면 어차피 잇지 않아요).
fn gap_ms(headers: &HeaderMap) -> Option<u64> {
    let gap = headers.get("x-gap-ms")?.to_str().ok()?;
    if gap.is_empty() || !gap.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    gap.parse().ok()
}

/// 앱이 보낸 화자 키의 이름이에요(`SPEAKERS`). 그 밖의 값은 이름이 없어요.
fn speaker_name(key: &str) -> Option<&'static str> {
    SPEAKERS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|&(_, name)| name)
}

/// Python `str()`처럼 쓴 발화 id예요: 글은 그대로, 없으면 "None", 참·거짓은 "True"/"False", 정수는 숫자예요.
/// 소수와 목록·객체는 JSON 글로 써요(앱은 늘 글 id를 보내요).
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        v => v.to_string(),
    }
}

/// 빈 본문은 `{}`예요. 읽을 수 없는 JSON이면 None이에요.
fn read_json(body: &[u8]) -> Option<Value> {
    if body.is_empty() {
        return Some(json!({}));
    }
    serde_json::from_slice(body).ok()
}

/// 앞 대화 줄들이에요. 목록이 아니면 없는 것으로, 글이 아닌 줄은 빼요.
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

async fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not found")
}

/// `{"error": what}` 본문이에요.
fn error(status: StatusCode, what: &str) -> Response {
    send(status, json!({"error": what}))
}

/// 한글은 그대로 두고(이스케이프하지 않아요) UTF-8 JSON으로 보내요.
fn send(status: StatusCode, body: Value) -> Response {
    let bytes = serde_json::to_vec(&body).expect("a JSON value always serializes");
    (
        status,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        bytes,
    )
        .into_response()
}
