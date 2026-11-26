//! Pipeline fakes: an embedder that gives fixed vectors, a reranker that gives preset p values or errors, an answer
//! model that gives preset lines or errors, a gate that gives preset decisions, a transcriber that gives preset
//! text in order, and a ledger model that gives preset text.
//! They keep the arguments they receive so tests can see what was passed. `services` bundles these fakes into
//! `Services`.

#![allow(dead_code)] // 테스트 파일마다 쓰는 가짜가 달라요

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use tacie_embed::{DIM, Embed};
use tacie_engine::errors::{Kind, ServiceError};
use tacie_engine::jev::{Gate, Rerank};
use tacie_engine::notes_kind::NotesKind;
use tacie_engine::prompts::Prompt;
use tacie_engine::services::{
    AnswerOut, Answerer, BoxFuture, ChatTurn, FollowOut, Gater, Hit, LedgerOut, MeetingSearch, NoteWriter, PanelWriter, PrepOut, Reranker,
    Services, Transcriber, WhyOut,
    Transcript,
};

/// 모든 글에 [4, 3, 2, 1, 0, ...]/√30을 줘요. 행이 e0, e1, e2, e3인 색인이면 dense 순서는 0, 1, 2, 3이에요.
/// `fail`이면 추론이 실패한 것처럼 오류를 내요. 부를 때마다 받은 글들과 `max_tokens`를 쌓아 두고(첫 글이 찾는
/// 말이에요), `delay`만큼 걸려요(추론 시간이에요). `spans`는 부를 때마다의 (시작, 끝) 시각이에요.
#[derive(Default)]
pub struct FakeEmbed {
    pub calls: AtomicUsize,
    pub fail: bool,
    pub delay: Duration,
    pub texts: Mutex<Vec<Vec<String>>>,
    pub max_tokens: Mutex<Vec<Option<usize>>>,
    pub spans: Mutex<Vec<(Instant, Instant)>>,
}

impl FakeEmbed {
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn texts(&self) -> Vec<Vec<String>> {
        self.texts.lock().unwrap().clone()
    }

    pub fn max_tokens(&self) -> Vec<Option<usize>> {
        self.max_tokens.lock().unwrap().clone()
    }

    pub fn spans(&self) -> Vec<(Instant, Instant)> {
        self.spans.lock().unwrap().clone()
    }
}

impl Embed for FakeEmbed {
    fn embed(&self, texts: &[&str], max_tokens: Option<usize>) -> anyhow::Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.texts
            .lock()
            .unwrap()
            .push(texts.iter().map(|t| t.to_string()).collect());
        self.max_tokens.lock().unwrap().push(max_tokens);
        let start = Instant::now();
        std::thread::sleep(self.delay);
        self.spans.lock().unwrap().push((start, Instant::now()));
        anyhow::ensure!(!self.fail, "onnx run failed");
        let mut v = vec![0.0f32; DIM];
        for (i, x) in [4.0f32, 3.0, 2.0, 1.0].into_iter().enumerate() {
            v[i] = x / 30f32.sqrt();
        }
        Ok(texts.iter().map(|_| v.clone()).collect())
    }
}

/// 가짜 임베더예요: 글마다 `crc32(글) % 1024` 자리만 1.0이에요. 같은 글은 같은 벡터라 다시 쓴 벡터와 새로
/// 만든 벡터가 구별되지 않아요(갱신 테스트는 몇 번 불렸는지와 받은 글만 봐요). 부를 때마다 받은 글을 쌓아요.
#[derive(Default)]
pub struct HashEmbed {
    pub texts: Mutex<Vec<Vec<String>>>,
}

impl HashEmbed {
    /// 부른 수예요.
    pub fn calls(&self) -> usize {
        self.texts.lock().unwrap().len()
    }

    /// 임베딩한 글 수예요(모든 호출을 더해요).
    pub fn embedded(&self) -> usize {
        self.texts.lock().unwrap().iter().map(Vec::len).sum()
    }
}

impl Embed for HashEmbed {
    fn embed(&self, texts: &[&str], _max_tokens: Option<usize>) -> anyhow::Result<Vec<Vec<f32>>> {
        self.texts
            .lock()
            .unwrap()
            .push(texts.iter().map(|t| t.to_string()).collect());
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0.0f32; DIM];
                v[crc32fast::hash(t.as_bytes()) as usize % DIM] = 1.0;
                v
            })
            .collect())
    }
}

