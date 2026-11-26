//! The Tacie AI panel harness (docs/38 3절). The model picks read tools (search, meeting list, ledger filter,
//! transcript window, next meeting prep) a few times and then writes the answer. Tool rounds go up to [`ROUNDS`];
//! after that tools are turned off and only an answer is requested.
//!
//! Each line of a tool result gets a number [n] within this question before it goes to the model, and the [n] in
//! the answer is renumbered to the order of the cards the app draws. Only sources the answer cites become cards
//! (4-2절). Permissions are filtered inside `search` before the search, as now. Meeting ledgers and transcripts are
//! the user's own meetings on this Mac, so they are not filtered.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};
use tacie_index::Index;
use tacie_sources::meeting::{self, Meeting};

use crate::lang::{self, Lang};
use crate::panel_write::{self, Place};
use crate::pipeline::{Engine, Scope, head};
use crate::errors::{Kind, ServiceError};
use crate::prompts::Prompt;
use crate::services::{Hit, ToolCall};

/// 도구를 쓰는 라운드 수예요. 이만큼 쓰면 한 번 더 불러 도구 없이 답을 받아요.
pub const ROUNDS: usize = 3;
/// 한 라운드에 돌리는 도구 호출 수예요. 넘는 호출은 버려요.
pub const CALLS_PER_ROUND: usize = 4;
const SEARCH_N: usize = 8;
/// iPhone 엔진에서 한 질문이 서버 회의 검색에 보내는 질문 글 수 상한이에요(docs/40 4-4절).
pub const SERVER_SEARCHES_MAX: usize = 4;
/// 서버 회의 검색이 질문 글 하나에 돌려주는 회의 수예요.
pub const SEARCH_TOP: usize = 8;
const CHUNK_CHARS: usize = 600;
const LEDGER_MAX: usize = 60;
const MEETINGS_MAX: usize = 30;
const WINDOW_AROUND: usize = 4;
const WINDOW_AROUND_MAX: usize = 20;
const WINDOW_LINES_MAX: usize = 40;
/// 화자 하나를 읽을 때 그 화자 발화 몇 개까지, 앞뒤 몇 줄이에요(`transcript_window`의 `speaker`).
const SPEAKER_LINES_MAX: usize = 8;
const SPEAKER_AROUND: usize = 2;
const WINDOW_MATCHES: usize = 4;
/// filter_ledger 결과가 이만큼 이하이고 대화 기록이 있으면, 항목마다 그 항목을 말한 대화를 같이 붙여요. 모델에게 따로
/// transcript_window를 부르라고 하면 이유 질문 10개 중 4개는 부르지 않았어요(docs/38 9-2절).
const LEDGER_WITH_LINES: usize = 3;
/// 붙이는 대화는 발화 key 앞뒤로 이만큼이에요(지금 이유 보기의 `PanelStore.windowRadius`와 같아요).
const ITEM_AROUND: usize = 6;
/// 앞 대화는 마지막 이만큼의 질문·답이에요(`/panel/ask`와 같아요). 앞 턴의 도구 결과는 다시 싣지 않아요.
pub const HISTORY_TURNS: usize = 6;
/// 도구 결과 하나의 글자 수 상한이에요. 중계 서버는 60,000자까지 받아요.
const RESULT_CHARS: usize = 50_000;
/// find_words가 모델에게 보여 주는 곳 수예요. 넘으면 수만 알려요.
const FIND_MAX: usize = 120;
/// 회의 정리를 읽을 때 장부 항목 수 상한이에요.
const NOTES_LEDGER_MAX: usize = 80;
/// iPhone 검색 결과에서 회의 하나의 정리 글자 수 상한이에요(docs/40 7절: 회의당 최대 약 2천 자).
const MEETING_CHARS: usize = 2000;
/// 서버 회의 검색의 질문 글 하나의 글자 수 상한이에요(`/meetings/search`가 500자까지 받아요).
pub const QUERY_CHARS: usize = 500;
/// 검색 상한을 넘은 `search`가 모델에게 돌려주는 말이에요.
const SEARCH_CAPPED: &str = "검색을 더 할 수 없어요. 찾은 것으로 답해요.";

/// 앱이 보낸 질문 하나예요.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub text: String,
    /// 앞 대화의 (질문, 답)이에요.
    pub history: Vec<(String, String)>,
    /// 회의 페이지에서 시작한 대화면 그 회의와 이어진 앞 회의들(앱의 순서, 그 회의가 처음), 아니면 비어요.
    pub meeting_ids: Vec<String>,
    /// 오늘 날짜(`YYYY-MM-DD`)예요. 앱이 Mac의 시간대로 보내요.
    pub today: String,
    /// 회의 id마다 대화 기록 발화(`{key, speaker, at, text}`)예요. 회의 페이지 대화에서만 앱이 붙여요(3-3절).
    pub transcripts: HashMap<String, Vec<Value>>,
    /// 회의 id마다 장부 항목 순서대로, 그 항목을 말한 발화 key들이에요(앱 `LedgerEntry.refs`). 없으면 낱말로 찾아요.
    pub refs: HashMap<String, Vec<Vec<String>>>,
}

/// 답에 달 수 있는 출처 하나예요. `card`는 앱이 그리는 카드(조각 카드 또는 회의 카드)예요.
#[derive(Debug, Clone)]
enum Source {
    Chunk { card: Value },
    Meeting { card: Option<Value> },
    Item { meeting_id: String, index: usize, card: Option<Value> },
    Line { meeting_id: String, key: String, card: Option<Value> },
}

impl Source {
    fn card(&self) -> Option<&Value> {
        match self {
            Source::Chunk { card } => Some(card),
            Source::Meeting { card, .. } | Source::Item { card, .. } | Source::Line { card, .. } => card.as_ref(),
        }
    }
}

/// 한 질문 동안의 상태: 번호 붙인 출처, 쓴 돈, 읽은 회의와 자료예요.
struct Run<'a> {
    engine: &'a Engine,
    meeting_dir: &'a Path,
    req: &'a Request,
    ix: std::sync::Arc<Index>,
    meetings: Vec<Meeting>,
    sources: Vec<Source>,
    cost: f64,
    /// prepare_meeting을 불렀으면 그 준비 전체예요. 앱이 답 밑에 지금 준비 화면 그대로 그려요(모델 답은 요약이라 항목을 빠뜨려요).
    prep: Option<Value>,
    /// find_words가 찾은 곳이에요. 모델은 `w{차례+1}`로 가리켜요.
    found: Vec<Place>,
    /// 쓰기 도구가 만든 제안이에요(docs/38 12절). 같은 고치기를 다시 부르면 같은 자리를 새 제안으로 바꿔요.
    proposals: Vec<Value>,
    /// 아직 앱에 보내지 않은 제안이에요.
    pending: Vec<Value>,
    /// iPhone: 이번 질문에서 서버 회의 검색에 보낸 질문 글 수예요([`SERVER_SEARCHES_MAX`]까지).
    searched: usize,
    /// iPhone: 라운드 앞에서 미리 받아 둔 검색어마다의 결과예요. `search`가 자기 검색어 자리를 읽어요.
    fetched: HashMap<String, Fetched>,
    /// 질문을 끝낼 실패예요(iPhone 회의 검색의 한도·로그인 실패). 도구 하나가 끝날 때마다 봐요.
    stop: Option<ServiceError>,
    /// iPhone: `filter_ledger`에 넘긴 `kinds`를 모두 합친 것이에요. 비어 있지 않으면 답이 인용한 장부 항목 중 이 종류가
    /// 아닌 것을 빼요(검색 결과의 장부에서 묻지 않은 종류를 끌어오는 걸 막아요).
    asked_kinds: Vec<String>,
}

/// iPhone 모든 회의 검색의 미리 받은 결과 하나예요.
#[derive(Debug, Clone)]
enum Fetched {
    /// 서버가 돌려준 가까운 회의예요.
    Hits(Vec<Hit>),
    /// 서버 검색이 안 됐어요(네트워크, 서버 오류): `meetings.json`에서 낱말로 찾아요.
    Keywords,
    /// 한 질문의 서버 검색 상한을 넘었어요.
    Capped,
}

