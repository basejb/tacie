//! A transcription test with a real model. Runs only when both `TACIE_STT_MODEL_FILE` (ggml model) and
//! `TACIE_STT_TEST_WAV` (16kHz mono s16 Korean utterance) are set; otherwise skipped.

use std::path::PathBuf;

use tacie_engine::local_stt::{Device, LocalWhisper};

fn silence_wav(seconds: u32) -> Vec<u8> {
    let data = vec![0u8; (16_000 * 2 * seconds) as usize];
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    wav
}

#[test]
fn transcribes_korean_question() {
    let (Some(model), Some(wav_path)) = (
        std::env::var_os("TACIE_STT_MODEL_FILE").map(PathBuf::from),
        std::env::var_os("TACIE_STT_TEST_WAV").map(PathBuf::from),
    ) else {
        eprintln!("건너뛰어요: TACIE_STT_MODEL_FILE, TACIE_STT_TEST_WAV가 필요해요");
        return;
    };
    let whisper = LocalWhisper::load(&model, Device::Metal).expect("모델 로드");

    let wav = std::fs::read(&wav_path).expect("WAV 읽기");
    let t = whisper.transcribe(&wav, "").expect("받아쓰기");
    eprintln!("text={:?} words={}", t.text, t.words.len());
    assert!(t.text.contains('죠'), "{}", t.text);
    assert!(!t.words.is_empty());
    // 한글이 토큰에 걸쳐 깨지지 않아야 하고, 단어를 이으면 글과 같아야 해요(공백만 달라요).
    assert!(
        t.words.iter().all(|w| !w.text.contains('\u{FFFD}')),
        "{:?}",
        t.words
    );
    let joined: String = t.words.iter().map(|w| w.text.as_str()).collect();
    let flat: String = t.text.split_whitespace().collect();
    assert_eq!(joined, flat);

    let quiet = whisper
        .transcribe(&silence_wav(2), "")
        .expect("무음 받아쓰기");
    assert_eq!(quiet.text, "", "무음에서 글이 나왔어요");
    assert!(quiet.words.is_empty());
}
