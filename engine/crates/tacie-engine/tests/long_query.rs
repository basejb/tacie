//! Long questions are cut at 2,048 tokens (docs/04 2-1). Filling up to 8,192 takes 17–53 s and about 22 GB on CPU,
//! and other questions are blocked meanwhile. Skipped when the model folder (`TACIE_MODEL_DIR`) is missing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tacie_embed::{Device, Embed, OnnxEmbedder};
use tacie_index::{Index, Mask};

const MINI: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tacie-index/tests/fixtures/mini"
);
const LIMIT: Duration = Duration::from_secs(15);

fn model() -> Option<OnnxEmbedder> {
    let Some(dir) = std::env::var_os("TACIE_MODEL_DIR").map(PathBuf::from) else {
        eprintln!("skip: TACIE_MODEL_DIR unset");
        return None;
    };
    Some(OnnxEmbedder::load(&dir, Device::Cpu).expect("load bge-m3"))
}

fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let took = start.elapsed();
    assert!(took < LIMIT, "{what} took {took:?}");
    out
}

#[test]
fn long_questions_are_cut_at_2048_tokens() {
    let Some(e) = model() else { return };
    let a = "가나다 라마 ".repeat(3000);
    let b = "가나다 라마 ".repeat(6000);
    let va = timed("3,000 repeats", || e.embed(&[&a], None).unwrap());
    let vb = timed("6,000 repeats", || e.embed(&[&b], None).unwrap());
    // 둘 다 앞 2,048토큰만 남아서 같은 벡터예요.
    let worst = va[0]
        .iter()
        .zip(&vb[0])
        .map(|(x, y)| (x - y).abs())
        .fold(0f32, f32::max);
    assert!(worst < 1e-6, "vectors differ by {worst}");
}

#[test]
fn long_context_thread_is_cut_too() {
    let Some(e) = model() else { return };
    let b = "가나다 라마 ".repeat(6000);
    let index = Index::open(Path::new(MINI), Mask::Plans, &Default::default()).unwrap();
    let ids = timed("candidates", || {
        index
            .candidates(&e, "포트", std::slice::from_ref(&b), &format!("나: {b}"))
            .unwrap()
    });
    assert!(ids.len() <= 50, "{}", ids.len());
}
