//! "Next meeting prep" in the main window panel (docs/36 1-3, 2-2절). Gathers unfinished items from saved meetings'
//! ledgers, finds related text in connected meetings and sources, and has the model write a one-line tip and groups
//! of related topics.
//! Ledger items are returned as they are even if the model drops them (this part is built without the model).

use std::collections::HashSet;
use std::path::Path;

use serde_json::{Value, json};
use tacie_sources::meeting::{self, Meeting, OpenItem};

use crate::errors::Kind;
use crate::harness::{QUERY_CHARS, SEARCH_TOP, keyword_meetings};
use crate::pipeline::{Engine, NO_ANSWER, Scope, head};

/// 준비하는 동안 앱에 알리는 단계예요.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// 읽은 회의(요청한 것 중 저장된 것)예요.
    Meetings { ids: Vec<String> },
    /// 같이 본 다른 회의예요(최대 2개).
    Related { ids: Vec<String> },
    /// 같이 본 자료 카드 수예요(최대 3개).
    Sources { count: usize },
    /// 모델이 쓰는 중이에요(항목이 있을 때만 나와요).
    Writing,
}

const RELATED_MAX: usize = 2;
const SOURCES_MAX: usize = 3;
const QUERY_LINES: usize = 3;

/// 회의의 요약과 주제 제목 첫 줄들이에요.
fn query_lines(m: &Meeting) -> Vec<String> {
    m.summary
        .iter()
        .cloned()
        .chain(m.topics.iter().map(|t| t.title.clone()))
        .filter(|l| !l.trim().is_empty())
        .take(QUERY_LINES)
        .collect()
}

fn item_json(i: &OpenItem) -> Value {
    json!({"meeting_id": i.meeting_id, "index": i.index, "kind": i.kind, "text": i.text,
           "owner": i.owner, "due": i.due})
}

/// `ids` 회의의 장부로 다음 회의를 준비해요. 돌려주는 값은 `{advice, open, todos, sources, meetings_read,
/// sources_read, cost}`이고, 모델이나 검색이 실패하면 `{error, error_kind}`에 `open`·`todos`를 그대로 담아요.
pub async fn prep(engine: &Engine, meeting_dir: &Path, ids: &[String], on_step: impl FnMut(Step)) -> Value {
    prep_searched(engine, meeting_dir, ids, true, on_step).await
}

