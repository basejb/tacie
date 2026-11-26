//! Refresh GPU embedding (docs/01 "동기화 임베딩을 GPU로"): with a GPU embedder it embeds `GPU_BATCH` items at a time
//! without the lock, loads it on first use and unloads it with `release_gpu`. If it cannot load or fails midway, it
//! falls back to CPU (`EMBED_BATCH` items at a time).

mod common {
    pub mod fakes;
    pub mod fixture;
}

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::fakes::{FakeAnswerer, FakeEmbed, FakeReranker, services};
use common::fixture::{engine_with, index};
use tacie_embed::Embed;
use tacie_engine::pipeline::{EMBED_BATCH, Engine, GPU_BATCH, GpuLoad};

/// CPU 임베더와, 부를 때마다 `gpu`를 내주는 GPU 로더를 단 엔진이에요. 로더는 부른 수를 `loads`에 세요.
/// `gpu`가 None이면 로더가 실패해요.
fn engine(gpu: Option<Arc<FakeEmbed>>) -> (Engine, Arc<FakeEmbed>, Arc<AtomicUsize>) {
    let (engine, _) = engine_with(
        index(),
        services(
            Arc::new(FakeReranker::all(0.9)),
            Arc::new(FakeAnswerer::ok("")),
        ),
    );
    let cpu = Arc::new(FakeEmbed::default());
    let loads = Arc::new(AtomicUsize::new(0));
    let load: GpuLoad = {
        let loads = loads.clone();
        Arc::new(move || {
            loads.fetch_add(1, Ordering::SeqCst);
            match &gpu {
                Some(g) => Ok(g.clone() as Arc<dyn Embed>),
                None => anyhow::bail!("Failed to load the default metallib"),
            }
        })
    };
    let mut engine = engine.with_gpu(Some(load));
    engine.embedder = cpu.clone();
    (engine, cpu, loads)
}

fn texts(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("조각 {i}")).collect()
}

#[test]
fn passages_go_to_the_gpu_in_batches_of_eight_and_load_once() {
    assert_eq!(GPU_BATCH, 8);
    let gpu = Arc::new(FakeEmbed::default());
    let (engine, cpu, loads) = engine(Some(gpu.clone()));
    let texts = texts(19);
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut seen = Vec::new();
    let out = engine
        .embed_passages(&refs, &mut |done, total| seen.push((done, total)))
        .unwrap();
    assert_eq!(out.len(), 19);
    assert_eq!(
        gpu.texts().iter().map(Vec::len).collect::<Vec<_>>(),
        [8, 8, 3]
    );
    assert_eq!(gpu.texts().concat(), texts);
    assert_eq!(gpu.max_tokens(), [Some(512); 3]);
    assert_eq!(seen, [(8, 19), (16, 19), (19, 19)]);
    assert_eq!(cpu.calls(), 0);
    // 다음 묶음(갱신은 `WATCH_EVERY`개씩 불러요)은 올린 것을 그대로 써요
    engine.embed_passages(&refs, &mut |_, _| {}).unwrap();
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    // 갱신이 끝나 내리면 다음 갱신이 다시 올려요
    engine.release_gpu();
    engine.embed_passages(&refs[..1], &mut |_, _| {}).unwrap();
    assert_eq!(loads.load(Ordering::SeqCst), 2);
    assert_eq!(cpu.calls(), 0);
}

#[test]
fn without_a_gpu_passages_stay_on_the_cpu_and_loading_is_not_retried() {
    let (engine, cpu, loads) = engine(None);
    let texts = texts(5);
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let out = engine.embed_passages(&refs, &mut |_, _| {}).unwrap();
    assert_eq!(out.len(), 5);
    assert_eq!(
        cpu.texts().iter().map(Vec::len).collect::<Vec<_>>(),
        [2, 2, 1]
    );
    engine.release_gpu();
    engine.embed_passages(&refs, &mut |_, _| {}).unwrap();
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    assert_eq!(cpu.calls(), 6);
}

#[test]
fn a_gpu_failure_finishes_the_rest_on_the_cpu() {
    let gpu = Arc::new(FakeEmbed {
        fail: true,
        ..Default::default()
    });
    let (engine, cpu, loads) = engine(Some(gpu.clone()));
    let texts = texts(5);
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut seen = Vec::new();
    let out = engine
        .embed_passages(&refs, &mut |done, total| seen.push((done, total)))
        .unwrap();
    assert_eq!(out.len(), 5);
    assert_eq!(gpu.calls(), 1);
    assert_eq!(cpu.texts().concat(), texts); // 실패한 배치부터 CPU로 다시 해요
    assert_eq!(
        cpu.texts().iter().map(Vec::len).collect::<Vec<_>>(),
        [EMBED_BATCH, EMBED_BATCH, 1]
    );
    assert_eq!(seen, [(2, 5), (4, 5), (5, 5)]);
    // 그 뒤로는 GPU를 다시 올리지 않아요
    engine.embed_passages(&refs[..1], &mut |_, _| {}).unwrap();
    assert_eq!((loads.load(Ordering::SeqCst), gpu.calls()), (1, 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ask_does_not_wait_for_gpu_batches() {
    // GPU 배치 하나가 2 s예요. 질문이 GPU 배치를 기다리면 2 s가 넘어요(CPU 갱신은 배치마다 자물쇠를 같이 써요)
    let gpu = Arc::new(FakeEmbed {
        delay: Duration::from_secs(2),
        ..Default::default()
    });
    let (engine, _, _) = engine(Some(gpu.clone()));
    let engine = Arc::new(engine);
    let refresh = {
        let engine = engine.clone();
        std::thread::spawn(move || {
            let texts = texts(2 * GPU_BATCH);
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            engine.embed_passages(&refs, &mut |_, _| {}).unwrap().len()
        })
    };
    while gpu.calls() == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let start = Instant::now();
    let out = engine.ask("메일 탭 401 어디부터 봐요?", &[], 0.8).await;
    assert!(out["error"].is_null(), "{out}");
    assert!(
        start.elapsed() < Duration::from_millis(1500),
        "ask waited {:?}",
        start.elapsed()
    );
    assert_eq!(refresh.join().unwrap(), 2 * GPU_BATCH);
}
