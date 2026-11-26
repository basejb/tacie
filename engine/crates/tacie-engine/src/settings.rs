//! Overrides passed by the app's developer tab (TACIE_* environment variables, read once at start) and the key
//! status the connect screen shows. The viewer (Slack) is also set from the config folder (`~/.config`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::header::HeaderMap;
use serde_json::{Value, json};

use crate::errors::Kind;
use crate::keys::Keys;

pub struct Settings {
    pub stt_model: String,
    pub answer_model: String,
    pub jev_model: Option<String>,
    pub gate: f64,
    pub show: f64,
    /// 말한 사람이 회의를 끝내는 중이라고 볼 기준이에요(Jev gate의 `p_end`, `TACIE_END_THR`).
    pub end: f64,
    /// 개발용: 들은 말과 판단 점수를 로그에 남겨요(`TACIE_LOG_TEXT=1`).
    pub log_text: bool,
}

/// (stt 모델, 답 모델, gate 기준, show 기준, end 기준)
pub const DEFAULTS: (&str, &str, f64, f64, f64) = (
    "elevenlabs/scribe-v2",
    "anthropic/claude-haiku-5.5",
    0.7,
    0.8,
    0.8,
);

/// 연결 화면에 나오는 키 파일이에요. 이 순서로 보여요.
const KEY_FILES: [(&str, &str); 3] = [
    ("openrouter", "openrouter/config.json"),
    ("typesafe", "typesafe/config.json"),
    ("slack", "slack/config.json"),
];

/// 기본값에 올바른 덮어쓰기만 얹어요: 모델은 비어 있지 않을 때, 기준은 [0, 1] 안의 실수일 때만요.
pub fn resolve(env: &HashMap<String, String>) -> Settings {
    let model = |name: &str| {
        env.get(name)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let threshold = |name: &str, default: f64| {
        // 앞뒤 공백은 넘기고, NaN과 무한대는 범위 검사에서 빠져요.
        env.get(name)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| (0.0..=1.0).contains(v))
            .unwrap_or(default)
    };
    Settings {
        stt_model: model("TACIE_STT_MODEL").unwrap_or_else(|| DEFAULTS.0.to_string()),
        answer_model: model("TACIE_ANSWER_MODEL").unwrap_or_else(|| DEFAULTS.1.to_string()),
        jev_model: model("TACIE_JEV_MODEL"),
        gate: threshold("TACIE_GATE_THR", DEFAULTS.2),
        show: threshold("TACIE_SHOW_THR", DEFAULTS.3),
        end: threshold("TACIE_END_THR", DEFAULTS.4),
        log_text: env.get("TACIE_LOG_TEXT").is_some_and(|v| v == "1"),
    }
}

/// 키마다: 없으면 missing, 그 키를 쓰는 서비스의 마지막 거절이 있으면 refused(키가 credit보다 앞서요),
/// credit, 아니면 ok예요. 순서는 openrouter, typesafe, slack이에요. AI 키(openrouter, typesafe)는 `keys`(앱이 넘긴
/// 키, 개발 엔진은 키 파일도)로, 슬랙은 설정 폴더의 키 파일로 봐요. `in_app`은 앱이 넘긴 키인지예요. 키 글자는
/// 넣지 않아요.
pub fn key_states(
    config_dir: &Path,
    keys: &Keys,
    last: &HashMap<String, Kind>,
    jev_provider: Option<&str>,
) -> Vec<Value> {
    KEY_FILES
        .iter()
        .map(|&(name, rel)| {
            // jev는 공급자의 키를, 나머지 서비스(stt, answer, ledger)는 OpenRouter 키를 써요.
            let uses = |service: &str| match name {
                // `chat`은 사용자가 넣은 AI 주소의 실패예요(2-A): OpenRouter 키 탓이 아니에요
                // `stt_provider`는 사용자가 고른 받아쓰기 업체의 실패예요(2-B)
                "openrouter" => !matches!(service, "jev" | "chat" | "stt_provider") || jev_provider == Some("openrouter"),
                "typesafe" => service == "jev" && jev_provider == Some("typesafe"),
                _ => false,
            };
            let kinds: HashSet<Kind> = last
                .iter()
                .filter(|(s, _)| uses(s))
                .map(|(_, &k)| k)
                .collect();
            let present = match name {
                "slack" => config_dir.join(rel).exists(),
                _ => keys.has(name),
            };
            let state = if !present {
                "missing"
            } else if kinds.contains(&Kind::Key) {
                "refused"
            } else if kinds.contains(&Kind::Credit) {
                "credit"
            } else {
                "ok"
            };
            json!({"name": name, "path": format!("~/.config/{rel}"), "state": state,
                   "in_app": keys.in_app(name)})
        })
        .collect()
}

