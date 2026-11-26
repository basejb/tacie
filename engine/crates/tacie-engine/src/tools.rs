//! The meeting and ledger lists the local MCP tools read (docs/38 4절). They only filter meetings the app sent
//! (`POST /meetings`), and transcribed text is not here. Character counts and item counts are cut so a tool result
//! fits on one line (100KB).

use serde_json::{Value, json};
use tacie_sources::meeting::Meeting;

/// 회의 목록에 한 번에 싣는 회의 수예요.
const MEETINGS_CAP: usize = 30;
/// 장부 목록에 한 번에 싣는 항목 수예요.
const LEDGER_CAP: usize = 50;
/// 회의 목록에서 요약은 앞 두 줄만 실어요.
const SUMMARY_LINES: usize = 2;

pub struct MeetingQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub text: Option<String>,
    pub ids: Vec<String>,
    pub limit: usize,
}

pub struct LedgerQuery {
    pub kind: Option<String>,
    pub owner: Option<String>,
    pub done: Option<bool>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub meeting_ids: Vec<String>,
    pub limit: usize,
}

/// 0이나 상한 넘는 값은 상한이에요.
fn cap(limit: usize, max: usize) -> usize {
    if limit == 0 { max } else { limit.min(max) }
}

/// `start`의 날짜(`YYYY-MM-DD`)가 `from`~`to`(그날 하루 전체) 안인지예요. 날짜끼리 글자로 견줘요.
fn in_range(start: &str, from: Option<&str>, to: Option<&str>) -> bool {
    let day: String = start.chars().take(10).collect();
    let after = from.is_none_or(|f| day.as_str() >= f.get(..10).unwrap_or(f));
    let before = to.is_none_or(|t| day.as_str() <= t.get(..10).unwrap_or(t));
    after && before
}

fn has(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.to_lowercase())
}

/// 가장 늦은 회의가 앞이에요.
fn newest_first(all: &[Meeting]) -> Vec<&Meeting> {
    let mut v: Vec<&Meeting> = all.iter().collect();
    v.sort_by(|a, b| b.start.cmp(&a.start));
    v
}

pub fn list_meetings(all: &[Meeting], q: &MeetingQuery) -> Value {
    let text = q.text.as_deref().filter(|t| !t.trim().is_empty());
    let hits: Vec<&Meeting> = newest_first(all)
        .into_iter()
        .filter(|m| q.ids.is_empty() || q.ids.contains(&m.id))
        .filter(|m| in_range(&m.start, q.from.as_deref(), q.to.as_deref()))
        .filter(|m| text.is_none_or(|t| has(&m.title, t) || m.summary.iter().any(|s| has(s, t))))
        .collect();
    let n = cap(q.limit, MEETINGS_CAP);
    let more = hits.len() > n;
    let detail = !q.ids.is_empty();
    let meetings: Vec<Value> = hits
        .into_iter()
        .take(n)
        .map(|m| {
            let mut item = json!({
                "id": m.id,
                "title": m.title,
                "start": m.start,
                "end": m.end,
                "summary": m.summary.iter().take(SUMMARY_LINES).collect::<Vec<_>>(),
            });
            if detail {
                item["topics"] = json!(m.topics);
                item["ledger"] = json!(m.ledger);
            }
            item
        })
        .collect();
    json!({"meetings": meetings, "more": more})
}

pub fn list_ledger(all: &[Meeting], q: &LedgerQuery) -> Value {
    let owner = q.owner.as_deref().filter(|o| !o.trim().is_empty());
    let mut hits: Vec<Value> = Vec::new();
    for m in newest_first(all) {
        if !q.meeting_ids.is_empty() && !q.meeting_ids.contains(&m.id) {
            continue;
        }
        if !in_range(&m.start, q.from.as_deref(), q.to.as_deref()) {
            continue;
        }
        for e in &m.ledger {
            if q.kind.as_deref().is_some_and(|k| k != e.kind)
                || q.done.is_some_and(|d| d != e.done)
                || owner.is_some_and(|o| !e.owner.as_deref().is_some_and(|w| has(w, o)))
            {
                continue;
            }
            let mut item = json!({
                "kind": e.kind,
                "text": e.text,
                "owner": e.owner,
                "due": e.due,
                "done": e.done,
                "meeting": {"id": m.id, "title": m.title, "start": m.start},
            });
            if let Some(r) = &e.replaces {
                item["replaces"] = json!(r);
            }
            hits.push(item);
        }
    }
    let n = cap(q.limit, LEDGER_CAP);
    let more = hits.len() > n;
    hits.truncate(n);
    json!({"entries": hits, "more": more})
}

/// 검색 도구 카드의 발췌 글자 수 상한이에요.
const SNIPPET_CAP: usize = 600;
/// 검색 도구가 돌려주는 카드 수예요.
pub const SEARCH_CARDS: usize = 8;

/// `ask_scoped_turns`의 결과를 MCP 검색 결과로 바꿔요(docs/38 4절). 답은 쓰지 않아요.
/// `confident`는 앱 카드와 같은 기준(최고 점수 ≥ 보여 주는 기준)이고, 리랭커가 실패하면 점수 없이 임베딩 순서 그대로라
/// `ranked: false`와 `reason`(오류 종류)을 달아요.
pub fn mcp_cards(found: &Value) -> Value {
    let cards: Vec<Value> = found["cards"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|c| {
            let snippet: String = c["snippet"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(SNIPPET_CAP)
                .collect();
            let mut out = json!({
                "title": c["title"],
                "source": c["kind"],
                "url": c["url"],
                "snippet": snippet,
                "score": c["score"],
            });
            if let Some(id) = c["path"].as_str().and_then(|p| p.strip_prefix("meeting/")) {
                out["meeting_id"] = json!(id);
            }
            out
        })
        .collect();
    let reason = found.get("error_kind").filter(|k| !k.is_null());
    let ranked = reason.is_none();
    let mut out = json!({
        "cards": cards,
        "confident": ranked && found["shown"].as_bool().unwrap_or(false),
        "ranked": ranked,
    });
    if let Some(k) = reason {
        out["reason"] = k.clone();
    }
    out
}
