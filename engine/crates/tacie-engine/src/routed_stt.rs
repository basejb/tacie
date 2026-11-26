//! Decides for each utterance whether to transcribe on the server or on this Mac (on-device speech recognition,
//! docs/22 3절·5-2).
//!
//! There is one rule: when the setting is on and there is a model path, an utterance **never goes to the server**.
//! While the model is loading it waits, and if local fails it returns a failure (`Kind::Model`). Audio must never
//! slip out of the Mac.
//! Utterances go to the server only when the setting is off or there is no model path.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::Notify;

use crate::errors::{Kind, ServiceError};
use crate::local_stt::{Device, LocalWhisper};
use crate::services::{BoxFuture, SttVia, Transcriber, Transcript};

/// 연속으로 이만큼 실패하면 모델을 한 번 다시 올려요. 그 뒤에도 이만큼 연속으로 실패하면 고장이에요.
const FAILS_BEFORE_RELOAD: u32 = 3;

/// 이 Mac에서 도는 받아쓰기 모델이에요. 테스트는 가짜로 바꿔요.
pub trait LocalModel: Send + Sync {
    fn transcribe(&self, wav: &[u8], prompt: &str) -> anyhow::Result<Transcript>;
}

impl LocalModel for LocalWhisper {
    fn transcribe(&self, wav: &[u8], prompt: &str) -> anyhow::Result<Transcript> {
        LocalWhisper::transcribe(self, wav, prompt)
    }
}

/// 모델 파일을 장치에 올려요(블로킹이라 `spawn_blocking`에서 불러요).
pub type LoadModel =
    Arc<dyn Fn(&Path, Device) -> anyhow::Result<Arc<dyn LocalModel>> + Send + Sync>;

/// 앱이 설정 화면에 보여 주는 상태예요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SttStatus {
    pub on: bool,
    pub ready: bool,
    pub loading: bool,
    pub broken: bool,
    pub device: Option<Device>,
}

#[derive(Default)]
struct State {
    on: bool,
    model: Option<PathBuf>,
    /// `set`이 불릴 때마다 올라가요. 오래된 올리기 결과가 새 설정을 덮지 않게 해요.
    generation: u64,
    metal: Option<Arc<dyn LocalModel>>,
    /// 동기화 중에만 있는 CPU 컨텍스트예요.
    cpu: Option<Arc<dyn LocalModel>>,
    /// 모델을 (다시) 올릴 때마다 올라가요. 낡은 모델이 아직 돌던 추론의 결과가 새 모델의 실패·성공으로 세어지지 않게 해요.
    metal_epoch: u64,
    cpu_epoch: u64,
    /// CPU 컨텍스트를 누가 올리는 중이에요. 동시에 온 발화는 기다려요(컨텍스트 하나가 0.8 GB쯤이에요).
    cpu_loading: bool,
    /// 이번 동기화에서 CPU 컨텍스트를 못 올렸어요. 동기화가 끝날 때까지 Metal로 해요.
    cpu_failed: bool,
    loading: bool,
    broken: bool,
    fails: u32,
    reloaded: bool,
    last_device: Option<Device>,
}

impl State {
    /// 이 설정에서 발화가 로컬로 가야 하나요.
    fn local(&self) -> bool {
        self.on && self.model.is_some()
    }
}

struct Shared {
    server: Arc<dyn Transcriber>,
    load: LoadModel,
    sync_running: Arc<dyn Fn() -> bool + Send + Sync>,
    prompt: Arc<dyn Fn() -> String + Send + Sync>,
    state: Mutex<State>,
    /// 올리기가 끝났거나 설정이 바뀌었어요.
    changed: Notify,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Metal 컨텍스트를 뒤에서 올려요. 부르는 쪽이 `loading = true`로 미리 바꿔 둬요.
    fn spawn_load(self: &Arc<Self>, path: PathBuf, generation: u64) {
        let this = self.clone();
        tokio::spawn(async move {
            let load = this.load.clone();
            let p = path.clone();
            let out = tokio::task::spawn_blocking(move || load(&p, Device::Metal)).await;
            let mut st = this.lock();
            if st.generation == generation {
                st.loading = false;
                match out {
                    Ok(Ok(m)) => st.metal = Some(m),
                    // 못 올렸어요. 서버로 돌리지 않고 고장으로 둬요(새 모델을 고르면 풀려요).
                    _ => st.broken = true,
                }
            }
            drop(st);
            this.changed.notify_waiters();
        });
    }
}

pub struct RoutedStt {
    shared: Arc<Shared>,
}

