//! Runs bge-m3 on the Apple GPU (MLX). Only sync's passage embeddings use it (docs/01 "동기화 임베딩을 GPU로").
//!
//! Weights are read straight from the official `model.onnx_data` (`layout.rs`; no extra files are made). Vectors match
//! the CPU (ONNX Runtime) ones (cosine at least 0.999999), so the embedding cache is used as is. 546 chunks go from
//! CPU 80.6 s → 34 s, with the CPU barely used meanwhile (M4 Pro). MLX arrays and compiled functions are used only on
//! the thread that made them, so one dedicated thread holds the model and `embed` sends work to that thread and waits.

mod layout;

use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, PoisonError};
use std::thread::JoinHandle;

use anyhow::{Context, anyhow, ensure};
use mlx_rs::error::Exception;
use mlx_rs::fast::{layer_norm, scaled_dot_product_attention};
use mlx_rs::ops::{addmm, cumsum, erf};
use mlx_rs::{Array, Dtype};
use serde_json::Value;

use crate::tokenize::Tok;
use crate::{DIM, Embed, WEIGHTS};

/// 다 쓴 버퍼를 MLX가 다시 쓰려고 들고 있는 상한이에요. 배치마다 모양이 달라서 묶지 않으면 19 GB까지 커졌어요
/// (512 MB면 최대 footprint 3.6 GB, 속도는 같아요).
const CACHE_BYTES: usize = 512 << 20;

/// 층 하나의 가중치예요(`layer`가 이 순서로 받아요).
const LAYER_WEIGHTS: [&str; 16] = [
    "attention.self.query.weight",
    "attention.self.query.bias",
    "attention.self.key.weight",
    "attention.self.key.bias",
    "attention.self.value.weight",
    "attention.self.value.bias",
    "attention.output.dense.weight",
    "attention.output.dense.bias",
    "attention.output.LayerNorm.weight",
    "attention.output.LayerNorm.bias",
    "intermediate.dense.weight",
    "intermediate.dense.bias",
    "output.dense.weight",
    "output.dense.bias",
    "output.LayerNorm.weight",
    "output.LayerNorm.bias",
];
const HEADS: i32 = 16;
const LAYERS: usize = 24;
const EPS: f32 = 1e-5;

/// 컴파일한 층 함수예요(`layer`).
type Layer = Box<dyn for<'a> FnMut(&'a [Array]) -> Result<Vec<Array>, Exception>>;
type Reply = Sender<anyhow::Result<Vec<Vec<f32>>>>;
type Job = (Vec<String>, Option<usize>, Reply);

pub struct GpuEmbedder {
    jobs: Mutex<Option<Sender<Job>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl GpuEmbedder {
    /// 모델 폴더의 공식 bge-m3 ONNX 가중치를 GPU로 올리고 한 번 돌려 봐요. Metal이나 `mlx.metallib`이 없거나
    /// 파일이 다르면 오류예요(엔진은 CPU로 해요).
    pub fn load(model_dir: &Path) -> anyhow::Result<Self> {
        let dir = model_dir.to_path_buf();
        let (jobs, rx) = channel::<Job>();
        let (ready, loaded) = channel::<anyhow::Result<()>>();
        let worker = std::thread::Builder::new()
            .name("embed-gpu".into())
            .spawn(move || {
                let model =
                    match bundled_kernels()
                        .and_then(|()| Model::load(&dir))
                        .and_then(|mut m| {
                            m.embed(&["warmup"], None)?;
                            Ok(m)
                        }) {
                        Ok(m) => {
                            let _ = ready.send(Ok(()));
                            m
                        }
                        Err(e) => {
                            let _ = ready.send(Err(e));
                            return;
                        }
                    };
                let mut model = model;
                for (texts, max_tokens, reply) in rx {
                    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
                    let _ = reply.send(model.embed(&refs, max_tokens));
                }
                drop(model);
                let _ = mlx_rs::memory::clear_cache();
            })?;
        let loaded = loaded
            .recv()
            .map_err(|_| anyhow!("gpu embedder thread stopped while loading"))
            .and_then(|r| r);
        if let Err(e) = loaded {
            let _ = worker.join();
            return Err(e);
        }
        Ok(Self {
            jobs: Mutex::new(Some(jobs)),
            worker: Mutex::new(Some(worker)),
        })
    }
}

impl Embed for GpuEmbedder {
    fn embed(&self, texts: &[&str], max_tokens: Option<usize>) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let (reply, answer) = channel();
        let texts = texts.iter().map(|t| t.to_string()).collect();
        self.jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .ok_or_else(|| anyhow!("gpu embedder closed"))?
            .send((texts, max_tokens, reply))
            .map_err(|_| anyhow!("gpu embedder thread stopped"))?;
        answer
            .recv()
            .map_err(|_| anyhow!("gpu embedder thread stopped"))?
    }
}

