//! Jev (TypeSafe) calls: the lookup decision (gate) and the chunk reranker (rerank).
//!
//! The prompt text is exactly what was measured in docs/01. Changing even one character invalidates those numbers
//! and the result caches.
//! eval (`eval/enginelib/jev.py`) has the same text. `tests/jev_requests_fixture.rs` checks that the two copies
//! match.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::{Map, Value, json};

use crate::errors::{self, Kind, ServiceError};
use crate::keys::{Found, Keys};
use crate::lang::{self, Lang};
use crate::prompts;
use crate::services::{BoxFuture, Gater, Reranker};
use crate::settings::{self, Settings};

/// TypeSafe는 토큰 수만 돌려줘요. 비용은 OpenRouter의 jev-1.13 입력 단가로 어림해요.
pub const USD_PER_INPUT_TOKEN: f64 = 4.2e-8;

const TYPESAFE_URL: &str = "https://api.typesafe.ai/v1/systemone";
const OPENROUTER_URL: &str = "https://openrouter.ai/api/alpha/decisions";
const LONG_CONTEXT_HEADER: &str = "TypeSafe-Danger-Zone-Long-Context";

// 2026-11-10: 이미 정해 둔 결정(어느 업체로, 언제, 누가 맡기로, 얼마로, 왜)도 찾아요. 예시 문장 60개에서 지난 결정 질문
// 12–13/30 → 30/30, 잡담·의견 0/20 그대로. 실제 회의 850줄에서 잡힘 49 → 65, 빠진 줄 없음(docs/04 5절).
#[cfg(feature = "local-prompts")]
const LOOKUP_TRUE: &str = "The current utterance asks for a fact that is written down in this company's own source code or internal documents: how our product behaves, where something is implemented, a rule, limit, default or setting, a design decision or its reason, a team process or convention. Showing the matching passage would help now. This holds even when the speaker asks a colleague in the room, as long as the answer is such a fact. So is asking how something works or what to do (~하면 되나요?, ~하면 안 돼요?, ~해야 돼요?), even when it sounds like asking for permission. So is asking what the team already decided and wrote down: which company, vendor or tool was chosen, when something was set to happen, who was assigned to it, a number that was set, or why.";
#[cfg(feature = "local-prompts")]
const LOOKUP_FALSE: &str = "No lookup in company material is needed: small talk, opinions, feelings, planning a schedule or handing out work right now, status of someone's own work, rhetorical questions, questions about the outside world or news of other companies, questions whose answer is the listeners' consent, opinion or a decision being made right now, not a fact that could be written down (이렇게 해도 되는 거죠?, 이거 없애도 되죠?, ~하는 게 낫지 않을까요?), worries or thinking aloud (~할까 봐, ~해야 되는 건가 싶고), half-finished fragments, statements that ask nothing, or answers to someone else's question.";
// 2026-10-30: 받아쓰기의 문장 부호를 믿지 말라는 안내를 더했어요(docs/04 5절). 회의록 10개(발화 449개)에서 헛카드가
// 13–16장에서 7–8장으로 줄고 잡은 질문은 84–85개에서 87–88개가 됐어요. 물음표가 빠진 질문은 이 안내로도 잘 못 잡아요.
// 영어 회의의 판단이에요(2026-11-11). 위 셋의 한국어 예시를 영어 예시로 바꾸고 영어 질문 꼴을 알려 줘요. AMI 공개 영어
// 회의 12개(8,646줄, 찾아볼 질문 44개)에서 잡은 질문이 11개에서 19개로 늘고 엉뚱한 카드는 10개에서 17개가 됐어요
// (AUC 0.985 → 0.987, docs/04 5절). 한국어 회의는 위 글 그대로예요.
#[cfg(feature = "local-prompts")]
const LOOKUP_INSTRUCTIONS_EN: &str = "Does the current utterance need a lookup in our company's code or documents? The lines are a live speech-recognition transcript, so punctuation is unreliable: a question often ends with a period, with no mark at all, or is cut off. Judge from the wording (question forms such as did we/do we/is it/are we/can we/should we, question words such as what/where/which/how/why/when/who/how much, tag questions such as isn't it?/right?), not from punctuation.";
#[cfg(feature = "local-prompts")]
const LOOKUP_TRUE_EN: &str = "The current utterance asks for a fact that is written down in this company's own source code or internal documents: how our product behaves, where something is implemented, a rule, limit, default or setting, a design decision or its reason, a team process or convention. Showing the matching passage would help now. This holds even when the speaker asks a colleague in the room, as long as the answer is such a fact. So is asking how something works or what to do (how do I...?, can we just...?, do we need to...?), even when it sounds like asking for permission. So is asking what the team already decided and wrote down: which company, vendor or tool was chosen, when something was set to happen, who was assigned to it, a number that was set, or why.";
#[cfg(feature = "local-prompts")]
const LOOKUP_FALSE_EN: &str = "No lookup in company material is needed: small talk, opinions, feelings, planning a schedule or handing out work right now, status of someone's own work, rhetorical questions, questions about the outside world or news of other companies, questions whose answer is the listeners' consent, opinion or a decision being made right now, not a fact that could be written down (this is fine, right?, can we drop this?, wouldn't it be better to...?), worries or thinking aloud (I'm worried that..., I wonder if we should...), half-finished fragments, statements that ask nothing, or answers to someone else's question.";
#[cfg(feature = "local-prompts")]
const LOOKUP_INSTRUCTIONS: &str = "Does the current utterance need a lookup in our company's code or documents? The lines are a live speech-recognition transcript, so punctuation is unreliable: a question often ends with a period or with no mark at all. Judge from the wording (question endings such as -요?/-나요/-까/-죠/-어요, question words such as 어디/뭐/얼마/왜/언제/어떻게), not from punctuation.";
// 회의가 끝나는지예요. gate와 같은 요청에서 물어서 호출이 늘지 않아요. 2026-10-25에 시험한 값이에요:
// 끝맺는 말은 0.85–0.97, "자 이제 끝내자"는 0.65, 안건 하나가 끝나거나 나중·지난 회의 이야기는 0.22 이하였고,
// p_lookup은 0.01 이하로만 움직였어요.
#[cfg(feature = "local-prompts")]
const END_TRUE: &str = "The speaker is closing this meeting right now: saying it is over, wrapping up, thanking everyone for their time, or saying goodbye to the group (오늘은 여기까지, 회의 마치겠습니다, 수고하셨습니다, 회의 종료).";
#[cfg(feature = "local-prompts")]
const END_FALSE: &str = "The meeting goes on: ending a topic or a task, something ending later or in the past, planning the next meeting, a question, or any other talk.";
#[cfg(feature = "local-prompts")]
const SOURCE_CRITERIA: [(&str, &str); 3] = [
    (
        "code",
        "The answer lives in source code: behavior, conditions, limits, values, where it is implemented.",
    ),
    (
        "docs",
        "The answer lives in written documents: decisions and their reasons, plans, specs, processes, conventions.",
    ),
    ("none", "No lookup is needed."),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Typesafe,
    Openrouter,
}

