//! Outside services the engine calls (gate, rerank, answer, transcription, ledger). In the app these are Jev (gate,
//! rerank) and OpenRouter (answer, transcription, ledger); tests plug in fakes.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use crate::errors::ServiceError;
use crate::jev::{Gate, Rerank};
use crate::prompts::Prompt;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 후보 조각마다 p(답이 있어요)를 넘긴 순서대로 줘요. 앱에서는 Jev예요.
pub trait Reranker: Send + Sync {
    fn rerank<'a>(
        &'a self,
        context: &'a [String],
        line: &'a str,
        passages: &'a [String],
    ) -> BoxFuture<'a, Result<Rerank, ServiceError>>;
}

/// 이 발화가 찾아볼 일인지, 말한 사람이 회의를 끝내는 중인지예요. 앱에서는 Jev예요.
/// context = 앞 발화들, line = "말한 사람: 글".
pub trait Gater: Send + Sync {
    fn gate<'a>(
        &'a self,
        context: &'a [String],
        line: &'a str,
    ) -> BoxFuture<'a, Result<Gate, ServiceError>>;
}

/// 보여 준 카드만 근거로 한두 문장을 써요. 앱에서는 OpenRouter의 답 모델이에요.
pub trait Answerer: Send + Sync {
    /// passages는 `{"path", "line_start", "snippet"}`이에요.
    fn answer<'a>(
        &'a self,
        question: &'a str,
        context: &'a [String],
        passages: &'a [Value],
    ) -> BoxFuture<'a, Result<AnswerOut, ServiceError>>;
}

/// 메인 창 패널의 일마다 한 번씩 답 모델을 불러요(docs/36 2-2절). 번호는 모두 넘긴 슬라이스의 자리(0부터)예요.
/// 범위 밖 번호와 window 밖 key는 버려요.
pub trait PanelWriter: Send + Sync {
    /// 다음 회의 준비: items는 `{"kind", "text", "owner", "due", "date"}`, passages는 `answer`와 같은 카드예요.
    fn prep<'a>(
        &'a self,
        items: &'a [Value],
        passages: &'a [Value],
    ) -> BoxFuture<'a, Result<PrepOut, ServiceError>>;

    /// 결정 이유: window는 `{"key", "speaker", "at", "text"}` 발화들이에요.
    fn why<'a>(
        &'a self,
        decision: &'a str,
        window: &'a [Value],
    ) -> BoxFuture<'a, Result<WhyOut, ServiceError>>;

    /// 준비한 항목(`{"text"}`)마다 새 회의의 장부(`{"kind", "text"}`)에서 정했는지 가려요.
    fn followup<'a>(
        &'a self,
        prepared: &'a [Value],
        ledger: &'a [Value],
    ) -> BoxFuture<'a, Result<FollowOut, ServiceError>>;

    /// 패널 하네스의 한 번(docs/38 3-1절): 첫 메시지(`prompt`), 도구 묶음 이름(`tools`), 그 뒤의 도구 호출·결과
    /// 턴(`turns`)을 보내고 모델의 글이나 도구 호출을 받아요. `choice`는 OpenRouter `tool_choice`예요: "auto",
    /// "none"(도구를 끄고 답만), "required"(도구를 꼭 불러요).
    fn chat_turn<'a>(
        &'a self,
        prompt: Prompt,
        tools: &'static str,
        turns: &'a [Value],
        choice: &'static str,
    ) -> BoxFuture<'a, Result<ChatTurn, ServiceError>> {
        let _ = (prompt, tools, turns, choice);
        Box::pin(async {
            Err(ServiceError { kind: tacie_sources::Kind::Down, message: "chat_turn is not served here".into() })
        })
    }
}

/// 모델의 한 번 답이에요: 글, 도구 호출, 잘렸는지, 걸린 ms, 비용.
#[derive(Debug, Clone, Default)]
pub struct ChatTurn {
    pub content: String,
    pub calls: Vec<ToolCall>,
    pub cut: bool,
    pub ms: u64,
    pub cost: f64,
}

/// 모델이 부른 도구 하나예요. `arguments`는 모델이 쓴 JSON 글 그대로예요.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct PrepOut {
    pub advice: String,
    /// 같은 이야기인 항목 묶음(항목 자리)이에요.
    pub groups: Vec<Vec<usize>>,
    /// 쓴 자료의 자리(passages 슬라이스, 0부터)예요.
    pub cited: Vec<usize>,
    pub ms: u64,
    pub cost: f64,
}