impl Drop for GpuEmbedder {
    /// 일 통로를 닫으면 스레드가 모델을 내리고 GPU 메모리를 돌려준 뒤 끝나요.
    fn drop(&mut self) {
        self.jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(w) = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = w.join();
        }
    }
}

/// 앱 안에서는 MLX의 GPU 커널(`mlx.metallib`)이 엔진 옆이 아니라 `Contents/Resources`에 있어요(`Contents/MacOS`에
/// 두면 codesign이 앱을 받지 않아요). 거기 있으면 MLX에 알려요. 없으면 MLX가 스스로 찾아요(`target/`에서 띄우면
/// 빌드 때 정한 `engine/target/mlx`예요).
fn bundled_kernels() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    if let Some(path) = exe.parent().map(|d| d.join("../Resources/mlx.metallib"))
        && path.is_file()
    {
        mlx_rs::metal::set_metallib_path(path.to_string_lossy())?;
    }
    Ok(())
}

struct Model {
    tok: Tok,
    w: HashMap<&'static str, Array>,
    pad: i64,
    layer: Layer,
}

impl Model {
    fn load(dir: &Path) -> anyhow::Result<Self> {
        let cfg: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("config.json"))?)?;
        let n = |k: &str| cfg[k].as_f64();
        ensure!(
            n("hidden_size") == Some(DIM as f64)
                && n("num_attention_heads") == Some(HEADS as f64)
                && n("num_hidden_layers") == Some(LAYERS as f64)
                && n("layer_norm_eps").is_some_and(|e| (e - f64::from(EPS)).abs() < 1e-9),
            "not the bge-m3 config the gpu embedder knows"
        );
        let pad = cfg["pad_token_id"]
            .as_i64()
            .context("config.json: pad_token_id")?;
        let file = File::open(dir.join(WEIGHTS)).with_context(|| format!("open {WEIGHTS}"))?;
        ensure!(
            file.metadata()?.len() == layout::DATA_BYTES,
            "{WEIGHTS} is not the official bge-m3 file"
        );
        mlx_rs::memory::set_cache_limit(CACHE_BYTES)?;
        let mut w = HashMap::new();
        for (name, offset, shape) in layout::TENSORS {
            let len = shape.iter().product::<i32>() as usize;
            let mut values = vec![0f32; len];
            // SAFETY: `values`는 f32 len개로 이미 채워진 버퍼라 바이트로 보면 len * 4바이트예요. f32는 아무 비트나
            // 담을 수 있고, 파일은 리틀 엔디언 f32이며 Apple Silicon도 리틀 엔디언이에요.
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(values.as_mut_ptr().cast::<u8>(), len * 4)
            };
            file.read_exact_at(bytes, offset)
                .with_context(|| format!("read {name}"))?;
            w.insert(name, Array::from_slice(&values, shape));
        }
        mlx_rs::transforms::eval(w.values())?;
        Ok(Self {
            tok: Tok::load(&dir.join("tokenizer.json"))?,
            w,
            pad,
            // 층 하나를 묶어 컴파일하면 원소별 연산이 합쳐져서 9–12% 빨라요(docs/01)
            layer: Box::new(mlx_rs::transforms::compile::compile(layer, None)),
        })
    }

    fn w(&self, name: &str) -> anyhow::Result<&Array> {
        self.w
            .get(name)
            .ok_or_else(|| anyhow!("missing tensor {name}"))
    }

    fn embed(
        &mut self,
        texts: &[&str],
        max_tokens: Option<usize>,
    ) -> anyhow::Result<Vec<Vec<f32>>> {
        let (ids, mask) = self.tok.encode(texts, max_tokens)?;
        let (b, s) = (ids.len() as i32, ids[0].len() as i32);
        let ids: Vec<i32> = ids.concat().into_iter().map(|x| x as i32).collect();
        let bias: Vec<f32> = mask
            .concat()
            .into_iter()
            .map(|m| if m == 1 { 0.0 } else { -1e9 })
            .collect();
        let ids = Array::from_slice(&ids, &[b, s]);
        let bias = Array::from_slice(&bias, &[b, 1, 1, s]);
        // XLM-R은 pad가 아닌 id를 padding_idx + 1부터 세어 위치로 써요. 글에 `<pad>`가 들어 있으면 그 토큰도
        // pad로 세요(attention mask로 세면 그런 조각 하나가 0.9966으로 어긋났어요, docs/01)
        let real = ids
            .ne(Array::from_int(self.pad as i32))?
            .as_dtype(Dtype::Int32)?;
        let pos = cumsum(&real, 1, None, None)?
            .multiply(&real)?
            .add(Array::from_int(self.pad as i32))?;
        let x = self
            .w("embeddings.word_embeddings.weight")?
            .take_axis(&ids, 0)?
            .add(
                self.w("embeddings.position_embeddings.weight")?
                    .take_axis(&pos, 0)?,
            )?
            .add(self.w("embeddings.token_type_embeddings.weight")?)?;
        let mut x = layer_norm(
            &x,
            self.w("embeddings.LayerNorm.weight")?,
            self.w("embeddings.LayerNorm.bias")?,
            EPS,
        )?;
        for i in 0..LAYERS {
            let mut args = vec![x, bias.clone()];
            for n in LAYER_WEIGHTS {
                args.push(self.w(&format!("encoder.layer.{i}.{n}"))?.clone());
            }
            x = (self.layer)(&args)?.remove(0);
            // 층마다 계산해요: 24층을 한 그래프로 두면 층마다의 임시 값이 다 살아 있어서 512토큰 조각 하나에도
            // 1.1 GB가 더 들었어요(속도는 같아요)
            x.eval()?;
        }
        let cls = x
            .take_axis(Array::from_slice(&[0i32], &[1]), 1)?
            .reshape(&[b, DIM as i32])?;
        cls.eval()?;
        Ok(cls
            .as_slice::<f32>()
            .chunks(DIM)
            .map(|v| crate::onnx::normalize(v.to_vec()))
            .collect())
    }
}

