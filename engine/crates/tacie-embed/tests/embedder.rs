//! Tests for the bge-m3 ONNX embedder. Skipped when there is no model folder (`TACIE_MODEL_DIR`). Loads on the CPU
//! by default.

use std::path::PathBuf;
use std::sync::OnceLock;

use tacie_embed::{DIM, Device, Embed, OnnxEmbedder, PASSAGE_TOKENS};

fn model_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("TACIE_MODEL_DIR").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("skip: TACIE_MODEL_DIR unset");
    }
    dir
}

/// 모델이 2.27GB라 테스트마다 올리지 않고 한 번 올려서 나눠 써요.
fn embedder() -> Option<&'static OnnxEmbedder> {
    static EMBEDDER: OnceLock<OnnxEmbedder> = OnceLock::new();
    let dir = model_dir()?;
    Some(EMBEDDER.get_or_init(|| OnnxEmbedder::load(&dir, Device::Cpu).expect("load bge-m3")))
}

fn norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() / (norm(a) * norm(b))
}

#[test]
fn embeds_unit_vectors_of_dim_1024() {
    let Some(e) = embedder() else { return };
    let vs = e
        .embed(&["warmup", "회의 중에 답하는 에이전트"], None)
        .unwrap();
    assert_eq!(vs.len(), 2);
    for v in &vs {
        assert_eq!(v.len(), DIM);
        assert!((norm(v) - 1.0).abs() < 1e-3, "norm {}", norm(v));
    }
}

#[test]
fn same_text_same_vector() {
    let Some(e) = embedder() else { return };
    let text = "포트는 51234예요";
    let alone = e.embed(&[text], None).unwrap();
    // 두 번째는 더 긴 글과 한 배치로 넣어서 패딩이 붙어도 같은 벡터인지 봐요.
    let padded = e
        .embed(
            &[
                text,
                "엔진은 앱이 띄우고, 첫 줄에 포트 번호를 적은 뒤 색인을 올려요.",
            ],
            None,
        )
        .unwrap();
    let c = cosine(&alone[0], &padded[0]);
    assert!(c > 0.9999, "cosine {c}");
}

#[test]
fn passage_truncates_at_512_tokens_query_does_not() {
    let Some(e) = embedder() else { return };
    let dir = model_dir().unwrap();

    // 앞 512 토큰 너머에 다른 이야기가 붙어 있어서, 잘리면 벡터가 달라져야 해요.
    // 너무 길면(2000 토큰 넘게) ORT가 수십 GB를 써서 700 토큰 남짓으로 맞췄어요.
    let long = format!(
        "{}{}",
        "회의록 정리 방법을 이야기해요. ".repeat(70),
        "배포는 금요일 오후에 하고 포트는 51234를 써요. ".repeat(10)
    );
    let tok = tokenizers::Tokenizer::from_file(dir.join("tokenizer.json")).unwrap();
    let n = tok.encode(long.as_str(), true).unwrap().get_ids().len();
    assert!(n > PASSAGE_TOKENS, "long text is only {n} tokens");

    let cut = e.embed(&[long.as_str()], Some(PASSAGE_TOKENS)).unwrap();
    let full = e.embed(&[long.as_str()], None).unwrap();
    let c = cosine(&cut[0], &full[0]);
    assert!(c < 0.999, "long text cosine {c} ({n} tokens)");

    let short = "포트는 51234예요";
    let cut = e.embed(&[short], Some(PASSAGE_TOKENS)).unwrap();
    let full = e.embed(&[short], None).unwrap();
    let c = cosine(&cut[0], &full[0]);
    assert!(c > 0.9999, "short text cosine {c}");
}

#[test]
fn load_reports_a_device() {
    let Some(e) = embedder() else { return };
    let d = e.device();
    eprintln!("device: {d:?}");
    assert!(matches!(d, Device::CoreMl | Device::Cpu));
}

/// CoreML 로드는 느리고 기기마다 달라서 `TACIE_TEST_COREML=1`일 때만 돌려요.
#[test]
fn coreml_load_falls_back_or_runs() {
    if std::env::var("TACIE_TEST_COREML").as_deref() != Ok("1") {
        eprintln!("skip: TACIE_TEST_COREML unset");
        return;
    }
    let Some(dir) = model_dir() else { return };
    let e = OnnxEmbedder::load(&dir, Device::CoreMl).expect("load bge-m3 on CoreML");
    let vs = e.embed(&["warmup"], None).unwrap();
    assert_eq!(vs[0].len(), DIM);
    assert!((norm(&vs[0]) - 1.0).abs() < 1e-3, "norm {}", norm(&vs[0]));
}

/// GPU(MLX) 벡터는 CPU와 같아요: 임베딩 캐시를 같이 써요(docs/01 "동기화 임베딩을 GPU로"). 글 속의 `<pad>`, 512토큰에서
/// 잘리는 긴 글, 길이가 다른 글을 한 배치에, 질문 길이(None)도 봐요.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn gpu_vectors_match_cpu() {
    let Some(cpu) = embedder() else { return };
    let gpu =
        tacie_embed::GpuEmbedder::load(&model_dir().unwrap()).expect("load bge-m3 on the gpu");
    let long = "fn padded_len(longest: usize) -> usize { longest } // 문단은 512토큰에서 잘려요\n"
        .repeat(80);
    let texts = [
        "회의 중에 답하는 에이전트",
        "토크나이저 특수 토큰 <pad> 이 글 속에 있어요",
        long.as_str(),
        "Notion pages and GitHub issues share one index.",
    ];
    for max_tokens in [Some(PASSAGE_TOKENS), None] {
        let (a, b) = (
            cpu.embed(&texts, max_tokens).unwrap(),
            gpu.embed(&texts, max_tokens).unwrap(),
        );
        assert_eq!(b.len(), texts.len());
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            assert_eq!(y.len(), DIM);
            let c = cosine(x, y);
            assert!(c > 0.99999, "text {i} ({max_tokens:?}): cosine {c}");
        }
    }
    assert!(gpu.embed(&[], Some(PASSAGE_TOKENS)).unwrap().is_empty());
}
