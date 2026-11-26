//! Measurement tool: embeds each JSON string line from stdin and writes it to stdout as npy (N x 1024 f32).
//! Usage: embed_jsonl --model-dir DIR --device coreml|cpu [--passage]
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Instant;

use tacie_embed::{DIM, Device, Embed, OnnxEmbedder, PASSAGE_TOKENS};

fn npy_bytes(rows: &[Vec<f32>]) -> Vec<u8> {
    let mut header = format!(
        "{{'descr': '<f4', 'fortran_order': False, 'shape': ({}, {}), }}",
        rows.len(),
        DIM
    );
    // 10바이트 앞머리 + 헤더(공백 패딩, 마지막 \n)가 64의 배수가 되게 맞춘다.
    while (10 + header.len() + 1) % 64 != 0 {
        header.push(' ');
    }
    header.push('\n');
    let mut out = Vec::with_capacity(10 + header.len() + rows.len() * DIM * 4);
    out.extend_from_slice(b"\x93NUMPY\x01\x00");
    out.extend_from_slice(&(header.len() as u16).to_le_bytes());
    out.extend_from_slice(header.as_bytes());
    for row in rows {
        for v in row {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn main() -> anyhow::Result<()> {
    let mut model_dir = None;
    let mut device = Device::CoreMl;
    let mut passage = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--model-dir" => model_dir = args.next().map(PathBuf::from),
            "--device" => {
                device = match args.next().as_deref() {
                    Some("coreml") => Device::CoreMl,
                    Some("cpu") => Device::Cpu,
                    other => anyhow::bail!("unknown device: {other:?}"),
                }
            }
            "--passage" => passage = true,
            other => anyhow::bail!("unknown flag: {other}"),
        }
    }
    let model_dir = model_dir.ok_or_else(|| anyhow::anyhow!("--model-dir is required"))?;

    let mut texts = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        texts.push(serde_json::from_str::<String>(&line)?);
    }

    let t = Instant::now();
    let embedder = OnnxEmbedder::load(&model_dir, device)?;
    let load_s = t.elapsed().as_secs_f64();

    let max_tokens = if passage { Some(PASSAGE_TOKENS) } else { None };
    let name = match embedder.device() {
        Device::CoreMl => "coreml",
        Device::Cpu => "cpu",
    };
    // 질문(--passage 없음)은 첫 실행과 안정 실행을 둘 다 재려고 두 번 돌린다. 마지막 줄이 안정 실행이다.
    let runs = if passage { 1 } else { 2 };
    let mut rows: Vec<Vec<f32>> = Vec::new();
    for _ in 0..runs {
        let t = Instant::now();
        rows = Vec::with_capacity(texts.len());
        // 질문은 한 번에, 조각은 4개씩(CPU에서 8개, 16개도 546개 98 s로 같고 메모리만 2.4 GB, 3.2 GB로 커짐).
        let batch = if passage { 4 } else { texts.len().max(1) };
        for chunk in texts.chunks(batch) {
            let refs: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
            rows.extend(embedder.embed(&refs, max_tokens)?);
        }
        let secs = t.elapsed().as_secs_f64();
        eprintln!();
        eprintln!(
            "embedded {} texts in {secs:.3} s on {name} (load {load_s:.1} s)",
            rows.len()
        );
    }

    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&npy_bytes(&rows))?;
    stdout.flush()?;
    Ok(())
}
