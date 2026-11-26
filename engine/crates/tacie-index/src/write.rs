//! Writes an index folder and rolls back an interrupted swap.
//! The file format does not change down to the byte: chunks.json and meta.json use `pyjson`, emb.npy uses `npy`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use serde_json::{Map, Value, json};
use tacie_embed::{DIM, PASSAGE_TOKENS};

use crate::chunk::{RawChunk, read_meta, read_raw_chunks};
use crate::embcache::EmbCache;
use crate::npy::{read_f32_2d, write_f32_2d};
use crate::pyjson;

/// 임베딩 모델 이름이에요(meta.json `embed.model`, 캐시 폴더 `BAAI__bge-m3@512`). ONNX로 돌려도 벡터가 같아서
/// 예전에 만든 색인과 캐시를 그대로 이어 써요.
pub const MODEL: &str = "BAAI/bge-m3";

/// 글들을 받아 같은 순서의 벡터를 주는 임베더예요(갱신은 `/ask`와 같은 자물쇠로 나눠 불러요).
pub type EmbedFn<'a> = dyn FnMut(&[&str]) -> anyhow::Result<Vec<Vec<f32>>> + 'a;

/// 색인의 세 파일이에요. 셋이 다 있어야 다 쓴 색인이에요.
const FILES: [&str; 3] = ["chunks.json", "emb.npy", "meta.json"];

/// 옛 색인의 벡터를 캐시에 넣어도 되는지예요: 캐시와 같은 모델과 글 길이로 임베딩했어야 해요.
/// meta.json에 "embed"가 없던 때의 색인은 BAAI/bge-m3, 512로 만들었어요. meta.json을 못 읽으면 false예요.
pub fn seedable(old: &Path, cache: &EmbCache) -> bool {
    check_seedable(old, cache).unwrap_or(false)
}

/// `seedable`인데 meta.json을 못 읽으면 Err예요.
fn check_seedable(old: &Path, cache: &EmbCache) -> anyhow::Result<bool> {
    let meta = read_meta(&old.join("meta.json"))?; // 파일이 없으면 빈 meta예요
    Ok(match meta.get("embed") {
        Some(embed) => *embed == json!({"model": cache.model(), "tokens": cache.tokens()}),
        None => cache.model() == MODEL && cache.tokens() == PASSAGE_TOKENS,
    })
}

fn text_of(c: &RawChunk, i: usize) -> anyhow::Result<&str> {
    c.get("text")
        .and_then(Value::as_str)
        .with_context(|| format!("chunk {i} has no text"))
}

