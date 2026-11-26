//! Unpacks the weights, halved (fp16) only for download, into the official fp32 weights on the first run.
//!
//! The official bge-m3 ONNX `model.onnx_data` is an array of 389 float32 tensors packed back to back, and every value
//! is exactly representable in fp16 (checked 2026-10-26: converting to fp16 and back gives the same bytes). So we
//! download `model.onnx_data.f16` (2 bytes per value) and widen it to 4 bytes here, which gives back the official file
//! as is. The graph (`model.onnx`), the vectors and the embedding cache names do not change. The producer side is
//! `eval/pack_model.py`.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::Path;

use anyhow::{Context, bail};
use half::f16;
use half::slice::HalfFloatSliceExt;

/// 엔진이 읽는 가중치 파일이에요.
pub const WEIGHTS: &str = "model.onnx_data";
/// 받는 가중치 파일이에요(fp16, 리틀 엔디언).
pub const PACKED_WEIGHTS: &str = "model.onnx_data.f16";
const PARTIAL: &str = "model.onnx_data.partial";

/// 읽기 단위예요(값 4M개, 8 MiB). 짝수여야 해요.
const CHUNK_BYTES: usize = 8 << 20;

/// `dir`에 `model.onnx_data`가 없고 `model.onnx_data.f16`이 있으면 풀어서 `model.onnx_data`를 만들고
/// `.f16`을 지워요. 풀었으면 true, 할 일이 없었으면 false예요.
///
/// `.partial`에 다 쓰고 디스크에 내린 뒤 이름을 바꿔서, 중간에 끊겨도 반쯤 쓴 `model.onnx_data`는
/// 남지 않아요(다음 실행이 처음부터 다시 풀어요).
pub fn unpack_weights(dir: &Path) -> anyhow::Result<bool> {
    let dst = dir.join(WEIGHTS);
    let src = dir.join(PACKED_WEIGHTS);
    if dst.exists() || !src.is_file() {
        return Ok(false);
    }
    let partial = dir.join(PARTIAL);
    let result = write_unpacked(&src, &partial);
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result?;
    fs::rename(&partial, &dst).with_context(|| format!("rename to {}", dst.display()))?;
    // 다 풀었으니 받은 파일은 지워요(디스크 1.1GB). 못 지워도 엔진은 돌아요.
    let _ = fs::remove_file(&src);
    Ok(true)
}

fn write_unpacked(src: &Path, partial: &Path) -> anyhow::Result<()> {
    let mut input = File::open(src).with_context(|| format!("open {}", src.display()))?;
    let len = input.metadata()?.len();
    if len == 0 || len % 2 != 0 {
        bail!(
            "{} has {len} bytes: not a list of fp16 values",
            src.display()
        );
    }
    let out = File::create(partial).with_context(|| format!("create {}", partial.display()))?;
    let mut writer = BufWriter::with_capacity(CHUNK_BYTES * 2, out);
    let mut bytes = vec![0u8; CHUNK_BYTES];
    let mut halves = vec![f16::ZERO; CHUNK_BYTES / 2];
    let mut floats = vec![0f32; CHUNK_BYTES / 2];
    let mut done: u64 = 0;
    while done < len {
        let n = ((len - done) as usize).min(CHUNK_BYTES);
        input
            .read_exact(&mut bytes[..n])
            .context("read packed weights")?;
        let count = n / 2;
        for (h, pair) in halves[..count]
            .iter_mut()
            .zip(bytes[..n].as_chunks::<2>().0)
        {
            *h = f16::from_le_bytes(*pair);
        }
        halves[..count].convert_to_f32_slice(&mut floats[..count]);
        for x in &floats[..count] {
            writer.write_all(&x.to_le_bytes())?;
        }
        done += n as u64;
    }
    let out = writer.into_inner().map_err(|e| e.into_error())?;
    out.sync_all().context("flush unpacked weights")?;
    let written = out.metadata()?.len();
    if written != len * 2 {
        bail!("unpacked {written} bytes, expected {}", len * 2);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(values: &[f32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|&x| f16::from_f32(x).to_le_bytes())
            .collect()
    }

    fn floats(bytes: &[u8]) -> Vec<f32> {
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect()
    }

    #[test]
    fn unpacks_to_the_same_float32_bytes() {
        // fp16으로 정확히 나타나는 값들: 0, -0, 부호, 정규수 끝, 비정규수, 무한대
        let values = [
            0.0f32,
            -0.0,
            1.0,
            -1.8544922,
            0.000_061_035_156,
            5.960_464_5e-8,
            65504.0,
            f32::INFINITY,
        ];
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(PACKED_WEIGHTS), packed(&values)).unwrap();

        assert!(unpack_weights(dir.path()).unwrap());

        let out = fs::read(dir.path().join(WEIGHTS)).unwrap();
        let got = floats(&out);
        assert_eq!(got.len(), values.len());
        for (g, v) in got.iter().zip(values) {
            assert_eq!(g.to_bits(), v.to_bits(), "{g} vs {v}");
        }
        assert!(!dir.path().join(PACKED_WEIGHTS).exists());
        assert!(!dir.path().join(PARTIAL).exists());
    }

    #[test]
    fn every_fp16_value_round_trips() {
        // 받는 쪽이 푼 값을 다시 fp16으로 바꾸면 같은 비트예요(NaN만 빼고).
        let all: Vec<f16> = (0..=u16::MAX).map(f16::from_bits).collect();
        let mut out = vec![0f32; all.len()];
        all.convert_to_f32_slice(&mut out);
        for (h, x) in all.iter().zip(&out) {
            if h.is_nan() {
                assert!(x.is_nan());
            } else {
                assert_eq!(f16::from_f32(*x).to_bits(), h.to_bits());
            }
        }
    }

    #[test]
    fn spans_more_than_one_read() {
        let n = CHUNK_BYTES / 2 + 3;
        let values: Vec<f32> = (0..n).map(|i| (i % 2048) as f32).collect();
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(PACKED_WEIGHTS), packed(&values)).unwrap();

        assert!(unpack_weights(dir.path()).unwrap());

        assert_eq!(floats(&fs::read(dir.path().join(WEIGHTS)).unwrap()), values);
    }

    #[test]
    fn leaves_existing_weights_alone() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(WEIGHTS), b"official").unwrap();
        fs::write(dir.path().join(PACKED_WEIGHTS), packed(&[1.0])).unwrap();

        assert!(!unpack_weights(dir.path()).unwrap());

        assert_eq!(fs::read(dir.path().join(WEIGHTS)).unwrap(), b"official");
    }

    #[test]
    fn nothing_to_do_without_a_packed_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!unpack_weights(dir.path()).unwrap());
        assert!(!dir.path().join(WEIGHTS).exists());
    }

    #[test]
    fn an_odd_length_file_fails_without_leaving_weights() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(PACKED_WEIGHTS), [0u8, 60, 1]).unwrap();

        let err = unpack_weights(dir.path()).unwrap_err();

        assert!(
            format!("{err:#}").contains("not a list of fp16 values"),
            "{err:#}"
        );
        assert!(!dir.path().join(WEIGHTS).exists());
        assert!(!dir.path().join(PARTIAL).exists());
        assert!(dir.path().join(PACKED_WEIGHTS).exists());
    }
}
