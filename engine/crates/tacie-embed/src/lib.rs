//! Turns text into normalized 1024-dimension vectors with bge-m3. It does not depend on HTTP, so the relay server can
//! use it too.

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod gpu;
mod onnx;
mod pack;
mod tokenize;
pub mod voice;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use gpu::GpuEmbedder;

pub use onnx::{Device, OnnxEmbedder};
pub use pack::{PACKED_WEIGHTS, WEIGHTS, unpack_weights};

/// 벡터 길이예요.
pub const DIM: usize = 1024;

/// 문단(색인 조각)을 자르는 토큰 수예요. 특수 토큰을 포함해요.
pub const PASSAGE_TOKENS: usize = 512;

pub trait Embed: Send + Sync {
    /// 각 글의 정규화 벡터(길이 DIM). max_tokens = Some(512)면 문단, None이면 질문이고 2,048토큰에서 잘라요(docs/04 2-1).
    fn embed(&self, texts: &[&str], max_tokens: Option<usize>) -> anyhow::Result<Vec<Vec<f32>>>;
}