/// (앞 대화, 이번 줄, 조각들)
pub type RerankCall = (Vec<String>, String, Vec<String>);

pub struct FakeReranker {
    result: Result<Vec<f64>, (Kind, String)>,
    pub calls: Mutex<Vec<RerankCall>>,
}

impl FakeReranker {
    pub fn ok(p: &[f64]) -> Self {
        Self {
            result: Ok(p.to_vec()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// 조각 수만큼 같은 p를 줘요.
    pub fn all(p: f64) -> Self {
        Self::ok(&[p; 64])
    }

    pub fn fail(kind: Kind, message: &str) -> Self {
        Self {
            result: Err((kind, message.to_string())),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<RerankCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl Reranker for FakeReranker {
    fn rerank<'a>(
        &'a self,
        context: &'a [String],
        line: &'a str,
        passages: &'a [String],
    ) -> BoxFuture<'a, Result<Rerank, ServiceError>> {
        self.calls
            .lock()
            .unwrap()
            .push((context.to_vec(), line.to_string(), passages.to_vec()));
        let out = match &self.result {
            Ok(p) => Ok(Rerank {
                p: p[..passages.len().min(p.len())].to_vec(),
                ms: 5.0,
                usage: json!({"input_tokens": 50000, "cost": 0.0021}),
            }),
            Err((kind, message)) => Err(ServiceError {
                kind: *kind,
                message: message.clone(),
            }),
        };
        Box::pin(async move { out })
    }
}

/// (질문, 앞 대화, 자료들)
pub type AnswerCall = (String, Vec<String>, Vec<Value>);

pub struct FakeAnswerer {
    result: Result<String, (Kind, String)>,
    pub calls: Mutex<Vec<AnswerCall>>,
}

impl FakeAnswerer {
    pub fn ok(text: &str) -> Self {
        Self {
            result: Ok(text.to_string()),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn fail(kind: Kind, message: &str) -> Self {
        Self {
            result: Err((kind, message.to_string())),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<AnswerCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl Answerer for FakeAnswerer {
    fn answer<'a>(
        &'a self,
        question: &'a str,
        context: &'a [String],
        passages: &'a [Value],
    ) -> BoxFuture<'a, Result<AnswerOut, ServiceError>> {
        self.calls.lock().unwrap().push((
            question.to_string(),
            context.to_vec(),
            passages.to_vec(),
        ));
        let out = match &self.result {
            Ok(text) => Ok(AnswerOut {
                answer: text.clone(),
                ms: 1.0,
                cost: 0.0004,
            }),
            Err((kind, message)) => Err(ServiceError {
                kind: *kind,
                message: message.clone(),
            }),
        };
        Box::pin(async move { out })
    }
}

/// (앞 대화, 이번 줄)
pub type GateCall = (Vec<String>, String);
type GateFn = Box<dyn Fn(&[String], &str) -> Result<Gate, ServiceError> + Send + Sync>;

/// 판단 가짜예요. 받은 (앞 대화, 이번 줄)을 쌓아 둬요.
/// `ok`, `ending`, `fail`은 늘 같은 답을, `from_fn`은 줄마다 다른 답을 줘요.
/// `set_delay`면 그만큼 기다렸다가 답해요(기다리는 중에 부른 쪽이 그만두는 경우를 볼 때).
pub struct FakeGater {
    answer: GateFn,
    delay: Mutex<Duration>,
    pub calls: Mutex<Vec<GateCall>>,
}

impl FakeGater {
    /// p_end가 없는 판단이에요(진짜 Jev는 늘 주지만 가짜는 빼도 돼요).
    pub fn ok(p_lookup: f64) -> Self {
        Self::ending(p_lookup, None)
    }

    pub fn ending(p_lookup: f64, p_end: Option<f64>) -> Self {
        Self::from_fn(move |_, _| Ok(fake_gate(p_lookup, p_end)))
    }

    pub fn fail(kind: Kind, message: &str) -> Self {
        let message = message.to_string();
        Self::from_fn(move |_, _| {
            Err(ServiceError {
                kind,
                message: message.clone(),
            })
        })
    }

    /// (앞 대화, 이번 줄)로 답을 정해요. 답은 `fake_gate`로 만들면 돼요.
    pub fn from_fn(
        f: impl Fn(&[String], &str) -> Result<Gate, ServiceError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            answer: Box::new(f),
            delay: Mutex::new(Duration::ZERO),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// 다음 호출부터 이만큼 기다렸다가 답해요.
    pub fn set_delay(&self, delay: Duration) {
        *self.delay.lock().unwrap() = delay;
    }

    pub fn calls(&self) -> Vec<GateCall> {
        self.calls.lock().unwrap().clone()
    }
}

/// 가짜 판단 값이에요: ms 1, 출처 확률과 usage는 비어 있어요.
pub fn fake_gate(p_lookup: f64, p_end: Option<f64>) -> Gate {
    Gate {
        p_lookup,
        source: Map::new(),
        p_end,
        ms: 1.0,
        usage: json!({}),
    }
}

impl Gater for FakeGater {
    fn gate<'a>(
        &'a self,
        context: &'a [String],
        line: &'a str,
    ) -> BoxFuture<'a, Result<Gate, ServiceError>> {
        self.calls
            .lock()
            .unwrap()
            .push((context.to_vec(), line.to_string()));
        let out = (self.answer)(context, line);
        let delay = *self.delay.lock().unwrap();
        Box::pin(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            out
        })
    }
}

/// 받아쓰기 가짜예요. 부를 때마다 `script`의 결과를 차례로 주고, 다 쓰면 마지막 것을
/// 계속 줘요. 받은 WAV 바이트를 쌓아 둬요. `with_delay`면 그만큼 기다렸다가 답해요(장부가 받아쓰기를 기다리는지 볼 때).
pub struct FakeTranscriber {
    script: Vec<Result<Transcript, (Kind, String)>>,
    delay: Duration,
    pub calls: Mutex<Vec<Vec<u8>>>,
}

impl FakeTranscriber {
    /// 늘 이 글이에요(confidence 없음).
    pub fn ok(text: &str) -> Self {
        Self::script(vec![Ok(transcript(text, None))])
    }

    pub fn fail(kind: Kind, message: &str) -> Self {
        Self::script(vec![Err((kind, message.to_string()))])
    }

    /// 차례로 줄 결과예요. 비어 있으면 안 돼요.
    pub fn script(results: Vec<Result<Transcript, (Kind, String)>>) -> Self {
        assert!(
            !results.is_empty(),
            "FakeTranscriber needs at least one result"
        );
        Self {
            script: results,
            delay: Duration::ZERO,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn calls(&self) -> Vec<Vec<u8>> {
        self.calls.lock().unwrap().clone()
    }
}

/// 가짜 받아쓰기 값이에요: ms 1, 비용 0.
pub fn transcript(text: &str, confidence: Option<f64>) -> Transcript {
    Transcript {
        text: text.to_string(),
        ms: 1.0,
        cost: 0.0,
        confidence,
        words: vec![],
        via: tacie_engine::services::SttVia::Server,
    }
}

impl Transcriber for FakeTranscriber {
    fn transcribe<'a>(&'a self, wav: &'a [u8]) -> BoxFuture<'a, Result<Transcript, ServiceError>> {
        let n = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(wav.to_vec());
            calls.len()
        };
        let out = match &self.script[(n - 1).min(self.script.len() - 1)] {
            Ok(t) => Ok(t.clone()),
            Err((kind, message)) => Err(ServiceError {
                kind: *kind,
                message: message.clone(),
            }),
        };
        let delay = self.delay;
        Box::pin(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            out
        })
    }
}

/// 장부 모델 가짜예요. 정해 둔 글(모델이 쓴 JSON 글 자리)이나 오류를 주고,
/// 받은 회의 글과 참석자를 쌓아 둬요.
pub struct FakeNoteWriter {
    result: Result<String, (Kind, String)>,
    pub calls: Mutex<Vec<String>>,
    pub attendees: Mutex<Vec<Vec<String>>>,
    /// 분류 답(ms 2.0, cost 0.005)이에요. 없으면 분류가 실패해요(그러면 엔진은 회의로 써요).
    classify: Option<String>,
    /// 장부를 쓸 때 받은 유형이에요.
    pub kinds: Mutex<Vec<NotesKind>>,
    /// 분류에 받은 앞부분이에요.
    pub classify_calls: Mutex<Vec<String>>,
    /// 질문 판정에 받은 (대화 기록, 질문 줄)이에요. 답은 장부와 같은 `result`예요.
    pub reviews: Mutex<Vec<(String, String)>>,
    /// 바뀐 결정 고르기에 받은 (이번 결정 줄, 지난 결정 줄)이에요. 답은 장부와 같은 `result`예요.
    pub changes: Mutex<Vec<(String, String)>>,
}

impl FakeNoteWriter {
    pub fn ok(text: &str) -> Self {
        Self {
            result: Ok(text.to_string()),
            calls: Mutex::new(Vec::new()),
            attendees: Mutex::new(Vec::new()),
            classify: None,
            kinds: Mutex::new(Vec::new()),
            classify_calls: Mutex::new(Vec::new()),
            reviews: Mutex::new(Vec::new()),
            changes: Mutex::new(Vec::new()),
        }
    }

    pub fn fail(kind: Kind, message: &str) -> Self {
        Self {
            result: Err((kind, message.to_string())),
            calls: Mutex::new(Vec::new()),
            attendees: Mutex::new(Vec::new()),
            classify: None,
            kinds: Mutex::new(Vec::new()),
            classify_calls: Mutex::new(Vec::new()),
            reviews: Mutex::new(Vec::new()),
            changes: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub fn with_classify(mut self, text: &str) -> Self {
        self.classify = Some(text.to_string());
        self
    }

    pub fn classify_fail(mut self) -> Self {
        self.classify = None;
        self
    }

    pub fn attendee_calls(&self) -> Vec<Vec<String>> {
        self.attendees.lock().unwrap().clone()
    }
}

impl NoteWriter for FakeNoteWriter {
    fn decision_changes<'a>(
        &'a self,
        new_decisions: &'a str,
        past_decisions: &'a str,
    ) -> BoxFuture<'a, Result<LedgerOut, ServiceError>> {
        self.changes
            .lock()
            .unwrap()
            .push((new_decisions.to_string(), past_decisions.to_string()));
        let out = match &self.result {
            Ok(text) => Ok(LedgerOut {
                text: text.clone(),
                ms: 1.0,
                cost: 0.01,
            }),
            Err((kind, message)) => Err(ServiceError {
                kind: *kind,
                message: message.clone(),
            }),
        };
        Box::pin(async move { out })
    }

    fn review_questions<'a>(
        &'a self,
        transcript: &'a str,
        _attendees: &'a [String],
        questions: &'a str,
    ) -> BoxFuture<'a, Result<LedgerOut, ServiceError>> {
        self.reviews
            .lock()
            .unwrap()
            .push((transcript.to_string(), questions.to_string()));
        let out = match &self.result {
            Ok(text) => Ok(LedgerOut {
                text: text.clone(),
                ms: 1.0,
                cost: 0.01,
            }),
            Err((kind, message)) => Err(ServiceError {
                kind: *kind,
                message: message.clone(),
            }),
        };
        Box::pin(async move { out })
    }

    fn ledger<'a>(
        &'a self,
        transcript: &'a str,
        attendees: &'a [String],
        kind: NotesKind,
    ) -> BoxFuture<'a, Result<LedgerOut, ServiceError>> {
        self.kinds.lock().unwrap().push(kind);
        self.calls.lock().unwrap().push(transcript.to_string());
        self.attendees.lock().unwrap().push(attendees.to_vec());
        let out = match &self.result {
            Ok(text) => Ok(LedgerOut {
                text: text.clone(),
                ms: 1.0,
                cost: 0.01,
            }),
            Err((kind, message)) => Err(ServiceError {
                kind: *kind,
                message: message.clone(),
            }),
        };
        Box::pin(async move { out })
    }

    fn classify<'a>(&'a self, sample: &'a str) -> BoxFuture<'a, Result<LedgerOut, ServiceError>> {
        self.classify_calls.lock().unwrap().push(sample.to_string());
        let out = match &self.classify {
            Some(text) => Ok(LedgerOut {
                text: text.clone(),
                ms: 2.0,
                cost: 0.005,
            }),
            None => Err(ServiceError {
                kind: Kind::Down,
                message: "no classifier".into(),
            }),
        };
        Box::pin(async move { out })
    }
}

/// 리랭커와 답 모델을 넣은 서비스 묶음이에요. 판단, 받아쓰기, 장부는 부르면 down("not faked")으로 실패해요:
/// 그게 필요한 테스트는 `Services { gater: g.clone(), ..services(r, a) }`처럼 바꿔 끼워요.
pub fn services(reranker: Arc<FakeReranker>, answerer: Arc<FakeAnswerer>) -> Services {
    Services {
        reranker,
        gater: Arc::new(FakeGater::fail(Kind::Down, "not faked")),
        answerer,
        transcriber: Arc::new(FakeTranscriber::fail(Kind::Down, "not faked")),
        note_writer: Arc::new(FakeNoteWriter::fail(Kind::Down, "not faked")),
        panel: Arc::new(FakePanel),
        meeting_search: None,
    }
}

/// 패널 가짜예요: 부르면 down("not faked")으로 실패해요. 패널을 쓰는 테스트는 `panel`을 바꿔 끼워요.
pub struct FakePanel;

fn not_faked<T>() -> BoxFuture<'static, Result<T, ServiceError>> {
    Box::pin(async {
        Err(ServiceError {
            kind: Kind::Down,
            message: "not faked".into(),
        })
    })
}