impl Provider {
    fn name(self) -> &'static str {
        match self {
            Provider::Typesafe => "typesafe",
            Provider::Openrouter => "openrouter",
        }
    }

    fn url(self) -> &'static str {
        match self {
            Provider::Typesafe => TYPESAFE_URL,
            Provider::Openrouter => OPENROUTER_URL,
        }
    }
}

/// 한 번의 판단에 얼마나 기다리고 몇 번 보낼지예요.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JevKw {
    pub timeout: Duration,
    pub attempts: u32,
}

impl JevKw {
    /// 파이프라인 값이에요: 앱은 기다리느니 다른 길로 가요.
    pub const PIPELINE: JevKw = JevKw {
        timeout: Duration::from_millis(2500),
        attempts: 2,
    };
    /// 평가 스크립트가 쓰는 값이에요.
    pub const EVAL: JevKw = JevKw {
        timeout: Duration::from_secs(60),
        attempts: 5,
    };
}

impl Default for JevKw {
    fn default() -> Self {
        Self::PIPELINE
    }
}

#[derive(Debug, Clone)]
pub struct Gate {
    pub p_lookup: f64,
    /// code, docs, none의 확률이에요(응답 순서 그대로).
    pub source: Map<String, Value>,
    /// 말한 사람이 지금 회의를 끝내는 중일 확률이에요(`end` 질문). 기준과 비교하는 일은 세션(Plan 2)이 해요.
    /// 진짜 Jev 응답에는 늘 있어요(없으면 down). 테스트 가짜는 None을 줄 수 있어요.
    pub p_end: Option<f64>,
    pub ms: f64,
    pub usage: Value,
}