/// `~/.config`예요. 키 파일, Jev 공급자, 슬랙 사용자를 여기서 읽어요.
pub fn config_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".config")
}

/// Tacie를 쓰는 사람의 주체(principal)예요. 모두에게 열리지 않은 문서(비공개 슬랙 채널)를 거를 때 써요.
/// `slack/config.json`의 `me`(슬랙 사용자 id)가 있으면 `{"slack:<me>"}`, 없으면 빈 집합이에요.
pub fn viewer(config_dir: &Path) -> HashSet<String> {
    read_json(&config_dir.join("slack/config.json"))
        .and_then(|v| {
            v.get("me")?
                .as_str()
                .filter(|me| !me.is_empty())
                .map(|me| format!("slack:{me}"))
        })
        .into_iter()
        .collect()
}

/// 있는데 JSON으로 못 읽는 키 파일마다 한 줄이에요(`config: unreadable <name>/config.json`). 파일 내용은 쓰지 않아요.
/// `ai_files`가 아니면(제품 엔진) AI 키 파일은 열지 않고 슬랙만 봐요.
pub fn warn_unreadable(config_dir: &Path, ai_files: bool) -> Vec<String> {
    KEY_FILES
        .iter()
        .filter(|&&(name, _)| ai_files || name == "slack")
        .filter(|&&(_, rel)| {
            let path = config_dir.join(rel);
            path.exists() && read_json(&path).is_none()
        })
        .map(|&(_, rel)| format!("config: unreadable {rel}"))
        .collect()
}

/// 이 주소가 이 Mac 안(loopback)인지예요. 슬랙 클라이언트도 같은 판단을 써서 `tacie-sources`에 있어요.
pub use tacie_sources::is_loopback;

/// 공식 앱의 Tacie AI 중계 서버예요(docs/14): 로그인한 사용자의 AI 요청을 넘겨요. release 빌드도 이 주소는 받아요.
/// 서울, 예전 앱이 쓰는 주소, 미국(중계만, 아시아·태평양 밖의 Mac)이에요. 오픈소스판은 로그인이 없어 부르지 않아요.
pub const RELAYS: [&str; 3] = [
    SEOUL_RELAY,
    "https://auth.tacie.app/ai/",
    US_RELAY,
];

/// 서울 중계예요.
pub const SEOUL_RELAY: &str = "https://ai.tacie.app/ai/";
/// 미국 중계예요. 연결이 안 되면 서울로 한 번 더 보내요([`seoul_fallback`]).
pub const US_RELAY: &str = "https://ai-us.tacie.app/ai/";
/// 미국 중계에 연결을 기다리는 시간이에요. 넘으면 연결 실패라 서울로 보내요.
const US_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 미국 중계 주소면 같은 경로의 서울 중계 주소예요. 아니면 None이에요. 계정 표가 두 리전에 복제돼 있어서 같은
/// 세션이 서울에서도 통해요.
pub fn seoul_fallback(url: &str) -> Option<String> {
    url.strip_prefix(US_RELAY)
        .map(|rest| format!("{SEOUL_RELAY}{rest}"))
}

/// 이 실패 뒤에 서울로 다시 보낼 주소예요: 미국 중계로 보낸 요청이 연결 단계에서 실패했을 때(이름 풀기, 연결 거절,
/// TLS, 연결 시간 초과)만이에요. 그때는 서버가 요청을 받지 못해서 돈이 나가지 않았고, 다시 보내도 두 번 처리되지
/// 않아요(임시 키 발급, 받아쓰기, 답 모두요). 보낸 뒤 끊기거나 늦은 건 다시 보내지 않아요.
pub fn retry_in_seoul(url: &str, e: &reqwest::Error) -> Option<String> {
    if e.is_connect() {
        seoul_fallback(url)
    } else {
        None
    }
}

