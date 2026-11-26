//! The open-source edition's transcription provider (docs/41 0절·8절, 2-B): Soniox (with region) or ElevenLabs as the
//! user picked, and its key. The app passes it as `stt` in `POST /ai/credentials` and the engine keeps it in memory
//! only. With Soniox, `/stt/stream-key` gets a temporary key with this key; with ElevenLabs, per-utterance
//! transcription goes to ElevenLabs. The official app does not pass it.

use serde_json::Value;

use crate::keys::MAX_KEY_LEN;

#[derive(Clone, PartialEq, Eq)]
pub enum SttProvider {
    /// `region`: `us`, `jp`, `eu` (`soniox::Region`).
    Soniox { key: String, region: &'static str },
    ElevenLabs { key: String },
}

/// 키는 찍지 않아요.
impl std::fmt::Debug for SttProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SttProvider::Soniox { region, .. } => write!(f, "Soniox {{ region: {region}, key: … }}"),
            SttProvider::ElevenLabs { .. } => write!(f, "ElevenLabs {{ key: … }}"),
        }
    }
}

impl SttProvider {
    /// `stt` 칸을 읽어요: `null`은 끄기(None), 틀린 값은 이유예요.
    pub fn parse(v: &Value) -> Result<Option<SttProvider>, String> {
        if v.is_null() {
            return Ok(None);
        }
        const SEND: &str = "stt must be null or {provider: soniox|elevenlabs, key, region?: us|jp|eu}";
        let o = v.as_object().ok_or(SEND)?;
        let key = o.get("key").and_then(Value::as_str).map(str::trim).filter(|k| !k.is_empty()).ok_or(SEND)?;
        if key.len() > MAX_KEY_LEN || !key.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("stt key: printable ASCII only".into());
        }
        let key = key.to_string();
        match o.get("provider").and_then(Value::as_str) {
            Some("soniox") => {
                let region = match o.get("region").and_then(Value::as_str) {
                    None | Some("us") => "us",
                    Some("jp") => "jp",
                    Some("eu") => "eu",
                    Some(_) => return Err("stt region must be us, jp or eu".into()),
                };
                Ok(Some(SttProvider::Soniox { key, region }))
            }
            Some("elevenlabs") => Ok(Some(SttProvider::ElevenLabs { key })),
            _ => Err(SEND.into()),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            SttProvider::Soniox { .. } => "soniox",
            SttProvider::ElevenLabs { .. } => "elevenlabs",
        }
    }

    pub fn key(&self) -> &str {
        match self {
            SttProvider::Soniox { key, .. } | SttProvider::ElevenLabs { key } => key,
        }
    }

    /// 오류 글에서 이 키를 가려요(앞뒤 4자만 남겨요).
    pub fn mask(&self, text: &str) -> String {
        let k = self.key();
        if k.len() > 8 {
            text.replace(k, &format!("{}…{}", &k[..4], &k[k.len() - 4..]))
        } else {
            text.replace(k, "…")
        }
    }
}
