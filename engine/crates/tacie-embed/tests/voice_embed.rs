//! Checks that ERes2Net speaker embeddings match sherpa-onnx. The model (40 MB) is not in git, so this is skipped
//! without `TACIE_VOICE_MODEL`.

use std::path::PathBuf;

use serde_json::Value;
use tacie_embed::voice::{VOICE_DIM, VoiceEmbed, VoiceEmbedder};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/voice")
        .join(name)
}

fn embedder() -> Option<VoiceEmbedder> {
    let Some(model) = std::env::var_os("TACIE_VOICE_MODEL") else {
        eprintln!("skip: TACIE_VOICE_MODEL is not set");
        return None;
    };
    Some(VoiceEmbedder::load(model.as_ref()).unwrap())
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}

fn print_of(v: &VoiceEmbedder, name: &str) -> Vec<f32> {
    v.embed(&std::fs::read(fixture(name)).unwrap())
        .unwrap()
        .unwrap()
        .vector
}

#[test]
fn embeddings_match_sherpa_onnx() {
    let Some(v) = embedder() else { return };
    let want: Value =
        serde_json::from_str(&std::fs::read_to_string(fixture("embeddings.json")).unwrap())
            .unwrap();
    for name in ["a_1.wav", "a_2.wav", "b_1.wav", "b_2.wav"] {
        let p = v
            .embed(&std::fs::read(fixture(name)).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(p.vector.len(), VOICE_DIM);
        let norm: f32 = p.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "{name} norm {norm}");
        let w: Vec<f32> = want[name]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_f64().unwrap() as f32)
            .collect();
        let c = cosine(&p.vector, &w);
        assert!(c >= 0.999, "{name} cosine {c}");
        assert!(
            (p.seconds - 3.0).abs() < 0.01,
            "{name} seconds {}",
            p.seconds
        );
    }
}

#[test]
fn same_speaker_is_closer_than_other_speaker() {
    let Some(v) = embedder() else { return };
    let (a1, a2, b1) = (
        print_of(&v, "a_1.wav"),
        print_of(&v, "a_2.wav"),
        print_of(&v, "b_1.wav"),
    );
    assert!(cosine(&a1, &a2) > cosine(&a1, &b1));
}

#[test]
fn embed_returns_none_for_no_frames() {
    let Some(v) = embedder() else { return };
    // 헤더만 있는 WAV(샘플 0개)
    let wav = std::fs::read(fixture("a_1.wav")).unwrap();
    let mut empty = wav[..44].to_vec();
    empty[40..44].copy_from_slice(&0u32.to_le_bytes());
    assert!(v.embed(&empty).unwrap().is_none());
}

#[test]
fn embed_rejects_a_broken_wav() {
    let Some(v) = embedder() else { return };
    assert!(v.embed(b"not a wav").is_err());
}

/// iPhone runs only the fine-tuned CAM++ (docs/27 7-11): its vector must be the one the two-model embedder groups with.
/// Needs both `TACIE_VOICE_MODEL` and `TACIE_DIAR_MODEL`.
#[test]
fn diar_only_gives_the_same_grouping_vector() {
    let (Some(voice), Some(diar)) = (
        std::env::var_os("TACIE_VOICE_MODEL"),
        std::env::var_os("TACIE_DIAR_MODEL"),
    ) else {
        eprintln!("skip: TACIE_VOICE_MODEL or TACIE_DIAR_MODEL is not set");
        return;
    };
    let mut both = VoiceEmbedder::load(voice.as_ref()).unwrap();
    both.add_diar(diar.as_ref()).unwrap();
    let only = VoiceEmbedder::diar_only(diar.as_ref()).unwrap();
    assert_eq!(only.diar_dim(), both.diar_dim());
    for name in ["a_1.wav", "a_2.wav", "b_1.wav", "b_2.wav"] {
        let wav = std::fs::read(fixture(name)).unwrap();
        let a = both.embed(&wav).unwrap().unwrap();
        let b = only.embed(&wav).unwrap().unwrap();
        assert_eq!(a.vector, b.vector, "{name}");
        assert_eq!(a.seconds, b.seconds, "{name}");
        assert_eq!(b.ident, None, "{name}");
    }
}