/// 미국 중계가 이 상태로 답했을 때 서울로 다시 보낼 주소예요: 401(세션을 모름)만이에요. 서울에서 막 로그인한 세션이
/// 미국 복제본에 아직 닿지 않았을 수 있어요. 401은 중계가 공급자를 부르기 전에 거절한 거라 돈이 나가지 않았고,
/// 어느 호출이든 다시 보내도 안전해요. 서울 주소에는 None이라 한 번만 옮겨요.
pub fn retry_in_seoul_after(url: &str, status: u16) -> Option<String> {
    if status == 401 {
        seoul_fallback(url)
    } else {
        None
    }
}

/// 이 주소가 Tacie AI 중계 서버(`RELAYS` 아래)인지예요.
pub fn is_relay(url: &str) -> bool {
    RELAYS.iter().any(|r| url.starts_with(r))
}

/// 바깥 서비스 주소 덮어쓰기(`TACIE_JEV_URL`, `TACIE_OPENROUTER_URL`)예요. 없거나 비었으면 None(기본 주소를 써요).
/// `allow_any`(debug 빌드)면 아무 값이나, 아니면(release 빌드) loopback과 Tacie AI 중계(`RELAYS` 아래)만 받아요. 받지
/// 않은 값은 stderr 한 줄을 남기고 None이에요. 값은 찍지 않아요(docs/04 2-1).
pub fn service_url(env: &HashMap<String, String>, name: &str, allow_any: bool) -> Option<String> {
    let value = env.get(name).filter(|v| !v.is_empty())?;
    if allow_any || is_loopback(value) || is_relay(value) {
        return Some(value.clone());
    }
    eprintln!(
        "config: {name} ignored: release builds take loopback addresses and the Tacie AI relay only"
    );
    None
}

/// `base`로 부르는 클라이언트예요. 바깥 주소는 프록시(환경 변수, macOS 시스템 설정)를 따르고, loopback 주소(테스트의
/// 가짜, eval 캐시 프록시)는 프록시를 쓰지 않아요: 프록시로 새면 요청 본문이 프록시에 남아요.
pub fn http_client(base: &str, headers: HeaderMap) -> reqwest::Client {
    let builder = reqwest::Client::builder().default_headers(headers);
    // 미국 중계는 연결을 오래 기다리지 않아요: 연결 실패로 끝나야 서울로 다시 보내요(`retry_in_seoul`).
    let builder = if seoul_fallback(base).is_some() {
        builder.connect_timeout(US_CONNECT_TIMEOUT)
    } else {
        builder
    };
    let builder = if is_loopback(base) {
        builder.no_proxy()
    } else {
        builder
    };
    builder
        .build()
        .expect("an HTTP client with rustls always builds")
}