/// 질문 하나를 끝까지 돌려요. `emit`은 앱에 보낼 단계 줄(`{"step": …}`)을 받아요. 돌려주는 값이 마지막 `done`이에요:
/// `{answer, cards, items, keys, lines, rounds, tools, cost, prep?}`, 실패하면 `error`·`error_kind`가 더해지고 `answer`는 null이에요.
/// iPhone 모든 회의 대화에서 모델이 고칠 회의를 넘기면(`open_meeting`) `answer`는 빈 글이고 `handoff: {meeting_id}`가 붙어요.
pub async fn run(engine: &Engine, meeting_dir: &Path, req: &Request, mut emit: impl FnMut(Value)) -> Value {
    let mut run = Run {
        engine,
        meeting_dir,
        req,
        ix: engine.index.load_full(),
        meetings: meeting::all(meeting_dir),
        sources: Vec::new(),
        cost: 0.0,
        prep: None,
        found: Vec::new(),
        proposals: Vec::new(),
        pending: Vec::new(),
        searched: 0,
        fetched: HashMap::new(),
        stop: None,
        asked_kinds: Vec::new(),
    };
    let questions: Vec<String> = req.history.iter().map(|(q, _)| q.clone()).collect();
    let lang = lang::of_question(&req.text, &questions);
    let prompt = run.prompt(lang);
    let scoped = !req.meeting_ids.is_empty();
    let (_, set) = names(engine.iphone, scoped, !req.transcripts.is_empty());
    emit(json!({"step": {"tool": "start"}}));

    let mut turns: Vec<Value> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    let mut answer = String::new();
    let mut rounds = 0;
    // 고치기는 찾고(find_words) 제안하기(fix_words)까지 라운드가 하나 더 들어요: 찾기를 했는데 아직 제안이 없으면 한 번 더 줘요.
    let mut limit = ROUNDS;
    let mut round = 0;
    // 회의 페이지에서 고치기 대신 되물으면("바꿔도 될지 알려 주세요") 그 턴을 버리고 도구를 꼭 부르게 한 번 다시 보내요.
    let mut nudged = false;
    let mut nudge = false;
    // 이 Mac의 작은 모델(이슈 #54): 엔진이 근거를 먼저 읽어 넣고(`given_tools`), 모델은 도구 없이 한 번에 답해요.
    // 쓰기 도구(고치기)는 쓰지 않아요.
    if !engine.iphone && engine.services.panel.small_model() {
        limit = 0;
        nudged = true;
        for (i, (name, args)) in given_tools(req).into_iter().enumerate() {
            let id = format!("given{i}");
            turns.push(json!({"role": "assistant", "content": null, "tool_calls": [{
                "id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}));
            emit(json!({"step": {"tool": name, "args": shown_args(&args)}}));
            let (result, count) = run.tool(name, &args).await;
            emit(json!({"step": {"tool": name, "done": true, "count": count}}));
            used.push(name.to_string());
            let wrapped = format!("<결과>\n{}\n</결과>", crate::openrouter::defuse(&result, "결과"));
            turns.push(json!({"role": "tool", "tool_call_id": id,
                              "content": wrapped.chars().take(RESULT_CHARS).collect::<String>()}));
        }
    }
    loop {
        if round == ROUNDS && limit == ROUNDS && used.iter().any(|t| t == "find_words") && run.proposals.is_empty() {
            limit += 1;
        }
        let use_tools = round < limit || nudge;
        let choice = if nudge { "required" } else if use_tools { "auto" } else { "none" };
        nudge = false;
        round += 1;
        let turn = match engine.services.panel.chat_turn(prompt.clone(), set, &turns, choice).await {
            Ok(t) => t,
            Err(e) => {
                return json!({"answer": null, "cards": [], "items": [], "keys": [], "lines": [], "rounds": rounds,
                              "tools": used, "cost": run.cost, "proposals": run.proposals,
                              "error": head(&e.message), "error_kind": e.kind});
            }
        };
        run.cost += turn.cost;
        if turn.calls.is_empty() || !use_tools {
            if scoped && !nudged && run.proposals.is_empty() && asks_back(&turn.content) {
                nudged = true;
                nudge = true;
                continue;
            }
            answer = turn.content;
            break;
        }
        rounds += 1;
        let calls: Vec<ToolCall> = turn.calls.into_iter().take(CALLS_PER_ROUND).collect();
        let text = (!turn.content.is_empty()).then(|| turn.content.chars().take(1500).collect::<String>());
        turns.push(json!({"role": "assistant", "content": text, "tool_calls": calls.iter().map(|c| json!({
            "id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.arguments}})).collect::<Vec<_>>()}));
        // iPhone 모든 회의: 고칠 회의를 넘기면(open_meeting) 이 라운드의 다른 도구는 돌리지 않고 질문을 끝내요(docs/40 12절).
        if let Some(out) = run.handoff(&calls, rounds, &used, &mut emit) {
            return out;
        }
        // iPhone: 이 라운드의 검색은 서버에 한 요청으로 보내요. 한도(credit)나 로그인(key) 실패면 질문을 끝내요.
        if let Err(e) = run.prefetch(&calls).await {
            return stopped(&run, rounds, &used, &e);
        }
        for c in &calls {
            let args: Value = serde_json::from_str(&c.arguments).unwrap_or_else(|_| json!({}));
            emit(json!({"step": {"tool": c.name, "args": shown_args(&args)}}));
            let (result, count) = run.tool(&c.name, &args).await;
            emit(json!({"step": {"tool": c.name, "done": true, "count": count}}));
            for p in run.pending.drain(..) {
                emit(json!({"proposal": p}));
            }
            used.push(c.name.clone());
            let wrapped = format!("<결과>\n{}\n</결과>", crate::openrouter::defuse(&result, "결과"));
            turns.push(json!({"role": "tool", "tool_call_id": c.id,
                              "content": wrapped.chars().take(RESULT_CHARS).collect::<String>()}));
            if let Some(e) = run.stop.take() {
                return stopped(&run, rounds, &used, &e);
            }
        }
    }
    let mut out = run.cite(&answer);
    out["rounds"] = json!(rounds);
    out["tools"] = json!(used);
    out["cost"] = json!(run.cost);
    out["proposals"] = json!(run.proposals);
    if let Some(p) = run.prep.take() {
        out["prep"] = p;
    }
    if answer.trim().is_empty() {
        out["answer"] = json!(format!("{}.", crate::pipeline::NO_ANSWER));
    }
    out
}

/// 작은 모델에게 엔진이 먼저 읽어 주는 도구들이에요: 회의 페이지면 그 회의의 정리와 대화 줄, 아니면 질문으로 한 번 찾은
/// 결과와 최근 장부예요.
pub fn given_tools(req: &Request) -> Vec<(&'static str, Value)> {
    match req.meeting_ids.first() {
        Some(id) => {
            let mut out = vec![("meeting_notes", json!({"meeting_id": id}))];
            // 짧은 회의는 대화 기록 전체(가운데 줄 앞뒤 20줄), 긴 회의는 질문 낱말이 나온 곳의 앞뒤예요.
            if let Some(lines) = req.transcripts.get(id).filter(|l| !l.is_empty()) {
                let args = if lines.len() <= 2 * WINDOW_AROUND_MAX + 1 {
                    json!({"meeting_id": id, "key": lines[lines.len() / 2]["key"], "around": WINDOW_AROUND_MAX})
                } else {
                    json!({"meeting_id": id, "find": req.text, "around": 6})
                };
                out.push(("transcript_window", args));
            }
            out
        }
        // 모든 회의: 질문으로 찾은 결과와 최근 장부(결정·할 일, 담당·기한 포함)예요. 검색 결과의 회의 조각은 요약뿐이라
        // "누가 언제까지"를 놓쳤어요(2026-11-26).
        None => vec![("search", json!({"query": req.text.chars().take(QUERY_CHARS).collect::<String>()})),
                     ("filter_ledger", json!({}))],
    }
}

/// 앱에 넘긴 질문의 `done`이에요: 답 글 없이 `handoff: {meeting_id}`만 있어요. 앱이 그 회의의 대화 기록을 붙여 같은
/// 질문을 다시 보내요(docs/40 12절). `handoff`를 모르는 앱도 읽을 수 있게 다른 칸은 그대로예요.
fn handed_off(run: &Run, rounds: usize, used: &[String], id: &str) -> Value {
    json!({"answer": "", "cards": [], "items": [], "keys": [], "lines": [], "rounds": rounds,
           "tools": used, "cost": run.cost, "proposals": [], "handoff": {"meeting_id": id}})
}

/// 서비스 실패로 끝난 질문의 `done`이에요: `answer`는 null이고 `error`·`error_kind`가 붙어요.
fn stopped(run: &Run, rounds: usize, used: &[String], e: &ServiceError) -> Value {
    json!({"answer": null, "cards": [], "items": [], "keys": [], "lines": [], "rounds": rounds,
           "tools": used, "cost": run.cost, "proposals": run.proposals,
           "error": head(&e.message), "error_kind": e.kind})
}

/// 프롬프트 이름(언어 전)과 도구 묶음이에요. 회의 페이지 대화(`scoped`)는 쓰기 도구를 받고, 대화 기록이 없는 회의(다른
/// 기기에서 받아 온 회의)는 대화 기록 도구 없이 정리만 고쳐요. iPhone 엔진은 iPhone 짝을 써요(docs/40 4-3, 4-5절): 연결한
/// 서비스가 없고, 화자 합치기·줄 옮기기·형식 바꾸기가 빠져요.
pub fn names(iphone: bool, scoped: bool, has_transcript: bool) -> (&'static str, &'static str) {
    match (iphone, scoped, has_transcript) {
        (false, false, _) => ("panel_chat", "panel"),
        (false, true, true) => ("panel_chat", "panel_meeting_edit"),
        (false, true, false) => ("panel_chat", "panel_notes_edit"),
        (true, false, _) => ("panel_chat_iphone", "panel_iphone"),
        (true, true, true) => ("panel_chat_iphone", "panel_iphone_meeting"),
        (true, true, false) => ("panel_chat_iphone", "panel_iphone_notes"),
    }
}

/// iPhone 묶음에 없는 도구예요. 모델이 불러도 돌리지 않아요.
const NOT_ON_IPHONE: [&str; 4] = ["merge_speakers", "move_lines", "assign_lines", "change_kind"];

/// 답이 사용자에게 되묻고 끝나는지예요: 고치기 대신 확인을 받으려 할 때의 말이에요.
fn asks_back(answer: &str) -> bool {
    let tail: String = answer.trim().chars().rev().take(160).collect::<Vec<_>>().into_iter().rev().collect();
    const ASKS: [&str; 12] = ["알려 주세요", "알려주세요", "말씀해 주세요", "말씀해주세요", "할까요", "올릴까요", "바꿀까요",
                              "드릴까요", "확인해 주세요", "정해 주세요", "괜찮을까요", "될까요"];
    tail.trim_end().ends_with('?') || ASKS.iter().any(|a| tail.contains(a))
}

/// 단계 줄에 보여 줄 수 있는 인자만이에요: 앱이 정해진 글로 바꿔 그려요(모델 글을 그대로 보여 주지 않아요).
fn shown_args(args: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for k in ["query", "from", "to", "kinds", "owner", "meeting_id", "find", "wide", "word", "heard", "meant", "speaker", "name", "into", "to", "kind", "title", "moves"] {
        if let Some(v) = args.get(k).filter(|v| !v.is_null()) {
            let v = match v.as_str() {
                Some(s) => json!(s.chars().take(40).collect::<String>()),
                None => v.clone(),
            };
            out.insert(k.into(), v);
        }
    }
    Value::Object(out)
}

static MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[(\d+)\]").expect("mark regex"));
/// find_words의 곳 번호(`[w3]`, `[w1–w9]`, `(w2, w5)`)예요: 도구 안에서만 쓰는 번호라 답에서 지워요.
static PLACE_MARK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s?[\[(]w\d+(?:\s*[–\-~,]\s*w?\d+)*[\])]").expect("place mark regex"));

impl Run<'_> {
    fn prompt(&self, lang: Lang) -> Prompt {
        let recent = &self.req.history[self.req.history.len().saturating_sub(HISTORY_TURNS)..];
        let lines: Vec<String> = recent
            .iter()
            .map(|(q, a)| crate::notes::one_line(&format!("나: {q} / Tacie: {a}")))
            .collect();
        let context = match (lines.is_empty(), lang) {
            (true, _) => String::new(),
            (false, Lang::Ko) => format!("앞 대화:\n{}\n\n", lines.join("\n")),
            (false, Lang::En) => format!("Earlier in the conversation:\n{}\n\n", lines.join("\n")),
        };
        let today = if self.req.today.is_empty() {
            chrono::Local::now().format("%Y-%m-%d").to_string()
        } else {
            self.req.today.clone()
        };
        Prompt {
            name: lang::prompt_name(names(self.engine.iphone, false, false).0, lang),
            fields: vec![
                ("today", today),
                ("scope", self.scope_line(lang)),
                ("context", context),
                ("question", self.req.text.clone()),
            ],
        }
    }

    fn scope_line(&self, lang: Lang) -> String {
        let ids = &self.req.meeting_ids;
        if ids.is_empty() {
            return match (self.engine.iphone, lang) {
                (true, Lang::Ko) => "이 iPhone의 모든 회의".into(),
                (true, Lang::En) => "All meetings on this iPhone".into(),
                (false, Lang::Ko) => "모든 회의와 연결한 서비스".into(),
                (false, Lang::En) => "All meetings and connected services".into(),
            };
        }
        let list: Vec<String> = ids
            .iter()
            .filter_map(|id| self.meeting(id))
            .map(|m| format!("{} {} (id {})", date(&m.start), m.title, m.id))
            .collect();
        let who = self.speaker_line(&ids[0]);
        match lang {
            Lang::Ko => format!(
                "회의 페이지에서 연 대화예요. 지금 보는 회의: {}. 이 회의로 이어진 앞 회의: {}{}",
                list.first().map(String::as_str).unwrap_or(""),
                if list.len() > 1 { list[1..].join(", ") } else { "없음".into() },
                who.map(|w| format!(". 이 회의의 화자: {w}")).unwrap_or_default()
            ),
            Lang::En => format!(
                "A conversation opened on a meeting page. The meeting on the page: {}. Earlier meetings it continues: {}{}",
                list.first().map(String::as_str).unwrap_or(""),
                if list.len() > 1 { list[1..].join(", ") } else { "none".into() },
                who.map(|w| format!(". Speakers of this meeting: {w}")).unwrap_or_default()
            ),
        }
    }

    /// 회의의 화자들: 번호(`voice`, 앱의 화자 key), 보이는 이름, 줄 수예요. 대화 기록 순서대로예요. 번호가 없는 줄(화자 분리
    /// 전 회의)은 보이는 이름을 번호로 써요.
    fn speakers(&self, id: &str) -> Vec<(String, String, usize)> {
        let mut out: Vec<(String, String, usize)> = Vec::new();
        for l in self.req.transcripts.get(id).into_iter().flatten() {
            let name = l["speaker"].as_str().unwrap_or("").to_string();
            let key = l["voice"].as_str().filter(|v| !v.is_empty()).map(String::from).unwrap_or_else(|| name.clone());
            if key.is_empty() {
                continue;
            }
            match out.iter_mut().find(|(k, _, _)| *k == key) {
                Some(e) => e.2 += 1,
                None => out.push((key, name, 1)),
            }
        }
        out
    }

    /// "화자 1(서연) 30줄, 화자 2 40줄"; 화자가 없으면 None이에요.
    fn speaker_line(&self, id: &str) -> Option<String> {
        let list = self.speakers(id);
        (!list.is_empty()).then(|| {
            list.iter()
                .map(|(k, n, c)| if n == k || n.is_empty() { format!("{k} {c}줄") } else { format!("{k}({n}) {c}줄") })
                .collect::<Vec<_>>()
                .join(", ")
        })
    }

    /// 사용자가 말한 화자(번호나 이름, 띄어쓰기 무시)의 (번호, 이름, 줄 수)예요.
    fn speaker(&self, id: &str, said: &str) -> Option<(String, String, usize)> {
        let bare = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
        let b = bare(said);
        let list = self.speakers(id);
        list.iter().find(|(k, _, _)| bare(k) == b).or_else(|| list.iter().find(|(_, n, _)| bare(n) == b)).cloned()
    }

    /// 도구 결과의 자리 말이에요: "이 Mac에" 또는 "이 iPhone에"예요.
    fn here(&self) -> &'static str {
        if self.engine.iphone { "이 iPhone에" } else { "이 Mac에" }
    }

    fn meeting(&self, id: &str) -> Option<&Meeting> {
        self.meetings.iter().find(|m| m.id == id)
    }

    /// 회의 카드예요. 색인에 회의 조각이 있으면 그 카드가 먼저예요(Mac은 acl로 걸러요). 조각이 없으면(iPhone 엔진이거나
    /// 아직 색인에 안 들어갔으면) 회의 목록의 회의로 만들어요.
    fn meeting_card(&self, id: &str) -> Option<Value> {
        let path = format!("meeting/{id}");
        match self.ix.chunks.iter().position(|c| c.path == path) {
            Some(i) => (self.ix.allowed.get(i) == Some(&true))
                .then(|| crate::cards::card(&self.ix.chunks[i], None, &self.ix.meta, None, i)),
            None => self.meeting(id).map(crate::cards::meeting_card),
        }
    }

    fn push(&mut self, s: Source) -> usize {
        self.sources.push(s);
        self.sources.len()
    }

    async fn tool(&mut self, name: &str, args: &Value) -> (String, usize) {
        if self.engine.iphone && NOT_ON_IPHONE.contains(&name) {
            return (format!("없는 도구예요: {name}"), 0);
        }
        match name {
            "search" => self.search(args).await,
            "list_meetings" => self.list_meetings(args),
            "filter_ledger" => self.filter_ledger(args),
            "transcript_window" => self.transcript_window(args),
            "prepare_meeting" => self.prepare_meeting(args).await,
            "meeting_notes" => self.meeting_notes(args),
            "find_words" => self.find_words(args),
            "fix_words" => self.fix_words(args),
            "edit_ledger" => self.edit_ledger(args),
            "name_speaker" => self.name_speaker(args),
            "merge_speakers" => self.merge_speakers(args),
            "move_lines" => self.move_lines(args),
            "assign_lines" => self.assign_lines(args),
            "rewrite_notes" => self.rewrite_notes(args),
            "change_kind" => self.change_kind(args),
            "rename_title" => self.rename_title(args),
            "open_meeting" => self.open_meeting(args),
            other => (format!("없는 도구예요: {other}"), 0),
        }
    }

    /// 이 라운드에 `open_meeting`이 있으면 처리해요(iPhone 모든 회의 대화만, docs/40 12절). `meetings.json`에 있는 회의면
    /// 단계 줄(그 회의 제목을 붙여요, 끝 단계는 보내지 않아요: 앱의 다음 요청이 이어 가요)을 보내고 넘긴 `done`을 돌려줘요.
    /// 없는 회의면 그 호출에 실패를 돌려주도록 None이에요(`tool`이 모델에게 알려요). 회의 페이지 대화와 Mac 엔진에서는
    /// 넘기지 않아요: 그 묶음에는 이 도구가 없고, 불러도 없는 도구예요.
    fn handoff(&mut self, calls: &[ToolCall], rounds: usize, used: &[String], emit: &mut impl FnMut(Value)) -> Option<Value> {
        if !self.engine.iphone || !self.req.meeting_ids.is_empty() {
            return None;
        }
        for c in calls.iter().filter(|c| c.name == "open_meeting") {
            let args: Value = serde_json::from_str(&c.arguments).unwrap_or_else(|_| json!({}));
            let id = args["meeting_id"].as_str().unwrap_or("").trim();
            let Some(m) = self.meeting(id) else { continue };
            let (id, title) = (m.id.clone(), m.title.clone());
            emit(json!({"step": {"tool": "open_meeting", "args": {"meeting_id": id,
                        "title": title.chars().take(40).collect::<String>()}}}));
            let mut used = used.to_vec();
            used.push("open_meeting".into());
            return Some(handed_off(self, rounds, &used, &id));
        }
        None
    }

    /// 넘기지 못한 `open_meeting`이에요: 없는 회의 id이거나, 넘길 수 없는 대화예요.
    fn open_meeting(&self, args: &Value) -> (String, usize) {
        if !self.engine.iphone || !self.req.meeting_ids.is_empty() {
            return ("없는 도구예요: open_meeting".into(), 0);
        }
        let id = args["meeting_id"].as_str().unwrap_or("").trim();
        (format!("그런 회의가 없어요(id {id}). search나 list_meetings 결과에 있는 회의 id를 넣어요."), 0)
    }

    /// iPhone 모든 회의 대화에서, 라운드의 `search` 검색어들을 서버에 한 번에 보내 `fetched`에 담아요(docs/40 4-4절).
    /// 한 질문의 상한을 넘는 검색어는 `Capped`, 서버 검색이 안 되면 `Keywords`예요. credit·key 실패는 돌려줘서 질문을
    /// 끝내요. Mac 엔진, 회의 페이지 대화, 검색이 없는 엔진에서는 아무것도 안 해요.
    async fn prefetch(&mut self, calls: &[ToolCall]) -> Result<(), ServiceError> {
        if !self.engine.iphone || !self.req.meeting_ids.is_empty() {
            return Ok(());
        }
        let Some(searcher) = self.engine.services.meeting_search.clone() else {
            return Ok(());
        };
        let mut queries: Vec<String> = Vec::new();
        for c in calls.iter().filter(|c| c.name == "search") {
            let args: Value = serde_json::from_str(&c.arguments).unwrap_or_else(|_| json!({}));
            let q = search_query(&args);
            if !q.is_empty() && !queries.contains(&q) && !self.fetched.contains_key(&q) {
                queries.push(q);
            }
        }
        let room = SERVER_SEARCHES_MAX.saturating_sub(self.searched);
        let send: Vec<String> = queries.iter().take(room).cloned().collect();
        for q in queries.iter().skip(room) {
            self.fetched.insert(q.clone(), Fetched::Capped);
        }
        if send.is_empty() {
            return Ok(());
        }
        self.searched += send.len();
        match searcher.search(&send, SEARCH_TOP).await {
            Ok(mut results) => {
                results.resize(send.len(), Vec::new());
                for (q, hits) in send.into_iter().zip(results) {
                    self.fetched.insert(q, Fetched::Hits(hits));
                }
                Ok(())
            }
            Err(e) if matches!(e.kind, Kind::Credit | Kind::Key) => Err(e),
            Err(_) => {
                for q in send {
                    self.fetched.insert(q, Fetched::Keywords);
                }
                Ok(())
            }
        }
    }

    async fn search(&mut self, args: &Value) -> (String, usize) {
        let query = args["query"].as_str().unwrap_or("").trim().to_string();
        if query.is_empty() {
            return ("검색어가 비었어요.".into(), 0);
        }
        if self.engine.iphone {
            return self.search_meetings(args);
        }
        let wide = args["wide"].as_bool().unwrap_or(false);
        let scope = if wide || self.req.meeting_ids.is_empty() {
            Scope::All
        } else {
            Scope::Meetings(self.req.meeting_ids.clone())
        };
        let found = self.engine.ask_scoped_n(&query, &[], &scope, 0.0, SEARCH_N).await;
        self.cost += found["cost_usd"].as_f64().unwrap_or(0.0);
        if let Some(k) = found.get("error_kind").filter(|_| found["cards"].as_array().is_none_or(Vec::is_empty)) {
            return (format!("찾지 못했어요(오류: {}).", k.as_str().unwrap_or("")), 0);
        }
        let ids = crate::server::card_chunk_ids(&self.ix, &found["cards"]);
        let cards: Vec<Value> = found["cards"].as_array().cloned().unwrap_or_default();
        let mut lines = Vec::new();
        for card in cards {
            let Some(cid) = card["chunk"].as_u64().map(|c| c as usize).filter(|c| ids.contains(c)) else {
                continue;
            };
            let text: String = self.ix.chunks[cid].text.chars().take(CHUNK_CHARS).collect();
            let label = format!(
                "{} · {}",
                card["source"].as_str().or(card["kind"].as_str()).unwrap_or(""),
                card["title"].as_str().unwrap_or("")
            );
            let score = card["score"].as_f64().map(|p| format!(" · 점수 {p:.2}")).unwrap_or_default();
            let n = self.push(Source::Chunk { card });
            lines.push(format!("[{n}] {label}{score}\n{text}"));
        }
        if lines.is_empty() {
            return ("찾은 글이 없어요.".into(), 0);
        }
        let count = lines.len();
        (lines.join("\n\n"), count)
    }

    /// iPhone 엔진의 `search`예요(docs/40 4-4절). `wide`는 보지 않아요. 회의 페이지 대화면 서버에 묻지 않고 범위의 회의를
    /// 그대로, 모든 회의면 미리 받은 서버 결과(없으면 낱말 찾기)의 회의를 `meetings.json`에서 꺼내 출처로 붙여요.
    fn search_meetings(&mut self, args: &Value) -> (String, usize) {
        let ids: Vec<String> = if !self.req.meeting_ids.is_empty() {
            self.req.meeting_ids.clone()
        } else {
            let query = search_query(args);
            let fetched = match self.engine.services.meeting_search {
                Some(_) => self.fetched.get(&query).cloned().unwrap_or(Fetched::Capped),
                None => Fetched::Keywords,
            };
            match fetched {
                Fetched::Capped => return (SEARCH_CAPPED.into(), 0),
                Fetched::Hits(hits) => hits.into_iter().map(|h| h.id).collect(),
                Fetched::Keywords => {
                    keyword_meetings(&self.meetings, &query, SEARCH_TOP).into_iter().map(|m| m.id.clone()).collect()
                }
            }
        };
        let mut lines = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        for id in &ids {
            // 아직 meetings.json에 없는 회의는 건너뛰어요.
            if seen.contains(&id.as_str()) {
                continue;
            }
            let Some(m) = self.meeting(id).cloned() else { continue };
            seen.push(id);
            let card = self.meeting_card(&m.id);
            let n = self.push(Source::Meeting { card: card.clone() });
            let body = self.meeting_body(&m, card);
            lines.push(format!("[{n}] {} 「{}」 (id {})\n{body}", date(&m.start), m.title, m.id));
        }
        if lines.is_empty() {
            return ("찾은 회의가 없어요.".into(), 0);
        }
        let count = lines.len();
        (lines.join("\n\n"), count)
    }

    /// iPhone `search`가 돌려주는 회의 정리예요: 요약, 장부, 주제를 [`MEETING_CHARS`]까지요. 장부 항목은 `filter_ledger`와
    /// 같은 글(종류 이름, 담당·기한)로 하나마다 번호를 받아요. 그래야 모델이 항목을 회의 번호가 아니라 그 항목의 번호로
    /// 가리키고(답의 `items`), 두 도구가 같은 항목을 같은 말로 보여요. 영어 회의는 영어 꼬리표이고, 바꾼 이전 결정은
    /// 날짜와 회의 제목까지 붙여요.
    fn meeting_body(&mut self, m: &Meeting, card: Option<Value>) -> String {
        let en = meeting::is_english(m);
        let mut out: Vec<String> = Vec::new();
        let mut used = 0usize;
        let fits = |used: &mut usize, line: &str| {
            let n = line.chars().count() + 1;
            if *used + n > MEETING_CHARS {
                return false;
            }
            *used += n;
            true
        };
        for s in &m.summary {
            if !fits(&mut used, s) {
                return out.join("\n");
            }
            out.push(s.clone());
        }
        for kind in ["decision", "commitment", "open_question"] {
            for (i, e) in m.ledger.iter().enumerate().filter(|(_, e)| e.kind == kind) {
                let text = format!("{}: {}", kind_name(&e.kind, en), item_body(e, en, true));
                // 번호 자리(" [nn] ")만큼 넉넉히 세요
                if !fits(&mut used, &format!("[000] {text}")) {
                    return out.join("\n");
                }
                let k = self.push(Source::Item { meeting_id: m.id.clone(), index: i, card: card.clone() });
                out.push(format!("[{k}] {text}"));
            }
        }
        for t in &m.topics {
            for line in std::iter::once(format!("{}: {}", meeting::topic_label(en), t.title)).chain(t.points.iter().map(|p| format!("- {p}"))) {
                if !fits(&mut used, &line) {
                    return out.join("\n");
                }
                out.push(line);
            }
        }
        out.join("\n")
    }

    fn list_meetings(&mut self, args: &Value) -> (String, usize) {
        let (from, to) = (args["from"].as_str(), args["to"].as_str());
        let text = args["text"].as_str().unwrap_or("").trim().to_lowercase();
        let mut found: Vec<Meeting> = self
            .meetings
            .iter()
            .filter(|m| in_range(&m.start, from, to))
            .filter(|m| {
                text.is_empty()
                    || m.title.to_lowercase().contains(&text)
                    || m.summary.iter().any(|s| s.to_lowercase().contains(&text))
                    || m.topics.iter().any(|t| t.title.to_lowercase().contains(&text))
            })
            .cloned()
            .collect();
        found.sort_by(|a, b| a.start.cmp(&b.start));
        let total = found.len();
        let mut lines = Vec::new();
        for m in found.into_iter().take(MEETINGS_MAX) {
            let card = self.meeting_card(&m.id);
            let n = self.push(Source::Meeting { card });
            let first = m.summary.first().map(|s| format!(" — {s}")).unwrap_or_default();
            lines.push(format!("[{n}] {} 「{}」 (id {}){first}", date(&m.start), m.title, m.id));
        }
        if lines.is_empty() {
            return ("맞는 회의가 없어요.".into(), 0);
        }
        if total > MEETINGS_MAX {
            lines.push(format!("그 밖에 {}개 더 있어요. 기간을 좁혀 보세요.", total - MEETINGS_MAX));
        }
        let count = lines.len().min(total);
        (lines.join("\n"), count)
    }

    fn filter_ledger(&mut self, args: &Value) -> (String, usize) {
        let asked: Vec<String> = args["meeting_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        let base: Vec<String> = if !asked.is_empty() {
            asked
        } else if !self.req.meeting_ids.is_empty() {
            self.req.meeting_ids.clone()
        } else {
            self.meetings.iter().map(|m| m.id.clone()).collect()
        };
        let kinds: Vec<String> = args["kinds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        if self.engine.iphone {
            for k in &kinds {
                if !self.asked_kinds.contains(k) {
                    self.asked_kinds.push(k.clone());
                }
            }
        }
        let owner = args["owner"].as_str().unwrap_or("").trim().to_lowercase();
        // 낱말마다 항목 글이나 그 회의 제목에 있으면 돼요: "환불"은 회의 제목에만 있고 항목 글에는 없기 쉬워요.
        let words: Vec<String> =
            args["text"].as_str().unwrap_or("").split_whitespace().map(|w| w.to_lowercase()).collect();
        let done = args["done"].as_bool();
        let (from, to) = (args["from"].as_str(), args["to"].as_str());
        let mut ms: Vec<Meeting> = self
            .meetings
            .iter()
            .filter(|m| base.contains(&m.id) && in_range(&m.start, from, to))
            .cloned()
            .collect();
        ms.sort_by(|a, b| a.start.cmp(&b.start));
        let mut hits = Vec::new();
        for m in &ms {
            let title = m.title.to_lowercase();
            for (i, e) in m.ledger.iter().enumerate() {
                let body = e.text.to_lowercase();
                let ok = (kinds.is_empty() || kinds.contains(&e.kind))
                    && (owner.is_empty()
                        || e.owner.as_deref().is_some_and(|o| o.to_lowercase().contains(&owner))
                        || e.text.to_lowercase().contains(&owner))
                    && words.iter().all(|w| body.contains(w) || title.contains(w))
                    && done.is_none_or(|d| e.kind == "commitment" && e.done == d);
                if ok {
                    hits.push((m.clone(), i));
                }
            }
        }
        let total = hits.len();
        let mut lines = Vec::new();
        for (m, i) in hits.into_iter().take(LEDGER_MAX) {
            let e = &m.ledger[i];
            let card = self.meeting_card(&m.id);
            let n = self.push(Source::Item { meeting_id: m.id.clone(), index: i, card });
            lines.push(format!("[{n}] {} 「{}」 {}: {}", date(&m.start), m.title, kind_ko(&e.kind), item_text(e)));
            if total <= LEDGER_WITH_LINES
                && let Some(said) = self.item_lines(&m.id, i, &e.text)
            {
                lines.push(format!("  이 항목을 말한 대화:\n{said}"));
            }
        }
        if lines.is_empty() {
            return ("맞는 항목이 없어요.".into(), 0);
        }
        if total > LEDGER_MAX {
            lines.push(format!("그 밖에 {}개 더 있어요.", total - LEDGER_MAX));
        }
        (lines.join("\n"), total.min(LEDGER_MAX))
    }

    fn transcript_window(&mut self, args: &Value) -> (String, usize) {
        let id = args["meeting_id"].as_str().unwrap_or("");
        let Some(lines) = self.req.transcripts.get(id) else {
            return ("이 회의의 대화 기록은 볼 수 없어요.".into(), 0);
        };
        let around = args["around"].as_u64().map_or(WINDOW_AROUND, |a| (a as usize).min(WINDOW_AROUND_MAX));
        let mut centers: Vec<usize> = Vec::new();
        if let Some(k) = args["key"].as_str()
            && let Some(i) = lines.iter().position(|l| l["key"].as_str() == Some(k))
        {
            centers.push(i);
        }
        if let Some(f) = args["find"].as_str() {
            centers.extend(best_lines(lines, f, around));
        }
        // 화자 하나의 모든 발화와 앞뒤 두 줄이에요: 잘못 나뉜 화자의 말이 누구 말인지 맥락으로 볼 때 써요.
        let speaker = args["speaker"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        if let Some(said) = &speaker {
            let Some((key, _, _)) = self.speaker(id, said) else {
                let all = self.speaker_line(id).unwrap_or_default();
                return (format!("이 회의에 「{said}」 화자가 없어요. 화자는 {all}예요."), 0);
            };
            let lines = self.req.transcripts.get(id).cloned().unwrap_or_default();
            let mine: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| l["voice"].as_str().filter(|v| !v.is_empty()).or(l["speaker"].as_str()) == Some(key.as_str()))
                .map(|(i, _)| i)
                .take(SPEAKER_LINES_MAX)
                .collect();
            let around = args["around"].as_u64().map_or(SPEAKER_AROUND, |a| (a as usize).min(WINDOW_AROUND_MAX));
            return self.render_lines(id, &lines, &mine, around, true);
        }
        if centers.is_empty() {
            return ("대화 기록에서 그 말을 찾지 못했어요.".into(), 0);
        }
        let lines = lines.clone();
        self.render(id, &lines, &centers, around)
    }

    /// 장부 항목 하나를 말한 대화예요: 앱이 보낸 발화 key가 있으면 그 앞뒤, 없으면 항목 글의 낱말이 많이 나온 곳이에요.
    fn item_lines(&mut self, id: &str, index: usize, text: &str) -> Option<String> {
        let lines = self.req.transcripts.get(id)?.clone();
        let mut centers: Vec<usize> = self
            .req
            .refs
            .get(id)
            .and_then(|r| r.get(index))
            .into_iter()
            .flatten()
            .filter_map(|k| lines.iter().position(|l| l["key"].as_str() == Some(k.as_str())))
            .collect();
        if centers.is_empty() {
            centers = best_lines(&lines, text, ITEM_AROUND).into_iter().take(1).collect();
        }
        if centers.is_empty() {
            return None;
        }
        Some(self.render(id, &lines, &centers, ITEM_AROUND).0)
    }

    /// `centers` 발화 앞뒤 `around`줄을 번호 붙여 써요. 줄마다 출처(발화 key)가 하나씩 생겨요.
    fn render(&mut self, id: &str, lines: &[Value], centers: &[usize], around: usize) -> (String, usize) {
        self.render_lines(id, lines, centers, around, false)
    }

    /// `keys`: 화자 이름 뒤에 화자 번호도 붙여요("서연(화자 3)"). 화자를 고칠 때 이름이 같은 두 화자를 가려요.
    fn render_lines(&mut self, id: &str, lines: &[Value], centers: &[usize], around: usize, keys: bool) -> (String, usize) {
        let mut keep = vec![false; lines.len()];
        for &c in centers {
            for k in keep.iter_mut().take((c + around + 1).min(lines.len())).skip(c.saturating_sub(around)) {
                *k = true;
            }
        }
        let card = self.meeting_card(id);
        let mut out = Vec::new();
        let mut last: Option<usize> = None;
        for (i, l) in lines.iter().enumerate().filter(|(i, _)| keep[*i]).take(WINDOW_LINES_MAX) {
            if last.is_some_and(|p| p + 1 != i) {
                out.push("…".to_string());
            }
            last = Some(i);
            let key = l["key"].as_str().unwrap_or("").to_string();
            let n = self.push(Source::Line { meeting_id: id.to_string(), key, card: card.clone() });
            let name = l["speaker"].as_str().unwrap_or("");
            let who = match l["voice"].as_str().filter(|v| keys && !v.is_empty() && *v != name) {
                Some(v) => format!("{name}({v})"),
                None => name.to_string(),
            };
            out.push(format!("[{n}] {who}: {}", l["text"].as_str().unwrap_or("")));
        }
        let count = out.iter().filter(|l| l.starts_with('[')).count();
        (out.join("\n"), count)
    }

    async fn prepare_meeting(&mut self, args: &Value) -> (String, usize) {
        let id = args["meeting_id"].as_str().unwrap_or("").to_string();
        let ids: Vec<String> = match self.req.meeting_ids.iter().position(|m| *m == id) {
            Some(p) => self.req.meeting_ids[p..].to_vec(),
            None => vec![id.clone()],
        };
        // iPhone: 준비의 관련 회의 찾기도 한 질문의 서버 검색 상한에 들어가요.
        let server = self.searched < SERVER_SEARCHES_MAX;
        if server && self.engine.iphone && self.engine.services.meeting_search.is_some() {
            self.searched += 1;
        }
        let done = crate::panel::prep_searched(self.engine, self.meeting_dir, &ids, server, |_| {}).await;
        self.cost += done["cost"].as_f64().unwrap_or(0.0);
        let mut whole = done.clone();
        if let Some(o) = whole.as_object_mut() {
            o.remove("cost");
            o.insert("meeting_id".into(), json!(id));
        }
        self.prep = Some(whole);
        // iPhone 준비의 회의 검색이 한도(credit)나 로그인(key)으로 실패했으면 `search`처럼 질문을 끝내요.
        let kind = match done["error_kind"].as_str() {
            Some("credit") => Some(Kind::Credit),
            Some("key") => Some(Kind::Key),
            _ => None,
        };
        if let (true, Some(kind)) = (self.engine.iphone, kind) {
            let message = done["error"].as_str().unwrap_or("").to_string();
            self.stop = Some(ServiceError { kind, message });
        }
        let mut lines = Vec::new();
        if let Some(a) = done["advice"].as_str().filter(|a| !a.is_empty()) {
            lines.push(format!("조언: {a}"));
        }
        let mut count = 0;
        for (field, title) in [("open", "아직 안 정한 것"), ("todos", "안 끝난 할 일")] {
            let items = done[field].as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                continue;
            }
            lines.push(format!("{title}:"));
            for it in items {
                let (Some(mid), Some(index)) = (it["meeting_id"].as_str(), it["index"].as_u64()) else {
                    continue;
                };
                let card = self.meeting_card(mid);
                let n = self.push(Source::Item { meeting_id: mid.to_string(), index: index as usize, card });
                let owner = it["owner"].as_str().map(|o| format!(" (담당 {o})")).unwrap_or_default();
                let when = self.meeting(mid).map(|m| format!("{} ", date(&m.start))).unwrap_or_default();
                lines.push(format!("[{n}] {when}{}{owner}", it["text"].as_str().unwrap_or("")));
                count += 1;
            }
        }
        for card in done["sources"].as_array().cloned().unwrap_or_default() {
            let title = card["title"].as_str().unwrap_or("").to_string();
            let n = self.push(Source::Chunk { card });
            lines.push(format!("[{n}] 관련 자료: {title}"));
        }
        if let Some(k) = done.get("error_kind") {
            lines.push(format!("(관련 자료 찾기는 실패했어요: {})", k.as_str().unwrap_or("")));
        }
        if lines.is_empty() {
            return ("이 회의에 남은 항목이 없어요.".into(), 0);
        }
        (lines.join("\n"), count)
    }

    /// 고칠 수 있는 회의예요: 회의 페이지에서 연 대화의 그 회의 하나뿐이에요(docs/38 12-2절).
    fn editable(&self, args: &Value) -> Result<String, String> {
        let Some(page) = self.req.meeting_ids.first() else {
            return Err("고치기는 회의 페이지에서 연 대화에서만 할 수 있어요. 그 회의를 열고 다시 물어봐 달라고 답해요.".into());
        };
        match args["meeting_id"].as_str().filter(|m| !m.is_empty()) {
            None => Ok(page.clone()),
            Some(m) if m == page => Ok(page.clone()),
            Some(_) => Err("지금 보는 회의만 고칠 수 있어요. 다른 회의는 그 회의를 열고 고쳐 달라고 답해요.".into()),
        }
    }

    /// 제안을 넣어요. 같은 `slot`(같은 회의, 같은 고치기)의 앞 제안은 새것으로 바꿔요. 바꾼 제안은 같은 id예요.
    fn propose(&mut self, slot: &str, mut p: Value) -> String {
        let at = self.proposals.iter().position(|q| q["slot"].as_str() == Some(slot));
        let id = match at {
            Some(i) => self.proposals[i]["id"].as_str().unwrap_or("").to_string(),
            None => format!("p{}", self.proposals.len() + 1),
        };
        p["id"] = json!(id);
        p["slot"] = json!(slot);
        match at {
            Some(i) => self.proposals[i] = p.clone(),
            None => self.proposals.push(p.clone()),
        }
        self.pending.push(p);
        id
    }

    /// 회의 하나의 정리(제목, 요약, 주제, 장부)예요. 장부 항목마다 [번호]가 붙고, edit_ledger가 그 번호를 받아요.
    fn meeting_notes(&mut self, args: &Value) -> (String, usize) {
        let id = args["meeting_id"].as_str().filter(|m| !m.is_empty())
            .or(self.req.meeting_ids.first().map(String::as_str)).unwrap_or("").to_string();
        let Some(m) = self.meeting(&id).cloned() else {
            return ("그 회의를 찾지 못했어요.".into(), 0);
        };
        let mut out = vec![format!("{} 「{}」 (id {})", date(&m.start), m.title, m.id)];
        if let Some(w) = self.speaker_line(&id) {
            out.push(format!("화자: {w}"));
        }
        if !m.summary.is_empty() {
            out.push("요약:".into());
            out.extend(m.summary.iter().map(|s| format!("- {s}")));
        }
        for t in &m.topics {
            out.push(format!("주제: {}", t.title));
            out.extend(t.points.iter().map(|p| format!("  - {p}")));
        }
        let card = self.meeting_card(&m.id);
        let mut count = 0;
        if !m.ledger.is_empty() {
            out.push("장부:".into());
        }
        for (i, e) in m.ledger.iter().enumerate().take(NOTES_LEDGER_MAX) {
            let n = self.push(Source::Item { meeting_id: m.id.clone(), index: i, card: card.clone() });
            let mut extra = Vec::new();
            if let Some(o) = &e.owner {
                extra.push(format!("담당 {o}"));
            }
            if let Some(d) = &e.due {
                extra.push(format!("기한 {d}"));
            }
            if e.kind == "commitment" && e.done {
                extra.push("끝남".into());
            }
            let extra = if extra.is_empty() { String::new() } else { format!(" ({})", extra.join(", ")) };
            out.push(format!("[{n}] {}: {}{extra}", kind_ko(&e.kind), e.text));
            count += 1;
        }
        (out.join("\n"), count.max(1))
    }

    /// 회의의 대화 기록과 정리에서 글자가 그대로 나온 곳을 모두 찾아요. 곳마다 w번호가 붙고, fix_words의 only가 받아요.
    fn find_words(&mut self, args: &Value) -> (String, usize) {
        let id = match self.editable(args) {
            Ok(id) => id,
            Err(e) => return (e, 0),
        };
        let word = args["word"].as_str().unwrap_or("").trim().to_string();
        if word.is_empty() {
            return ("찾을 말이 비었어요.".into(), 0);
        }
        let places = panel_write::find(self.meeting(&id), self.req.transcripts.get(&id), &word);
        if places.is_empty() {
            return (format!("「{word}」은 이 회의의 대화 기록과 정리에 없어요."), 0);
        }
        let total = places.len();
        let mut out = vec![format!("「{word}」이 나온 곳 {total}곳이에요(대화 기록 {}곳).",
                                   places.iter().filter(|p| p.in_transcript()).count())];
        for (k, p) in places.into_iter().enumerate() {
            self.found.push(p);
            let w = self.found.len();
            if k < FIND_MAX {
                let p = &self.found[w - 1];
                out.push(format!("w{w} {}: …{}【{}】{}…", p.label(), p.before, p.word, p.after));
            }
        }
        if total > FIND_MAX {
            out.push(format!("그 밖에 {}곳 더 있어요.", total - FIND_MAX));
        }
        (out.join("\n"), total)
    }

    /// 잘못 들린 말을 맞는 말로 바꾸자는 제안이에요. `only`가 비면 그 말이 나온 곳 모두예요.
    fn fix_words(&mut self, args: &Value) -> (String, usize) {
        let id = match self.editable(args) {
            Ok(id) => id,
            Err(e) => return (e, 0),
        };
        let heard = args["heard"].as_str().unwrap_or("").trim().to_string();
        let meant = args["meant"].as_str().unwrap_or("").trim().to_string();
        if heard.is_empty() || meant.is_empty() || heard == meant {
            return ("heard와 meant에 서로 다른 말을 넣어요.".into(), 0);
        }
        let skip: Vec<usize> = args["skip"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .filter_map(|w| w.trim().trim_start_matches(['w', 'W']).parse::<usize>().ok())
            .collect();
        let skipped: Vec<Place> = skip.iter().filter_map(|&w| self.found.get(w.wrapping_sub(1))).cloned().collect();
        if skipped.len() != skip.len() || skipped.iter().any(|p| !panel_write::same_word(&p.word, &heard)) {
            return ("skip의 w번호는 find_words에 heard와 같은 말을 넣어 찾은 곳이어야 해요. 다시 찾아요.".into(), 0);
        }
        let places: Vec<Place> = panel_write::find(self.meeting(&id), self.req.transcripts.get(&id), &heard)
            .into_iter()
            .filter(|p| !skipped.contains(p))
            .collect();
        if places.is_empty() {
            return (format!("「{heard}」을 이 회의에서 찾지 못해서 제안하지 않았어요."), 0);
        }
        let p = panel_write::fix_words("", &id, &heard, &meant, &places);
        let left = skipped.len();
        let (lines, notes) = (p["lines"].as_u64().unwrap_or(0), p["notes"].as_u64().unwrap_or(0));
        self.propose(&format!("fix:{id}:{heard}"), p);
        (format!(
            "「{heard}」을 「{meant}」으로 바꾸자는 제안을 사용자에게 보였어요: 대화 기록 {lines}곳, 회의 정리 {notes}곳(뺀 곳 {left}곳). \
             아직 바뀌지 않았고, 사용자가 확인하면 바뀌어요. 회의 정리 다시 쓰기와 다음 회의부터 바로 알아듣기도 사용자가 같이 골라요."
        ), places.len())
    }

    /// 장부 항목을 고치자는 제안이에요. 항목은 이번 질문에서 받은 [번호]로 가리켜요.
    fn edit_ledger(&mut self, args: &Value) -> (String, usize) {
        let id = match self.editable(args) {
            Ok(id) => id,
            Err(e) => return (e, 0),
        };
        let Some(m) = self.meeting(&id).cloned() else {
            return ("그 회의를 찾지 못했어요.".into(), 0);
        };
        let mut changes = Vec::new();
        let mut wrong = Vec::new();
        for c in args["changes"].as_array().into_iter().flatten() {
            let n = c["item"].as_u64().map(|n| n as usize).unwrap_or(0);
            let index = match self.sources.get(n.wrapping_sub(1)) {
                Some(Source::Item { meeting_id, index, .. }) if *meeting_id == id => *index,
                _ => {
                    wrong.push(n);
                    continue;
                }
            };
            let Some(e) = m.ledger.get(index) else {
                wrong.push(n);
                continue;
            };
            if let Some(ch) = panel_write::ledger_change(index, &e.text, c) {
                changes.retain(|x: &Value| x["index"].as_u64() != Some(index as u64));
                changes.push(ch);
            }
        }
        let adds: Vec<Value> = args["add"].as_array().into_iter().flatten().filter_map(panel_write::ledger_add).collect();
        if !wrong.is_empty() && changes.is_empty() && adds.is_empty() {
            return (format!("이 회의 장부 항목의 번호가 아니에요: {wrong:?}. meeting_notes나 filter_ledger로 번호를 받아요."), 0);
        }
        if changes.is_empty() && adds.is_empty() {
            return ("바꿀 것이 없어서 제안하지 않았어요.".into(), 0);
        }
        let count = changes.len() + adds.len();
        let p = json!({"kind": "edit_ledger", "meeting_id": id, "changes": changes, "add": adds});
        self.propose(&format!("ledger:{id}"), p);
        let mut said = format!(
            "장부 {count}곳을 고치자는 제안을 사용자에게 보였어요. 아직 바뀌지 않았고, 사용자가 확인하면 바뀌어요."
        );
        if !wrong.is_empty() {
            said.push_str(&format!(" 번호 {wrong:?}는 이 회의 장부 항목이 아니라 뺐어요."));
        }
        (said, count)
    }

    /// 화자에게 이름을 붙이자는 제안이에요. 화자는 번호("화자 1")나 지금 이름이에요. 이름이 이미 다른 화자의 것이면 같은
    /// 사람이라는 뜻이라 합치기 제안으로 바꿔요.
    fn name_speaker(&mut self, args: &Value) -> (String, usize) {
        let id = match self.editable(args) {
            Ok(id) => id,
            Err(e) => return (e, 0),
        };
        let asked = args["speaker"].as_str().unwrap_or("").trim().to_string();
        let name = args["name"].as_str().unwrap_or("").trim().to_string();
        if asked.is_empty() || name.is_empty() {
            return ("speaker와 name을 넣어요.".into(), 0);
        }
        let all = self.speaker_line(&id).unwrap_or_default();
        let Some((key, shown, count)) = self.speaker(&id, &asked) else {
            return (format!("이 회의에 「{asked}」 화자가 없어요. 화자는 {all}예요."), 0);
        };
        if let Some(into) = self.speaker(&id, &name).filter(|t| t.0 != key) {
            // iPhone은 화자 합치기가 동기화되지 않아서 합치기를 빼 뒀어요(docs/40 4-5절)
            if self.engine.iphone {
                return (format!(
                    "iPhone에서는 아직 화자를 합칠 수 없어요. 「{name}」은 이미 이 회의의 다른 화자({})의 이름이라, 이 이름을 붙이면 \
                     두 화자를 합치는 일이 돼요. 제안하지 않았고, 사용자에게 그렇게 답해요.", into.0
                ), 0);
            }
            return self.propose_merge(&id, vec![(key, shown, count)], into);
        }
        if shown == name {
            return (format!("「{key}」은 이미 「{name}」이에요."), 0);
        }
        let p = json!({"kind": "name_speaker", "meeting_id": id, "key": key, "speaker": shown, "name": name, "lines": count});
        self.propose(&format!("speaker:{id}:{key}"), p);
        (format!(
            "「{shown}」에게 「{name}」 이름을 붙이자는 제안을 사용자에게 보였어요(대화 기록 {count}줄). 이름은 회의 전체에 같이 붙어요. \
             아직 바뀌지 않았고, 사용자가 확인하면 바뀌어요."
        ), 1)
    }

    /// 같은 사람인데 여러 화자로 나뉜 것을 한 화자로 합치자는 제안이에요.
    fn merge_speakers(&mut self, args: &Value) -> (String, usize) {
        let id = match self.editable(args) {
            Ok(id) => id,
            Err(e) => return (e, 0),
        };
        let all = self.speaker_line(&id).unwrap_or_default();
        let into_said = args["into"].as_str().unwrap_or("").trim().to_string();
        let Some(into) = self.speaker(&id, &into_said) else {
            return (format!("이 회의에 「{into_said}」 화자가 없어요. 화자는 {all}예요. 합칠 대상은 지금 있는 화자여야 해요."), 0);
        };
        let mut from = Vec::new();
        let mut missing = Vec::new();
        for f in args["from"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
            match self.speaker(&id, f.trim()) {
                Some(t) if t.0 != into.0 && !from.iter().any(|x: &(String, String, usize)| x.0 == t.0) => from.push(t),
                Some(_) => {}
                None => missing.push(f.trim().to_string()),
            }
        }
        if from.is_empty() {
            return (format!("합칠 화자를 찾지 못했어요: {missing:?}. 화자는 {all}예요."), 0);
        }
        let (said, n) = self.propose_merge(&id, from, into);
        if missing.is_empty() { (said, n) } else { (format!("{said} 「{}」은 이 회의 화자가 아니라 뺐어요.", missing.join(", ")), n) }
    }

    /// 대화 기록 몇 줄만 다른 화자의 말로 옮기자는 제안이에요. 줄은 transcript_window의 [번호]나 발화 key예요. `to`가 이
    /// 회의의 화자가 아니면 그 이름의 새 사람이에요(앱의 "이 줄만"과 같아요).
    fn move_lines(&mut self, args: &Value) -> (String, usize) {
        let id = match self.editable(args) {
            Ok(id) => id,
            Err(e) => return (e, 0),
        };
        let to_said = args["to"].as_str().unwrap_or("").trim().to_string();
        if to_said.is_empty() {
            return ("to에 옮길 화자를 넣어요.".into(), 0);
        }
        let transcript = self.req.transcripts.get(&id).cloned().unwrap_or_default();
        let mut keys: Vec<String> = Vec::new();
        let mut wrong = Vec::new();
        for v in args["lines"].as_array().into_iter().flatten() {
            let key = match (v.as_u64(), v.as_str()) {
                (Some(n), _) => match self.sources.get((n as usize).wrapping_sub(1)) {
                    Some(Source::Line { meeting_id, key, .. }) if *meeting_id == id => Some(key.clone()),
                    _ => None,
                },
                (None, Some(t)) => {
                    let t = t.trim().trim_start_matches('[').trim_end_matches(']');
                    match t.parse::<usize>() {
                        Ok(n) => match self.sources.get(n.wrapping_sub(1)) {
                            Some(Source::Line { meeting_id, key, .. }) if *meeting_id == id => Some(key.clone()),
                            _ => None,
                        },
                        Err(_) => transcript.iter().any(|l| l["key"].as_str() == Some(t)).then(|| t.to_string()),
                    }
                }
                _ => None,
            };
            match key {
                Some(k) if !keys.contains(&k) => keys.push(k),
                Some(_) => {}
                None => wrong.push(v.to_string()),
            }
        }
        if keys.is_empty() {
            return ("옮길 줄을 찾지 못했어요. transcript_window로 그 말을 찾아 [번호]를 넣어요.".into(), 0);
        }
        let to = self.speaker(&id, &to_said);
        let lines: Vec<Value> = keys
            .iter()
            .filter_map(|k| transcript.iter().find(|l| l["key"].as_str() == Some(k.as_str())))
            .map(|l| {
                let text: String = l["text"].as_str().unwrap_or("").chars().take(60).collect();
                json!({"key": l["key"], "speaker": l["speaker"], "text": text})
            })
            .collect();
        let target = match &to {
            Some((k, n, _)) => json!({"key": k, "name": n}),
            None => json!({"name": to_said}),
        };
        let shown = to.as_ref().map(|t| t.1.clone()).unwrap_or(to_said.clone());
        let n = lines.len();
        let p = json!({"kind": "move_lines", "meeting_id": id, "moved": lines, "target": target});
        self.propose(&format!("move:{id}:{}", keys.join(",")), p);
        let mut said = format!(
            "대화 기록 {n}줄을 「{shown}」의 말로 옮기자는 제안을 사용자에게 보였어요{}. 아직 바뀌지 않았고, 사용자가 확인하면 바뀌어요.",
            if to.is_none() { "(이 회의의 화자가 아니라 새 이름으로 붙어요)" } else { "" }
        );
        if !wrong.is_empty() {
            said.push_str(&format!(" {}는 이 회의 대화 기록 줄이 아니라 뺐어요.", wrong.join(", ")));
        }
        (said, n)
    }

    /// 줄마다 그 말을 한 화자를 다시 정하자는 제안이에요(화자 분리가 한두 줄짜리 화자를 만들었을 때). 줄은
    /// transcript_window의 [번호], 화자는 이 회의 화자의 번호나 이름이에요.
    fn assign_lines(&mut self, args: &Value) -> (String, usize) {
        let id = match self.editable(args) {
            Ok(id) => id,
            Err(e) => return (e, 0),
        };
        let transcript = self.req.transcripts.get(&id).cloned().unwrap_or_default();
        let all = self.speaker_line(&id).unwrap_or_default();
        let mut moves: Vec<Value> = Vec::new();
        let mut wrong: Vec<String> = Vec::new();
        for m in args["moves"].as_array().into_iter().flatten() {
            let n = m["line"].as_u64().map(|n| n as usize)
                .or_else(|| m["line"].as_str().and_then(|t| t.trim().trim_matches(|c| c == '[' || c == ']').parse().ok()));
            let key = match n.and_then(|n| self.sources.get(n.wrapping_sub(1))) {
                Some(Source::Line { meeting_id, key, .. }) if *meeting_id == id => key.clone(),
                _ => {
                    wrong.push(m["line"].to_string());
                    continue;
                }
            };
            let to_said = m["to"].as_str().unwrap_or("").trim();
            let Some((to_key, to_name, _)) = self.speaker(&id, to_said) else {
                wrong.push(format!("{to_said}(화자 아님)"));
                continue;
            };
            let Some(l) = transcript.iter().find(|l| l["key"].as_str() == Some(key.as_str())) else { continue };
            let now = l["voice"].as_str().filter(|v| !v.is_empty()).or(l["speaker"].as_str()).unwrap_or("");
            if now == to_key || moves.iter().any(|x| x["key"] == json!(key)) {
                continue;
            }
            let text: String = l["text"].as_str().unwrap_or("").chars().take(60).collect();
            moves.push(json!({"key": key, "speaker": l["speaker"], "text": text, "to": {"key": to_key, "name": to_name}}));
        }
        if moves.is_empty() {
            return (format!("옮길 줄이 없어요{}. 화자는 {all}예요. transcript_window의 speaker로 그 화자의 줄을 읽고 [번호]를 넣어요.",
                            if wrong.is_empty() { String::new() } else { format!("(맞지 않은 것: {})", wrong.join(", ")) }), 0);
        }
        let n = moves.len();
        let p = json!({"kind": "assign_lines", "meeting_id": id, "assigned": moves});
        self.propose(&format!("assign:{id}"), p);
        let mut said = format!("대화 기록 {n}줄의 화자를 다시 정하자는 제안을 사용자에게 보였어요. 아직 바뀌지 않았고, 사용자가 줄마다 확인하고 바꿔요.");
        if !wrong.is_empty() {
            said.push_str(&format!(" 맞지 않아 뺀 것: {}.", wrong.join(", ")));
        }
        (said, n)
    }

    /// 회의 정리를 사용자 요청대로 다시 쓰자는 제안이에요. 다시 쓰기는 앱이 이 Mac의 대화 기록으로 해요.
    fn rewrite_notes(&mut self, args: &Value) -> (String, usize) {
        let id = match self.editable(args) {
            Ok(id) => id,
            Err(e) => return (e, 0),
        };
        if !self.req.transcripts.contains_key(&id) {
            return (format!("이 회의는 대화 기록이 {} 없어서 정리를 다시 쓸 수 없어요.", self.here()), 0);
        }
        let request: String = args["request"].as_str().unwrap_or("").trim().chars().take(300).collect();
        let p = json!({"kind": "rewrite_notes", "meeting_id": id, "request": request});
        self.propose(&format!("rewrite:{id}"), p);
        ("회의 정리를 다시 쓰자는 제안을 사용자에게 보였어요. 아직 바뀌지 않았고, 사용자가 확인하면 다시 써요. 지금 정리는 되돌리기로 돌아올 수 있어요.".into(), 1)
    }

    /// 정리 형식(회의, 인터뷰·대담, 강의·발표)을 바꿔 다시 쓰자는 제안이에요.
    fn change_kind(&mut self, args: &Value) -> (String, usize) {
        let id = match self.editable(args) {
            Ok(id) => id,
            Err(e) => return (e, 0),
        };
        if !self.req.transcripts.contains_key(&id) {
            return (format!("이 회의는 대화 기록이 {} 없어서 정리 형식을 바꿀 수 없어요.", self.here()), 0);
        }
        let kind = args["kind"].as_str().unwrap_or("").trim().to_string();
        if !["meeting", "interview", "talk"].contains(&kind.as_str()) {
            return ("kind는 meeting, interview, talk 중 하나예요.".into(), 0);
        }
        let p = json!({"kind": "change_kind", "meeting_id": id, "notes_kind": kind});
        self.propose(&format!("kind:{id}"), p);
        ("정리 형식을 바꿔 다시 쓰자는 제안을 사용자에게 보였어요. 아직 바뀌지 않았고, 사용자가 확인하면 바뀌어요.".into(), 1)
    }

    /// 회의 제목을 바꾸자는 제안이에요.
    fn rename_title(&mut self, args: &Value) -> (String, usize) {
        let id = match self.editable(args) {
            Ok(id) => id,
            Err(e) => return (e, 0),
        };
        let title: String = args["title"].as_str().unwrap_or("").trim().chars().take(80).collect();
        if title.is_empty() {
            return ("title에 새 제목을 넣어요.".into(), 0);
        }
        let was = self.meeting(&id).map(|m| m.title.clone()).unwrap_or_default();
        if was == title {
            return ("이미 그 제목이에요.".into(), 0);
        }
        let p = json!({"kind": "rename_title", "meeting_id": id, "title": title, "was": was});
        self.propose(&format!("title:{id}"), p);
        (format!("제목을 「{title}」로 바꾸자는 제안을 사용자에게 보였어요. 아직 바뀌지 않았고, 사용자가 확인하면 바뀌어요."), 1)
    }

    fn propose_merge(&mut self, id: &str, from: Vec<(String, String, usize)>, into: (String, String, usize)) -> (String, usize) {
        let lines: usize = from.iter().map(|f| f.2).sum();
        let names: Vec<String> = from.iter().map(|f| f.1.clone()).collect();
        let p = json!({
            "kind": "merge_speakers", "meeting_id": id, "lines": lines,
            "from": from.iter().map(|(k, n, c)| json!({"key": k, "name": n, "lines": c})).collect::<Vec<_>>(),
            "into": {"key": into.0, "name": into.1, "lines": into.2},
        });
        self.propose(&format!("merge:{id}:{}", into.0), p);
        (format!(
            "「{}」을 「{}」 한 사람으로 합치자는 제안을 사용자에게 보였어요(대화 기록 {lines}줄이 옮겨 가요). \
             아직 바뀌지 않았고, 사용자가 확인하면 바뀌어요.", names.join(", "), into.1
        ), 1)
    }

    /// 답의 [n]을 카드 순서 번호로 바꾸고, 없는 번호는 지워요. 인용한 출처만 카드·항목·발화 key로 돌려줘요.
    fn cite(&self, answer: &str) -> Value {
        let answer = &PLACE_MARK.replace_all(answer, "").into_owned();
        let mut cards: Vec<Value> = Vec::new();
        let mut items: Vec<Value> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        // 발화 key는 회의마다 따로라서, 앱에는 어느 회의의 발화인지 같이 줘요.
        let mut lines: Vec<Value> = Vec::new();
        let mut number: HashMap<usize, usize> = HashMap::new();
        for cap in MARK.captures_iter(answer) {
            let Some(n) = cap[1].parse::<usize>().ok().filter(|n| (1..=self.sources.len()).contains(n)) else {
                continue;
            };
            if number.contains_key(&n) {
                continue;
            }
            let s = &self.sources[n - 1];
            if let Source::Item { meeting_id, index, .. } = s
                && !self.asked_kinds.is_empty()
                && self
                    .meeting(meeting_id)
                    .and_then(|m| m.ledger.get(*index))
                    .is_some_and(|e| !self.asked_kinds.contains(&e.kind))
            {
                // 묻지 않은 종류의 항목이에요: 카드 없는 출처처럼 표시를 지우고 items에도 넣지 않아요.
                number.insert(n, 0);
                continue;
            }
            match s {
                Source::Item { meeting_id, index, .. } => {
                    let it = json!({"meeting_id": meeting_id, "index": index});
                    if !items.contains(&it) {
                        items.push(it);
                    }
                }
                Source::Line { meeting_id, key, .. } => {
                    let line = json!({"meeting_id": meeting_id, "key": key});
                    if !lines.contains(&line) {
                        lines.push(line);
                        keys.push(key.clone());
                    }
                }
                _ => {}
            }
            if let Some(card) = s.card() {
                let path = card["path"].as_str().unwrap_or("");
                let at = match cards.iter().position(|c| c["path"].as_str() == Some(path)) {
                    Some(p) => p,
                    None => {
                        cards.push(card.clone());
                        cards.len() - 1
                    }
                };
                number.insert(n, at + 1);
            } else {
                number.insert(n, 0);
            }
        }
        // 같은 카드를 가리키는 번호가 이어지면([2][2]) 하나로 줄여요.
        let replaced = MARK.replace_all(answer, |c: &regex::Captures| {
            match c[1].parse::<usize>().ok().and_then(|n| number.get(&n)) {
                Some(&k) if k > 0 => format!("[{k}]"),
                _ => String::new(),
            }
        });
        let mut text = replaced.into_owned();
        for k in 1..=cards.len() {
            let twice = format!("[{k}][{k}]");
            while text.contains(&twice) {
                text = text.replace(&twice, &format!("[{k}]"));
            }
        }
        json!({"answer": text.trim(), "cards": cards, "items": items, "keys": keys, "lines": lines})
    }
}

/// `find`의 낱말이 많이 나온 발화 [`WINDOW_MATCHES`]곳이에요(말한 사람 이름도 세요). 낱말마다 드문 정도(그 낱말이
/// 나온 발화가 적을수록 무거워요)로 세요: "차관"처럼 말한 사람 이름에 늘 나오는 낱말이 "과기부" 같은 낱말을 누르지
/// 않게요. 점수가 같으면 앞의 발화예요.
/// 서버에 보내는 검색어예요: 앞뒤 공백을 빼고 [`QUERY_CHARS`]자까지예요.
fn search_query(args: &Value) -> String {
    args["query"].as_str().unwrap_or("").trim().chars().take(QUERY_CHARS).collect()
}

/// 검색어 낱말이에요: 공백과 문장 부호로 나누고 소문자로, 한 글자 낱말은 빼요.
fn query_words(query: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for w in query.split(|c: char| !c.is_alphanumeric()) {
        let w = w.to_lowercase();
        if w.chars().count() >= 2 && !words.contains(&w) {
            words.push(w);
        }
    }
    words
}

/// 낱말 하나가 글에 나오는지예요. 세 글자 이상 한글 낱말은 끝 글자(조사, "가격은"의 "은")를 뺀 말도 봐요.
fn word_in(hay: &str, word: &str) -> bool {
    if hay.contains(word) {
        return true;
    }
    let chars: Vec<char> = word.chars().collect();
    chars.len() >= 3
        && chars.last().is_some_and(|c| ('가'..='힣').contains(c))
        && hay.contains(&chars[..chars.len() - 1].iter().collect::<String>())
}

/// 서버 회의 검색이 안 될 때의 대체 검색이에요(docs/40 4-4절). 회의의 제목·요약·주제 제목·장부 글에 검색어 낱말이
/// 몇 개 나오는지로 점수를 매겨(제목은 두 배) 높은 순서로 `n`개까지 줘요. 점수가 0인 회의는 빼요.
pub fn keyword_meetings<'a>(meetings: &'a [Meeting], query: &str, n: usize) -> Vec<&'a Meeting> {
    let words = query_words(query);
    if words.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &Meeting)> = meetings
        .iter()
        .map(|m| {
            let title = m.title.to_lowercase();
            let mut rest = m.summary.join("\n");
            for t in &m.topics {
                rest.push('\n');
                rest.push_str(&t.title);
            }
            for e in &m.ledger {
                rest.push('\n');
                rest.push_str(&e.text);
            }
            let rest = rest.to_lowercase();
            let score = words.iter().map(|w| 2 * word_in(&title, w) as usize + word_in(&rest, w) as usize).sum();
            (score, m)
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().take(n).map(|(_, m)| m).collect()
}

fn best_lines(lines: &[Value], find: &str, around: usize) -> Vec<usize> {
    let words: Vec<String> = find.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let hays: Vec<String> = lines
        .iter()
        .map(|l| format!("{} {}", l["speaker"].as_str().unwrap_or(""), l["text"].as_str().unwrap_or("")).to_lowercase())
        .collect();
    let n = hays.len() as f64;
    let weight: Vec<f64> = words
        .iter()
        .map(|w| {
            let df = hays.iter().filter(|h| h.contains(w.as_str())).count() as f64;
            if df == 0.0 { 0.0 } else { (1.0 + n / df).ln() }
        })
        .collect();
    let mut scored: Vec<(usize, f64)> = hays
        .iter()
        .enumerate()
        .map(|(i, h)| (i, words.iter().zip(&weight).filter(|(w, _)| h.contains(w.as_str())).map(|(_, x)| x).sum()))
        .filter(|&(_, x)| x > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    // 고른 발화 가까이(앞뒤 `around`)의 발화는 건너뛰어요: 비슷한 줄이 몰린 한 곳이 구간을 다 차지하지 않게요.
    let mut out: Vec<usize> = Vec::new();
    for (i, _) in scored {
        if out.len() == WINDOW_MATCHES {
            break;
        }
        if out.iter().all(|&o| o.abs_diff(i) > around) {
            out.push(i);
        }
    }
    out.sort_unstable();
    out
}

fn date(start: &str) -> String {
    start.chars().take(10).collect()
}

/// 회의 시작 날짜(그 회의의 시간대 그대로)가 `from`–`to` 안이에요. 둘 다 그날을 넣어요.
fn in_range(start: &str, from: Option<&str>, to: Option<&str>) -> bool {
    let d = date(start);
    from.is_none_or(|f| f.is_empty() || d.as_str() >= f) && to.is_none_or(|t| t.is_empty() || d.as_str() <= t)
}

/// 장부 항목 한 줄의 글이에요: 글, 담당·기한·끝남, 이전 결정이에요. `filter_ledger`는 한국어에 이전 결정은 글만
/// (`item_text`), iPhone `search`는 회의 언어에 이전 결정의 날짜와 회의 제목까지 써요(`full`).
fn item_body(e: &meeting::Entry, en: bool, full: bool) -> String {
    let (owner, due, done, before) = if en { ("owner", "due", "done", "earlier decision") } else { ("담당", "기한", "끝남", "이전 결정") };
    let mut extra = Vec::new();
    if let Some(o) = &e.owner {
        extra.push(format!("{owner} {o}"));
    }
    if let Some(d) = &e.due {
        extra.push(format!("{due} {d}"));
    }
    if e.kind == "commitment" && e.done {
        extra.push(done.into());
    }
    let extra = if extra.is_empty() { String::new() } else { format!(" ({})", extra.join(", ")) };
    let earlier = e.replaces.as_ref().map(|r| {
        let at = [r.at.as_str(), r.title.as_str()].into_iter().filter(|x| full && !x.is_empty()).collect::<Vec<_>>().join(" ");
        if at.is_empty() { format!(" ({before}: {})", r.text) } else { format!(" ({before} {at}: {})", r.text) }
    });
    format!("{}{extra}{}", e.text, earlier.unwrap_or_default())
}

/// `filter_ledger`의 항목 글이에요(Mac과 iPhone 같아요).
fn item_text(e: &meeting::Entry) -> String {
    item_body(e, false, false)
}

/// 장부 종류 이름이에요. `filter_ledger`와 같은 한국어 이름이고, 영어 회의는 영어예요.
fn kind_name(kind: &str, en: bool) -> &'static str {
    if !en {
        return kind_ko(kind);
    }
    match kind {
        "decision" => "Decision",
        "commitment" => "To-do",
        "open_question" => "Open discussion",
        _ => "Item",
    }
}

fn kind_ko(kind: &str) -> &'static str {
    match kind {
        "decision" => "결정",
        "commitment" => "할 일",
        "open_question" => "남은 논의",
        _ => "항목",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_inclusive() {
        assert!(in_range("2025-02-17T14:00:00+09:00", Some("2025-01-01"), Some("2025-02-17")));
        assert!(!in_range("2024-11-26T15:00:00+09:00", Some("2025-01-01"), None));
        assert!(in_range("2024-11-26T15:00:00+09:00", None, None));
    }

    #[test]
    fn transcript_find_ranks_lines_by_words() {
        let l = |s: &str, t: &str| json!({"speaker": s, "text": t});
        let lines = vec![
            l("민호 팀장", "안녕하세요"),
            l("지윤 매니저", "환불 보증은 20%예요"),
            l("민호 팀장", "환불을 고객한테 통지하면 이중결제처럼 처리돼요"),
            l("민호 팀장", "네"),
        ];
        assert!(best_lines(&lines, "민호 환불 통지", 0).contains(&2));
        // a word on every speaker line weighs little: the line with the rare word wins
        let lines2: Vec<Value> = (0..10)
            .map(|i| l("서연 팀장", if i == 7 { "결제팀은 재무 소속인가요" } else { "결제팀 계획은 저희가 세워요" }))
            .collect();
        assert!(best_lines(&lines2, "팀장 결제팀 재무", 1).contains(&7));
        assert_eq!(best_lines(&lines, "이중결제", 0), vec![2]);
        assert!(best_lines(&lines, "  ", 0).is_empty());
    }

    #[test]
    fn place_marks_leave_the_answer() {
        let a = "w 번호는 지워요 [w3]. 여러 곳도 (w1, w5) 지우고 [w1–w9] 범위도요. 출처 [2]는 남아요.";
        assert_eq!(PLACE_MARK.replace_all(a, ""), "w 번호는 지워요. 여러 곳도 지우고 범위도요. 출처 [2]는 남아요.");
    }

    #[test]
    fn asking_back_is_told_from_an_answer() {
        assert!(asks_back("화자 6만 김 대표로 옮기는 제안을 올릴까요, 아니면 그대로 둘까요?"));
        assert!(asks_back("이 판단으로 바꿔도 되는지 알려 주세요."));
        assert!(!asks_back("화자 5와 6의 줄 3개를 하늘 님으로 옮기자는 카드를 띄웠어요. 아래에서 확인해 주시면 바뀌어요."));
        assert!(!asks_back("이번 달에 미뤄진 결정은 세 가지예요 [1]."));
    }

    #[test]
    fn keyword_meetings_rank_title_first_and_drop_zero() {
        let m = |id: &str, title: &str, summary: &str| Meeting {
            id: id.into(), title: title.into(), start: String::new(), end: String::new(),
            summary: vec![summary.into()], topics: vec![], ledger: vec![],
        };
        let all = [m("a", "채용 회의", "가격 이야기도 잠깐 했어요."), m("b", "가격 정책", ""), m("c", "주간 회의", "출시")];
        let ids: Vec<&str> = keyword_meetings(&all, "가격은 어떻게 정했죠?", 8).iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["b", "a"]);
        assert!(keyword_meetings(&all, "?", 8).is_empty());
    }

    #[test]
    fn names_pick_iphone_sets() {
        assert_eq!(names(false, false, false), ("panel_chat", "panel"));
        assert_eq!(names(false, false, true), ("panel_chat", "panel"));
        assert_eq!(names(false, true, true), ("panel_chat", "panel_meeting_edit"));
        assert_eq!(names(false, true, false), ("panel_chat", "panel_notes_edit"));
        assert_eq!(names(true, false, false), ("panel_chat_iphone", "panel_iphone"));
        assert_eq!(names(true, false, true), ("panel_chat_iphone", "panel_iphone"));
        assert_eq!(names(true, true, true), ("panel_chat_iphone", "panel_iphone_meeting"));
        assert_eq!(names(true, true, false), ("panel_chat_iphone", "panel_iphone_notes"));
    }

    #[test]
    fn shown_args_keep_only_safe_fields() {
        let a = shown_args(&json!({"query": "가".repeat(50), "text": "x", "wide": true}));
        assert_eq!(a["query"].as_str().unwrap().chars().count(), 40);
        assert!(a.get("text").is_none());
        assert_eq!(a["wide"], true);
    }
}
