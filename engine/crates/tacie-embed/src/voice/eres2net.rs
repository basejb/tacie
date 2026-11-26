//! Makes one utterance's speaker embedding with 3D-Speaker ERes2Net base (Apache-2.0). Input `x` [1, T, 80], output
//! `embedding` [1, 512]. If the diarization model (CAM++ fine-tune, same input, output [1, 192]) is present, it also
//! makes that embedding from the same fbank (docs/04 4절 "화자 나누기와 참석자 확인"): grouping speakers within a
//! meeting uses that embedding, and matching against remembered attendees uses ERes2Net.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use anyhow::{anyhow, ensure};
use ort::environment::Environment;
use ort::ep;
use ort::session::Session;
use ort::session::builder::AutoDevicePolicy;
use ort::value::Tensor;

use super::fbank::{MEL_BINS, SAMPLE_RATE, fbank, subtract_mean};
use super::wav::pcm16_mono;
use crate::onnx::environment;

pub const VOICE_DIM: usize = 512;

/// 정규화한 화자 임베딩과 발화 길이예요. `vector`는 회의 안에서 화자를 묶는 임베딩이고, `ident`는 기억한 참석자와 견주는
/// ERes2Net 임베딩이에요. 화자 나누기 모델이 없으면 둘이 같아서 `ident`는 None이에요(`ident()`가 `vector`를 줘요).
#[derive(Debug, Clone, PartialEq)]
pub struct Voiceprint {
    pub vector: Vec<f32>,
    pub seconds: f32,
    pub ident: Option<Vec<f32>>,
}

impl Voiceprint {
    /// 기억한 참석자와 견주는 임베딩이에요(길이 `VOICE_DIM`).
    pub fn ident(&self) -> &[f32] {
        self.ident.as_deref().unwrap_or(&self.vector)
    }
}

pub trait VoiceEmbed: Send + Sync {
    /// 발화 WAV(16 kHz mono 16-bit)의 임베딩이에요. 프레임이 하나도 없으면(0.005초 미만) None이에요.
    fn embed(&self, wav: &[u8]) -> anyhow::Result<Option<Voiceprint>>;
    /// 화자 나누기 모델의 임베딩 길이예요. None이면 화자 나누기도 ERes2Net으로 해요(`vector` == `ident`).
    fn diar_dim(&self) -> Option<usize> {
        None
    }
}

pub struct VoiceEmbedder {
    // ort의 `run`이 `&mut self`라서 잠가요. bge-m3의 자물쇠와 따로라서 동기화 임베딩이 오래 걸려도 회의는 기다리지 않아요.
    /// ERes2Net이에요. iPhone처럼 화자 나누기 모델 하나만 돌리면 None이에요(`diar_only`).
    session: Option<Mutex<Session>>,
    /// 화자 나누기 모델과 그 임베딩 길이예요.
    diar: Option<(Mutex<Session>, usize)>,
}

impl VoiceEmbedder {
    pub fn load(model: &Path) -> anyhow::Result<Self> {
        let session = open(&environment()?, model)?;
        Ok(Self {
            session: Some(Mutex::new(session)),
            diar: None,
        })
    }

    /// 화자 나누기 모델(CAM++ 파인튜닝) 하나만 돌려요(iPhone, docs/27 7-11절). `ident`가 없어서 기억한 참석자와 견줄 수
    /// 없어요: 이 임베더를 쓰는 엔진에는 기억한 사람을 넘기지 않아요.
    pub fn diar_only(model: &Path) -> anyhow::Result<Self> {
        let mut embedder = Self {
            session: None,
            diar: None,
        };
        embedder.add_diar(model)?;
        Ok(embedder)
    }

    /// ERes2Net에 화자 나누기 모델을 더해요. 길이는 1초짜리 빈 입력을 한 번 돌려 알아내요.
    pub fn add_diar(&mut self, model: &Path) -> anyhow::Result<()> {
        let mut session = open(&environment()?, model)?;
        let dim = run(&mut session, vec![0.0; 100 * MEL_BINS], 100)?.len();
        self.diar = Some((Mutex::new(session), dim));
        Ok(())
    }
}

