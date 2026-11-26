//! Tests `RoutedStt`, which decides per utterance whether to transcribe on the server or on this Mac, with a fake
//! server and a fake local model.
//! Rule: when on and there is a model path, utterances do not go to the server (even while loading, even on
//! failure).

mod common {
    pub mod fakes;
}

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::fakes::{FakeTranscriber, transcript};
use tacie_engine::errors::Kind;
use tacie_engine::local_stt::Device;
use tacie_engine::routed_stt::{LoadModel, LocalModel, RoutedStt};
use tacie_engine::services::{SttVia, Transcriber, Transcript};

/// 부른 횟수, 받은 프롬프트, 쓰인 장치를 기록하는 가짜 로컬 모델이에요. `fail`이면 늘 실패해요.
struct FakeLocal {
    device: Device,
    fail: bool,
    index: usize,
    log: Arc<Log>,
}

#[derive(Default)]
struct Log {
    calls: AtomicUsize,
    loads: Mutex<Vec<Device>>,
    used: Mutex<Vec<Device>>,
    prompts: Mutex<Vec<String>>,
    /// 부를 때마다 앞에서 하나씩 꺼내 그만큼 기다려요(비면 안 기다려요).
    delays: Mutex<std::collections::VecDeque<Duration>>,
    /// 이 번호 이후로 올린 모델은 실패하지 않아요(기본은 끝까지 실패).
    good_from: AtomicUsize,
}

impl LocalModel for FakeLocal {
    fn transcribe(&self, _wav: &[u8], prompt: &str) -> anyhow::Result<Transcript> {
        self.log.calls.fetch_add(1, Ordering::SeqCst);
        self.log.used.lock().unwrap().push(self.device);
        self.log.prompts.lock().unwrap().push(prompt.to_string());
        let delay = self.log.delays.lock().unwrap().pop_front();
        if let Some(d) = delay {
            std::thread::sleep(d);
        }
        if self.fail && self.index < self.log.good_from.load(Ordering::SeqCst) {
            anyhow::bail!("가짜 실패");
        }
        let mut t = transcript("로컬", None);
        t.via = SttVia::Local;
        Ok(t)
    }
}

struct Rig {
    stt: Arc<RoutedStt>,
    server: Arc<FakeTranscriber>,
    log: Arc<Log>,
    syncing: Arc<AtomicBool>,
}

fn rig(fail: bool, load_delay: Duration, cpu_delay: Duration) -> Rig {
    let log = Arc::new(Log::default());
    log.good_from.store(usize::MAX, Ordering::SeqCst);
    let server = Arc::new(FakeTranscriber::ok("서버"));
    let syncing = Arc::new(AtomicBool::new(false));
    let l = log.clone();
    let load: LoadModel = Arc::new(move |_: &Path, device: Device| {
        std::thread::sleep(match device {
            Device::Metal => load_delay,
            Device::Cpu => cpu_delay,
        });
        let index = {
            let mut loads = l.loads.lock().unwrap();
            loads.push(device);
            loads.len() - 1
        };
        Ok(Arc::new(FakeLocal {
            device,
            fail,
            index,
            log: l.clone(),
        }) as Arc<dyn LocalModel>)
    });
    let s = syncing.clone();
    let stt = RoutedStt::new(
        server.clone(),
        load,
        Arc::new(move || s.load(Ordering::SeqCst)),
        Arc::new(|| "회의 용어: 테이시".to_string()),
    );
    Rig {
        stt,
        server,
        log,
        syncing,
    }
}

fn path() -> Option<PathBuf> {
    Some(PathBuf::from("/model.bin"))
}

async fn hear(stt: &RoutedStt) -> Result<Transcript, tacie_engine::errors::ServiceError> {
    stt.transcribe(b"wav").await
}

