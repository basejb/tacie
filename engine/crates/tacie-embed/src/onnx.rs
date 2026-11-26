//! The bge-m3 ONNX session. Tries CoreML first and falls back to the CPU.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use anyhow::{anyhow, ensure};
use ndarray::{Ix3, s};
use ort::environment::Environment;
use ort::ep;
use ort::session::Session;
use ort::session::builder::AutoDevicePolicy;
use ort::value::Tensor;

use crate::tokenize::Tok;
use crate::{DIM, Embed};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    CoreMl,
    Cpu,
}

pub struct OnnxEmbedder {
    tok: Tok,
    // ort의 `run`이 `&mut self`라서 잠가서 써요.
    session: Mutex<Session>,
    device: Device,
}

impl OnnxEmbedder {
    /// model_dir에 model.onnx, tokenizer.json. prefer=CoreMl이면 먼저 시도하고 실패하면 Cpu로 내려가며 stderr에 한 줄 적는다.
    pub fn load(model_dir: &Path, prefer: Device) -> anyhow::Result<Self> {
        let tok = Tok::load(&model_dir.join("tokenizer.json"))?;
        let model = model_dir.join("model.onnx");
        let env = environment()?;
        if prefer == Device::CoreMl {
            // 세션은 만들어져도 첫 실행에서 실패할 수 있어서, 한 번 돌려 본 뒤에 CoreML로 정해요.
            let tried = open(&env, &model, Device::CoreMl)
                .map_err(anyhow::Error::from)
                .and_then(|mut session| {
                    forward(&tok, &mut session, &["warmup"], None)?;
                    Ok(session)
                });
            match tried {
                Ok(session) => return Ok(Self::new(tok, session, Device::CoreMl)),
                Err(e) => eprintln!(
                    "embed: coreml unavailable, using cpu ({})",
                    format!("{e:#}").replace('\n', " ")
                ),
            }
        }
        let session = open(&env, &model, Device::Cpu)?;
        Ok(Self::new(tok, session, Device::Cpu))
    }

    pub fn device(&self) -> Device {
        self.device
    }

    fn new(tok: Tok, session: Session, device: Device) -> Self {
        Self {
            tok,
            session: Mutex::new(session),
            device,
        }
    }
}

impl Embed for OnnxEmbedder {
    fn embed(&self, texts: &[&str], max_tokens: Option<usize>) -> anyhow::Result<Vec<Vec<f32>>> {
        // 다른 호출이 세션을 잡은 채 패닉했어도 계속 써요: 세션은 호출 사이에 들고 있는 값이 없어요. 독을 실패로
        // 보면 갱신 중의 패닉 하나가 그 뒤의 모든 /ask를 막아요
        let mut session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        forward(&self.tok, &mut session, texts, max_tokens)
    }
}

/// ort는 한 프로세스에 환경을 하나만 만들 수 있어요. 처음 만든 걸 계속 나눠 써요.
pub(crate) fn environment() -> anyhow::Result<Environment> {
    static ENV: Mutex<Option<Environment>> = Mutex::new(None);
    let mut env = ENV.lock().unwrap_or_else(PoisonError::into_inner);
    if env.is_none() {
        *env = Some(ort::init().with_name("tacie-embed").build()?);
    }
    Ok(env.clone().expect("environment was just set"))
}

fn open(env: &Environment, model: &Path, device: Device) -> ort::Result<Session> {
    let mut builder = Session::builder(env)?;
    match device {
        Device::CoreMl => {
            // 기본 형식(NeuralNetwork)은 이 모델의 노드를 거의 받지 못해서 CPU와 똑같이 돌아요.
            // MLProgram이어야 CoreML이 실제로 계산해요.
            let coreml =
                ep::CoreML::default().with_model_format(ep::coreml::ModelFormat::MLProgram);
            builder = builder.with_execution_providers([coreml.build().error_on_failure()])?;
        }
        Device::Cpu => {
            // ort는 세션에 "전력 효율 우선"(NPU 먼저) 정책을 기본으로 걸어서, 그대로 두면 ORT가 CoreML을
            // 스스로 붙여요. 그러면 문단 546개에 메모리가 12–19 GB까지 커지고 스왑 때문에 6905 s가 걸렸어요.
            // CPU만 쓰게 하면 로드 0.6–2.0 s, 최대 RSS 1.9 GB, 546개 90 s예요(docs/01).
            // CPU EP를 직접 붙여도 이 정책은 그대로라서 정책을 꼭 바꿔야 해요.
            builder = builder
                .with_auto_device(AutoDevicePolicy::PreferCPU)?
                // 아레나는 한 번 커지면 줄지 않고, 메모리 패턴은 입력 모양마다 따로 잡혀요.
                // 둘 다 꺼도 속도는 같았어요(90 s, 켜면 92 s).
                .with_execution_providers([ep::CPU::default().with_arena_allocator(false).build()])?
                .with_memory_pattern(false)?;
            if let Some(n) = performance_cores() {
                // 효율 코어까지 쓰면 7% 느렸어요(98 s, 성능 코어만 92 s).
                builder = builder.with_intra_threads(n)?;
            }
        }
    }
    builder.commit_from_file(model)
}