impl PanelWriter for FakePanel {
    fn prep<'a>(&'a self, _: &'a [Value], _: &'a [Value]) -> BoxFuture<'a, Result<PrepOut, ServiceError>> {
        not_faked()
    }
    fn why<'a>(&'a self, _: &'a str, _: &'a [Value]) -> BoxFuture<'a, Result<WhyOut, ServiceError>> {
        not_faked()
    }
    fn followup<'a>(&'a self, _: &'a [Value], _: &'a [Value]) -> BoxFuture<'a, Result<FollowOut, ServiceError>> {
        not_faked()
    }
}

/// 화자 임베딩 가짜예요. WAV 바이트마다 정해 둔 임베딩을 주고, 없는 WAV는 None이에요. `failing`이면 늘 오류예요.
pub struct FakeVoice {
    prints: std::collections::HashMap<Vec<u8>, tacie_embed::voice::Voiceprint>,
    /// `by_level`: 샘플 평균이 가장 가까운 값의 벡터예요(창마다 다른 목소리를 흉내 내요).
    levels: Vec<(i16, Vec<f32>)>,
    fail: bool,
    /// 임베딩을 뽑은 횟수예요.
    pub calls: AtomicUsize,
}

impl FakeVoice {
    pub fn by_wav(pairs: &[(&[u8], &[f32], f32)]) -> Self {
        let prints = pairs
            .iter()
            .map(|(wav, v, seconds)| {
                let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                let print = tacie_embed::voice::Voiceprint {
                    vector: v.iter().map(|x| x / n).collect(),
                    seconds: *seconds,
                    ident: None,
                };
                (wav.to_vec(), print)
            })
            .collect();
        Self {
            prints,
            levels: Vec::new(),
            fail: false,
            calls: AtomicUsize::new(0),
        }
    }

