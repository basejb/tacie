//! Checks that the speaker embedding's input (Kaldi fbank) matches kaldi-native-fbank. The fixed values come from
//! `eval/voice_fixture.py`.

use std::path::PathBuf;

use serde_json::Value;
use tacie_embed::voice::{fbank, pcm16_mono, subtract_mean};

const CLIPS: [&str; 4] = ["a_1.wav", "a_2.wav", "b_1.wav", "b_2.wav"];

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/voice")
        .join(name)
}

fn expected() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture("fbank.json")).unwrap()).unwrap()
}

/// 16 kHz mono 16-bit WAV 헤더 + 샘플이에요.
fn wav(rate: u32, channels: u16, bits: u16, data: &[u8]) -> Vec<u8> {
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits) / 8).to_le_bytes());
    w.extend_from_slice(&(channels * bits / 8).to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(data);
    w
}

#[test]
fn fbank_matches_kaldi_native_fbank() {
    let exp = expected();
    for name in CLIPS {
        let pcm = pcm16_mono(&std::fs::read(fixture(name)).unwrap()).unwrap();
        let got = fbank(&pcm);
        assert_eq!(
            got.len() as u64,
            exp["num_frames"][name].as_u64().unwrap(),
            "{name} frames"
        );
        let want = exp["frames"][name].as_array().unwrap();
        for (i, row) in want.iter().enumerate() {
            for (j, v) in row.as_array().unwrap().iter().enumerate() {
                let d = (got[i][j] - v.as_f64().unwrap() as f32).abs();
                assert!(
                    d < 1e-3,
                    "{name} frame {i} bin {j}: got {} want {v}",
                    got[i][j]
                );
            }
        }
    }
}

#[test]
fn fbank_frame_count_without_snip_edges() {
    assert_eq!(fbank(&vec![0.0; 16_000]).len(), 100);
    assert_eq!(fbank(&vec![0.0; 159]).len(), 1);
}

#[test]
fn fbank_of_short_audio_is_empty() {
    assert!(fbank(&[]).is_empty());
    assert!(fbank(&vec![0.0; 79]).is_empty());
}

#[test]
fn pcm16_mono_reads_samples_and_rejects_other_formats() {
    let data: Vec<u8> = [0i16, 16384, -32768]
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    assert_eq!(
        pcm16_mono(&wav(16_000, 1, 16, &data)).unwrap(),
        vec![0.0, 0.5, -1.0]
    );
    assert!(pcm16_mono(&wav(44_100, 1, 16, &data)).is_err());
    assert!(pcm16_mono(&wav(16_000, 2, 16, &data)).is_err());
    assert!(pcm16_mono(&wav(16_000, 1, 8, &data)).is_err());
    assert!(pcm16_mono(b"RIFF").is_err());
}

#[test]
fn pcm16_mono_skips_extra_chunks() {
    let data: Vec<u8> = [8192i16].iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut w = wav(16_000, 1, 16, &data);
    // fmt 청크 뒤에 LIST 청크(홀수 길이 + 채움 바이트)를 끼워요
    let list = [b"LIST".as_slice(), &3u32.to_le_bytes(), b"abc", &[0]].concat();
    w.splice(36..36, list);
    assert_eq!(pcm16_mono(&w).unwrap(), vec![0.25]);
}

#[test]
fn subtract_mean_zeroes_each_bin_mean() {
    let pcm = pcm16_mono(&std::fs::read(fixture("a_1.wav")).unwrap()).unwrap();
    let mut frames = fbank(&pcm);
    subtract_mean(&mut frames);
    for j in 0..80 {
        let mean: f32 = frames.iter().map(|f| f[j]).sum::<f32>() / frames.len() as f32;
        assert!(mean.abs() < 1e-4, "bin {j} mean {mean}");
    }
}