#[derive(Debug, Clone)]
pub struct WhyOut {
    pub reason: String,
    pub keys: Vec<String>,
    pub ms: u64,
    pub cost: f64,
}

#[derive(Debug, Clone)]
pub struct FollowOut {
    /// (항목 자리, 정했는지, 무엇으로)예요.
    pub results: Vec<(usize, bool, Option<String>)>,
    pub ms: u64,
    pub cost: f64,
}

/// 발화 하나(WAV 바이트)를 글로 받아써요. 앱에서는 OpenRouter의 받아쓰기 모델이에요.
pub trait Transcriber: Send + Sync {
    fn transcribe<'a>(&'a self, wav: &'a [u8]) -> BoxFuture<'a, Result<Transcript, ServiceError>>;
}

/// 회의 글로 결정·할 일·남은 질문을 써요. 앱에서는 OpenRouter의 장부 모델이에요.
pub trait NoteWriter: Send + Sync {
    /// transcript는 `[발화 id] 말한 사람: 글` 줄들이에요. attendees는 그 줄에 보인 사람 이름이에요(나온 순서).
    /// `kind`는 녹음 유형이에요: 그 유형의 프롬프트 두 개로 써요(`NotesKind::prompt_names`).
    fn ledger<'a>(
        &'a self,
        transcript: &'a str,
        attendees: &'a [String],
        kind: crate::notes_kind::NotesKind,
    ) -> BoxFuture<'a, Result<LedgerOut, ServiceError>>;

    /// `ledger`를 사용자 요청과 함께 써요(패널의 다시 쓰기, docs/38 12절). 요청이 없으면 `ledger`와 같아요. 기본은 요청을
    /// 버리고 `ledger`예요(테스트의 가짜).
    fn ledger_asked<'a>(
        &'a self,
        transcript: &'a str,
        attendees: &'a [String],
        kind: crate::notes_kind::NotesKind,
        request: Option<&'a str>,
    ) -> BoxFuture<'a, Result<LedgerOut, ServiceError>> {
        let _ = request;
        self.ledger(transcript, attendees, kind)
    }

    /// `ledger_asked`를 정한 언어로 써요(설정 → 회의 → "회의 정리 언어", 2026-11-19). `lang`이 없으면 회의 언어대로이고
    /// `ledger_asked`와 같아요. 기본은 언어를 버리고 `ledger_asked`예요(테스트의 가짜).
    fn ledger_in<'a>(
        &'a self,
        transcript: &'a str,
        attendees: &'a [String],
        kind: crate::notes_kind::NotesKind,
        request: Option<&'a str>,
        lang: Option<crate::lang::Lang>,
    ) -> BoxFuture<'a, Result<LedgerOut, ServiceError>> {
        let _ = lang;
        self.ledger_asked(transcript, attendees, kind, request)
    }

    /// 회의에서 답을 찾지 못한 질문마다 회의 안에서 답이 나왔는지, 문서가 필요한지, 질문이 아닌지를 판정하고 혼자
    /// 읽히게 다시 써요(`notes_questions`, docs/24 4절). questions는 `- [id] 나온 발화 [발화 id]: 질문` 줄들이에요.
    /// 기본은 없어요(앱은 원문 질문을 그대로 보여 줘요).
    fn review_questions<'a>(
        &'a self,
        _transcript: &'a str,
        _attendees: &'a [String],
        _questions: &'a str,
    ) -> BoxFuture<'a, Result<LedgerOut, ServiceError>> {
        Box::pin(async {
            Err(ServiceError {
                kind: crate::errors::Kind::Down,
                message: "no question reviewer".into(),
            })
        })
    }

    /// 이번 결정(`[1] 글` 줄)이 지난 결정(`[1] 날짜 · 회의 제목 · 글` 줄)을 바꿨는지 골라요: `{"changes": [{"new",
    /// "past"}]}` JSON 글이에요(번호는 1부터). 기본은 없어요(그러면 바뀐 결정을 적지 않아요).
    fn decision_changes<'a>(
        &'a self,
        _new_decisions: &'a str,
        _past_decisions: &'a str,
    ) -> BoxFuture<'a, Result<LedgerOut, ServiceError>> {
        Box::pin(async {
            Err(ServiceError {
                kind: crate::errors::Kind::Down,
                message: "no decision comparer".into(),
            })
        })
    }

    /// 대화 기록 앞부분(`notes_kind::classify_sample`)으로 녹음 유형을 정해요: `{"kind", "confidence"}` JSON 글이에요.
    /// 기본은 없어요(그러면 회의로 써요).
    fn classify<'a>(&'a self, _sample: &'a str) -> BoxFuture<'a, Result<LedgerOut, ServiceError>> {
        Box::pin(async {
            Err(ServiceError {
                kind: crate::errors::Kind::Down,
                message: "no classifier".into(),
            })
        })
    }

    /// 팀 용어마다 한글로 적힐 법한 표기를 JSON 객체 글로 써요(`terms::readings_prompt`). 받아쓰기 고치기에
    /// 써요(`terms`). 앱에서는 장부와 같은 답 모델이에요. 기본은 없어요(테스트 가짜는 이걸 쓰지 않아요).
    fn readings<'a>(
        &'a self,
        _terms: &'a [String],
    ) -> BoxFuture<'a, Result<LedgerOut, ServiceError>> {
        Box::pin(async {
            Err(ServiceError {
                kind: crate::errors::Kind::Down,
                message: "no readings writer".into(),
            })
        })
    }
}