#[derive(Debug, Clone)]
pub struct Rerank {
    /// 조각마다 p(답이 있어요), 넘긴 순서 그대로예요.
    pub p: Vec<f64>,
    pub ms: f64,
    pub usage: Value,
}

/// 요청 하나의 길이에요([`Jev::route`]). 공급자 하나에서 모두 꺼냈어요.
pub struct Route {
    /// "typesafe" 또는 "openrouter"
    pub provider: &'static str,
    pub url: String,
    pub model: String,
    /// `Authorization` 값이에요(민감 값). 키 파일에 `apiKey`가 없으면 None이에요.
    pub bearer: Option<HeaderValue>,
}

type Backoff = Box<dyn Fn(u32) -> Duration + Send + Sync>;

pub struct Jev {
    /// AI 키예요. 앱이 넘긴 키가 먼저고, 개발 엔진은 키 파일도 봐요. 공급자는 부를 때마다 키로 정해요.
    keys: Arc<Keys>,
    /// `TACIE_JEV_MODEL`이에요(없으면 공급자의 기본 모델).
    model_override: Option<String>,
    /// `TACIE_JEV_URL`이에요(없으면 공급자 주소).
    url_override: Option<String>,
    /// `TACIE_JEV_FAIL=1`: 키를 망가뜨리지 않고 앱의 대체 길을 시험해요.
    fail: bool,
    /// 키는 요청마다 `Authorization` 헤더로 실어요(민감 값이라 로그나 오류 글에 나오지 않아요).
    client: reqwest::Client,
    /// 서비스별 마지막 거절이에요(`AppState.last`와 같은 표).
    last: Arc<Mutex<HashMap<String, Kind>>>,
    /// 429/5xx 뒤에 쉬는 시간이에요. 시도 번호(0부터)를 받아요.
    backoff: Backoff,
}

impl Jev {
    /// 키 파일만 보는 Jev예요(테스트, 평가 도구): [`Jev::new`]에 `config_dir`의 키 파일을 줘요.
    pub fn from_config(
        config_dir: &Path,
        settings: &Settings,
        env: &HashMap<String, String>,
        last: Arc<Mutex<HashMap<String, Kind>>>,
    ) -> Self {
        Self::new(Arc::new(Keys::from_files(config_dir)), settings, env, last)
    }

    /// TypeSafe 키가 있으면 TypeSafe(모델은 키 파일의 `model`, 없으면 `jev-latest`), 아니면 OpenRouter 키가 있으면
    /// OpenRouter(`typesafe/jev-1.13`), 둘 다 없으면 공급자 없음이에요. 키가 바뀌면 다음 요청부터 따라가요.
    /// `TACIE_JEV_MODEL`(settings)은 쓰는 모델을, `TACIE_JEV_URL`은 보낼 주소를 덮어요.
    pub fn new(
        keys: Arc<Keys>,
        settings: &Settings,
        env: &HashMap<String, String>,
        last: Arc<Mutex<HashMap<String, Kind>>>,
    ) -> Self {
        // 덮어쓰기는 debug 빌드면 아무 값이나, release 빌드면 loopback만 받아요(docs/04 2-1).
        let url_override = settings::service_url(env, "TACIE_JEV_URL", cfg!(debug_assertions));
        // 프록시를 쓸지는 주소로 정해요: 공급자 주소는 둘 다 바깥이에요.
        let client = settings::http_client(
            url_override.as_deref().unwrap_or(TYPESAFE_URL),
            HeaderMap::new(),
        );
        Self {
            keys,
            model_override: settings.jev_model.clone(),
            url_override,
            fail: env.get("TACIE_JEV_FAIL").is_some_and(|v| v == "1"),
            client,
            last,
            backoff: Box::new(|attempt| Duration::from_secs_f64(1.5 * f64::from(attempt + 1))),
        }
    }