/// 글은 이 순서로 가져와요: 캐시(`cache`의 EmbCache)에 있는 글은 거기서, `reuse_from`(없으면 `out_dir`)의
/// 옛 색인에 있는 글은 거기서(같은 글이 여럿이면 뒤의 것) 가져와요. 옛 색인은 캐시가 없거나 `seedable`일 때만 봐요.
/// 나머지만 순서대로 `embed`하고, 캐시가 있으면 가져온 것과 새 것을 색인 순서로 넣은 뒤 태그가 이번 글 전부를
/// 가리키게 해요. 그리고 emb.npy(조각이 없으면 (0, 1024)), chunks.json, meta.json(1칸 들여쓰기)을 써요.
/// 임베딩한 수를 돌려줘요. 길이가 1024가 아닌 벡터가 하나라도 있으면 아무것도 쓰지 않고 Err예요.
pub fn write_index(
    out_dir: &Path,
    chunks: &[RawChunk],
    embed: &mut EmbedFn<'_>,
    meta: &Map<String, Value>,
    reuse_from: Option<&Path>,
    cache: Option<(&EmbCache, &str)>,
) -> anyhow::Result<usize> {
    fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    let old = reuse_from.unwrap_or(out_dir);
    let texts: Vec<&str> = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| text_of(c, i))
        .collect::<anyhow::Result<_>>()?;
    let have = match cache {
        Some((cache, _)) => cache.get(&texts)?,
        None => HashMap::new(),
    };

    let mut seeded: HashMap<usize, Vec<f32>> = HashMap::new();
    let seed = old.join("emb.npy").exists()
        && match cache {
            Some((cache, _)) => check_seedable(old, cache)?,
            None => true,
        };
    if seed {
        let (old_emb, rows, cols) = read_f32_2d(&old.join("emb.npy"))?;
        let old_chunks = read_raw_chunks(&old.join("chunks.json"))?;
        let mut by_text: HashMap<&str, usize> = HashMap::new();
        for (i, c) in old_chunks.iter().enumerate() {
            ensure!(
                i < rows,
                "{} has {rows} rows but its chunks.json has {} chunks",
                old.join("emb.npy").display(),
                old_chunks.len()
            );
            by_text.insert(text_of(c, i)?, i); // 같은 글이면 뒤의 줄이 이겨요
        }
        for (i, t) in texts.iter().enumerate() {
            if have.contains_key(&i) {
                continue;
            }
            if let Some(&row) = by_text.get(t) {
                seeded.insert(i, old_emb[row * cols..(row + 1) * cols].to_vec());
            }
        }
    }

    let todo: Vec<usize> = (0..chunks.len())
        .filter(|i| !have.contains_key(i) && !seeded.contains_key(i))
        .collect();
    let new = if todo.is_empty() {
        Vec::new()
    } else {
        embed(&todo.iter().map(|&i| texts[i]).collect::<Vec<_>>())?
    };
    ensure!(
        new.len() == todo.len(),
        "the embedder gave {} vectors for {} texts",
        new.len(),
        todo.len()
    );
    let fresh: HashMap<usize, Vec<f32>> = todo.iter().copied().zip(new).collect();

    let mut emb = Vec::with_capacity(chunks.len() * DIM);
    for i in 0..chunks.len() {
        let v = fresh
            .get(&i)
            .or_else(|| seeded.get(&i))
            .or_else(|| have.get(&i))
            .expect("every chunk is cached, seeded or embedded");
        // 모양이 다른 벡터는 쌓지 않아요. 1024가 아니면 색인이 아니에요.
        ensure!(
            v.len() == DIM,
            "chunk {i}'s vector has {} values, not {DIM}",
            v.len()
        );
        emb.extend_from_slice(v);
    }

    if let Some((cache, tag)) = cache {
        let mut add: Vec<usize> = seeded.keys().chain(fresh.keys()).copied().collect();
        add.sort_unstable();
        let add_texts: Vec<&str> = add.iter().map(|&i| texts[i]).collect();
        let add_vecs: Vec<Vec<f32>> = add
            .iter()
            .map(|&i| emb[i * DIM..(i + 1) * DIM].to_vec())
            .collect();
        cache.put(&add_texts, &add_vecs)?;
        cache.retag(tag, &texts)?;
    }
    write_f32_2d(&out_dir.join("emb.npy"), &emb, chunks.len(), DIM)?;
    let path = out_dir.join("chunks.json");
    fs::write(&path, pyjson::objects_to_string(chunks, None))
        .with_context(|| format!("write {}", path.display()))?;
    let path = out_dir.join("meta.json");
    fs::write(
        &path,
        pyjson::to_string(&Value::Object(meta.clone()), Some(1)),
    )
    .with_context(|| format!("write {}", path.display()))?;
    Ok(todo.len())
}

/// 두 번의 이름 바꾸기 사이에서 끊긴 바꿔 넣기는 `index`를 남기지 않아요: `index.old`를, 없으면 다 쓴(세 파일이 다
/// 있는) `index.new`를 `index`로 되돌려요. 색인을 읽기 전에 불러요. 되돌린 폴더를 돌려줘요.
pub fn recover(index_dir: &Path) -> std::io::Result<Option<PathBuf>> {
    if index_dir.exists() {
        return Ok(None);
    }
    let name = index_dir.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} has no folder name", index_dir.display()),
        )
    })?;
    let side = |suffix: &str| {
        let mut n = name.to_os_string();
        n.push(suffix);
        index_dir.with_file_name(n)
    };
    let (old, new) = (side(".old"), side(".new"));
    let whole = new.is_dir() && FILES.iter().all(|f| new.join(f).is_file());
    let back = if old.is_dir() {
        old
    } else if whole {
        new
    } else {
        return Ok(None);
    };
    fs::rename(&back, index_dir)?;
    eprintln!(
        "index: put back {}, the last swap was cut off",
        back.file_name().unwrap_or_default().to_string_lossy()
    );
    Ok(Some(back))
}
