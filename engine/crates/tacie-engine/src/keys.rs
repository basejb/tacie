//! AI keys (TypeSafe, OpenRouter) (#7 step 5, docs/04 6절). Keys the app takes from the keychain and passes via
//! `POST /ai/credentials` are kept in memory only and used first. Key files (`apiKey` in
//! `~/.config/<name>/config.json`) are read only by an engine launched by the dev app (`TACIE_KEY_FILES=1`). The
//! product engine does not open the files.
//!
//! Keys never appear in logs, error text, responses or `Debug`. Outside, only whether a key is present (a boolean)
//! is reported.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

use reqwest::header::HeaderValue;

use crate::settings;

/// 앱이 넘길 수 있는 AI 키 이름이에요. 이 순서로 답해요.
pub const AI_KEYS: [&str; 2] = ["typesafe", "openrouter"];
/// 키 한 개의 길이 한도예요. 진짜 키는 100자 안쪽이에요.
pub const MAX_KEY_LEN: usize = 4096;

/// 찾은 키예요. `key`가 None이면 키 파일은 있는데 `apiKey`가 없거나 읽을 수 없는 거예요: 헤더 없이 보내서
/// 서버가 401로 답하고 연결 화면에 거절로 보여요(키 파일만 쓰던 때와 같아요).
pub struct Found {
    pub key: Option<String>,
    /// 키 파일의 `model`이에요(TypeSafe 키 파일만 써요). 키와 같은 한 번의 읽기에서 꺼내요. 앱 키는 None이에요.
    pub model: Option<String>,
    /// 앱이 넘긴 키예요(아니면 키 파일).
    pub in_app: bool,
}

impl Found {
    /// `Authorization: Bearer <키>` 값이에요. 헤더에 쓸 수 없는 글자가 있으면 None(헤더 없이 보내요).
    pub fn bearer(&self) -> Option<HeaderValue> {
        let mut value = HeaderValue::from_str(&format!("Bearer {}", self.key.as_deref()?)).ok()?;
        value.set_sensitive(true);
        Some(value)
    }
}

pub struct Keys {
    /// 키 파일을 읽는 설정 폴더예요. None이면 파일을 읽지 않아요(제품 엔진).
    files: Option<PathBuf>,
    memory: RwLock<HashMap<&'static str, String>>,
    /// 앱이 넘긴 AI 주소예요(2-A, `ai_address.rs`). 키와 같이 메모리에만 둬요.
    address: RwLock<Option<crate::ai_address::AiAddress>>,
    /// 앱이 넘긴 받아쓰기 업체예요(2-B, `stt_provider.rs`).
    stt: RwLock<Option<crate::stt_provider::SttProvider>>,
}

impl fmt::Debug for Keys {
    /// 키 글자는 쓰지 않고 어디서 오는지만 써요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Keys");
        d.field("files", &self.files.is_some());
        for name in AI_KEYS {
            d.field(
                name,
                &self
                    .find(name)
                    .map(|k| if k.in_app { "app" } else { "file" }),
            );
        }
        d.finish()
    }
}

impl Keys {
    /// `files`가 있으면 그 설정 폴더의 키 파일도 읽어요(개발 엔진, 테스트).
    pub fn new(files: Option<PathBuf>) -> Self {
        Self {
            files,
            memory: RwLock::new(HashMap::new()),
            address: RwLock::new(None),
            stt: RwLock::new(None),
        }
    }

    /// 키 파일만 읽는 키예요(앱이 넘긴 키가 없어요): 테스트와 평가 도구가 써요.
    pub fn from_files(config_dir: &Path) -> Self {
        Self::new(Some(config_dir.to_path_buf()))
    }

    /// 키 파일을 읽는지예요.
    pub fn reads_files(&self) -> bool {
        self.files.is_some()
    }