#[derive(Debug, Clone)]
pub struct AnswerOut {
    /// 모델이 쓴 줄(앞뒤 공백을 뺐어요)이에요. 검사는 부르는 쪽(`check_answer`)이 해요.
    pub answer: String,
    pub ms: f64,
    pub cost: f64,
}

/// 받아쓴 단어 하나와 그 시각(초, 발화 WAV 처음부터)이에요.
#[derive(Debug, Clone, PartialEq)]
pub struct SpokenWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// 받아쓰기를 어디서 했는지예요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SttVia {
    Server,
    Local,
    /// 앱이 Soniox 실시간 연결에서 받아 같이 보낸 글이에요(docs/25). 엔진은 받아쓰지 않았어요.
    Stream,
}

#[derive(Debug, Clone)]
pub struct Transcript {
    /// 받아쓴 글이에요. 빈 글이면 아무 말도 없었던 거예요.
    pub text: String,
    pub ms: f64,
    pub cost: f64,
    /// 받아쓰기의 확신이에요(공급자 값, 없으면 단어 점수 평균). 기록만 하고 아직 쓰지 않아요.
    pub confidence: Option<f64>,
    /// 단어마다 시각이에요. 공급자가 시각을 안 주면 비어 있어요.
    pub words: Vec<SpokenWord>,
    /// 서버에서 받아썼는지, 이 Mac에서 받아썼는지예요.
    pub via: SttVia,
}

#[derive(Debug, Clone)]
pub struct LedgerOut {
    /// 모델이 쓴 글(JSON 글)이에요. 읽고 검사하는 일은 부르는 쪽이 해요.
    pub text: String,
    pub ms: f64,
    pub cost: f64,
}

/// 서버 회의 검색이 돌려준 회의 하나예요(`/meetings/search`, docs/40 5-1절). 글은 없고 id와 점수만이에요.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub id: String,
    pub score: f64,
}

/// iPhone 엔진의 회의 검색이에요(docs/40 4-4절). 질문 글들을 한 요청으로 보내고, 질문마다 가까운 회의를 `top`개까지
/// 받아요(넘긴 순서대로). 앱에서는 서울 중계 서버의 `/meetings/search`예요.
pub trait MeetingSearch: Send + Sync {
    fn search<'a>(&'a self, queries: &'a [String], top: usize) -> BoxFuture<'a, Result<Vec<Vec<Hit>>, ServiceError>>;
}

/// 엔진이 쓰는 서비스 여섯 개예요. 앱은 main에서 Jev와 OpenRouter로, 테스트는 가짜로 묶어요.
#[derive(Clone)]
pub struct Services {
    pub reranker: Arc<dyn Reranker>,
    pub gater: Arc<dyn Gater>,
    pub answerer: Arc<dyn Answerer>,
    pub transcriber: Arc<dyn Transcriber>,
    pub note_writer: Arc<dyn NoteWriter>,
    pub panel: Arc<dyn PanelWriter>,
    /// 회의 검색이에요(iPhone 엔진만). 없으면 iPhone 하네스는 `meetings.json`에서 낱말로 찾아요.
    pub meeting_search: Option<Arc<dyn MeetingSearch>>,
}