/// `prep`과 같은데, iPhone 엔진이 관련 회의를 서버 회의 검색에 물어도 되는지(`server`)를 정해요. 하네스는 한 질문의
/// 서버 검색 상한을 다 썼으면 `false`를 넘겨요. 그러면 `meetings.json`에서 낱말로 찾아요. Mac 엔진은 보지 않아요.
pub async fn prep_searched(
    engine: &Engine,
    meeting_dir: &Path,
    ids: &[String],
    server: bool,
    mut on_step: impl FnMut(Step),
) -> Value {
    let loaded = meeting::load(meeting_dir, ids);
    let loaded_ids: Vec<String> = loaded.iter().map(|m| m.id.clone()).collect();
    on_step(Step::Meetings {
        ids: loaded_ids.clone(),
    });

    // 관련 회의와 자료를 찾아요.
    let query = loaded
        .iter()
        .flat_map(query_lines)
        .collect::<Vec<_>>()
        .join("\n");
    let mut ask_error: Option<(Value, Value)> = None;
    let mut related: Vec<String> = Vec::new();
    let mut sources: Vec<Value> = Vec::new();
    let mut cost = 0.0;
    if !query.is_empty() && engine.iphone {
        // iPhone 엔진에는 검색 모델과 색인이 없어요: 관련 회의는 하네스의 `search`와 같은 회의 검색으로 찾고, 자료는
        // 없어요(docs/40 4-4절). 서버 검색이 안 되면 낱말로 찾고, 한도(credit)나 로그인(key) 실패만 실패로 돌려줘요.
        let q: String = query.chars().take(QUERY_CHARS).collect();
        let mut found: Option<Vec<String>> = None;
        if let (Some(searcher), true) = (&engine.services.meeting_search, server) {
            match searcher.search(std::slice::from_ref(&q), SEARCH_TOP).await {
                Ok(results) => {
                    found = Some(results.into_iter().next().unwrap_or_default().into_iter().map(|h| h.id).collect());
                }
                Err(e) if matches!(e.kind, Kind::Credit | Kind::Key) => {
                    ask_error = Some((json!(head(&e.message)), json!(e.kind)));
                    found = Some(Vec::new());
                }
                Err(_) => {}
            }
        }
        let found = found.unwrap_or_else(|| {
            keyword_meetings(&meeting::all(meeting_dir), &query, SEARCH_TOP).into_iter().map(|m| m.id.clone()).collect()
        });
        // 이 기기에 없는 회의는 자리를 차지하기 전에 빼요.
        let here: HashSet<String> = meeting::all(meeting_dir).into_iter().map(|m| m.id).collect();
        for id in found.into_iter().filter(|id| here.contains(id)) {
            if !ids.contains(&id) && !related.contains(&id) && related.len() < RELATED_MAX {
                related.push(id);
            }
        }
    } else if !query.is_empty() {
        let found = engine
            .ask_scoped_n(
                &query,
                &[],
                &Scope::All,
                engine.settings.show,
                SOURCES_MAX + ids.len() + RELATED_MAX,
            )
            .await;
        cost += found["cost_usd"].as_f64().unwrap_or(0.0);
        let cards = found["cards"].as_array().cloned().unwrap_or_default();
        if !found["error_kind"].is_null() && cards.is_empty() {
            ask_error = Some((found["error"].clone(), found["error_kind"].clone()));
        }
        for c in cards {
            match c["path"].as_str().and_then(|p| p.strip_prefix("meeting/")) {
                Some(id) => {
                    let id = id.to_string();
                    // 카드 점수가 보여 주는 기준을 못 넘으면 관련 회의로 치지 않아요.
                    let passes = c["score"].as_f64().is_some_and(|s| s >= engine.settings.show);
                    if passes && !ids.contains(&id) && !related.contains(&id) && related.len() < RELATED_MAX {
                        related.push(id);
                    }
                }
                None if sources.len() < SOURCES_MAX => sources.push(c),
                None => {}
            }
        }
    }
    let other = meeting::load(meeting_dir, &related);
    let related: Vec<String> = other.iter().map(|m| m.id.clone()).collect();
    on_step(Step::Related {
        ids: related.clone(),
    });
    on_step(Step::Sources {
        count: sources.len(),
    });

    let all: Vec<Meeting> = loaded.into_iter().chain(other).collect();
    // 항목은 요청한 회의 것이 앞, 관련 회의 것이 뒤예요. 관련 회의 항목은 모델이 묶는 데만 쓰고 목록에는 안 넣어요.
    let items = meeting::open_items(&all);
    let own_items: Vec<&OpenItem> = items.iter().filter(|i| loaded_ids.contains(&i.meeting_id)).collect();
    let kind_is = |i: &&OpenItem, k: &str| i.kind == k;
    let todos: Vec<Value> = own_items
        .iter()
        .copied()
        .filter(|i| kind_is(i, "commitment"))
        .map(item_json)
        .collect();
    let mut open: Vec<Value> = own_items
        .iter()
        .copied()
        .filter(|i| kind_is(i, "open_question"))
        .map(item_json)
        .collect();
    let meetings_read = loaded_ids.len() + related.len();
    let sources_read = sources.len();
    let base = |open: Vec<Value>, todos: Vec<Value>| {
        json!({"open": open, "todos": todos, "meetings_read": meetings_read,
               "sources_read": sources_read})
    };
    let fail = |error: Value, kind: Value, open: Vec<Value>, todos: Vec<Value>, cost: f64, sources: &[Value]| {
        let mut out = base(open, todos);
        out["error"] = error;
        out["error_kind"] = kind;
        out["advice"] = json!("");
        out["sources"] = json!(sources);
        out["cost"] = json!(cost);
        out
    };

    if let Some((error, kind)) = ask_error {
        for o in &mut open {
            o["also"] = json!([]);
        }
        return fail(error, kind, open, todos, cost, &sources);
    }
    if own_items.is_empty() {
        let mut out = base(open, todos);
        out["advice"] = json!("");
        out["sources"] = json!(sources);
        out["cost"] = json!(cost);
        return out;
    }

    on_step(Step::Writing);
    let date_of = |id: &str| -> String {
        all.iter()
            .find(|m| m.id == id)
            .map(|m| m.start.chars().take(10).collect())
            .unwrap_or_default()
    };
    let payload: Vec<Value> = items
        .iter()
        .map(|i| {
            json!({"kind": i.kind, "text": i.text, "owner": i.owner, "due": i.due,
                   "date": date_of(&i.meeting_id)})
        })
        .collect();
    match engine.services.panel.prep(&payload, &sources).await {
        Err(e) => {
            for o in &mut open {
                o["also"] = json!([]);
            }
            fail(json!(head(&e.message)), json!(e.kind), open, todos, cost, &sources)
        }
        Ok(out) => {
            for o in &mut open {
                let me = o["meeting_id"].as_str().unwrap_or("").to_string();
                let idx = o["index"].as_u64().unwrap_or(0) as usize;
                let pos = items
                    .iter()
                    .position(|i| i.meeting_id == me && i.index == idx);
                let mut also: Vec<String> = Vec::new();
                if let Some(pos) = pos {
                    for g in out.groups.iter().filter(|g| g.contains(&pos)) {
                        for &j in g {
                            if let Some(it) = items.get(j) {
                                if it.meeting_id != me && !also.contains(&it.meeting_id) {
                                    also.push(it.meeting_id.clone());
                                }
                            }
                        }
                    }
                }
                o["also"] = json!(also);
            }
            let seen: HashSet<usize> = out.cited.iter().copied().collect();
            let cited: Vec<Value> = sources
                .iter()
                .enumerate()
                .filter(|(i, _)| seen.contains(i))
                .map(|(_, c)| c.clone())
                .collect();
            let mut res = base(open, todos);
            res["advice"] = json!(out.advice);
            // 모델이 쓴 자료가 있으면 그것만, 없으면 찾은 자료를 그대로 보여 줘요.
            res["sources"] = json!(if cited.is_empty() { sources.clone() } else { cited });
            res["cost"] = json!(cost + out.cost);
            res
        }
    }
}