    /// 실제 WAV를 읽어 샘플 평균이 가장 가까운 값의 벡터를 줘요. 길이는 WAV 길이예요.
    pub fn by_level(pairs: &[(i16, &[f32])]) -> Self {
        Self {
            prints: Default::default(),
            levels: pairs.iter().map(|(l, v)| (*l, v.to_vec())).collect(),
            fail: false,
            calls: AtomicUsize::new(0),
        }
    }

    pub fn failing() -> Self {
        Self {
            prints: Default::default(),
            levels: Vec::new(),
            fail: true,
            calls: AtomicUsize::new(0),
        }
    }
}

impl tacie_embed::voice::VoiceEmbed for FakeVoice {
    fn embed(&self, wav: &[u8]) -> anyhow::Result<Option<tacie_embed::voice::Voiceprint>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            anyhow::bail!("voice model failed");
        }
        if self.levels.is_empty() {
            return Ok(self.prints.get(wav).cloned());
        }
        let samples = tacie_embed::voice::pcm16_mono(wav)?;
        if samples.is_empty() {
            return Ok(None);
        }
        let mean = samples.iter().sum::<f32>() / samples.len() as f32 * 32768.0;
        let (_, v) = self
            .levels
            .iter()
            .min_by(|a, b| {
                (f32::from(a.0) - mean)
                    .abs()
                    .total_cmp(&(f32::from(b.0) - mean).abs())
            })
            .unwrap();
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        Ok(Some(tacie_embed::voice::Voiceprint {
            vector: v.iter().map(|x| x / n).collect(),
            seconds: samples.len() as f32 / 16000.0,
            ident: None,
        }))
    }
}

