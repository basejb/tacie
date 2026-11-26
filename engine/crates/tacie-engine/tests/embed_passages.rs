//! Refresh embedding (`Engine::embed_passages`): takes the same lock as `/ask` 2 items at a time, reports progress
//! after releasing it, and rests 0.02 s. A batch of 8 on CPU takes about 1.3 s and a question waits that long, so
//! it was cut to 2.
//! The lock is not fair, so without the rest the next batch grabs it again right away and a question waits for the
//! whole refresh (233 s on 2026-10-23).

mod common {
    pub mod fakes;
    pub mod fixture;
}

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::fakes::{FakeAnswerer, FakeEmbed, FakeReranker, services};
use common::fixture::{engine_with, index};
use tacie_embed::{Device, Embed, OnnxEmbedder, PASSAGE_TOKENS};
use tacie_engine::pipeline::{EMBED_BATCH, EMBED_PAUSE, Engine};

fn engine(embed: FakeEmbed) -> (Engine, Arc<FakeEmbed>) {
    let (mut engine, _) = engine_with(
        index(),
        services(
            Arc::new(FakeReranker::all(0.9)),
            Arc::new(FakeAnswerer::ok("")),
        ),
    );
    let embed = Arc::new(embed);
    engine.embedder = embed.clone();
    (engine, embed)
}

fn texts(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("조각 {i}")).collect()
}

#[test]
fn passages_embed_in_batches_of_two_with_progress() {
    assert_eq!(EMBED_BATCH, 2);
    assert_eq!(EMBED_PAUSE, Duration::from_millis(20));
    let (engine, embed) = engine(FakeEmbed::default());
    let texts = texts(5);
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut seen = Vec::new();
    let start = Instant::now();
    let out = engine
        .embed_passages(&refs, &mut |done, total| seen.push((done, total)))
        .unwrap();
    assert_eq!(out.len(), 5);
    let sizes: Vec<usize> = embed.texts().iter().map(Vec::len).collect();
    assert_eq!(sizes, [2, 2, 1]);
    assert_eq!(embed.texts().concat(), texts); // 차례 그대로예요
    assert_eq!(embed.max_tokens(), [Some(512); 3]); // 문단 길이(PASSAGE_TOKENS)예요
    assert_eq!(seen, [(2, 5), (4, 5), (5, 5)]);
    // 배치 사이마다 쉬어요(그동안 자물쇠는 놓여 있어요). 마지막 배치 뒤에도 쉬어요. 이 시험이 쉬기를 직접 봐요:
    // macOS의 std Mutex는 기다리는 쪽에 넘겨줘서, 쉬지 않아도 아래 굶주림 시험이 여기서는 통과해요
    let ended = Instant::now();
    let spans = embed.spans();
    for pair in spans.windows(2) {
        let gap = pair[1].0 - pair[0].1;
        assert!(gap >= EMBED_PAUSE, "only {gap:?} between batches");
    }
    assert!(ended - spans[2].1 >= EMBED_PAUSE);
    assert!(start.elapsed() >= EMBED_PAUSE * 3);

    // 빈 목록은 부르지도 알리지도 않아요
    let (engine, embed) = self::engine(FakeEmbed::default());
    let mut seen = Vec::new();
    let out = engine
        .embed_passages(&[], &mut |done, total| seen.push((done, total)))
        .unwrap();
    assert!(out.is_empty() && seen.is_empty());
    assert_eq!(embed.calls(), 0);

    // 추론이 실패하면 그대로 올라오고 남은 배치는 하지 않아요
    let (engine, embed) = self::engine(FakeEmbed {
        fail: true,
        ..Default::default()
    });
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut seen = Vec::new();
    let err = engine
        .embed_passages(&refs, &mut |done, total| seen.push((done, total)))
        .unwrap_err();
    assert!(err.to_string().contains("onnx run failed"));
    assert_eq!(embed.calls(), 1);
    assert!(seen.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ask_is_not_starved_while_passages_embed() {
    // 배치마다 100 ms: 50배치면 5 s가 넘어요. 그동안 묻는 말이 갱신 전체를 기다리면 5 s가 넘게 걸려요.
    let (engine, embed) = engine(FakeEmbed {
        delay: Duration::from_millis(100),
        ..Default::default()
    });
    let engine = Arc::new(engine);
    let done = Arc::new(AtomicBool::new(false));
    let refresh = {
        let (engine, done) = (engine.clone(), done.clone());
        std::thread::spawn(move || {
            let texts = texts(50 * EMBED_BATCH);
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            let out = engine.embed_passages(&refs, &mut |_, _| {}).unwrap();
            done.store(true, Ordering::SeqCst);
            out.len()
        })
    };
    while embed.calls() == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    for i in 0..3 {
        let start = Instant::now();
        let out = engine.ask("메일 탭 401 어디부터 봐요?", &[], 0.8).await;
        let took = start.elapsed();
        assert!(out["error"].is_null(), "{out}");
        assert!(
            took < Duration::from_secs(3),
            "ask {i} waited {took:?} behind the refresh"
        );
        assert!(
            !done.load(Ordering::SeqCst),
            "the refresh ended before ask {i}"
        );
    }
    assert_eq!(refresh.join().unwrap(), 50 * EMBED_BATCH);
}

/// 배치를 줄여도 벡터는 같아요: 길이가 다른 문단 8개를 한 배치로 넣은 것과 갱신처럼 2개씩 넣은 것을 비교해요(패딩이
/// 달라져요). 모델 폴더(`TACIE_MODEL_DIR`)가 없으면 건너뛰어요.
#[test]
fn smaller_batches_give_the_same_vectors() {
    let Some(dir) = std::env::var_os("TACIE_MODEL_DIR").map(PathBuf::from) else {
        eprintln!("skip: TACIE_MODEL_DIR unset");
        return;
    };
    let onnx = Arc::new(OnnxEmbedder::load(&dir, Device::Cpu).expect("load bge-m3"));
    let texts: Vec<String> = [
        "포트는 51234예요",
        "엔진은 앱이 띄우고, 첫 줄에 포트 번호를 적은 뒤 색인을 올려요.",
        "fn embed_passages(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> { todo!() }",
        "갱신은 새 조각을 작은 배치로 임베딩해요. 배치마다 자물쇠를 놓고 잠깐 쉬어서, 그사이 들어온 질문이 먼저 \
         임베딩돼요. 그래서 갱신 중에도 카드가 1초 안에 떠요.",
        "end ",
        "# Tacie\n\n회의 중에 답하는 에이전트",
        "권한은 검색 전에 걸러요. 볼 수 없는 조각은 리랭커나 답 모델로도 보내지 않아요.",
        &"긴 문단이에요. ".repeat(200), // 512 토큰에서 잘려요
    ]
    .map(String::from)
    .into();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let whole = onnx.embed(&refs, Some(PASSAGE_TOKENS)).unwrap();

    let (mut engine, _) = engine_with(
        index(),
        services(
            Arc::new(FakeReranker::all(0.9)),
            Arc::new(FakeAnswerer::ok("")),
        ),
    );
    engine.embedder = onnx;
    let mut seen = Vec::new();
    let split = engine
        .embed_passages(&refs, &mut |done, _| seen.push(done))
        .unwrap();
    assert_eq!(seen, [2, 4, 6, 8]); // 네 배치예요

    assert_eq!(whole.len(), 8);
    assert_eq!(split.len(), 8);
    let max = whole
        .iter()
        .zip(&split)
        .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs()))
        .fold(0f32, f32::max);
    eprintln!("max abs difference {max:e}");
    assert!(max < 1e-6, "max abs difference {max}");
}