impl RoutedStt {
    pub fn new(
        server: Arc<dyn Transcriber>,
        load: LoadModel,
        sync_running: Arc<dyn Fn() -> bool + Send + Sync>,
        prompt: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self {
            shared: Arc::new(Shared {
                server,
                load,
                sync_running,
                prompt,
                state: Mutex::new(State::default()),
                changed: Notify::new(),
            }),
        })
    }

    /// 설정을 바꿔요. 바로 돌아오고, 모델 올리기는 뒤에서 해요(tokio 런타임 안에서 불러요).
    /// 꺼지거나 경로가 없어지면 모델을 내려요. 켜져 있는 채로 같은 경로가 다시 오면 그대로 둬요. 다만 고장 난 채로
    /// 같은 경로가 오면 다시 올려요(앱이 '잠시 멈췄어요'에 들어갈 때 한 번 보내요, docs/22).
    pub fn set(&self, on: bool, model: Option<PathBuf>) {
        let mut st = self.shared.lock();
        let same = st.on == on && st.model == model;
        let wants_local = on && model.is_some();
        if same && (!wants_local || ((st.loading || st.metal.is_some()) && !st.broken)) {
            return;
        }
        st.generation += 1;
        st.on = on;
        st.model = model.clone();
        st.metal = None;
        st.cpu = None;
        st.metal_epoch += 1;
        st.cpu_epoch += 1;
        st.cpu_loading = false;
        st.cpu_failed = false;
        st.loading = false;
        st.broken = false;
        st.fails = 0;
        st.reloaded = false;
        st.last_device = None;
        if let (true, Some(path)) = (on, model) {
            st.loading = true;
            let generation = st.generation;
            drop(st);
            self.shared.spawn_load(path, generation);
        } else {
            drop(st);
        }
        self.shared.changed.notify_waiters();
    }

    pub fn status(&self) -> SttStatus {
        self.release_idle_cpu();
        let st = self.shared.lock();
        let ready = st.local() && st.metal.is_some() && !st.broken;
        SttStatus {
            on: st.on,
            ready,
            loading: st.loading,
            broken: st.broken,
            device: if ready {
                st.last_device.or(Some(Device::Metal))
            } else {
                None
            },
        }
    }

    /// 동기화가 끝났으면 CPU 컨텍스트(0.8 GB쯤)를 내려요. 다음 발화를 기다리지 않게 `status()`와 엔진의 주기 점검이
    /// 불러요.
    pub fn release_idle_cpu(&self) {
        if (self.shared.sync_running)() {
            return;
        }
        let mut st = self.shared.lock();
        // 올리는 중인 CPU 컨텍스트는 끝나면 스스로 자리를 잡아요. 그 뒤 점검이 내려요.
        if st.cpu.is_some() || st.cpu_failed {
            st.cpu = None;
            st.cpu_epoch += 1;
            st.cpu_failed = false;
        }
    }

    /// CPU 컨텍스트가 올라와 있나요(테스트가 봐요).
    pub fn cpu_loaded(&self) -> bool {
        self.shared.lock().cpu.is_some()
    }
}

enum Route {
    Server,
    Wait,
    Fail(String),
    Local {
        model: Arc<dyn LocalModel>,
        device: Device,
        generation: u64,
        epoch: u64,
    },
    /// 동기화 중인데 CPU 컨텍스트가 아직 없어요.
    NeedCpu {
        path: PathBuf,
        generation: u64,
    },
}

impl Shared {
    fn route(&self) -> Route {
        let syncing = (self.sync_running)();
        let mut st = self.lock();
        if !st.local() {
            return Route::Server;
        }
        if st.broken {
            return Route::Fail("받아쓰기 모델이 고장 났어요. 설정에서 다시 골라 주세요".into());
        }
        if st.loading {
            return Route::Wait;
        }
        let Some(metal) = st.metal.clone() else {
            return Route::Wait;
        };
        if !syncing {
            // 동기화가 끝났으니 CPU 컨텍스트를 내려요.
            st.cpu = None;
            st.cpu_epoch += 1;
            st.cpu_failed = false;
            st.last_device = Some(Device::Metal);
            return Route::Local {
                model: metal,
                device: Device::Metal,
                generation: st.generation,
                epoch: st.metal_epoch,
            };
        }
        match st.cpu.clone() {
            Some(cpu) => {
                st.last_device = Some(Device::Cpu);
                Route::Local {
                    model: cpu,
                    device: Device::Cpu,
                    generation: st.generation,
                    epoch: st.cpu_epoch,
                }
            }
            // CPU를 못 올려도 Metal은 멀쩡해요. 동기화가 끝날 때까지 Metal로 해요.
            None if st.cpu_failed => {
                st.last_device = Some(Device::Metal);
                Route::Local {
                    model: metal,
                    device: Device::Metal,
                    generation: st.generation,
                    epoch: st.metal_epoch,
                }
            }
            // 이미 누가 올리는 중이면 끝나기를 기다려요.
            None if st.cpu_loading => Route::Wait,
            None => {
                st.cpu_loading = true;
                Route::NeedCpu {
                    path: st.model.clone().expect("local()이 보장해요"),
                    generation: st.generation,
                }
            }
        }
    }