/// 정해 둔 턴을 차례로 주는 채팅 가짜예요(`chat_turn`). 부를 때마다 (프롬프트 이름, 도구 묶음)과 받은 턴, 프롬프트의
/// `scope`를 쌓아 둬요. 턴이 떨어지면 빈 턴을 줘요.
pub struct ScriptedPanel {
    turns: Mutex<std::collections::VecDeque<ChatTurn>>,
    seen: Mutex<Vec<ScriptedCall>>,
}

#[derive(Clone)]
pub struct ScriptedCall {
    pub name: &'static str,
    pub tools: &'static str,
    pub turns: Vec<Value>,
    pub scope: String,
}

impl ScriptedPanel {
    pub fn new(turns: Vec<ChatTurn>) -> Arc<Self> {
        Arc::new(Self { turns: Mutex::new(turns.into()), seen: Mutex::new(vec![]) })
    }

    pub fn calls(&self) -> Vec<(&'static str, &'static str)> {
        self.seen.lock().unwrap().iter().map(|c| (c.name, c.tools)).collect()
    }

    pub fn seen(&self) -> Vec<ScriptedCall> {
        self.seen.lock().unwrap().clone()
    }
}

impl PanelWriter for ScriptedPanel {
    fn prep<'a>(&'a self, _: &'a [Value], _: &'a [Value]) -> BoxFuture<'a, Result<PrepOut, ServiceError>> {
        not_faked()
    }
    fn why<'a>(&'a self, _: &'a str, _: &'a [Value]) -> BoxFuture<'a, Result<WhyOut, ServiceError>> {
        not_faked()
    }
    fn followup<'a>(&'a self, _: &'a [Value], _: &'a [Value]) -> BoxFuture<'a, Result<FollowOut, ServiceError>> {
        not_faked()
    }
    fn chat_turn<'a>(
        &'a self,
        prompt: Prompt,
        tools: &'static str,
        turns: &'a [Value],
        _choice: &'static str,
    ) -> BoxFuture<'a, Result<ChatTurn, ServiceError>> {
        let scope = prompt.fields.iter().find(|(k, _)| *k == "scope").map(|(_, v)| v.clone()).unwrap_or_default();
        self.seen.lock().unwrap().push(ScriptedCall { name: prompt.name, tools, turns: turns.to_vec(), scope });
        let turn = self.turns.lock().unwrap().pop_front().unwrap_or_default();
        Box::pin(async move { Ok(turn) })
    }
}