/// 인코더 층 하나예요: `[x, bias, LAYER_WEIGHTS...]` → `[x]`. 컴파일하려고 바깥 값을 쓰지 않아요.
fn layer(a: &[Array]) -> Result<Vec<Array>, Exception> {
    let (x, bias, w) = (&a[0], &a[1], &a[2..]);
    let (b, s, hidden) = (x.shape()[0], x.shape()[1], x.shape()[2]);
    let hd = hidden / HEADS;
    let heads = |t: Array| t.reshape(&[b, s, HEADS, hd])?.transpose_axes(&[0, 2, 1, 3]);
    let q = heads(addmm(&w[1], x, &w[0], None, None)?)?;
    let k = heads(addmm(&w[3], x, &w[2], None, None)?)?;
    let v = heads(addmm(&w[5], x, &w[4], None, None)?)?;
    let at = scaled_dot_product_attention(&q, &k, &v, (hd as f32).powf(-0.5), bias, None)?
        .transpose_axes(&[0, 2, 1, 3])?
        .reshape(&[b, s, hidden])?;
    let x = layer_norm(
        addmm(&w[7], &at, &w[6], None, None)?.add(x)?,
        &w[8],
        &w[9],
        EPS,
    )?;
    let h = addmm(&w[11], &x, &w[10], None, None)?;
    // 정확한 GELU예요(erf)
    let h = h
        .multiply(
            erf(h.multiply(Array::from_f32(std::f32::consts::FRAC_1_SQRT_2))?)?
                .add(Array::from_f32(1.0))?,
        )?
        .multiply(Array::from_f32(0.5))?;
    Ok(vec![layer_norm(
        addmm(&w[13], &h, &w[12], None, None)?.add(&x)?,
        &w[14],
        &w[15],
        EPS,
    )?])
}
