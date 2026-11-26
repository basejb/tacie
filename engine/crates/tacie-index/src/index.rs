//! One index: chunks, vectors, meta, and the marks for which chunks this person may find.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, ensure};
use serde_json::{Map, Value};
use tacie_embed::{DIM, Embed};

use crate::chunk::{Chunk, RawChunk, chunk_of, read_meta, read_raw_chunks};
use crate::mask::{Mask, allowed};
use crate::npy::read_f32_2d;

/// 앞 대화가 있을 때 질문과 (앞 두 줄 + 이번 줄)에서 각각 뽑는 수예요.
pub const PER: usize = 30;
/// 후보 최대 수예요.
pub const CAP: usize = 50;

/// 조각, raw, 벡터는 `Arc`라서 meta만 바꾼 색인(`with_meta`)이 그것들을 복사하지 않아요.
pub struct Index {
    /// 검색과 카드가 읽는 조각이에요(`raw`에서 만들어요).
    pub chunks: Arc<Vec<Chunk>>,
    /// chunks.json을 읽은 그대로예요(키 순서와 null까지). 갱신이 이걸 비교하고 다시 써요.
    pub raw: Arc<Vec<RawChunk>>,
    /// N×1024, 행 우선이에요.
    pub emb: Arc<Vec<f32>>,
    pub meta: Map<String, Value>,
    pub allowed: Vec<bool>,
    pub mask: Mask,
    pub viewer: Vec<String>,
}

impl Index {
    pub fn open(dir: &Path, mask: Mask, viewer: &HashSet<String>) -> anyhow::Result<Self> {
        let chunks_path = dir.join("chunks.json");
        let raw = read_raw_chunks(&chunks_path)?;
        let chunks: Vec<Chunk> = raw
            .iter()
            .enumerate()
            .map(|(i, c)| {
                chunk_of(c).with_context(|| format!("{} chunk {i}", chunks_path.display()))
            })
            .collect::<anyhow::Result<_>>()?;
        let emb_path = dir.join("emb.npy");
        let (emb, rows, cols) = read_f32_2d(&emb_path)?;
        ensure!(
            (rows, cols) == (chunks.len(), DIM),
            "{} is ({rows}, {cols}) but the index has {} chunks of dim {DIM}",
            emb_path.display(),
            chunks.len()
        );
        let meta = read_meta(&dir.join("meta.json"))?;
        let allowed = allowed(&chunks, mask, viewer);
        let mut viewer: Vec<String> = viewer.iter().cloned().collect();
        viewer.sort();
        Ok(Self {
            chunks: Arc::new(chunks),
            raw: Arc::new(raw),
            emb: Arc::new(emb),
            meta,
            allowed,
            mask,
            viewer,
        })
    }

    /// meta만 바꾼 같은 색인이에요: 조각, raw, 벡터는 `Arc`를 나눠 써서 복사하지 않아요(바뀐 게 없는 갱신).
    pub fn with_meta(&self, meta: Map<String, Value>) -> Index {
        Index {
            chunks: self.chunks.clone(),
            raw: self.raw.clone(),
            emb: self.emb.clone(),
            meta,
            allowed: self.allowed.clone(),
            mask: self.mask,
            viewer: self.viewer.clone(),
        }
    }

    /// 찾아도 되는 조각 중 위 k개예요. 가린 조각은 몇 개가 남든 자리를 채우지 않아요.
    /// 점수는 벡터 내적, 내림차순이고 동점은 낮은 id가 먼저예요.
    pub fn dense(&self, vec: &[f32], k: usize) -> Vec<usize> {
        self.dense_where(vec, k, &|_| true)
    }

    /// `dense`와 같지만 `keep`이 받아 준 조각 안에서만 위 k개를 골라요(가린 조각은 여기서도 빠져요).
    /// 좁힌 검색(회의 하나)이 전체 위 k개 밖으로 밀려나지 않게, 거르고 나서 순위를 매겨요.
    pub fn dense_where(&self, vec: &[f32], k: usize, keep: &dyn Fn(usize) -> bool) -> Vec<usize> {
        debug_assert_eq!(vec.len(), DIM);
        let mut scored: Vec<(f32, usize)> = self
            .emb
            .as_chunks::<DIM>()
            .0
            .iter()
            .enumerate()
            .filter(|&(id, _)| self.allowed[id] && keep(id))
            .map(|(id, row)| (row.iter().zip(vec).map(|(a, b)| a * b).sum(), id))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0)); // 안정 정렬이라 동점은 id 순서 그대로예요
        scored.into_iter().take(k).map(|(_, id)| id).collect()
    }

    /// 앞 대화가 없으면 질문으로 위 CAP개. 있으면 질문으로 위 PER개, (앞 두 줄 + 이번 줄)로 위 PER개를
    /// 그 순서로 이어 중복을 빼고 앞 CAP개예요. 대화를 붙여 한 번 찾는 건 도움이 안 됐어요(docs/01).
    pub fn candidates(
        &self,
        embedder: &dyn Embed,
        text: &str,
        context: &[String],
        line: &str,
    ) -> anyhow::Result<Vec<usize>> {
        self.candidates_where(embedder, text, context, line, &|_| true)
    }

    /// `candidates`와 같지만 `keep`이 받아 준 조각 안에서만 찾아요(`dense_where`).
    pub fn candidates_where(
        &self,
        embedder: &dyn Embed,
        text: &str,
        context: &[String],
        line: &str,
        keep: &dyn Fn(usize) -> bool,
    ) -> anyhow::Result<Vec<usize>> {
        let thread = (!context.is_empty()).then(|| {
            let mut lines: Vec<&str> = context[context.len().saturating_sub(2)..]
                .iter()
                .map(String::as_str)
                .collect();
            lines.push(line);
            lines.join("\n")
        });
        let mut texts = vec![text];
        texts.extend(thread.as_deref());
        let v = embedder.embed(&texts, None)?;
        ensure!(
            v.len() == texts.len(),
            "embedder gave {} vectors for {} texts",
            v.len(),
            texts.len()
        );

        let pick = match thread {
            None => self.dense_where(&v[0], CAP, keep),
            Some(_) => [self.dense_where(&v[0], PER, keep), self.dense_where(&v[1], PER, keep)].concat(),
        };
        let mut seen = HashSet::new();
        Ok(pick
            .into_iter()
            .filter(|id| seen.insert(*id))
            .take(CAP)
            .collect())
    }

    /// 찾을 조각이 하나라도 있는지예요.
    pub fn searchable(&self) -> bool {
        !self.chunks.is_empty() && self.allowed.iter().any(|&ok| ok)
    }
}