    /// 429/5xx 뒤에 쉬는 시간을 바꿔요(테스트는 0으로 둬요).
    pub fn with_backoff(
        mut self,
        backoff: impl Fn(u32) -> Duration + Send + Sync + 'static,
    ) -> Self {
        self.backoff = Box::new(backoff);
        self
    }

    /// Jev와 답 모델이 같이 쓰는 키예요(`POST /ai/credentials`가 여기에 둬요).
    pub fn keys(&self) -> &Arc<Keys> {
        &self.keys
    }

    /// 공급자와 그 키를 한 번에 골라요: TypeSafe 키가 있으면 TypeSafe, 아니면 OpenRouter 키, 둘 다 없으면 None.
    /// 키 파일은 공급자마다 한 번만 읽어요(키와 `model`을 같은 읽기에서 꺼내요).
    fn pick(&self) -> Option<(Provider, Found)> {
        [Provider::Typesafe, Provider::Openrouter]
            .into_iter()
            .find_map(|p| Some((p, self.keys.find(p.name())?)))
    }

    /// 요청 하나가 쓸 공급자, 주소, 모델, 키예요. 한 번 고른 공급자에서 모두 꺼내서, 요청 중에 키가 바뀌어도
    /// TypeSafe 키가 OpenRouter 주소로 가는 일이 없어요. 키가 없으면 None이에요.
    pub fn route(&self) -> Option<Route> {
        let (provider, found) = self.pick()?;
        Some(Route {
            provider: provider.name(),
            url: self.url_for(provider),
            model: self.model_for(provider, &found),
            bearer: found.bearer(),
        })
    }

    fn url_for(&self, provider: Provider) -> String {
        self.url_override
            .clone()
            .unwrap_or_else(|| provider.url().to_string())
    }

    fn model_for(&self, provider: Provider, found: &Found) -> String {
        self.model_override
            .clone()
            .unwrap_or_else(|| Self::default_for(provider, found))
    }

    fn default_for(provider: Provider, found: &Found) -> String {
        match provider {
            Provider::Typesafe => found
                .model
                .clone()
                .unwrap_or_else(|| "jev-latest".to_string()),
            Provider::Openrouter => "typesafe/jev-1.13".to_string(),
        }
    }

