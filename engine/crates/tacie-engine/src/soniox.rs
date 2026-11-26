//! Temporary keys for Soniox real-time transcription (docs/25). When the app connects to Soniox directly for shadow
//! transcription in dev builds, it passes this temporary key instead of the real key from the key file. The engine
//! does not open the connection; it makes only the one HTTP request that creates the key.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::errors::{Kind, ServiceError};

/// 미국 리전 실시간 주소예요. 일본 리전은 `TACIE_SONIOX_WS=wss://stt-rt.jp.soniox.com/transcribe-websocket`예요.
pub const WS_URL_DEFAULT: &str = "wss://stt-rt.soniox.com/transcribe-websocket";
/// 미국 리전 API 주소예요. 일본 리전은 `TACIE_SONIOX_API=https://api.jp.soniox.com`예요.
pub const API_BASE_DEFAULT: &str = "https://api.soniox.com";
/// 키 수명(최대 1시간)과 연결 최대 길이(5시간)예요. 회의 하나를 덮어요.
const EXPIRES_IN_SECONDS: u64 = 3600;
const MAX_SESSION_SECONDS: u64 = 18000;
const TIMEOUT: Duration = Duration::from_secs(10);

/// 그림자 받아쓰기의 리전이에요(`TACIE_SONIOX_REGION`, 개발 앱의 개발자 설정). 리전마다 프로젝트와 키가 따로라서
/// 키 파일도 달라요: 미국 `~/.config/soniox`, 일본 `~/.config/soniox-jp`. 소리와 글은 고른 리전을 벗어나지 않아요.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// `jp`, `us` or `eu`: what the relay is asked for.
    pub name: &'static str,
    pub key_name: &'static str,
    pub api: &'static str,
    pub ws: &'static str,
}