/// fbank 프레임 `t`개(평균을 뺀 것)의 정규화한 임베딩이에요.
fn run(session: &mut Session, flat: Vec<f32>, t: usize) -> anyhow::Result<Vec<f32>> {
    let x = Tensor::from_array(([1, t, MEL_BINS], flat))?;
    let outputs = session.run(ort::inputs!["x" => x])?;
    let (shape, data) = outputs
        .get("embedding")
        .ok_or_else(|| anyhow!("model has no embedding output"))?
        .try_extract_tensor::<f32>()?;
    ensure!(
        shape.len() == 2 && shape[1] as usize == data.len() && !data.is_empty(),
        "unexpected embedding shape {shape:?}"
    );
    let norm = data.iter().map(|v| v * v).sum::<f32>().sqrt();
    ensure!(norm > 0.0, "zero embedding");
    Ok(data.iter().map(|v| v / norm).collect())
}

/// bge-m3의 CPU 설정과 같아요(onnx.rs `open`): 정책을 CPU로 두지 않으면 ORT가 CoreML을 스스로 붙여요.
/// 발화 하나는 10–80 ms라 스레드 두 개면 충분해요.
/// `TACIE_VOICE_THREADS`로 스레드 수를 바꿔 재요(docs/27 7-11절). CoreML은 이 모델의 길이가 정해지지 않은 입력을 받지 못해
/// CPU보다 15배 느렸어요(docs/01).
fn open(env: &Environment, model: &Path) -> ort::Result<Session> {
    let threads = std::env::var("TACIE_VOICE_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    Session::builder(env)?
        .with_auto_device(AutoDevicePolicy::PreferCPU)?
        .with_execution_providers([ep::CPU::default().with_arena_allocator(false).build()])?
        .with_memory_pattern(false)?
        .with_intra_threads(threads)?
        .commit_from_file(model)
}

impl VoiceEmbed for VoiceEmbedder {
    fn embed(&self, wav: &[u8]) -> anyhow::Result<Option<Voiceprint>> {
        let samples = pcm16_mono(wav)?;
        let mut frames = fbank(&samples);
        if frames.is_empty() {
            return Ok(None);
        }
        subtract_mean(&mut frames);
        let t = frames.len();
        let flat = frames.concat();
        let seconds = samples.len() as f32 / SAMPLE_RATE as f32;
        // 세션은 호출 사이에 들고 있는 값이 없어서, 다른 호출이 잡은 채 패닉했어도 계속 써요(onnx.rs와 같아요).
        let Some(session) = &self.session else {
            let (diar, _) = self.diar.as_ref().ok_or_else(|| anyhow!("no voice model"))?;
            let vector = {
                let mut session = diar.lock().unwrap_or_else(PoisonError::into_inner);
                run(&mut session, flat, t)?
            };
            return Ok(Some(Voiceprint {
                vector,
                seconds,
                ident: None,
            }));
        };
        let ident = {
            let mut session = session.lock().unwrap_or_else(PoisonError::into_inner);
            run(&mut session, flat.clone(), t)?
        };
        ensure!(
            ident.len() == VOICE_DIM,
            "unexpected embedding length {}",
            ident.len()
        );
        let Some((diar, _)) = &self.diar else {
            return Ok(Some(Voiceprint {
                vector: ident,
                seconds,
                ident: None,
            }));
        };
        let vector = {
            let mut session = diar.lock().unwrap_or_else(PoisonError::into_inner);
            run(&mut session, flat, t)?
        };
        Ok(Some(Voiceprint {
            vector,
            seconds,
            ident: Some(ident),
        }))
    }

    fn diar_dim(&self) -> Option<usize> {
        self.diar.as_ref().map(|(_, dim)| *dim)
    }
}