    /// "typesafe", "openrouter" 또는 None(키가 없어요)
    pub fn provider_name(&self) -> Option<&'static str> {
        self.pick().map(|(p, _)| p.name())
    }

    /// 실제로 부르는 모델이에요(`TACIE_JEV_MODEL`이 있으면 그 값). 공급자가 없으면 None이에요.
    pub fn model(&self) -> Option<String> {
        self.route().map(|r| r.model)
    }

    /// 요청을 보내는 주소예요(`TACIE_JEV_URL`을 받았으면 그 값, 아니면 공급자 주소, 공급자가 없으면 빈 글).
    pub fn url(&self) -> String {
        self.route().map(|r| r.url).unwrap_or_default()
    }

    /// 공급자의 기본 모델이에요.
    pub fn model_default(&self) -> Option<String> {
        let (provider, found) = self.pick()?;
        Some(Self::default_for(provider, &found))
    }

    /// Jev 요청 하나예요. (응답 JSON, ms)를 돌려줘요. 429/5xx는 다시 보내고, 길다고 거절하면 긴 맥락 헤더를 붙여요.
    /// 질문은 받은 그대로 보내요(eval, 테스트). 판단과 리랭크는 [`Jev::gate`], [`Jev::rerank`]예요.
    pub async fn decide(
        &self,
        state: Value,
        questions: Value,
        kw: JevKw,
    ) -> Result<(Value, f64), ServiceError> {
        self.send(state, Ask::Raw(questions), kw).await
    }

    async fn send(&self, state: Value, ask: Ask, kw: JevKw) -> Result<(Value, f64), ServiceError> {
        // 공급자를 한 번만 고르고 주소·모델·키를 모두 거기서 꺼내요. 헤더에 쓸 수 없는 키면 헤더 없이 보내요: 401로
        // 거절이 보여요.
        let Some(Route {
            provider,
            mut url,
            model,
            bearer,
        }) = self.route()
        else {
            return Err(ServiceError {
                kind: Kind::Key,
                message: "no Jev key".into(),
            });
        };
        if self.fail {
            return Err(ServiceError {
                kind: Kind::Down,
                message: "jev disabled by TACIE_JEV_FAIL".into(),
            });
        }
        let body = ask.body(&model, state, &url);
        let mut long_context = false;
        let mut attempt = 0;
        while attempt < kw.attempts {
            let t = Instant::now();
            let mut req = self.client.post(&url).json(&body).timeout(kw.timeout);
            if let Some(bearer) = &bearer {
                req = req.header(AUTHORIZATION, bearer.clone());
            }
            if long_context {
                req = req.header(LONG_CONTEXT_HEADER, "true");
            }
            let sent = match req.send().await {
                Ok(r) => {
                    let status = r.status().as_u16();
                    r.text().await.map(|text| (status, text))
                }
                Err(e) => Err(e),
            };
            let (status, text) = match sent {
                Ok(x) => x,
                // 미국 중계에 연결하지 못했으면 서울 중계로 한 번 옮겨요(시도를 쓰지 않아요). 서울 주소에는 다시
                // 옮기지 않아요.
                Err(e) if settings::retry_in_seoul(&url, &e).is_some() => {
                    url = settings::retry_in_seoul(&url, &e).unwrap_or_default();
                    eprintln!("relay: us unreachable, sending jev to seoul");
                    continue;
                }
                Err(_) if attempt + 1 < kw.attempts => {
                    attempt += 1;
                    continue;
                }
                Err(e) => {
                    // reqwest 글에는 원인(연결 거절, 시간 초과)이 없어서 이어 붙여요.
                    let mut message = format!("jev request failed: {e}");
                    let mut cause = std::error::Error::source(&e);
                    while let Some(c) = cause {
                        message.push_str(&format!(": {c}"));
                        cause = c.source();
                    }
                    return Err(ServiceError {
                        kind: Kind::Network,
                        message,
                    });
                }
            };
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            if status == 200 {
                errors::clear("jev", &self.last);
                return Ok((with_cost(&text)?, ms));
            }
            // 미국 중계가 세션을 모르면(서울 로그인이 아직 복제되지 않았을 때) 서울 중계로 한 번 옮겨요.
            if let Some(seoul) = settings::retry_in_seoul_after(&url, status) {
                url = seoul;
                eprintln!("relay: us refused the session, sending jev to seoul");
                continue;
            }
            // 긴 맥락 헤더로 한 번 더 보내요. 이 재시도도 시도 하나를 써요.
            if provider == "typesafe"
                && !long_context
                && matches!(status, 400 | 413 | 422)
                && ["long", "context", "token"]
                    .iter()
                    .any(|w| text.to_lowercase().contains(w))
            {
                long_context = true;
                attempt += 1;
                continue;
            }
            if matches!(status, 429 | 500 | 502 | 503 | 504) && attempt + 1 < kw.attempts {
                tokio::time::sleep((self.backoff)(attempt)).await;
                attempt += 1;
                continue;
            }
            let head: String = text.chars().take(300).collect();
            return Err(errors::from_status("jev", status, &head, &self.last));
        }
        Err(ServiceError {
            kind: Kind::Busy,
            message: "jev: no attempts left".into(),
        })
    }

    /// 이 발화가 찾아볼 일인지, 말한 사람이 회의를 끝내는 중인지예요. context = 앞 발화들, line = "말한 사람: 글".
    pub async fn gate(
        &self,
        context: &[String],
        line: &str,
        kw: JevKw,
    ) -> Result<Gate, ServiceError> {
        let recent = if context.is_empty() {
            "(start of meeting)".to_string()
        } else {
            context.join("\n")
        };
        let ask = match gate_lang(context, line) {
            Lang::Ko => Ask::Gate,
            Lang::En => Ask::GateEn,
        };
        let state = json!({"recent_conversation": recent, "current_utterance": line});
        let (out, ms) = self.send(state, ask, kw).await?;
        let a = &out["answers"];
        let p_lookup = a["lookup"]["noul"]
            .as_f64()
            .ok_or_else(|| malformed("answers.lookup.noul"))?;
        let source = a["source"]["probabilities"]
            .as_object()
            .cloned()
            .ok_or_else(|| malformed("answers.source.probabilities"))?;
        let p_end = a["end"]["noul"]
            .as_f64()
            .ok_or_else(|| malformed("answers.end.noul"))?;
        Ok(Gate {
            p_lookup,
            source,
            p_end: Some(p_end),
            ms,
            usage: out["usage"].clone(),
        })
    }

    /// 조각마다 p(답이 있어요)를 넘긴 순서대로, 요청 한 번에 받아요.
    pub async fn rerank(
        &self,
        context: &[String],
        line: &str,
        passages: &[String],
        kw: JevKw,
    ) -> Result<Rerank, ServiceError> {
        let joined = context.join("\n");
        let recent = if joined.is_empty() {
            "(no earlier messages)".to_string()
        } else {
            joined
        };
        let named: Map<String, Value> = passages
            .iter()
            .enumerate()
            .map(|(k, p)| (format!("p{k}"), Value::from(p.as_str())))
            .collect();
        let state =
            json!({"recent_conversation": recent, "current_message": line, "passages": named});
        let (out, ms) = self.send(state, Ask::Rerank(passages.len()), kw).await?;
        let a = &out["answers"];
        let p = (0..passages.len())
            .map(|k| {
                a[format!("p{k}").as_str()]["noul"]
                    .as_f64()
                    .ok_or_else(|| malformed(&format!("answers.p{k}.noul")))
            })
            .collect::<Result<Vec<f64>, _>>()?;
        Ok(Rerank {
            p,
            ms,
            usage: out["usage"].clone(),
        })
    }
}