impl Region {
    /// `jp`면 일본, `eu`면 EU이고, 그 밖의 값(없음 포함)은 미국이에요.
    pub fn from_env(value: Option<&str>) -> Self {
        match value {
            Some("eu") => Region {
                name: "eu",
                key_name: "soniox-eu",
                api: "https://api.eu.soniox.com",
                ws: "wss://stt-rt.eu.soniox.com/transcribe-websocket",
            },
            Some("jp") => Region {
                name: "jp",
                key_name: "soniox-jp",
                api: "https://api.jp.soniox.com",
                ws: "wss://stt-rt.jp.soniox.com/transcribe-websocket",
            },
            _ => Region {
                name: "us",
                key_name: "soniox",
                api: API_BASE_DEFAULT,
                ws: WS_URL_DEFAULT,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StreamKey {
    pub api_key: String,
    pub expires_at: String,
}

/// `api_base`의 `POST /v1/auth/temporary-api-key`로 실시간 연결용 임시 키를 만들어요.
pub async fn mint_key(
    http: &reqwest::Client,
    api_base: &str,
    key: &str,
) -> Result<StreamKey, ServiceError> {
    let url = format!(
        "{}/v1/auth/temporary-api-key",
        api_base.trim_end_matches('/')
    );
    let body = json!({
        "usage_type": "transcribe_websocket",
        "expires_in_seconds": EXPIRES_IN_SECONDS,
        "max_session_duration_seconds": MAX_SESSION_SECONDS,
    });
    let sent = async {
        let r = http
            .post(&url)
            .bearer_auth(key)
            .json(&body)
            .timeout(TIMEOUT)
            .send()
            .await?;
        let status = r.status().as_u16();
        Ok::<_, reqwest::Error>((status, r.text().await?))
    }
    .await;
    let (status, text) = sent.map_err(|e| ServiceError {
        kind: Kind::Network,
        message: format!("soniox key request failed: {e}"),
    })?;
    if !(200..300).contains(&status) {
        let kind = match status {
            401 | 403 => Kind::Key,
            402 => Kind::Credit,
            429 => Kind::Busy,
            _ => Kind::Down,
        };
        // 답 글에는 키가 없지만 길게 남기지 않아요.
        return Err(ServiceError {
            kind,
            message: format!(
                "soniox key {status}: {}",
                text.chars().take(200).collect::<String>()
            ),
        });
    }
    serde_json::from_str(&text).map_err(|e| ServiceError {
        kind: Kind::Down,
        message: format!("soniox key reply is not a key: {e}"),
    })
}

/// 중계 서버가 내준 임시 키예요(주소까지 같이 와요).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RelayKey {
    pub api_key: String,
    pub expires_at: String,
    pub ws_url: String,
}

/// 배포 앱의 엔진은 OpenRouter 주소가 Tacie 중계(`…/ai/openrouter`)예요. 같은 중계의 `…/ai/soniox/stream-key`에서
/// 임시 키를 받아요. 중계가 아니면 None이에요.
pub fn relay_key_url(openrouter_base: &str) -> Option<String> {
    // 로그인 세션을 실어 보내니 Tacie 중계(또는 테스트의 loopback 가짜)만이에요.
    if !crate::settings::is_relay(openrouter_base) && !crate::settings::is_loopback(openrouter_base)
    {
        return None;
    }
    let base = openrouter_base.trim_end_matches('/');
    base.strip_suffix("/ai/openrouter")
        .map(|root| format!("{root}/ai/soniox/stream-key"))
}

/// 중계에서 임시 키를 받아요. `token`은 로그인 세션이에요(중계 모드에서 앱이 `openrouter` 키로 넘겨요). 중계가 끈
/// 경우(503 off)는 down이라 앱은 지금 경로로 받아써요.
/// `region`은 앱이 정한 리전(`jp`·`us`·`eu`)이에요. 중계가 그 리전 프로젝트의 키를 내주고, 키가 없는 리전이면 일본으로요.
pub async fn relay_key(
    http: &reqwest::Client,
    url: &str,
    token: &str,
    region: &str,
) -> Result<RelayKey, ServiceError> {
    let once = |url: String| async move {
        let r = http
            .post(url)
            .bearer_auth(token)
            .json(&json!({"region": region}))
            .timeout(TIMEOUT)
            .send()
            .await?;
        let status = r.status().as_u16();
        Ok::<_, reqwest::Error>((status, r.text().await?))
    };
    // 미국 중계에 연결하지 못했으면(요청이 닿지 않아 키도 돈도 나가지 않았어요) 서울 중계에서 받아요.
    let sent = match once(url.to_string()).await {
        Err(e) => match crate::settings::retry_in_seoul(url, &e) {
            Some(seoul) => {
                eprintln!("relay: us unreachable, sending stream key to seoul");
                once(seoul).await
            }
            None => Err(e),
        },
        // 미국 중계가 세션을 모르면(서울 로그인이 아직 복제되지 않았을 때) 서울에서 받아요. 401이면 키를 만들지 않았어요.
        Ok((status, text)) => match crate::settings::retry_in_seoul_after(url, status) {
            Some(seoul) => {
                eprintln!("relay: us refused the session, sending stream key to seoul");
                once(seoul).await
            }
            None => Ok((status, text)),
        },
    };
    let (status, text) = sent.map_err(|e| ServiceError {
        kind: Kind::Network,
        message: format!("stream key request failed: {e}"),
    })?;
    if !(200..300).contains(&status) {
        let kind = match status {
            401 | 403 => Kind::Key,
            402 => Kind::Credit,
            429 => Kind::Busy,
            _ => Kind::Down,
        };
        return Err(ServiceError {
            kind,
            message: format!("stream key {status}"),
        });
    }
    serde_json::from_str(&text).map_err(|e| ServiceError {
        kind: Kind::Down,
        message: format!("stream key reply is not a key: {e}"),
    })
}

/// 사용자가 고른 리전(2-B)의 실시간 연결 주소예요. 덮어쓰기(시험용)가 있으면 그 주소예요.
pub fn provider_ws(region: &str, ws_override: Option<&str>) -> String {
    ws_override.map_or_else(|| Region::from_env(Some(region)).ws.to_string(), str::to_string)
}

/// 사용자 키(2-B)로 그 리전의 Soniox에서 임시 키를 받아요: (임시 키, 연결 주소). `api_override`·`ws_override`는
/// 시험용 가짜 서버예요(배포 엔진에는 넘기지 않아요, server.rs).
pub async fn provider_key(
    region: &str,
    key: &str,
    api_override: Option<&str>,
    ws_override: Option<&str>,
) -> Result<(StreamKey, String), ServiceError> {
    let api = api_override.map_or_else(|| Region::from_env(Some(region)).api.to_string(), str::to_string);
    let http = crate::settings::http_client(&api, reqwest::header::HeaderMap::new());
    let k = mint_key(&http, &api, key).await?;
    Ok((k, provider_ws(region, ws_override)))
}