pub(crate) fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;

    #[test]
    fn warn_unreadable_names_broken_files_only() {
        let dir = tempfile::tempdir().unwrap();
        assert!(warn_unreadable(dir.path(), true).is_empty());
        std::fs::create_dir_all(dir.path().join("slack")).unwrap();
        std::fs::write(dir.path().join("slack/config.json"), "{").unwrap();
        assert_eq!(
            warn_unreadable(dir.path(), true),
            ["config: unreadable slack/config.json"]
        );
        std::fs::write(dir.path().join("slack/config.json"), r#"{"me": "U1"}"#).unwrap();
        assert!(warn_unreadable(dir.path(), true).is_empty());
        // 제품 엔진은 AI 키 파일을 열지 않아요.
        std::fs::create_dir_all(dir.path().join("typesafe")).unwrap();
        std::fs::write(dir.path().join("typesafe/config.json"), "{").unwrap();
        assert!(warn_unreadable(dir.path(), false).is_empty());
        assert_eq!(
            warn_unreadable(dir.path(), true),
            ["config: unreadable typesafe/config.json"]
        );
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn resolve_matches_python_rules() {
        let s = resolve(&env(&[]));
        assert_eq!(
            (
                s.stt_model.as_str(),
                s.answer_model.as_str(),
                s.jev_model,
                s.gate,
                s.show,
                s.end,
                s.log_text
            ),
            (
                "elevenlabs/scribe-v2",
                "anthropic/claude-haiku-5.5",
                None,
                0.7,
                0.8,
                0.8,
                false
            )
        );
        assert_eq!(
            DEFAULTS,
            (
                "elevenlabs/scribe-v2",
                "anthropic/claude-haiku-5.5",
                0.7,
                0.8,
                0.8
            )
        );

        let s = resolve(&env(&[
            ("TACIE_GATE_THR", "0.65"),
            ("TACIE_SHOW_THR", "1.5"),
        ]));
        assert_eq!((s.gate, s.show), (0.65, 0.8));
        let s = resolve(&env(&[
            ("TACIE_GATE_THR", "abc"),
            ("TACIE_SHOW_THR", " 0 "),
        ]));
        assert_eq!((s.gate, s.show), (0.7, 0.0)); // 앞뒤 공백은 넘겨요
        for bad in ["", "nan", "inf", "-0.1", "1.0001"] {
            assert_eq!(
                resolve(&env(&[("TACIE_GATE_THR", bad)])).gate,
                0.7,
                "{bad:?}"
            );
        }
        assert_eq!(resolve(&env(&[("TACIE_SHOW_THR", "1")])).show, 1.0);
        // 회의 끝 기준도 같은 규칙이에요.
        assert_eq!(resolve(&env(&[("TACIE_END_THR", "0.9")])).end, 0.9);
        assert_eq!(resolve(&env(&[("TACIE_END_THR", "2")])).end, 0.8);
        for bad in ["", "abc", "nan", "-0.1"] {
            assert_eq!(resolve(&env(&[("TACIE_END_THR", bad)])).end, 0.8, "{bad:?}");
        }

        let s = resolve(&env(&[
            ("TACIE_STT_MODEL", " a/b "),
            ("TACIE_ANSWER_MODEL", "  "),
            ("TACIE_JEV_MODEL", "jev-x"),
            ("TACIE_LOG_TEXT", "1"),
        ]));
        assert_eq!(
            (
                s.stt_model.as_str(),
                s.answer_model.as_str(),
                s.jev_model.as_deref(),
                s.log_text
            ),
            ("a/b", "anthropic/claude-haiku-5.5", Some("jev-x"), true)
        );
        assert!(!resolve(&env(&[("TACIE_LOG_TEXT", "true")])).log_text);
        assert_eq!(resolve(&env(&[("TACIE_JEV_MODEL", " ")])).jev_model, None);
    }

    #[test]
    fn jev_provider_and_viewer_follow_config_files() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path();
        let write = |rel: &str, text: &str| {
            std::fs::create_dir_all(cfg.join(rel).parent().unwrap()).unwrap();
            std::fs::write(cfg.join(rel), text).unwrap();
        };
        let none = HashMap::new();
        // 키 파일은 부를 때마다 봐요: 한 Jev가 파일이 생기는 대로 따라가요.
        let jev = crate::jev::Jev::from_config(cfg, &resolve(&none), &none, Default::default());
        let seen = || (jev.provider_name(), jev.model_default());
        assert_eq!(seen(), (None, None));
        assert_eq!(viewer(cfg), HashSet::new());

        write("openrouter/config.json", r#"{"apiKey": "x"}"#);
        assert_eq!(
            seen(),
            (Some("openrouter"), Some("typesafe/jev-1.13".to_string()))
        );

        // TypeSafe 파일이 있으면 OpenRouter보다 앞서고, 파일의 model이 기본 모델이에요.
        write("typesafe/config.json", r#"{"apiKey": "x"}"#);
        assert_eq!(seen(), (Some("typesafe"), Some("jev-latest".to_string())));
        write(
            "typesafe/config.json",
            r#"{"apiKey": "x", "model": "jev-9"}"#,
        );
        assert_eq!(seen(), (Some("typesafe"), Some("jev-9".to_string())));

        write("slack/config.json", r#"{"token": "x"}"#);
        assert_eq!(viewer(cfg), HashSet::new());
        write("slack/config.json", r#"{"token": "x", "me": "U123"}"#);
        assert_eq!(viewer(cfg), HashSet::from(["slack:U123".to_string()]));
    }
}
