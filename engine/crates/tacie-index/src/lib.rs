//! Reads an index folder (chunks.json, emb.npy, meta.json) and finds candidates among the chunks the person can see.
//! Always writes the index in the same byte layout (the file format does not change) and rolls back an
//! interrupted swap.

pub mod chunk;
pub mod embcache;
pub mod index;
pub mod mask;
pub mod npy;
pub mod pyjson;
pub mod write;

pub use chunk::{Chunk, RawChunk};
pub use embcache::EmbCache;
pub use index::{CAP, Index, PER};
pub use mask::Mask;
pub use write::{EmbedFn, MODEL, recover, seedable, write_index};