/// 앱은 기다리느니 다른 길로 가요: 2.5초, 두 번(`JevKw::PIPELINE`).
impl Gater for Jev {
    fn gate<'a>(
        &'a self,
        context: &'a [String],
        line: &'a str,
    ) -> BoxFuture<'a, Result<Gate, ServiceError>> {
        Box::pin(Jev::gate(self, context, line, JevKw::default()))
    }
}

/// 리랭크도 판단과 같은 값이에요(2.5초, 두 번).
impl Reranker for Jev {
    fn rerank<'a>(
        &'a self,
        context: &'a [String],
        line: &'a str,
        passages: &'a [String],
    ) -> BoxFuture<'a, Result<Rerank, ServiceError>> {
        Box::pin(Jev::rerank(self, context, line, passages, JevKw::default()))
    }
}

/// 요청 하나가 묻는 것이에요.
pub enum Ask {
    /// 받은 질문 그대로예요.
    Raw(Value),
    /// 찾아볼지, 어디서, 회의를 끝내는지예요([`gate_questions`]).
    Gate,
    /// 같은 판단의 영어 회의판이에요([`gate_questions_en`]).
    GateEn,
    /// 조각 n개마다 답이 있는지예요([`rerank_question`]).
    Rerank(usize),
}

impl Ask {
    /// 요청 본문이에요. 중계 서버로 가면(`prompts::by_name`) 질문 대신 이름(`prompt`)만 실어요. 중계 서버가 그 자리를
    /// 같은 `questions`로 바꿔서 TypeSafe가 받는 본문은 같아요(apps/server/src/prompts.js).
    pub fn body(self, model: &str, state: Value, url: &str) -> Value {
        let named = match &self {
            Ask::Raw(_) => None,
            Ask::Gate => Some(json!({"name": "gate"})),
            Ask::GateEn => Some(json!({"name": "gate_en"})),
            Ask::Rerank(n) => Some(json!({"name": "rerank", "count": n})),
        };
        match (self, named) {
            (Ask::Raw(questions), _) => {
                json!({"model": model, "state": state, "questions": questions})
            }
            (_, Some(prompt)) if prompts::by_name(url) => {
                json!({"model": model, "state": state, "prompt": prompt})
            }
            #[cfg(feature = "local-prompts")]
            (ask, _) => json!({"model": model, "state": state, "questions": ask.questions()}),
            #[cfg(not(feature = "local-prompts"))]
            _ => unreachable!("without local prompts every request goes by name"),
        }
    }

