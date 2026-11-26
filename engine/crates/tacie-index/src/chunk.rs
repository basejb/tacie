//! One index chunk. Unknown fields (author, at ...) are kept as is in `extra`.

use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

#[derive(Serialize, Deserialize, Clone)]
pub struct Chunk {
    pub src: String,
    pub path: String,
    pub line: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// 키가 없으면 `None`(모두에게 열려요), 값이 `null`이면 `Some([])`(아무도 못 봐요)예요.
    #[serde(
        default,
        deserialize_with = "null_acl_is_empty",
        skip_serializing_if = "Option::is_none"
    )]
    pub acl: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `"acl": null`을 빈 목록으로 읽어 닫힌 쪽으로 둬요. 키가 아예 없을 때는 `#[serde(default)]`가 `None`을 줘요.
fn null_acl_is_empty<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
    Ok(Some(
        Option::<Vec<String>>::deserialize(d)?.unwrap_or_default(),
    ))
}

/// chunks.json의 조각 하나를 읽은 그대로예요: 키 순서와 null이 파일과 같아서 다시 쓰면 같은 바이트예요(`pyjson`).
/// 출처가 정한 키 순서로 만들고 `write_index`가 그대로 써요. 검색과 카드는 여기서 만든 `Chunk`를 써요.
pub type RawChunk = Map<String, Value>;

/// chunks.json을 조각 목록으로 읽어요. 같은 키가 두 번 있으면 뒤의 값이에요(Python `json.loads`와 같아요).
pub fn read_raw_chunks(path: &Path) -> anyhow::Result<Vec<RawChunk>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

/// 읽은 조각에서 검색용 `Chunk`를 만들어요(raw는 그대로 둬요).
pub fn chunk_of(raw: &RawChunk) -> serde_json::Result<Chunk> {
    Chunk::deserialize(raw)
}

pub fn read_chunks(path: &Path) -> anyhow::Result<Vec<Chunk>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

/// 파일이 없으면 빈 Map이에요.
pub fn read_meta(path: &Path) -> anyhow::Result<Map<String, Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}