    /// CPU 컨텍스트를 뒤에서 올려요(부르는 쪽이 `cpu_loading = true`로 미리 바꿔 둬요).
    /// 부른 발화가 취소돼도 끝까지 돌아서 `cpu_loading`을 풀고 기다리는 발화를 깨워요.
    fn spawn_cpu_load(self: &Arc<Self>, path: PathBuf, generation: u64) {
        let this = self.clone();
        tokio::spawn(async move {
            let load = this.load.clone();
            let out = tokio::task::spawn_blocking(move || load(&path, Device::Cpu)).await;
            let mut st = this.lock();
            if st.generation == generation {
                st.cpu_loading = false;
                match out {
                    Ok(Ok(m)) => {
                        st.cpu = Some(m);
                        st.cpu_epoch += 1;
                    }
                    _ => st.cpu_failed = true,
                }
            }
            drop(st);
            this.changed.notify_waiters();
        });
    }

    /// 로컬 결과를 세어요. 연속 실패 3번이면 같은 장치로 한 번 다시 올리고, 그 뒤에도 그러면 고장으로 둬요.
    fn record(self: &Arc<Self>, generation: u64, device: Device, epoch: u64, ok: bool) {
        let mut st = self.lock();
        let current = match device {
            Device::Metal => st.metal_epoch,
            Device::Cpu => st.cpu_epoch,
        };
        // 다시 올리기 전 모델이 돌던 추론의 결과는 새 모델과 상관없어요.
        if st.generation != generation || current != epoch {
            return;
        }
        if ok {
            st.fails = 0;
            st.reloaded = false;
            return;
        }
        st.fails += 1;
        if st.fails < FAILS_BEFORE_RELOAD {
            return;
        }
        st.fails = 0;
        if st.reloaded {
            st.broken = true;
            drop(st);
            self.changed.notify_waiters();
            return;
        }
        st.reloaded = true;
        match device {
            // CPU는 다음 발화가 새로 올려요.
            Device::Cpu => {
                st.cpu = None;
                st.cpu_epoch += 1;
            }
            Device::Metal => {
                st.metal = None;
                st.metal_epoch += 1;
                st.loading = true;
                let path = st.model.clone().expect("local()이 보장해요");
                drop(st);
                self.spawn_load(path, generation);
            }
        }
    }
}

fn model_error(message: String) -> ServiceError {
    ServiceError {
        kind: Kind::Model,
        message,
    }
}

impl Transcriber for RoutedStt {
    fn transcribe<'a>(&'a self, wav: &'a [u8]) -> BoxFuture<'a, Result<Transcript, ServiceError>> {
        Box::pin(async move {
            let shared = &self.shared;
            loop {
                // 알림을 먼저 걸어 두고 상태를 봐요: 그 사이에 끝나도 놓치지 않아요.
                let notified = shared.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                match shared.route() {
                    Route::Server => return shared.server.transcribe(wav).await,
                    Route::Wait => notified.await,
                    Route::Fail(message) => return Err(model_error(message)),
                    Route::NeedCpu { path, generation } => {
                        // 부른 쪽 future가 중간에 버려져도 올리기는 끝까지 가요. 기다리는 건 다른 발화와 같아요.
                        shared.spawn_cpu_load(path, generation);
                        notified.await;
                    }
                    Route::Local {
                        model,
                        device,
                        generation,
                        epoch,
                    } => return self.run_local(model, device, generation, epoch, wav).await,
                }
            }
        })
    }
}

impl RoutedStt {
    async fn run_local(
        &self,
        model: Arc<dyn LocalModel>,
        device: Device,
        generation: u64,
        epoch: u64,
        wav: &[u8],
    ) -> Result<Transcript, ServiceError> {
        let prompt = (self.shared.prompt)();
        let wav = wav.to_vec();
        let out = tokio::task::spawn_blocking(move || model.transcribe(&wav, &prompt)).await;
        let out = match out {
            Ok(r) => r,
            Err(e) => Err(anyhow::anyhow!("받아쓰기 작업이 멈췄어요: {e}")),
        };
        self.shared.record(generation, device, epoch, out.is_ok());
        match out {
            Ok(mut t) => {
                t.via = SttVia::Local;
                Ok(t)
            }
            Err(e) => Err(model_error(format!("이 Mac에서 받아쓰지 못했어요: {e}"))),
        }
    }
}