/// 결정 하나의 이유예요(docs/36 2-2절). 돌려주는 값은 `{reason, keys, cost}`이고, window가 비면 모델을 부르지 않고
/// `{reason: NO_ANSWER, keys: []}`예요. 모델이 실패하면 `{error, error_kind, reason: "", keys: [], cost: 0}`예요
/// (앱은 실패한 답도 같은 모양으로 읽어요).
pub async fn why(engine: &Engine, decision: &str, window: &[Value]) -> Value {
    if window.is_empty() {
        return json!({"reason": NO_ANSWER, "keys": [], "cost": 0.0});
    }
    match engine.services.panel.why(decision, window).await {
        Ok(out) => json!({"reason": out.reason, "keys": out.keys, "cost": out.cost}),
        Err(e) => json!({"error": head(&e.message), "error_kind": e.kind, "reason": "", "keys": [], "cost": 0.0}),
    }
}

/// 준비한 항목마다 새 회의에서 정했는지 가려요. 돌려주는 값은 `{results: [{i, decided, how}], cost}`이고,
/// 모델이 실패하면 `{error, error_kind}`예요.
pub async fn followup(engine: &Engine, prepared: &[Value], ledger: &[Value]) -> Value {
    if prepared.is_empty() {
        return json!({"results": [], "cost": 0.0});
    }
    match engine.services.panel.followup(prepared, ledger).await {
        Ok(out) => {
            let results: Vec<Value> = out
                .results
                .iter()
                .map(|(i, decided, how)| json!({"i": i, "decided": decided, "how": how}))
                .collect();
            json!({"results": results, "cost": out.cost})
        }
        Err(e) => json!({"error": head(&e.message), "error_kind": e.kind}),
    }
}
