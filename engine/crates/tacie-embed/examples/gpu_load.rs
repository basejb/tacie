//! Measurement tool: keeps embedding passages on the GPU (MLX), as sync does. Measures how much each slows the other
//! down when it overlaps other work (speech recognition).
//! Usage: gpu_load --model-dir DIR --texts FILE(JSON array of strings) --seconds N [--batch 8]
//! Every 10 seconds prints `gpu_load t=<초> passages_per_s=<수>` to stderr, and the overall average at the end.
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tacie_embed::{Embed, GpuEmbedder, PASSAGE_TOKENS};

fn main() -> anyhow::Result<()> {
    let (mut model_dir, mut texts, mut seconds, mut batch) = (None, None, 60u64, 8usize);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--model-dir" => model_dir = args.next().map(PathBuf::from),
            "--texts" => texts = args.next().map(PathBuf::from),
            "--seconds" => seconds = args.next().unwrap_or_default().parse()?,
            "--batch" => batch = args.next().unwrap_or_default().parse()?,
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let texts: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(texts.expect("--texts"))?)?;
    let gpu = GpuEmbedder::load(&model_dir.expect("--model-dir"))?;
    let start = Instant::now();
    let (mut done, mut window, mut mark) = (0usize, 0usize, Instant::now());
    let mut i = 0;
    while start.elapsed() < Duration::from_secs(seconds) {
        let slice: Vec<&str> = (0..batch)
            .map(|k| texts[(i + k) % texts.len()].as_str())
            .collect();
        gpu.embed(&slice, Some(PASSAGE_TOKENS))?;
        i += batch;
        done += batch;
        window += batch;
        if mark.elapsed() >= Duration::from_secs(10) {
            eprintln!(
                "gpu_load t={:.0} passages_per_s={:.1}",
                start.elapsed().as_secs_f64(),
                window as f64 / mark.elapsed().as_secs_f64()
            );
            window = 0;
            mark = Instant::now();
        }
    }
    eprintln!(
        "gpu_load total passages_per_s={:.1}",
        done as f64 / start.elapsed().as_secs_f64()
    );
    Ok(())
}