/// Apple Silicon의 성능 코어 수예요. 알 수 없으면 None이고, 그때는 ORT가 정해요.
#[cfg(target_os = "macos")]
fn performance_cores() -> Option<usize> {
    use std::ffi::{c_char, c_int, c_void};
    unsafe extern "C" {
        fn sysctlbyname(
            name: *const c_char,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *mut c_void,
            newlen: usize,
        ) -> c_int;
    }
    let mut n: c_int = 0;
    let mut len = std::mem::size_of::<c_int>();
    // SAFETY: 이름은 NUL로 끝나는 상수이고, oldp/oldlenp는 이 함수의 c_int 하나와 그 크기를 가리켜요.
    let rc = unsafe {
        sysctlbyname(
            c"hw.perflevel0.physicalcpu".as_ptr(),
            (&raw mut n).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && n > 0).then_some(n as usize)
}

#[cfg(not(target_os = "macos"))]
fn performance_cores() -> Option<usize> {
    None
}

/// 글을 넣어 마지막 hidden state의 CLS(토큰 0) 벡터를 L2 정규화해 돌려줘요.
fn forward(
    tok: &Tok,
    session: &mut Session,
    texts: &[&str],
    max_tokens: Option<usize>,
) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let (ids, mask) = tok.encode(texts, max_tokens)?;
    let shape = [ids.len(), ids[0].len()];
    let ids = Tensor::from_array((shape, ids.concat()))?;
    let mask = Tensor::from_array((shape, mask.concat()))?;
    // 첫 출력이 마지막 hidden state [batch, seq, DIM]예요(이 모델에선 이름이 token_embeddings).
    let output = session
        .outputs()
        .first()
        .ok_or_else(|| anyhow!("model has no outputs"))?
        .name()
        .to_owned();
    let outputs = session.run(ort::inputs!["input_ids" => ids, "attention_mask" => mask])?;
    let hidden = outputs[output.as_str()]
        .try_extract_array::<f32>()?
        .into_dimensionality::<Ix3>()?;
    ensure!(
        hidden.dim().0 == texts.len() && hidden.dim().2 == DIM,
        "unexpected {output} shape {:?}",
        hidden.shape()
    );
    Ok(hidden
        .slice(s![.., 0, ..])
        .outer_iter()
        .map(|cls| normalize(cls.to_vec()))
        .collect())
}

/// torch의 `F.normalize`처럼 길이가 0에 가까우면 1e-12로 나눠요.
pub(crate) fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    v.iter_mut().for_each(|x| *x /= n);
    v
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// 세션을 잡은 채 패닉한 호출이 있어도 다음 임베딩은 돼요. 모델 폴더(`TACIE_MODEL_DIR`)가 없으면 건너뛰어요.
    #[test]
    fn a_panic_holding_the_session_does_not_stop_embedding() {
        let Some(dir) = std::env::var_os("TACIE_MODEL_DIR").map(PathBuf::from) else {
            eprintln!("skip: TACIE_MODEL_DIR unset");
            return;
        };
        let e = OnnxEmbedder::load(&dir, Device::Cpu).unwrap();
        let poisoned = std::thread::scope(|s| {
            s.spawn(|| {
                let _held = e.session.lock().unwrap();
                panic!("poison the session");
            })
            .join()
        });
        assert!(poisoned.is_err() && e.session.is_poisoned());
        let v = e.embed(&["회의 중에 답하는 에이전트"], Some(512)).unwrap();
        assert_eq!(v[0].len(), DIM);
    }
}
