//! The open-source edition's AI address (docs/41 0절·8절, 2-A): the OpenAI-compatible address the user picked
//! (OpenRouter, OpenAI, or Ollama/LM Studio on this Mac), the model and the key. The app passes it as `chat` in
//! `POST /ai/credentials` and the engine keeps it in memory only. When set, every answer model call goes to this
//! address (`openrouter.rs`). The official app does not pass it.

use serde_json::Value;

use crate::keys::MAX_KEY_LEN;

const MAX_MODEL_LEN: usize = 200;
const CHAT_PATH: &str = "/chat/completions";

#[derive(Clone, PartialEq, Eq)]
pub struct AiAddress {
    /// `…/v1`까지예요(끝 `/`와 `/chat/completions`는 떼요).
    pub url: String,
    pub model: String,
    /// 로컬 서버는 키가 없어도 돼요.
    pub key: Option<String>,
}

/// 키는 찍지 않아요(있는지만).
impl std::fmt::Debug for AiAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiAddress")
            .field("url", &self.url)
            .field("model", &self.model)
            .field("key", &self.key.as_ref().map(|_| "…"))
            .finish()
    }
}

impl AiAddress {
    /// `chat` 칸을 읽어요: `null`은 끄기(None), 틀린 값은 이유예요.
    pub fn parse(v: &Value) -> Result<Option<AiAddress>, String> {
        if v.is_null() {
            return Ok(None);
        }
        const SEND: &str = "chat must be null or {url: http(s) address, model, key?}";
        let o = v.as_object().ok_or(SEND)?;
        let url = o.get("url").and_then(Value::as_str).ok_or(SEND)?.trim();
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err("chat url must start with http:// or https://".into());
        }
        let url = url.trim_end_matches('/');
        let url = url.strip_suffix(CHAT_PATH).unwrap_or(url).trim_end_matches('/');
        if url.split("://").nth(1).is_none_or(str::is_empty) {
            return Err(SEND.into());
        }
        let model = o.get("model").and_then(Value::as_str).ok_or(SEND)?.trim();
        if model.is_empty() || model.chars().count() > MAX_MODEL_LEN {
            return Err(format!("chat model must be 1–{MAX_MODEL_LEN} characters"));
        }
        let key = match o.get("key") {
            None | Some(Value::Null) => None,
            Some(Value::String(k)) => Some(k.trim()).filter(|k| !k.is_empty()),
            Some(_) => return Err(SEND.into()),
        };
        if key.is_some_and(|k| k.len() > MAX_KEY_LEN || !k.bytes().all(|b| b.is_ascii_graphic())) {
            return Err("chat key: printable ASCII only".into());
        }
        Ok(Some(AiAddress {
            url: url.to_string(),
            model: model.to_string(),
            key: key.map(str::to_string),
        }))
    }

    pub fn chat_url(&self) -> String {
        format!("{}{CHAT_PATH}", self.url)
    }

    /// 주소의 호스트만이에요(`/config`가 보여 줘요).
    pub fn host(&self) -> String {
        let rest = self.url.split("://").nth(1).unwrap_or(&self.url);
        rest.split(['/', ':']).next().unwrap_or(rest).to_string()
    }

    /// 이 Mac 안의 주소예요(127.x, localhost, ::1): Ollama, LM Studio, Tacie 번들 서버 같은 작은 모델이에요.
    pub fn is_local(&self) -> bool {
        let rest = self.url.split("://").nth(1).unwrap_or(&self.url);
        let host = if let Some(v6) = rest.strip_prefix('[') { v6.split(']').next().unwrap_or("") } else { rest.split(['/', ':']).next().unwrap_or("") };
        host == "localhost" || host == "::1" || host.starts_with("127.")
    }

    /// 오류 글에서 이 키를 가려요(앞뒤 4자만 남겨요). 서버가 키를 되돌려주는 일이 있어요(OpenAI 401).
    pub fn mask(&self, text: &str) -> String {
        match &self.key {
            Some(k) if k.len() > 8 => text.replace(k.as_str(), &format!("{}…{}", &k[..4], &k[k.len() - 4..])),
            Some(k) if !k.is_empty() => text.replace(k.as_str(), "…"),
            _ => text.to_string(),
        }
    }
}