    /// 앱이 넘긴 키를 두거나(Some) 지워요(None). 앞뒤 공백은 떼고, 빈 키는 지우는 거예요. 모르는 이름은 무시해요.
    /// 앱이 넘긴 키가 실제로 바뀌었으면 true예요(같은 키를 다시 넘기면 false).
    pub fn set(&self, name: &str, key: Option<&str>) -> bool {
        let Some(name) = AI_KEYS.into_iter().find(|n| *n == name) else {
            return false;
        };
        let mut memory = self.memory.write().unwrap_or_else(PoisonError::into_inner);
        let before = match key.map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => memory.insert(name, k.to_string()),
            None => memory.remove(name),
        };
        before.as_deref() != memory.get(name).map(String::as_str)
    }

    /// AI 주소를 두거나(Some) 지워요(None). 바뀌었으면 true예요.
    pub fn set_address(&self, a: Option<crate::ai_address::AiAddress>) -> bool {
        let mut cur = self.address.write().unwrap_or_else(PoisonError::into_inner);
        let changed = *cur != a;
        *cur = a;
        changed
    }

    /// 받아쓰기 업체를 두거나(Some) 지워요(None). 바뀌었으면 true예요.
    pub fn set_stt(&self, p: Option<crate::stt_provider::SttProvider>) -> bool {
        let mut cur = self.stt.write().unwrap_or_else(PoisonError::into_inner);
        let changed = *cur != p;
        *cur = p;
        changed
    }

    /// 지금 받아쓰기 업체예요(없으면 None).
    pub fn stt(&self) -> Option<crate::stt_provider::SttProvider> {
        self.stt.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// 지금 AI 주소예요(없으면 None).
    pub fn address(&self) -> Option<crate::ai_address::AiAddress> {
        self.address.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// 앱이 넘긴 키가 먼저고, 없으면(개발 엔진만) 키 파일이에요. 둘 다 없으면 None이에요.
    pub fn find(&self, name: &str) -> Option<Found> {
        let memory = self.memory.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(k) = memory.get(name) {
            return Some(Found {
                key: Some(k.clone()),
                model: None,
                in_app: true,
            });
        }
        drop(memory);
        let file = self.file(name)?;
        // 요청마다 읽어서, 새로 둔 키 파일을 엔진을 다시 띄우지 않고 써요.
        if !file.exists() {
            return None;
        }
        let json = settings::read_json(&file);
        let field = |name: &str| {
            json.as_ref()
                .and_then(|v| v.get(name)?.as_str().map(str::to_string))
        };
        Some(Found {
            key: field("apiKey"),
            model: field("model"),
            in_app: false,
        })
    }

    pub fn has(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// 앱이 넘긴 키가 있는지예요.
    pub fn in_app(&self, name: &str) -> bool {
        self.memory
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(name)
    }

    /// 키 파일 경로예요(파일을 읽지 않는 엔진이면 None).
    pub fn file(&self, name: &str) -> Option<PathBuf> {
        Some(self.files.as_ref()?.join(name).join("config.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_first_then_files_only_when_allowed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("typesafe")).unwrap();
        std::fs::write(
            dir.path().join("typesafe/config.json"),
            r#"{"apiKey": "from-file", "model": "jev-x"}"#,
        )
        .unwrap();

        let product = Keys::new(None);
        assert!(!product.has("typesafe"));

        let dev = Keys::from_files(dir.path());
        let f = dev.find("typesafe").unwrap();
        assert_eq!((f.key.as_deref(), f.in_app), (Some("from-file"), false));
        assert_eq!(f.model.as_deref(), Some("jev-x"));

        assert!(dev.set("typesafe", Some("  from-app \n")));
        assert!(
            !dev.set("typesafe", Some("from-app")),
            "the same key again changes nothing"
        );
        let f = dev.find("typesafe").unwrap();
        assert_eq!((f.key.as_deref(), f.in_app), (Some("from-app"), true));
        dev.set("typesafe", Some(" "));
        assert!(!dev.in_app("typesafe"));
        assert_eq!(
            dev.find("typesafe").unwrap().key.as_deref(),
            Some("from-file")
        );

        product.set("openrouter", Some("k"));
        assert!(!product.set("slack", Some("k")));
        assert!(!product.set("typesafe", None), "nothing to remove");
        assert!(product.has("openrouter") && !product.has("slack"));
        product.set("openrouter", None);
        assert!(!product.has("openrouter"));
    }

    #[test]
    fn debug_never_shows_a_key() {
        let keys = Keys::new(None);
        keys.set("openrouter", Some("sk-secret-123"));
        let shown = format!("{keys:?} {keys:#?}");
        assert!(!shown.contains("sk-secret-123"), "{shown}");
        assert!(shown.contains("app"));
        let bearer = keys.find("openrouter").unwrap().bearer().unwrap();
        assert!(bearer.is_sensitive());
        assert!(!format!("{bearer:?}").contains("sk-secret"));
    }
}
