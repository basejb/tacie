//! Takes a question and returns cards. `ask` is the ask window (searches right away, with no gate).
//!
//! The threshold and candidate rules are measured values (docs/01): show threshold 0.8, 50 candidates, 900
//! characters per chunk.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use regex::{Captures, Regex};
use serde_json::{Value, json};
use tacie_embed::voice::VoiceEmbed;
use tacie_embed::{Embed, PASSAGE_TOKENS};
use tacie_index::Index;

use crate::cards::{build_cards, split_chunk};
use crate::errors::Kind;
use crate::jev::Jev;
use crate::notes::{
    LEDGER_CHARS, LEDGER_PER_KIND, LEDGER_TEXT_MAX, keep_attendee_refs, one_line, parse_ledger_for,
    parse_overview, parse_question_review,
};
use crate::notes_kind::{NotesKind, classify_sample, parse_kind};
use crate::routed_stt::RoutedStt;
use crate::sample::{SAMPLE_BASE, Sample, embed_failure_kind};
use crate::services::Services;
use crate::session::Session;
use crate::settings::Settings;
use crate::terms::{READINGS_BATCH, Readings, Terms, extract};
use crate::voices::Voices;

// 서비스 trait은 services.rs로 옮겼어요. 예전 경로(`pipeline::Reranker` 등)도 그대로 쓸 수 있어요.
pub use crate::services::{AnswerOut, Answerer, BoxFuture, Reranker};

/// 리랭커에 보내는 조각 글자 수예요.
pub const PASSAGE_CHARS: usize = 900;
/// 답 모델에 보내는 조각 본문 글자 수예요.
pub const ANSWER_CHARS: usize = 1500;
/// 답 프롬프트가 정한 문장이에요. 앱도 이 앞부분만 맞춰 봐요.
pub const NO_ANSWER: &str = "찾은 자료에는 답이 없어요";
/// 프롬프트는 120자를 부탁해요. 이보다 길면 규칙을 따르지 않은 거예요.
pub const ANSWER_MAX: usize = 200;
/// 갱신은 새 조각을 이만큼씩 임베딩해요. 8개에서 일부러 줄였어요(2026-10-26):
/// CPU에서 512토큰 8개 배치는 약 1.3 s라 갱신 중 질문이 그만큼 기다려 스펙 6-4(카드 1초 안)를 못 맞췄어요.
/// 배치 크기는 벡터를 바꾸지 않아요(`smaller_batches_give_the_same_vectors`).
pub const EMBED_BATCH: usize = 2;
/// 배치마다 자물쇠를 놓고 이만큼 쉬어요.
pub const EMBED_PAUSE: Duration = Duration::from_millis(20);
/// GPU로 하는 갱신은 이만큼씩 임베딩해요. GPU는 질문(CPU)과 자물쇠를 나누지 않아서 질문이 기다리지 않고, 8개가
/// 가장 빨랐어요(2개 28.0 s, 8개 20.4 s, 32개 21.1 s, 조각 400개, docs/01 "동기화 임베딩을 GPU로").
pub const GPU_BATCH: usize = 8;

/// GPU 임베더를 올리는 함수예요(엔진이 뜰 때 정해요, 없으면 갱신도 CPU로 해요).
pub type GpuLoad = Arc<dyn Fn() -> anyhow::Result<Arc<dyn Embed>> + Send + Sync>;

/// 갱신의 문단 임베딩만 쓰는 GPU 임베더예요. 처음 쓸 때 올리고(약 1–2 s) 갱신이 끝나면 내려서, 쉬는 동안 GPU
/// 메모리(약 2.5 GB)를 들고 있지 않아요. 한 번 실패하면 엔진이 끝날 때까지 CPU로 해요.
#[derive(Default)]
struct Gpu {
    load: Option<GpuLoad>,
    slot: Mutex<Option<Arc<dyn Embed>>>,
    off: AtomicBool,
}

static CITATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d+(?:\s*,\s*\d+)*)\]").expect("citation regex"));
// 프롬프트가 막아도 모델이 가끔 "자료 1에 따르면"으로 시작해요(<자료> 태그로 감싼 뒤로 더 자주요).
// "자료 [1]", "1번 자료", "자료 1, 2", "자료(1)와 2"도 있어요.
const NUM: &str = r"[\[(]?\s*\d+\s*[\])]?";
static SOURCE_PHRASE: LazyLock<Regex> = LazyLock::new(|| {
    // 번호 사이에는 표시나 괄호가 있어요: 되돌아가며 찾을 일이 없어요.
    let nums = format!(r"{NUM}(?:\s*(?:,|·|와|과|및)\s*{NUM}|\s*[\[(]\s*\d+\s*[\])])*");
    Regex::new(&format!(
        r"^(?:자료\s*(?:{nums})?|{nums}\s*번\s*자료)\s*에\s*따르면\s*,?\s*"
    ))
    .expect("source phrase regex")
});
// 2026-11-04: 흐릿한 질문에는 질문을 되뇌고 시작하기도 해요("회의 질문 \"…\"에 대해 자료 1에 따르면 …"). 되뇐 앞머리를
// 떼면 "자료 1에 따르면"이 다시 맨 앞이에요.
static QUESTION_ECHO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(?:회의\s*)?질문\s*["“'‘「][^"”'’」]*["”'’」]\s*에\s*대(?:해서?|한\s*답은?)\s*,?\s*"#,
    )
    .expect("question echo regex")
});
// 앞머리를 떼고도 자료를 번호로 가리키면("자료 1에는 …라고 적혀 있어요") 고쳐 쓸 수 없어요: 답 줄을 숨기고 카드만 보여요.
// 회의 기록 41개의 답 45개 중 2개였어요(둘 다 흐릿한 질문).
static SOURCE_TALK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"자료\s*[\[(]?\s*\d|\d+\s*번\s*자료").expect("source talk regex"));
/// `argv[0]`은 코드지 근거 번호가 아니에요.
static CODE_SPAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(`[^`]*`)").expect("code span regex"));
static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("digits regex"));
static GITHUB_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(https://github\.com/|git@github\.com:)|\.git$").expect("github url regex")
});

pub struct Engine {
    /// 갱신이 다음 색인으로 바꿔 넣어요. 질문 하나는 처음 읽은 색인 하나로 끝까지 가요.
    pub index: ArcSwap<Index>,
    pub embedder: Arc<dyn Embed>,
    /// 판단, 리랭커, 답, 받아쓰기, 장부 모델이에요.
    pub services: Services,
    pub repo_dir: Option<PathBuf>,
    /// 키 파일, Jev 공급자, 슬랙 사용자를 읽는 설정 폴더예요(기본 `~/.config`, 시작할 때 한 번 정해요).
    pub config_dir: PathBuf,
    pub settings: Settings,
    /// 개발용: 묻는 말의 앞 대화를 stderr에 남겨요(`TACIE_LOG_TEXT=1`).
    pub log_text: bool,
    /// 공급자와 모델은 시작할 때 한 번 정해서 여기 있어요(/health, /config가 읽어요).
    pub jev: Arc<Jev>,
    /// 서비스("stt", "answer", "ledger", "jev")별 마지막 거절(key, credit)이에요.
    pub last: Arc<Mutex<HashMap<String, Kind>>>,
    /// repo_dir 체크아웃의 origin(owner/name)이에요. 엔진이 뜰 때 한 번 읽어요.
    origin: Option<String>,
    /// ORT 추론은 한 번에 하나예요(스펙 5-2). 뒤에 갱신도 이 자물쇠를 같이 써요.
    embed_lock: Arc<Mutex<()>>,
    /// 열린 회의 세션들이에요(id → 세션).
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    /// 다음 세션 번호예요(1부터, 지운 번호는 다시 쓰지 않아요).
    next_session: AtomicU64,
    /// 온보딩 연습의 예시 자료예요(`--sample-dir`). `sample: true` 요청만 여기서 찾아요.
    pub sample: Option<Sample>,
    /// 검색 모델 파일이 있어요. 없으면 엔진은 뜨지만 임베딩은 `model`로 실패해요(예시 질문은 빼고요).
    pub model_ready: bool,
    /// iPhone 엔진이에요(docs/27 8-7): 판단만 이 엔진이 하고, 질문이면 카드를 찾지 않고 질문과 앞 대화를 앱에 돌려줘요.
    /// 지난 회의는 앱이 서버(`/meetings/ask`)에 물어요. 검색 모델과 색인이 없어도 판단해요. Mac 엔진은 꺼져 있어요.
    pub gate_only: bool,
    /// iPhone 엔진이에요(docs/40 4-3절): 패널 하네스가 iPhone 짝 프롬프트·도구 묶음(`panel_chat_iphone`, `panel_iphone*`)을
    /// 고르고, 도구 결과의 자리 말도 "이 iPhone에"로 써요. Mac 엔진은 꺼져 있어요.
    pub iphone: bool,
    /// 화자 임베딩 모델이에요(docs/04 4절). 없으면 `voice`는 늘 null이고 나머지는 그대로예요.
    pub voice: Option<Arc<dyn VoiceEmbed>>,
    /// 이 Mac에서 받아쓰는 설정과 상태예요(`POST /stt`). 서버 받아쓰기만 쓰는 엔진(테스트)에는 없어요.
    pub stt: Option<Arc<RoutedStt>>,
    gpu: Gpu,
    /// 받아쓰기의 팀 용어 고치기예요(`terms`). 표기 파일이 없으면(테스트) 영문 고치기만 해요.
    term_state: Arc<TermState>,
}

/// 용어 고치기의 상태예요: 지금 색인으로 만든 고치기, 표기 파일 자리, 뒤에서 표기를 받는 중인지예요.
#[derive(Default)]
pub struct TermState {
    /// 이 Mac의 표기 파일이에요(`<data>/term_readings.json`). None이면 표기를 받지 않아요.
    file: Option<PathBuf>,
    /// (만든 색인의 주소, 고치기)예요. 색인이 바뀌거나 새 표기가 오면 다시 만들어요.
    built: Mutex<Option<(usize, Arc<Terms>)>>,
    fetching: AtomicBool,
    /// 마지막으로 표기 받기에 실패한 때예요. 10분 동안은 다시 묻지 않아요.
    failed_at: Mutex<Option<Instant>>,
}

/// 표기 받기에 실패하면 이만큼 기다렸다가 다시 물어요.
const READINGS_RETRY: Duration = Duration::from_secs(600);

/// 검색 범위예요. `Meetings(ids)`는 `meeting/<id>` 조각만 후보로 남겨요.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Meetings(Vec<String>),
}

impl Scope {
    /// 후보로 남길 문서 id(`path`)예요. `All`이면 걸러내지 않아요.
    fn paths(&self) -> Option<HashSet<String>> {
        match self {
            Scope::All => None,
            Scope::Meetings(ids) => Some(ids.iter().map(|id| format!("meeting/{id}")).collect()),
        }
    }
}

impl Engine {
    pub fn new(
        index: Index,
        embedder: Arc<dyn Embed>,
        services: Services,
        settings: Settings,
        jev: Arc<Jev>,
        last: Arc<Mutex<HashMap<String, Kind>>>,
    ) -> Self {
        Self {
            index: ArcSwap::from_pointee(index),
            embedder,
            services,
            repo_dir: None,
            config_dir: crate::settings::config_dir(),
            log_text: settings.log_text,
            settings,
            jev,
            last,
            origin: None,
            embed_lock: Arc::new(Mutex::new(())),
            sessions: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
            sample: None,
            model_ready: true,
            gate_only: false,
            iphone: false,
            voice: None,
            stt: None,
            gpu: Gpu::default(),
            term_state: Arc::new(TermState::default()),
        }
    }

    /// 팀 용어 표기 파일 자리예요(main이 데이터 폴더 안을 줘요).
    pub fn with_terms_file(mut self, file: Option<PathBuf>) -> Self {
        self.term_state = Arc::new(TermState {
            file,
            ..TermState::default()
        });
        self
    }

    /// 지금 색인으로 만든 용어 고치기예요. 색인이 바뀌었으면 다시 뽑고, 표기가 없는 용어가 있으면 뒤에서
    /// 받아요(회의 시작과 발화마다 불러도 색인이 같으면 바로 돌아와요).
    pub fn term_fixer(&self) -> Arc<Terms> {
        let index = self.index.load_full();
        let key = Arc::as_ptr(&index) as usize;
        let st = &self.term_state;
        if let Some((k, t)) = st
            .built
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            && *k == key
        {
            return t.clone();
        }
        let list = extract(&index.chunks, &index.allowed);
        let readings = st.file.as_deref().map(Readings::load).unwrap_or_default();
        let missing = readings.missing(&list);
        let terms = Arc::new(Terms::new(list, &readings));
        *st.built.lock().unwrap_or_else(PoisonError::into_inner) = Some((key, terms.clone()));
        if !missing.is_empty() {
            self.fetch_readings(missing);
        }
        terms
    }

    /// 표기가 없는 용어를 답 모델에 물어 파일에 더해요. 한 번에 하나만 돌고, 끝나면 고치기를 다시 만들게 해요.
    fn fetch_readings(&self, missing: Vec<String>) {
        let st = self.term_state.clone();
        let Some(file) = st.file.clone() else { return };
        let recent_fail = st
            .failed_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|t| t.elapsed() < READINGS_RETRY);
        if recent_fail
            || tokio::runtime::Handle::try_current().is_err()
            || st.fetching.swap(true, Ordering::SeqCst)
        {
            return;
        }
        let writer = self.services.note_writer.clone();
        tokio::spawn(async move {
            let mut readings = Readings::load(&file);
            let mut ok = true;
            for batch in missing.chunks(READINGS_BATCH) {
                match writer.readings(batch).await {
                    Ok(out) if readings.merge_reply(batch, &out.text) => {}
                    Ok(_) => {
                        eprintln!("terms: readings reply was not a JSON object");
                        ok = false;
                        break;
                    }
                    Err(e) => {
                        eprintln!("terms: readings failed ({:?})", e.kind);
                        ok = false;
                        break;
                    }
                }
            }
            if let Err(e) = readings.save(&file) {
                eprintln!("terms: save readings: {e}");
            }
            if !ok {
                *st.failed_at.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
            }
            // 다음 부를 때 새 표기로 다시 만들어요.
            *st.built.lock().unwrap_or_else(PoisonError::into_inner) = None;
            st.fetching.store(false, Ordering::SeqCst);
        });
    }

    pub fn with_gpu(mut self, load: Option<GpuLoad>) -> Self {
        self.gpu.load = load;
        self
    }

    /// 올라 있는 GPU 임베더이고, 없으면 올려요. GPU를 안 쓰거나 올리지 못했으면 None이에요(그때는 CPU로 해요).
    fn gpu_embedder(&self) -> Option<Arc<dyn Embed>> {
        let load = self.gpu.load.as_ref()?;
        if self.gpu.off.load(Ordering::Relaxed) {
            return None;
        }
        let mut slot = self.gpu.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            let t = Instant::now();
            match load() {
                Ok(e) => {
                    eprintln!("embed: gpu on ({:.1} s)", t.elapsed().as_secs_f64());
                    *slot = Some(e);
                }
                Err(e) => {
                    self.gpu_off(&e);
                    return None;
                }
            }
        }
        slot.clone()
    }

    fn gpu_off(&self, e: &anyhow::Error) {
        self.gpu.off.store(true, Ordering::Relaxed);
        eprintln!(
            "embed: gpu unavailable, using cpu ({})",
            format!("{e:#}").replace('\n', " ")
        );
    }

    /// 갱신이 끝나면 GPU 임베더를 내려요(GPU 메모리를 돌려줘요). 다음 갱신이 다시 올려요.
    pub fn release_gpu(&self) {
        let gpu = self
            .gpu
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(gpu); // 자물쇠 밖에서 모델 스레드가 끝나기를 기다려요
    }

    pub fn with_sample(mut self, sample: Option<Sample>) -> Self {
        self.sample = sample;
        self
    }

    pub fn with_model_ready(mut self, ready: bool) -> Self {
        self.model_ready = ready;
        self
    }

    pub fn with_gate_only(mut self, on: bool) -> Self {
        self.gate_only = on;
        self
    }

    pub fn with_iphone(mut self, on: bool) -> Self {
        self.iphone = on;
        self
    }

    /// 받아쓰기 경로예요. `services.transcriber`와 같은 `RoutedStt`를 넘겨요.
    pub fn with_stt(mut self, stt: Option<Arc<RoutedStt>>) -> Self {
        self.stt = stt;
        self
    }

    pub fn with_voice(mut self, voice: Option<Arc<dyn VoiceEmbed>>) -> Self {
        self.voice = voice;
        self
    }

    /// 설정 폴더를 바꿔요(테스트와 main이 Jev·OpenRouter에 준 것과 같은 값을 넘겨요).
    pub fn with_config_dir(mut self, dir: PathBuf) -> Self {
        self.config_dir = dir;
        self
    }

    /// 카드가 열 로컬 체크아웃이에요. 그 체크아웃의 origin을 여기서 한 번 읽어요.
    pub fn with_repo_dir(mut self, repo_dir: Option<PathBuf>) -> Self {
        self.origin = repo_dir.as_deref().and_then(checkout_repo);
        self.repo_dir = repo_dir;
        self
    }

    /// 카드는 색인이 이 체크아웃의 저장소로 만든 것일 때만 repo_dir을 가리켜요. 아니면 GitHub에서 열려요.
    pub fn local_repo_dir(&self, ix: &Index) -> Option<&Path> {
        let repo = ix
            .meta
            .get("repo")
            .and_then(Value::as_str)
            .filter(|r| !r.is_empty())?;
        let origin = self.origin.as_deref()?;
        if origin.to_lowercase() == repo.to_lowercase() {
            self.repo_dir.as_deref()
        } else {
            None
        }
    }

    /// 동기화 전이거나, 지웠거나, 이 사람이 볼 수 있는 조각이 없어요: 찾을 게 없으니 유료 호출도 하지 않아요.
    pub fn index_is_empty(&self) -> bool {
        !self.index.load().searchable()
    }

    /// `index_is_empty`의 예시 자료 쪽이에요: 예시 자료가 없으면 비어 있어요.
    pub fn index_is_empty_in(&self, sample: bool) -> bool {
        if sample {
            !self.sample.as_ref().is_some_and(|s| s.index.searchable())
        } else {
            self.index_is_empty()
        }
    }

    /// 찾기 + 리랭크예요. Jev가 실패하면 dense 순서로 돌아가요(low_confidence).
    pub async fn answer(
        &self,
        text: &str,
        context: &[String],
        line: &str,
        show_thr: f64,
        n: usize,
    ) -> Value {
        self.answer_in(false, text, context, line, show_thr, n)
            .await
    }

    /// `answer`를 내 자료(`sample == false`)나 예시 자료에서 해요. 예시 답에는 `"sample": true`가 붙어요.
    pub async fn answer_in(
        &self,
        sample: bool,
        text: &str,
        context: &[String],
        line: &str,
        show_thr: f64,
        n: usize,
    ) -> Value {
        self.answer_except(sample, text, context, line, show_thr, n, &[], None)
            .await
    }

    /// `answer_in`과 같지만 `exclude`에 적은 문서(`path`)는 후보에서 빼요. 예시 자료의 표시도 똑같이 해요.
    #[allow(clippy::too_many_arguments)]
    async fn answer_except(
        &self,
        sample: bool,
        text: &str,
        context: &[String],
        line: &str,
        show_thr: f64,
        n: usize,
        exclude: &[String],
        only: Option<&HashSet<String>>,
    ) -> Value {
        let mut out = self
            .search(sample, text, context, line, show_thr, n, exclude, only)
            .await;
        if sample {
            out["sample"] = json!(true);
            // 예시 조각 id는 SAMPLE_BASE를 더해서 내보내요(`/answer`가 id로 색인을 알아봐요).
            if let Some(cards) = out["cards"].as_array_mut() {
                for c in cards {
                    if let Some(id) = c["chunk"].as_u64() {
                        c["chunk"] = json!(id + SAMPLE_BASE as u64);
                    }
                }
            }
        }
        out
    }

    /// `exclude`: 후보에서 뺄 문서 id(`path`)예요. 지난 질문을 다시 찾을 때 그 질문이 나온 회의를 빼요(docs/30).
    /// `only`: 있으면 이 문서(`path`)의 조각 안에서만 찾아요(`Scope::Meetings`). 권한(`allowed`)은 그 전에 걸러요.
    #[allow(clippy::too_many_arguments)]
    async fn search(
        &self,
        sample: bool,
        text: &str,
        context: &[String],
        line: &str,
        show_thr: f64,
        n: usize,
        exclude: &[String],
        only: Option<&HashSet<String>>,
    ) -> Value {
        // 질문 하나에 색인 하나예요: 그사이 갱신이 다음 색인을 넣어도 이 질문은 이걸로 끝나요.
        let t = Instant::now();
        // 예시 자료는 조각 전부가 후보예요(임베딩 없이, 모델을 받기 전에도). 내 자료는 dense 검색이에요.
        let (ix, cands) = if sample {
            let Some(s) = &self.sample else {
                return json!({"cards": [], "best_p": null, "shown": false, "low_confidence": false,
                              "error": "no sample data", "error_kind": Kind::Down});
            };
            (s.index.clone(), Ok(s.candidates()))
        } else {
            let ix = self.index.load_full();
            let cands = if ix.searchable() {
                self.candidates(&ix, text, context, line, only).await
            } else {
                Ok(Vec::new())
            };
            (ix, cands)
        };
        let local = if sample {
            None
        } else {
            self.local_repo_dir(&ix)
        };
        let search = ms(t);
        let cands = match cands {
            Ok(cands) if exclude.is_empty() && only.is_none() => cands,
            Ok(mut cands) => {
                cands.retain(|&c| {
                    let path = &ix.chunks[c].path;
                    !exclude.contains(path) && only.is_none_or(|o| o.contains(path))
                });
                cands
            }
            // 임베딩이 실패했어요. 리랭커 실패처럼 이유를 실어 돌려줘요. 모델을 아직
            // 받지 않았으면 `model`이에요: 앱이 "검색 모델을 받고 있어요"를 보여 줘요.
            Err(e) => {
                return json!({"cards": [], "best_p": null, "shown": false, "low_confidence": true,
                              "error": head(&format!("search failed: {e:#}")), "error_kind": embed_failure_kind(&e),
                              "timing_ms": {"search": search}});
            }
        };
        if cands.is_empty() {
            // 빈 색인: 카드가 없고 리랭커도 부르지 않아요.
            return json!({"cards": [], "best_p": null, "shown": false, "low_confidence": false,
                          "timing_ms": {"search": search}});
        }
        let passages: Vec<String> = cands
            .iter()
            .map(|&c| ix.chunks[c].text.chars().take(PASSAGE_CHARS).collect())
            .collect();
        match self
            .services
            .reranker
            .rerank(context, line, &passages)
            .await
        {
            Err(e) => {
                let cards = build_cards(
                    &ix.chunks,
                    &cands,
                    &vec![None; cands.len()],
                    &ix.meta,
                    n,
                    local,
                );
                let shown = !cards.is_empty();
                json!({"cards": cards, "best_p": null, "shown": shown, "low_confidence": true,
                       "error": head(&e.message), "error_kind": e.kind, "timing_ms": {"search": search}})
            }
            Ok(r) => {
                let mut ranked: Vec<(usize, f64)> =
                    cands.iter().copied().zip(r.p.iter().copied()).collect();
                ranked.sort_by(|a, b| b.1.total_cmp(&a.1)); // 안정 정렬: 같은 p는 후보 순서 그대로예요
                let best = ranked.first().map_or(0.0, |&(_, p)| p);
                let order: Vec<usize> = ranked.iter().map(|&(c, _)| c).collect();
                let scores: Vec<Option<f64>> = ranked.iter().map(|&(_, p)| Some(p)).collect();
                let cards = build_cards(&ix.chunks, &order, &scores, &ix.meta, n, local);
                let cost = r.usage.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
                json!({"cards": cards, "best_p": best, "shown": best >= show_thr, "low_confidence": false,
                       "cost_usd": cost, "timing_ms": {"search": search, "rerank": r.ms}})
            }
        }
    }

    /// 묻기 창이에요: 앞 대화는 마지막 세 줄, 이번 줄은 "나: " + 질문이에요. `timing_ms.total`을 더해요.
    pub async fn ask(&self, text: &str, context: &[String], show_thr: f64) -> Value {
        self.ask_in(false, text, context, show_thr).await
    }

    /// `ask`를 예시 자료에서도 할 수 있어요(`sample: true`, 온보딩 연습).
    pub async fn ask_in(
        &self,
        sample: bool,
        text: &str,
        context: &[String],
        show_thr: f64,
    ) -> Value {
        self.ask_except(sample, text, context, show_thr, &[]).await
    }

    /// `ask_in`과 같지만 `exclude`에 적은 문서(`path`)는 후보에서 빼요(`/ask`의 `exclude_paths`, docs/30).
    pub async fn ask_except(
        &self,
        sample: bool,
        text: &str,
        context: &[String],
        show_thr: f64,
        exclude: &[String],
    ) -> Value {
        let t = Instant::now();
        let context = last_three(context);
        if self.log_text {
            eprintln!("[ask] context {}", json!(context));
        }
        let line = format!("나: {text}");
        let mut out = self
            .answer_except(sample, text, context, &line, show_thr, 3, exclude, None)
            .await;
        out["timing_ms"]["total"] = json!(ms(t));
        out
    }

    /// `ask`를 `scope` 안의 문서로만 좁혀요. `Scope::All`이면 `ask`와 같아요.
    pub async fn ask_scoped(
        &self,
        text: &str,
        context: &[String],
        scope: &Scope,
        show_thr: f64,
    ) -> Value {
        self.ask_scoped_n(text, context, scope, show_thr, 3).await
    }

    /// `ask_scoped`에서 카드를 `n`장까지 돌려줘요. 앞 대화는 마지막 세 줄이에요.
    pub async fn ask_scoped_n(
        &self,
        text: &str,
        context: &[String],
        scope: &Scope,
        show_thr: f64,
        n: usize,
    ) -> Value {
        self.ask_scoped_turns(text, context, scope, show_thr, n, 3)
            .await
    }

    /// `ask_scoped_n`에서 앞 대화를 마지막 `turns`줄까지 실어요(패널은 답 모델과 같은 줄 수예요).
    pub async fn ask_scoped_turns(
        &self,
        text: &str,
        context: &[String],
        scope: &Scope,
        show_thr: f64,
        n: usize,
        turns: usize,
    ) -> Value {
        let only = scope.paths();
        let t = Instant::now();
        let context = &context[context.len().saturating_sub(turns)..];
        if self.log_text {
            eprintln!("[ask] context {}", json!(context));
        }
        let line = format!("나: {text}");
        let mut out = self
            .answer_except(false, text, context, &line, show_thr, n, &[], only.as_ref())
            .await;
        out["timing_ms"]["total"] = json!(ms(t));
        out
    }

    /// 보여 준 카드의 조각 전체(스니펫만이 아니에요)로 한 줄 답을 써요. 근거 번호는 조각 id로 바꿔 돌려줘요.
    /// 지금 색인으로 `compose_answer_from`을 불러요.
    pub async fn compose_answer(
        &self,
        question: &str,
        context: &[String],
        chunk_ids: &[usize],
    ) -> Value {
        let ix = self.index.load_full();
        self.compose_answer_from(&ix, question, context, chunk_ids)
            .await
    }

    /// `compose_answer`를 주어진 색인으로 해요. `/answer`는 조각 id를 거를 때 읽은 색인을 그대로 넘겨요:
    /// 그사이 갱신이 다음 색인을 넣어도 걸러 둔 id가 다른 조각을 가리키지 않아요. id는 `ix` 안에 있어야 해요.
    pub async fn compose_answer_from(
        &self,
        ix: &Arc<Index>,
        question: &str,
        context: &[String],
        chunk_ids: &[usize],
    ) -> Value {
        self.compose_answer_keeping(ix, question, context, chunk_ids, 3)
            .await
    }

    /// `compose_answer_from` with the last `keep` lines of `context` (the panel's conversation sets its own).
    pub async fn compose_answer_keeping(
        &self,
        ix: &Arc<Index>,
        question: &str,
        context: &[String],
        chunk_ids: &[usize],
        keep: usize,
    ) -> Value {
        let passages: Vec<Value> = chunk_ids
            .iter()
            .map(|&cid| {
                let c = &ix.chunks[cid];
                let (head, body) = split_chunk(&c.text);
                let path = c
                    .title
                    .as_deref()
                    .filter(|t| !t.is_empty())
                    .unwrap_or(&c.path);
                let snippet: String = passage_text(&c.src, &head, &body)
                    .chars()
                    .take(ANSWER_CHARS)
                    .collect();
                json!({"path": path, "line_start": c.line, "snippet": snippet})
            })
            .collect();
        let context = &context[context.len().saturating_sub(keep)..];
        if self.log_text {
            eprintln!("[answer] context {}", json!(context));
        }
        match self
            .services
            .answerer
            .answer(question, context, &passages)
            .await
        {
            Err(e) => {
                json!({"answer": null, "cited": [], "dropped": null, "error": head(&e.message),
                             "error_kind": e.kind})
            }
            Ok(out) => {
                let (text, cited, dropped) = check_answer(&out.answer, chunk_ids.len());
                let cited: Vec<usize> = cited.iter().map(|&c| chunk_ids[c - 1]).collect();
                json!({"answer": text, "ms": out.ms, "cost": out.cost, "cited": cited, "dropped": dropped})
            }
        }
    }

    /// 참석자 없이 쓰는 [`Engine::notes_with`]예요(owner는 모델이 쓴 이름 그대로예요).
    pub async fn notes_from(&self, lines: &[(String, String)], asked: &HashSet<String>) -> Value {
        self.notes_with(lines, &[], asked, None, None).await
    }

    /// 회의 글 줄 `[(발화 id, "[id] 화자: 글")]`로 제목, 요약, 장부를 써요. Tacie가 찾아본 질문(`asked`)만 근거인
    /// 항목은 빼요: 그건 이미 답이나 문서 빈틈에 있어요. `attendees`는 (보인 이름, 화자 키)예요: 이름은 모델에
    /// 참석자로 보내고, 모델이 owner로 고른 이름은 키로 돌려줘요. 같은 이름이 둘이면 앞의 것이에요.
    pub async fn notes_with(
        &self,
        lines: &[(String, String)],
        attendees: &[(String, String)],
        asked: &HashSet<String>,
        kind: Option<NotesKind>,
        request: Option<&str>,
    ) -> Value {
        self.notes_in(lines, attendees, asked, kind, request, None).await
    }

    /// [`Engine::notes_with`]를 정한 언어로 써요(설정 → 회의 → "회의 정리 언어", 2026-11-19). `lang`이 없으면 회의 언어대로예요.
    pub async fn notes_in(
        &self,
        lines: &[(String, String)],
        attendees: &[(String, String)],
        asked: &HashSet<String>,
        kind: Option<NotesKind>,
        request: Option<&str>,
        lang: Option<crate::lang::Lang>,
    ) -> Value {
        // 아주 긴 회의는 뒷부분을 남겨요: 앞에서부터 줄을 버려 줄마다 글자 수 + 1(줄바꿈)이 40000자까지예요.
        let mut total: usize = lines.iter().map(|(_, l)| l.chars().count() + 1).sum();
        let mut start = 0;
        while start < lines.len() && total > LEDGER_CHARS {
            total -= lines[start].1.chars().count() + 1;
            start += 1;
        }
        let lines = &lines[start..];
        if lines.is_empty() {
            let (kind, source) = kind.map_or(("meeting", "auto"), |k| (k.as_str(), "user"));
            return json!({"kind": kind, "kind_source": source, "title": null, "summary": [], "topics": [], "items": [],
                          "dropped": 0, "ms": 0, "cost": 0});
        }
        let transcript = lines
            .iter()
            .map(|(_, l)| l.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let mut owners: HashMap<String, String> = HashMap::new();
        let mut names: Vec<String> = Vec::new();
        for (name, key) in attendees {
            if !owners.contains_key(name) {
                owners.insert(name.clone(), key.clone());
                names.push(name.clone());
            }
        }
        // 녹음 유형: 사람이 골랐으면 그대로, 아니면 앞부분으로 정해요. 분류가 실패하면 회의예요(docs/23 3-3절).
        let (kind, source, classify_ms, classify_cost) = match kind {
            Some(k) => (k, "user", 0.0, 0.0),
            None => match self
                .services
                .note_writer
                .classify(&classify_sample(&transcript))
                .await
            {
                Ok(c) => (parse_kind(&c.text), "auto", c.ms, c.cost),
                Err(_) => (NotesKind::Meeting, "auto", 0.0, 0.0),
            },
        };
        let out = match self
            .services
            .note_writer
            .ledger_in(&transcript, &names, kind, request, lang)
            .await
        {
            Ok(out) => out,
            Err(e) => {
                return json!({"items": [], "error": head(&e.message), "error_kind": e.kind});
            }
        };
        let ids: HashSet<String> = lines.iter().map(|(id, _)| id.clone()).collect();
        let Ok((items, dropped)) = parse_ledger_for(&out.text, &ids, &owners, kind.item_kinds())
        else {
            return json!({"items": [], "error": "unreadable"});
        };
        // Tacie가 찾아본 질문 하나만 근거인 항목은 그 질문을 다시 쓴 거예요. 받아쓰기가 "?"를 "."로 바꾸면
        // 결정으로 잘못 읽히기도 해요.
        let n = items.len();
        let kept: Vec<Value> = items
            .into_iter()
            .filter(|it| {
                !it["utterance_ids"].as_array().is_some_and(|refs| {
                    refs.iter()
                        .all(|u| u.as_str().is_some_and(|u| asked.contains(u)))
                })
            })
            .collect();
        let dropped = dropped + n - kept.len();
        // 키 순서예요: kind, kind_source, title, summary, topics, items, dropped, ms, cost.
        let mut overview = parse_overview(&out.text, &ids);
        keep_attendee_refs(&mut overview, &names.iter().cloned().collect());
        let mut notes = json!({"kind": kind.as_str(), "kind_source": source});
        if let (Some(n), Some(o)) = (notes.as_object_mut(), overview.as_object()) {
            n.extend(o.clone());
        }
        notes["items"] = json!(kept);
        notes["dropped"] = json!(dropped);
        notes["ms"] = json!(classify_ms + out.ms);
        notes["cost"] = json!(classify_cost + out.cost);
        notes
    }

    /// 이번 회의의 결정이 지난 회의의 결정을 바꿨는지예요(docs/30 8절). `decisions`는 이번 결정 글, `past`는
    /// (날짜, 회의 제목, 결정 글)이고 앱이 최근 것부터 보내요. 번호는 0부터예요: `{"changes": [{"decision", "past"}],
    /// "ms", "cost"}`, 실패면 빈 목록과 `error`(·`error_kind`)예요. 이번 결정 하나에 지난 결정은 하나예요.
    pub async fn decision_changes(
        &self,
        decisions: &[String],
        past: &[(String, String, String)],
    ) -> Value {
        let decisions = &decisions[..decisions.len().min(LEDGER_PER_KIND)];
        let past = &past[..past.len().min(PAST_DECISIONS_MAX)];
        if decisions.is_empty() || past.is_empty() {
            return json!({"changes": [], "ms": 0, "cost": 0});
        }
        let clip = |t: &str| -> String { one_line(t).chars().take(LEDGER_TEXT_MAX).collect() };
        let new_lines = decisions
            .iter()
            .enumerate()
            .map(|(i, d)| format!("[{}] {}", i + 1, clip(d)))
            .collect::<Vec<_>>()
            .join("\n");
        let past_lines = past
            .iter()
            .enumerate()
            .map(|(i, (at, title, d))| {
                format!("[{}] {} · {} · {}", i + 1, clip(at), clip(title), clip(d))
            })
            .collect::<Vec<_>>()
            .join("\n");
        match self
            .services
            .note_writer
            .decision_changes(&new_lines, &past_lines)
            .await
        {
            Err(e) => json!({"changes": [], "error": head(&e.message), "error_kind": e.kind}),
            Ok(out) => {
                let past_texts: Vec<&str> = past.iter().map(|(_, _, d)| d.as_str()).collect();
                let changes = parse_changes(&out.text, decisions, &past_texts);
                json!({"changes": changes, "ms": out.ms, "cost": out.cost})
            }
        }
    }

    /// 회의에서 답을 찾지 못한 질문의 판정이에요(docs/24 4절): `questions`는 (질문 id, 나온 발화 id, 질문 글)이에요.
    /// 대화 기록은 회의 정리처럼 뒤 40000자까지예요. → `{"questions": [...], "ms", "cost"}`, 실패면 빈 목록과
    /// `error`(·`error_kind`)예요.
    pub async fn review_questions(
        &self,
        lines: &[(String, String)],
        attendees: &[(String, String)],
        questions: &[(String, Option<String>, String)],
    ) -> Value {
        let mut total: usize = lines.iter().map(|(_, l)| l.chars().count() + 1).sum();
        let mut start = 0;
        while start < lines.len() && total > LEDGER_CHARS {
            total -= lines[start].1.chars().count() + 1;
            start += 1;
        }
        let lines = &lines[start..];
        if lines.is_empty() || questions.is_empty() {
            return json!({"questions": [], "ms": 0, "cost": 0});
        }
        let transcript = lines
            .iter()
            .map(|(_, l)| l.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let mut names: Vec<String> = Vec::new();
        for (name, _) in attendees {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        let ids: HashSet<String> = lines.iter().map(|(id, _)| id.clone()).collect();
        // 줄이 있다고 한 질문인데 그 줄이 남은 대화 기록에 없으면(아주 긴 회의의 앞부분) 판정하지 않아요: 앞뒤 대화 없이
        // 보면 질문이 아니라고 잘못 버릴 수 있어요. 앱은 그 질문을 원문 그대로 둬요. id는 줄바꿈 없이 보내요.
        let questions: Vec<(String, Option<String>, String)> = questions
            .iter()
            .filter(|(_, line, _)| line.as_ref().is_none_or(|l| ids.contains(l)))
            .map(|(id, line, text)| (crate::notes::one_line(id), line.clone(), text.clone()))
            .collect();
        if questions.is_empty() {
            return json!({"questions": [], "ms": 0, "cost": 0});
        }
        let asked = questions
            .iter()
            .map(|(id, line, text)| match line {
                Some(l) => format!("- [{id}] 나온 발화 [{l}]: {}", crate::notes::one_line(text)),
                None => format!("- [{id}] {}", crate::notes::one_line(text)),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let out = match self
            .services
            .note_writer
            .review_questions(&transcript, &names, &asked)
            .await
        {
            Ok(out) => out,
            Err(e) => {
                return json!({"questions": [], "error": head(&e.message), "error_kind": e.kind});
            }
        };
        let wanted: HashMap<String, Option<String>> = questions
            .iter()
            .map(|(id, line, _)| (id.clone(), line.clone()))
            .collect();
        let Ok(mut reviewed) = parse_question_review(&out.text, &wanted, &ids) else {
            return json!({"questions": [], "error": "unreadable"});
        };
        // 답 글의 [[이름]] 중 참석자가 아닌 것은 괄호를 풀어요(요약과 같은 규칙). 다시 쓴 질문은 앱이 이름을 바꿔
        // 보이지 않는 글이라 괄호를 모두 풀어요.
        let attendees: HashSet<String> = names.into_iter().collect();
        for q in &mut reviewed {
            let mut answer = json!({"summary": [q["answer"].clone()]});
            keep_attendee_refs(&mut answer, &attendees);
            q["answer"] = answer["summary"][0].clone();
            let mut question = json!({"summary": [q["question"].clone()]});
            keep_attendee_refs(&mut question, &HashSet::new());
            q["question"] = question["summary"][0].clone();
        }
        json!({"questions": reviewed, "ms": out.ms, "cost": out.cost})
    }

    /// 갱신의 새 조각 벡터예요: 색인이 계속 답하는 동안 만들어요.
    /// `EMBED_BATCH`개마다 `/ask`와 같은 자물쇠를 잡고 문단 길이(`PASSAGE_TOKENS`)로 임베딩한 뒤, 놓고 나서
    /// `progress(끝난 수, 전체)`를 부르고 `EMBED_PAUSE`만큼 쉬어요(마지막 배치 뒤에도). 자물쇠는 공정하지 않아서 쉬지
    /// 않으면 다음 배치가 바로 다시 잡고, 질문이 갱신 전체를 기다려요(233 s, 2026-10-23). 블로킹이라 갱신
    /// 스레드에서 불러요. 추론이 실패하면 그 오류이고 남은 배치는 하지 않아요.
    ///
    /// GPU 임베더가 있으면 그쪽으로 `GPU_BATCH`개씩, 자물쇠와 쉬는 시간 없이 해요(벡터는 CPU와 같아요). GPU가 도중에
    /// 실패하면 남은 조각은 CPU로 해요.
    pub fn embed_passages(
        &self,
        texts: &[&str],
        progress: &mut dyn FnMut(usize, usize),
    ) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        let mut done = 0;
        if let Some(gpu) = self.gpu_embedder() {
            for batch in texts.chunks(GPU_BATCH) {
                match gpu.embed(batch, Some(PASSAGE_TOKENS)) {
                    Ok(vectors) => out.extend(vectors),
                    Err(e) => {
                        self.gpu_off(&e);
                        self.release_gpu();
                        break;
                    }
                }
                done += batch.len();
                progress(done, texts.len());
            }
            if done == texts.len() {
                return Ok(out);
            }
        }
        for batch in texts[done..].chunks(EMBED_BATCH) {
            let vectors = {
                let _one = self
                    .embed_lock
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                self.embedder.embed(batch, Some(PASSAGE_TOKENS))?
            };
            out.extend(vectors);
            done += batch.len();
            progress(done, texts.len());
            std::thread::sleep(EMBED_PAUSE);
        }
        Ok(out)
    }

    /// 새 회의 세션이에요: id는 `s1`, `s2`, ... 예요.
    pub fn new_session(&self) -> Arc<Session> {
        self.new_session_in(false)
    }

    /// 예시 자료로 하는 회의(온보딩 회의 연습)는 발화·카드·장부가 모두 예시 색인을 써요.
    pub fn new_session_in(&self, sample: bool) -> Arc<Session> {
        self.new_session_with(
            sample,
            false,
            Voices::tuned(Vec::new(), Vec::new(), self.voice_tuning()),
        )
    }

    /// 화자를 묶는 값이에요: 화자 임베딩 모델에 화자 나누기 모델이 있으면 그 값, 아니면 ERes2Net 값이에요.
    pub fn voice_tuning(&self) -> crate::voices::Tuning {
        crate::voices::Tuning::for_dim(self.voice.as_ref().and_then(|v| v.diar_dim()))
    }

    /// `gate_empty`: 빈 색인에도 판단해요(`Session::gate_empty`). `voices`: 기억한 사람들과, 엔진이 다시 떴을 때
    /// 앱이 다시 넘긴 목소리예요.
    pub fn new_session_with(&self, sample: bool, gate_empty: bool, voices: Voices) -> Arc<Session> {
        let n = self.next_session.fetch_add(1, Ordering::Relaxed);
        let session = Arc::new(Session::new(format!("s{n}"), sample, gate_empty, voices));
        // 첫 발화 전에 용어를 뽑고, 표기가 없으면 미리 받아 둬요.
        if !sample {
            self.term_fixer();
        }
        self.session_table()
            .insert(session.id.clone(), session.clone());
        session
    }

    pub fn session(&self, sid: &str) -> Option<Arc<Session>> {
        self.session_table().get(sid).cloned()
    }

    /// 세션을 닫아요. 그런 세션이 없었으면 false예요.
    pub fn drop_session(&self, sid: &str) -> bool {
        self.session_table().remove(sid).is_some()
    }

    /// 다른 스레드가 잡은 채 죽었어도 표는 그대로 써요(넣고 빼기만 해서 깨질 게 없어요).
    fn session_table(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Session>>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 임베딩은 블로킹이라 블로킹 스레드에서, 한 번에 하나씩 해요.
    async fn candidates(
        &self,
        ix: &Arc<Index>,
        text: &str,
        context: &[String],
        line: &str,
        only: Option<&HashSet<String>>,
    ) -> anyhow::Result<Vec<usize>> {
        let (ix, embedder, lock) = (ix.clone(), self.embedder.clone(), self.embed_lock.clone());
        let (text, context, line) = (text.to_string(), context.to_vec(), line.to_string());
        let only = only.cloned();
        tokio::task::spawn_blocking(move || {
            let _one = lock.lock().unwrap_or_else(PoisonError::into_inner);
            // 범위가 있으면 그 문서의 조각 안에서 순위를 매겨요: 전체 위 50개를 거르면 회의 조각이 밀려나 빠져요.
            match &only {
                None => ix.candidates(embedder.as_ref(), &text, &context, &line),
                Some(o) => ix.candidates_where(embedder.as_ref(), &text, &context, &line, &|id| {
                    o.contains(&ix.chunks[id].path)
                }),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("search thread failed: {e}"))?
    }
}

/// 답 줄은 보여 준 카드를 근거로 달았을 때만 믿어요: (답 또는 None, 근거 번호 1..n, 버린 이유).
/// [1, 3]처럼 묶은 번호는 앱이 위첨자로 바꾸는 [1][3] 꼴로 돌려줘요.
pub fn check_answer(text: &str, n: usize) -> (Option<String>, Vec<usize>, Option<&'static str>) {
    if text.starts_with(NO_ANSWER) {
        return (Some(text.to_string()), vec![], None);
    }
    let text = QUESTION_ECHO.replace(text, "");
    let text = text.as_ref();
    let phrase = SOURCE_PHRASE.find(text).map(|m| m.as_str());
    let rest = &text[phrase.map_or(0, str::len)..];

    // [밖, 코드, 밖, 코드, ..., 밖]으로 나눠서 코드 구간 밖에서만 번호를 읽어요.
    let mut parts = Vec::new();
    let mut at = 0;
    for m in CODE_SPAN.find_iter(rest) {
        parts.push(&rest[at..m.start()]);
        parts.push(m.as_str());
        at = m.end();
    }
    parts.push(&rest[at..]);
    let mut cited: Vec<usize> = Vec::new();
    let mut joined = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i % 2 == 1 {
            joined.push_str(part);
            continue;
        }
        for m in CITATION.captures_iter(part) {
            for x in m[1].split(',') {
                let c = int(x);
                if !cited.contains(&c) {
                    cited.push(c);
                }
            }
        }
        let split = |m: &Captures| -> String {
            m[1].split(',').map(|x| format!("[{}]", x.trim())).collect()
        };
        joined.push_str(&CITATION.replace_all(part, split));
    }
    let mut text = joined;

    // 버린 문구가 이 줄의 근거를 말했어요: 그 번호는 끝에 표시로 돌아와요.
    if let Some(phrase) = phrase {
        let mut extra: Vec<usize> = Vec::new();
        for m in DIGITS.find_iter(phrase) {
            let c = int(m.as_str());
            if !extra.contains(&c) && !cited.contains(&c) {
                extra.push(c);
            }
        }
        if !extra.is_empty() {
            let marks: String = extra.iter().map(|c| format!("[{c}]")).collect();
            text = format!("{} {marks}", text.trim_end());
            cited.extend(extra);
        }
    }
    if cited.is_empty() || cited.iter().any(|c| !(1..=n).contains(c)) {
        // 정해 둔 문장이 잡글이나 따옴표 뒤에 있어요: 그래도 카드가 답하지 못하는 질문이에요.
        if text.contains(NO_ANSWER) {
            return (Some(format!("{NO_ANSWER}.")), vec![], None);
        }
        return (None, vec![], Some("citation"));
    }
    if SOURCE_TALK.is_match(&CODE_SPAN.replace_all(&text, "")) {
        return (None, vec![], Some("source"));
    }
    if text.chars().count() > crate::lang::cap(ANSWER_MAX, &text) {
        return (None, vec![], Some("length"));
    }
    (Some(text), cited, None)
}

/// 앞뒤 공백을 넘겨 읽어요. ASCII가 아닌 숫자(١ 같은)나 넘치는 수는 어떤 자료 번호도 아니라서
/// 범위 밖 값이에요(모델이 그렇게 쓰는 일은 없어요).
fn int(s: &str) -> usize {
    s.trim().parse().unwrap_or(usize::MAX)
}

/// 체크아웃 origin의 GitHub owner/name이에요(색인 meta의 `repo`와 같은 꼴). 못 읽으면 None이에요.
/// `Engine::with_repo_dir`가 엔진이 뜰 때 한 번 불러, 카드를 로컬 체크아웃으로 열지 정해요(스펙 6-4).
/// git을 띄우지 않고 파일만 읽어요: `.git`이 폴더면 그 `config`, 파일(워크트리)이면 `gitdir:` 폴더의 `commondir`
/// (있으면)의 `config`예요. `[remote "origin"]` 절의 첫 `url`이고, `insteadOf`는 따라가지 않아요.
pub fn checkout_repo(repo_dir: &Path) -> Option<String> {
    let git = repo_dir.join(".git");
    let git_dir = if git.is_dir() {
        git
    } else {
        let text = std::fs::read_to_string(&git).ok()?;
        let target = text.trim().strip_prefix("gitdir:")?.trim();
        let dir = repo_dir.join(target); // 절대 경로면 그것이에요
        match std::fs::read_to_string(dir.join("commondir")) {
            Ok(common) if !common.trim().is_empty() => dir.join(common.trim()),
            _ => dir,
        }
    };
    let config = std::fs::read_to_string(git_dir.join("config")).ok()?;
    let url = origin_url(&config)?;
    let repo = GITHUB_PREFIX.replace_all(&url, "").into_owned();
    (!repo.is_empty()).then_some(repo)
}

/// git config 글에서 `[remote "origin"]` 절의 첫 `url` 값이에요. 절 이름과 키는 대소문자를 가리지 않고, 하위 절
/// 이름(`origin`)은 가려요. `#`·`;` 뒤는 주석이고, 따옴표 안의 것은 값이에요.
fn origin_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('[') {
            let header = rest.split(']').next().unwrap_or_default().trim();
            in_origin = match header.split_once(char::is_whitespace) {
                Some((section, sub)) => {
                    section.eq_ignore_ascii_case("remote") && sub.trim() == "\"origin\""
                }
                None => false,
            };
            continue;
        }
        if !in_origin {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("url") {
            let value = config_value(value);
            return (!value.is_empty()).then_some(value);
        }
    }
    None
}

/// git config 값 하나예요: 앞뒤 공백을 떼고, 따옴표를 풀고(`\"`, `\\` 포함), 따옴표 밖의 `#`·`;`부터는 버려요.
fn config_value(raw: &str) -> String {
    let mut out = String::new();
    let (mut quoted, mut chars) = (false, raw.trim().chars());
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            '#' | ';' if !quoted => break,
            c => out.push(c),
        }
    }
    out.trim().to_string()
}

fn last_three(context: &[String]) -> &[String] {
    &context[context.len().saturating_sub(3)..]
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// 오류 글은 앞 200자만 실어요.
pub(crate) fn head(message: &str) -> String {
    message.chars().take(200).collect()
}

/// 답 모델에 보내는 조각 글이에요. 머리 줄(`meta:` 같은 것)은 빼지만, 회의 조각은 `meta` 줄("회의 · 2026-11-03")을
/// 맨 앞에 남겨요: 같은 일을 다르게 정한 회의가 여럿 걸리면 답 모델이 날짜로 나중 결정을 골라요.
pub fn passage_text(src: &str, head: &HashMap<String, String>, body: &[String]) -> String {
    let body = body.join("\n");
    match head.get("meta").filter(|_| src == "meeting") {
        Some(meta) => format!("{meta}\n{body}"),
        None => body,
    }
}

/// 지난 결정은 이만큼까지 비교해요(최근 것부터). 결정 한 줄이 200자까지라 4만 자 안쪽이에요.
pub const PAST_DECISIONS_MAX: usize = 200;

/// 모델이 쓴 `{"changes": [{"new", "past"}]}`(1부터)를 0부터인 `[{"decision", "past"}]`로 읽어요. 범위 밖 번호와
/// 이미 짝이 있는 이번 결정은 버려요.
pub fn parse_changes(raw: &str, new: &[String], past: &[&str]) -> Vec<Value> {
    let Some(obj) = crate::notes::first_object(raw) else {
        return Vec::new();
    };
    let (n_new, n_past) = (new.len(), past.len());
    let mut seen = HashSet::new();
    let pairs: Vec<(usize, usize)> = obj
        .get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let n = c.get("new")?.as_u64()? as usize;
            let p = c.get("past")?.as_u64()? as usize;
            let subject = c.get("subject")?.as_str()?;
            ((1..=n_new).contains(&n)
                && (1..=n_past).contains(&p)
                // 같은 대상이면 그 낱말이 두 결정에 다 나와요: 관계없는 짝을 모델이 지어낸 일이 있어요(docs/38 12-7절).
                && shares_subject(subject, &new[n - 1], past[p - 1])
                && seen.insert(n))
            .then_some((n, p))
        })
        .collect();
    // 지난 결정 하나에 이번 결정 셋 이상이 붙으면 모델이 한 지난 결정에 다 몰아 붙인 경우였어요: 그 짝은 버려요.
    let mut per_past: HashMap<usize, usize> = HashMap::new();
    for (_, p) in &pairs {
        *per_past.entry(*p).or_default() += 1;
    }
    pairs
        .into_iter()
        .filter(|(_, p)| per_past[p] < 3)
        .map(|(n, p)| json!({"decision": n - 1, "past": p - 1}))
        .collect()
}

/// `subject`("재시도", 또는 "catalog / 카탈로그")의 낱말이 두 결정에 각각 나오는지예요. 조사가 붙어도 되게 낱말 끝 한
/// 글자는 빼고 봐요(두 글자는 남겨요). 이번 결정과 지난 결정에 나온 낱말은 달라도 돼요(언어가 다를 때).
fn shares_subject(subject: &str, a: &str, b: &str) -> bool {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let stems: Vec<String> = subject
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .map(|w| {
            let w = w.to_lowercase();
            let n = w.chars().count();
            w.chars().take(if n > 2 { n - 1 } else { n }).collect()
        })
        .collect();
    if stems.iter().any(|s| a.contains(s.as_str())) && stems.iter().any(|s| b.contains(s.as_str())) {
        return true;
    }
    // 모델이 subject를 번역해 버린 경우(한국어 결정과 영어 결정): 두 결정에 같은 로마자 이름(CJ, Gusto, SQS)이 있으면 같은 대상이에요.
    let names = |t: &str| -> HashSet<String> {
        t.split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| w.len() >= 2 && w.chars().any(|c| c.is_ascii_alphabetic()))
            .map(str::to_string)
            .collect()
    };
    !names(&a).is_disjoint(&names(&b))
}