async fn wait_ready(stt: &RoutedStt) {
    for _ in 0..200 {
        if stt.status().ready {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("모델이 준비되지 않았어요");
}

#[tokio::test]
async fn off_uses_server() {
    let r = rig(false, Duration::ZERO, Duration::ZERO);
    r.stt.set(false, path());
    let t = hear(&r.stt).await.unwrap();
    assert_eq!(t.via, SttVia::Server);
    assert_eq!(r.server.calls().len(), 1);
    assert_eq!(r.log.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn on_without_model_uses_server() {
    let r = rig(false, Duration::ZERO, Duration::ZERO);
    r.stt.set(true, None);
    let t = hear(&r.stt).await.unwrap();
    assert_eq!(t.via, SttVia::Server);
    assert_eq!(r.server.calls().len(), 1);
}

#[tokio::test]
async fn loading_waits_and_never_goes_to_server() {
    let r = rig(false, Duration::from_millis(300), Duration::ZERO);
    r.stt.set(true, path());
    assert!(r.stt.status().loading);
    let t = hear(&r.stt).await.unwrap();
    assert_eq!(t.via, SttVia::Local);
    assert_eq!(r.server.calls().len(), 0);
}

#[tokio::test]
async fn ready_uses_metal_and_cpu_while_syncing() {
    let r = rig(false, Duration::ZERO, Duration::ZERO);
    r.stt.set(true, path());
    wait_ready(&r.stt).await;
    hear(&r.stt).await.unwrap();
    r.syncing.store(true, Ordering::SeqCst);
    hear(&r.stt).await.unwrap();
    r.syncing.store(false, Ordering::SeqCst);
    hear(&r.stt).await.unwrap();
    assert_eq!(
        *r.log.used.lock().unwrap(),
        vec![Device::Metal, Device::Cpu, Device::Metal]
    );
    assert_eq!(r.server.calls().len(), 0);
}

#[tokio::test]
async fn sync_switches_to_cpu_without_blocking() {
    let r = rig(false, Duration::ZERO, Duration::from_millis(200));
    r.stt.set(true, path());
    wait_ready(&r.stt).await;
    r.syncing.store(true, Ordering::SeqCst);
    let started = Instant::now();
    let t = hear(&r.stt).await.unwrap();
    assert_eq!(t.via, SttVia::Local);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(r.server.calls().len(), 0);
}

#[tokio::test]
async fn broken_model_never_falls_back_to_server() {
    let r = rig(true, Duration::ZERO, Duration::ZERO);
    r.stt.set(true, path());
    wait_ready(&r.stt).await;
    for _ in 0..10 {
        let e = hear(&r.stt).await.unwrap_err();
        assert_eq!(e.kind, Kind::Model);
    }
    // 처음 올림 하나, 연속 실패 3번 뒤 다시 올림 하나.
    assert_eq!(r.log.loads.lock().unwrap().len(), 2);
    let s = r.stt.status();
    assert!(s.broken);
    assert!(!s.ready);
    assert_eq!(r.server.calls().len(), 0);
}

#[tokio::test]
async fn new_model_clears_broken() {
    let r = rig(true, Duration::ZERO, Duration::ZERO);
    r.stt.set(true, path());
    wait_ready(&r.stt).await;
    for _ in 0..10 {
        let _ = hear(&r.stt).await;
    }
    assert!(r.stt.status().broken);
    r.stt.set(true, Some(PathBuf::from("/other.bin")));
    assert!(!r.stt.status().broken);
}

#[tokio::test]
async fn turning_off_routes_next_utterance_to_server_and_unloads() {
    let r = rig(false, Duration::ZERO, Duration::ZERO);
    r.stt.set(true, path());
    wait_ready(&r.stt).await;
    hear(&r.stt).await.unwrap();
    r.stt.set(false, path());
    let t = hear(&r.stt).await.unwrap();
    assert_eq!(t.via, SttVia::Server);
    // 모델이 내려갔으면 로그 말고는 아무도 로그를 쥐고 있지 않아요(모델 둘이 쥐던 참조가 사라져요).
    assert_eq!(Arc::strong_count(&r.log), 2); // r.log와 load 클로저
}

#[tokio::test]
async fn prompt_comes_from_terms() {
    let r = rig(false, Duration::ZERO, Duration::ZERO);
    r.stt.set(true, path());
    hear(&r.stt).await.unwrap();
    assert_eq!(*r.log.prompts.lock().unwrap(), vec!["회의 용어: 테이시"]);
}

#[tokio::test]
async fn concurrent_utterances_load_the_cpu_context_once() {
    let r = rig(false, Duration::ZERO, Duration::from_millis(200));
    r.stt.set(true, path());
    wait_ready(&r.stt).await;
    r.syncing.store(true, Ordering::SeqCst);
    let (a, b) = tokio::join!(hear(&r.stt), hear(&r.stt));
    assert_eq!(a.unwrap().via, SttVia::Local);
    assert_eq!(b.unwrap().via, SttVia::Local);
    let cpu_loads = r
        .log
        .loads
        .lock()
        .unwrap()
        .iter()
        .filter(|d| **d == Device::Cpu)
        .count();
    assert_eq!(cpu_loads, 1);
    assert_eq!(r.server.calls().len(), 0);
}

#[tokio::test]
async fn stale_inference_failures_do_not_break_the_reloaded_model() {
    let r = rig(true, Duration::ZERO, Duration::ZERO);
    // 첫 모델만 실패하고 다시 올린 모델은 멀쩡해요.
    r.log.good_from.store(1, Ordering::SeqCst);
    r.stt.set(true, path());
    wait_ready(&r.stt).await;
    // 낡은 모델에서 느리게 돌다 실패할 추론 셋, 그 뒤에 빠르게 실패하는 셋(다시 올리기를 일으켜요).
    {
        let mut d = r.log.delays.lock().unwrap();
        for _ in 0..3 {
            d.push_back(Duration::from_millis(400));
        }
    }
    let slow: Vec<_> = (0..3)
        .map(|_| {
            let stt = r.stt.clone();
            tokio::spawn(async move { hear(&stt).await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(100)).await;
    for _ in 0..3 {
        assert!(hear(&r.stt).await.is_err());
    }
    for h in slow {
        assert!(h.await.unwrap().is_err());
    }
    // 낡은 모델의 실패는 새 모델 몫이 아니에요.
    assert!(!r.stt.status().broken);
    let t = hear(&r.stt).await.unwrap();
    assert_eq!(t.via, SttVia::Local);
    assert_eq!(r.log.loads.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn turning_off_during_a_load_does_not_resurrect_the_model() {
    let r = rig(false, Duration::from_millis(300), Duration::ZERO);
    r.stt.set(true, path());
    tokio::time::sleep(Duration::from_millis(50)).await;
    r.stt.set(false, path());
    tokio::time::sleep(Duration::from_millis(500)).await;
    let s = r.stt.status();
    assert!(!s.on && !s.ready && !s.loading);
    assert_eq!(s.device, None);
    assert_eq!(hear(&r.stt).await.unwrap().via, SttVia::Server);
    assert_eq!(Arc::strong_count(&r.log), 2); // 올라온 모델은 버려졌어요
}

#[tokio::test]
async fn device_is_none_when_not_ready() {
    let r = rig(true, Duration::ZERO, Duration::ZERO);
    r.stt.set(true, path());
    wait_ready(&r.stt).await;
    assert_eq!(r.stt.status().device, Some(Device::Metal));
    for _ in 0..10 {
        let _ = hear(&r.stt).await;
    }
    let s = r.stt.status();
    assert!(s.broken);
    assert_eq!(s.device, None);
}

#[tokio::test]
async fn cancelled_cpu_load_does_not_hang_later_utterances() {
    let r = rig(false, Duration::ZERO, Duration::from_millis(300));
    r.stt.set(true, path());
    wait_ready(&r.stt).await;
    r.syncing.store(true, Ordering::SeqCst);
    // CPU 올리기가 한창일 때 부른 쪽이 포기해요.
    let cancelled = tokio::time::timeout(Duration::from_millis(50), hear(&r.stt)).await;
    assert!(cancelled.is_err());
    let t = tokio::time::timeout(Duration::from_secs(2), hear(&r.stt))
        .await
        .expect("뒤 발화가 멈췄어요")
        .unwrap();
    assert_eq!(t.via, SttVia::Local);
    assert_eq!(r.server.calls().len(), 0);
}

#[tokio::test]
async fn same_path_while_broken_reloads() {
    let r = rig(true, Duration::ZERO, Duration::ZERO);
    r.stt.set(true, path());
    wait_ready(&r.stt).await;
    for _ in 0..10 {
        let _ = hear(&r.stt).await;
    }
    assert!(r.stt.status().broken);
    let loads = r.log.loads.lock().unwrap().len();
    // 앱이 '잠시 멈췄어요'에 들어가며 같은 값을 다시 보내요: 고장을 풀고 다시 올려요.
    r.stt.set(true, path());
    assert!(!r.stt.status().broken);
    wait_ready(&r.stt).await;
    assert_eq!(r.log.loads.lock().unwrap().len(), loads + 1);
    // 멀쩡한 모델에 같은 값이 오면 그대로 둬요.
    r.stt.set(true, path());
    assert_eq!(r.log.loads.lock().unwrap().len(), loads + 1);
}

#[tokio::test]
async fn cpu_context_is_released_when_sync_ends_without_another_utterance() {
    let r = rig(false, Duration::ZERO, Duration::ZERO);
    r.stt.set(true, path());
    wait_ready(&r.stt).await;
    r.syncing.store(true, Ordering::SeqCst);
    hear(&r.stt).await.unwrap();
    assert!(r.stt.cpu_loaded());
    // 동기화 중에는 점검이 내리지 않아요.
    r.stt.release_idle_cpu();
    assert!(r.stt.cpu_loaded());
    r.syncing.store(false, Ordering::SeqCst);
    r.stt.release_idle_cpu();
    assert!(!r.stt.cpu_loaded());
    // status()도 내려요.
    r.syncing.store(true, Ordering::SeqCst);
    hear(&r.stt).await.unwrap();
    assert!(r.stt.cpu_loaded());
    r.syncing.store(false, Ordering::SeqCst);
    let _ = r.stt.status();
    assert!(!r.stt.cpu_loaded());
}