    #[cfg(feature = "local-prompts")]
    fn questions(&self) -> Value {
        match self {
            Ask::Raw(q) => q.clone(),
            Ask::Gate => gate_questions(),
            Ask::GateEn => gate_questions_en(),
            Ask::Rerank(n) => Value::Object(
                (0..*n)
                    .map(|k| (format!("p{k}"), rerank_question(&k.to_string())))
                    .collect(),
            ),
        }
    }
}

/// 판단할 발화의 언어예요. 앞 발화까지 보고 정해요("right?" 한 줄로는 정할 수 없어요). 앞에 붙은 "화자 N:" 같은
/// 말한 사람 표시는 빼고 세요.
pub fn gate_lang(context: &[String], line: &str) -> Lang {
    let text: Vec<&str> = context
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(line))
        .map(|l| l.split_once(": ").map_or(l, |(_, t)| t))
        .collect();
    lang::of(&text.join("\n"))
}

/// 판단 질문 셋이에요(찾아볼지, 어디서, 회의를 끝내는지).
#[cfg(feature = "local-prompts")]
pub fn gate_questions() -> Value {
    gate_with(LOOKUP_INSTRUCTIONS, LOOKUP_TRUE, LOOKUP_FALSE)
}

/// 영어 회의의 판단 질문 셋이에요. 찾아볼지만 영어 예시로 묻고, 어디서·끝내는지는 같아요.
#[cfg(feature = "local-prompts")]
pub fn gate_questions_en() -> Value {
    gate_with(LOOKUP_INSTRUCTIONS_EN, LOOKUP_TRUE_EN, LOOKUP_FALSE_EN)
}

#[cfg(feature = "local-prompts")]
fn gate_with(instructions: &str, lookup_true: &str, lookup_false: &str) -> Value {
    let source_criteria: Map<String, Value> = SOURCE_CRITERIA
        .iter()
        .map(|&(k, v)| (k.to_string(), Value::from(v)))
        .collect();
    json!({
        "lookup": {"type": "noul", "instructions": instructions,
                   "criteria": {"true": lookup_true, "false": lookup_false}},
        "source": {"type": "choice", "instructions": "Where would the answer to the current utterance be found?",
                   "criteria": source_criteria},
        "end": {"type": "noul", "instructions": "Is the speaker ending this meeting now?",
                "criteria": {"true": END_TRUE, "false": END_FALSE}},
    })
}

/// 조각 `p{k}` 하나의 질문이에요. `prompts.json`에는 `k`가 `"{k}"`인 틀로 들어가요.
#[cfg(feature = "local-prompts")]
pub fn rerank_question(k: &str) -> Value {
    json!({"type": "noul",
        "instructions": format!("Does passage p{k} answer what the current message asks (using the earlier messages for context), or show where it is decided?"),
        "criteria": {"true": format!("Passage p{k} contains the answer or the exact place it is decided."),
                     "false": format!("Passage p{k} is unrelated or only mentions the topic.")}})
}

/// 200 응답을 읽고 `usage.cost`가 없으면 input_tokens × 단가로 채워요.
fn with_cost(text: &str) -> Result<Value, ServiceError> {
    let mut out: Value = serde_json::from_str(text).map_err(|_| malformed("not JSON"))?;
    let usage = out
        .as_object_mut()
        .ok_or_else(|| malformed("not an object"))?
        .entry("usage")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| malformed("usage"))?;
    if !usage.contains_key("cost") {
        let tokens = usage
            .get("input_tokens")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        usage.insert("cost".into(), json!(tokens * USD_PER_INPUT_TOKEN));
    }
    Ok(out)
}

/// 200인데 모양이 달라요: 서비스가 이상하게 답한 거라 down이에요.
fn malformed(what: &str) -> ServiceError {
    ServiceError {
        kind: Kind::Down,
        message: format!("jev: unexpected response ({what})"),
    }
}