/// 회의 검색 가짜예요: 질문마다 같은 회의들(`ok`)을 주거나 정해 둔 종류로 실패해요(`fail`). 부를 때마다 받은 질문들을
/// 쌓아 둬요.
pub struct FakeMeetingSearch {
    reply: Result<Vec<Hit>, Kind>,
    seen: Mutex<Vec<Vec<String>>>,
}

impl FakeMeetingSearch {
    pub fn ok(ids: &[&str]) -> Arc<Self> {
        let hits = ids.iter().enumerate().map(|(i, id)| Hit { id: id.to_string(), score: 0.9 - i as f64 * 0.1 }).collect();
        Arc::new(Self { reply: Ok(hits), seen: Mutex::new(vec![]) })
    }

    pub fn fail(kind: Kind) -> Arc<Self> {
        Arc::new(Self { reply: Err(kind), seen: Mutex::new(vec![]) })
    }

    /// 부를 때마다 받은 질문들이에요.
    pub fn calls(&self) -> Vec<Vec<String>> {
        self.seen.lock().unwrap().clone()
    }
}

impl MeetingSearch for FakeMeetingSearch {
    fn search<'a>(&'a self, queries: &'a [String], _top: usize) -> BoxFuture<'a, Result<Vec<Vec<Hit>>, ServiceError>> {
        self.seen.lock().unwrap().push(queries.to_vec());
        let out = match &self.reply {
            Ok(hits) => Ok(queries.iter().map(|_| hits.clone()).collect()),
            Err(kind) => Err(ServiceError { kind: *kind, message: "fake meeting search".into() }),
        };
        Box::pin(async move { out })
    }
}
