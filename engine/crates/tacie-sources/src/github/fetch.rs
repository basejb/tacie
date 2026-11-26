//! GitHub incremental sync.
//!
//! Brings a repo's (`owner/name`) issues, PRs, comments, inline review comments and PR files and reviews up to date,
//! and rewrites only the item files that changed. Before anything else it asks about the repo (`probe_repo`): while
//! GitHub says it is gone, hidden or a different repo, it lists nothing and saves only that check
//! (`{**prev, "version": 2, "repo"}`). If all three lists have cursors, it lists only what changed after them (cursor −
//! `CURSOR_OVERLAP`) and refetches those documents whole, and reconciles when it is time (`RECONCILE_EVERY`). If even
//! one is missing, it lists everything. The state is written once at the very end: an interrupted fetch asks the same
//! range again next time. If reconcile fails, only key errors (key in `Service`) are passed up; the rest leave one line
//! on stderr and wait for the next refresh. Log lines are counts and times only (no issue, PR or comment text).
//!
//! Rules for odd shapes (shapes that neither the state Tacie writes nor GitHub's answers have):
//! - Cursors and `reconciled_at` are read like the ISO format of Python `datetime.strptime`
//!   (`%Y-%m-%dT%H:%M:%SZ`): it accepts single-digit month, day, hour, minute and second, a day with one leading space
//!   (`" 5"`), and lowercase `t`/`z`, and rejects seconds 60 and 61 and dates that do not exist. Only ASCII digits are
//!   accepted. Such a cursor counts as missing, so everything is listed again.
//! - A cursor earlier than 0001-01-01T03:00:00Z fails (`Other`).
//! - The `lost` returned when the repo is lost is the absolute value of `lost` in the state. The caller only checks
//!   whether it is 0, and Tacie never writes negative values.
//! - The number (an issue's `number`) accepts only integers 0 or above; otherwise it is an error (`Json`) before
//!   writing (same as `reconcile`). An issue with no number, a comment whose number cannot be read from its URL, and a
//!   comment with no id attached to a listed issue are also errors (`Json`) before writing.
//! - A truthy `updated_at` that is not a string is an error (`Json`) when setting the cursor.
//! - Comment (id, updated_at) equality is compared as JSON text (`1` and `1.0` differ; same as `reconcile`). List or
//!   object ids are errors.
//! - If a saved item's `extras` has an odd shape, the rule of `extras::incomplete` applies (it counts as incomplete).
//! - The parentheses in the reconcile failure line hold the kind of failure (`key`/`network`/`busy`/`down`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use chrono::{NaiveDate, NaiveDateTime, TimeDelta};
use regex::Regex;
use serde_json::{Map, Value};

use super::extras::{incomplete, pr_extras, update_extras};
use super::items::{
    STREAMS, Talk, build_items, load_items, load_state, number_of, save_item, save_state,
};
use super::probe::probe_repo;
use super::reconcile::{Reconciled, reconcile, talk_of};
use super::refetch::refetch_documents;
use super::{GitHub, ISO, now_iso};
use crate::error::{Kind, SourceError};
use crate::py;

/// 커서보다 이만큼 앞부터 다시 나열해요: 늦게 색인된 항목과 시계 차이 때문이에요. 저장된 것과 같은 문서는
/// 건너뛰니까 싸요.
pub const CURSOR_OVERLAP: TimeDelta = TimeDelta::hours(3);
/// 재조정 사이의 시간이에요.
pub const RECONCILE_EVERY: TimeDelta = TimeDelta::days(7);
/// 1년 1월 1일이에요. 이보다 이른 시각은 실패해요.
const PY_DATE_MIN: NaiveDate = NaiveDate::from_ymd_opt(1, 1, 1).unwrap();

/// `fetch`의 결과예요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchOut {
    /// 쓰거나 지운 항목 수예요.
    pub changed: usize,
    /// 저장소를 잃었다는 답이 이어진 수예요(나열했으면 0).
    pub lost: u64,
}

/// 한 번의 나열이 바꾼 것이에요: 쓰거나 지운 번호, 새 커서, 모든 목록이 온전히 왔는지(재조정만큼 좋아요).
struct Synced {
    changed: BTreeSet<u64>,
    cursors: Map<String, Value>,
    whole: bool,
}

/// 저장소의 이슈·PR을 최신으로 맞춰요. 모듈 설명을 보세요.
pub fn fetch(gh: &GitHub, repo: &str, raw: &Path) -> Result<FetchOut, SourceError> {
    fs::create_dir_all(raw)?;
    let mut prev = load_state(raw); // probe_repo와 reconcile이 고치고, 맨 마지막에 저장해요
    let started = now_iso();
    if !probe_repo(gh, repo, &mut prev)? {
        let lost = prev
            .get("lost")
            .and_then(Value::as_i64)
            .map_or(0, i64::unsigned_abs);
        let mut state = prev;
        state.insert("version".into(), Value::from(2));
        state.insert("repo".into(), Value::from(repo));
        save_state(raw, &state)?;
        return Ok(FetchOut { changed: 0, lost });
    }
    let cursors: Vec<Value> = STREAMS.iter().map(|s| cursor(&prev, s)).collect();
    let mut since = Vec::with_capacity(STREAMS.len());
    for c in &cursors {
        since.push(since_of(c)?);
    }
    let full = since.iter().any(Option::is_none);
    let paths = [
        format!("repos/{repo}/issues?state=all"),
        format!("repos/{repo}/issues/comments"),
        format!("repos/{repo}/pulls/comments"),
    ];
    let mut lists: [Vec<Value>; 3] = Default::default();
    for (i, stream) in STREAMS.iter().enumerate() {
        let since = if full { None } else { since[i].as_deref() };
        lists[i] = gh.rest(&paths[i], since, 3)?;
        match since {
            None => eprintln!("github {stream}: {} listed", lists[i].len()),
            Some(at) => eprintln!("github {stream}: {} listed since {at}", lists[i].len()),
        }
    }
    let (old, _) = load_items(raw)?;
    let synced = if full {
        full_sync(gh, repo, raw, &lists, &old, &prev, &started)?
    } else {
        incremental_sync(gh, repo, raw, &lists, &old, &mut prev, &started)?
    };
    let mut state = prev;
    state.insert("version".into(), Value::from(2));
    state.insert("repo".into(), Value::from(repo));
    state.insert("fetched_at".into(), Value::from(started.as_str()));
    state.insert("cursors".into(), Value::Object(synced.cursors));
    if synced.whole {
        // 모든 목록이 온전히 왔어요: 재조정만큼 좋아요
        state.insert("reconciled_at".into(), Value::from(started));
    }
    save_state(raw, &state)?;
    Ok(FetchOut {
        changed: synced.changed.len(),
        lost: 0,
    })
}

/// 모두 나열한 받기예요: 나열된 이슈마다 항목을 만들고(댓글은 저장된 것과 id로 합쳐요) PR 추가 정보를 모두 물은 뒤에
/// 다른 것만 써요. 커서는 항목에 들어간 것으로만 옮겨요.
fn full_sync(
    gh: &GitHub,
    repo: &str,
    raw: &Path,
    lists: &[Vec<Value>; 3],
    old: &BTreeMap<u64, Value>,
    prev: &Map<String, Value>,
    started: &str,
) -> Result<Synced, SourceError> {
    let [issues, comments, review_comments] = lists;
    // 받은 이슈·댓글이 이상한 모양이면 쓰기 전에 오류예요(`build_items`는 건너뛰어요)
    let numbers = issues
        .iter()
        .map(issue_number)
        .collect::<Result<Vec<u64>, _>>()?;
    let talk = talk_of(comments, review_comments)?;
    check_merge(&numbers, &talk, old)?;
    let mut items = build_items(issues, comments, review_comments, old);
    let pulls: Vec<u64> = items
        .iter()
        .filter(|(_, it)| it["kind"] == "pull")
        .map(|(n, _)| *n)
        .collect();
    let at: HashMap<u64, usize> = items
        .iter()
        .enumerate()
        .map(|(i, (n, _))| (*n, i))
        .collect();
    // 쓰기 전이에요: 여기서 실패하면 항목은 그대로예요
    for (n, x) in pr_extras(gh, repo, &pulls)? {
        if let (Some(x), Some(&i)) = (x, at.get(&n))
            && let Some(item) = items[i].1.as_object_mut()
        {
            item.insert("extras".into(), x); // 그 자리에서 바꿔요(키 순서 그대로)
        }
    }
    eprintln!("github pulls: {} asked for files and reviews", pulls.len());
    let mut changed = BTreeSet::new();
    for (n, it) in &items {
        if save_item(raw, it)? {
            changed.insert(*n);
        }
    }
    // 항목에 들어간 댓글만 커서를 옮겨요. 짧은 이슈 목록은 빠뜨린 항목 너머로 커서를 옮길 수 있지만, 그건 주간
    // 재조정이 다시 봐요
    let listed: HashSet<u64> = at.keys().copied().collect();
    let into_items = |list: &[Value], key: &str| -> Vec<Value> {
        list.iter()
            .filter(|c| number_of(c.get(key)).is_some_and(|n| listed.contains(&n)))
            .cloned()
            .collect()
    };
    let used = [
        issues.clone(),
        into_items(comments, "issue_url"),
        into_items(review_comments, "pull_request_url"),
    ];
    let mut cursors = Map::new();
    for (i, stream) in STREAMS.iter().enumerate() {
        let next = match top(&used[i])? {
            Some(at) => Value::from(at),
            None if !lists[i].is_empty() => cursor(prev, stream),
            // 다시 물어도 빈 목록은 지금부터예요: 아무것도 없는 목록(인라인 리뷰 댓글이 없는 저장소)이 받기마다 이리로
            // 돌려보내면 안 돼요
            None => Value::from(started),
        };
        cursors.insert(stream.to_string(), next);
    }
    // 빈 이슈 목록, 또는 이슈가 센 댓글이 있는데 빈 댓글 목록은 온전히 오지 않았어요(리뷰 댓글은 수가 없고 없는
    // 저장소가 많아요)
    let whole = !issues.is_empty() && (!comments.is_empty() || no_comments_counted(issues)?);
    Ok(Synced {
        changed,
        cursors,
        whole,
    })
}

/// 커서 뒤만 나열한 받기예요: 저장된 것과 다른 문서를 통째로 다시 받고, 때가 되면 재조정하고, 바뀌었거나 열렸거나
/// 추가 정보가 모자란 PR을 물어요.
fn incremental_sync(
    gh: &GitHub,
    repo: &str,
    raw: &Path,
    lists: &[Vec<Value>; 3],
    old: &BTreeMap<u64, Value>,
    prev: &mut Map<String, Value>,
    started: &str,
) -> Result<Synced, SourceError> {
    let [listed, comments, review_comments] = lists;
    let mut issues: HashMap<u64, Value> = HashMap::new();
    for it in listed {
        issues.insert(issue_number(it)?, it.clone()); // 같은 번호는 나중 것이에요
    }
    let talk = talk_of(comments, review_comments)?;
    let candidates: BTreeSet<u64> = issues.keys().chain(talk.keys()).copied().collect();
    let mut todo = BTreeSet::new();
    for &n in &candidates {
        if !unchanged(n, old.get(&n), issues.get(&n), talk.get(&n))? {
            todo.insert(n);
        }
    }
    eprintln!(
        "github: {} of {} listed documents downloaded again",
        todo.len(),
        candidates.len()
    );
    let (mut changed, _) = refetch_documents(gh, repo, raw, &todo, &issues, Some(&talk))?;
    // 나열된 댓글은 모두 이제 항목에 들어 있어요
    let mut cursors = Map::new();
    for (i, stream) in STREAMS.iter().enumerate() {
        let next = match top(&lists[i])? {
            Some(at) => Value::from(at),
            None => cursor(prev, stream),
        };
        cursors.insert(stream.to_string(), next);
    }
    if due(prev.get("reconciled_at"), started) {
        let r = match reconcile(gh, repo, raw, prev, Some(started)) {
            Ok(r) => r,
            Err(
                e @ SourceError::Service {
                    kind: Kind::Key, ..
                },
            ) => return Err(e),
            Err(e) => {
                // 위의 문서는 썼어요: 그 커서는 저장하고 재조정은 기다려요
                eprintln!(
                    "github reconcile failed ({}), tried again on the next refresh",
                    e.kind().as_str()
                );
                Reconciled {
                    skipped: Some("failed".to_string()),
                    ..Reconciled::default()
                }
            }
        };
        changed.extend(&r.removed);
        changed.extend(&r.refetched);
        todo.extend(&r.refetched); // 그 PR은 아래에서 파일과 리뷰를 물어요
    }
    let (stored, _) = load_items(raw)?; // 위에서 쓴 그대로예요
    let asked = wanted(&stored, &todo)?;
    changed.extend(update_extras(gh, repo, raw, &asked)?);
    eprintln!(
        "github pulls: {} asked for files and reviews; {} items changed",
        asked.len(),
        changed.len()
    );
    Ok(Synced {
        changed,
        cursors,
        whole: false,
    })
}

/// 상태의 한 목록 커서예요(`load_state`가 세 목록을 늘 채워요).
fn cursor(state: &Map<String, Value>, stream: &str) -> Value {
    state
        .get("cursors")
        .and_then(|c| c.get(stream))
        .cloned()
        .unwrap_or(Value::Null)
}

/// 커서 − `CURSOR_OVERLAP`이에요. 믿을 커서가 없으면 None이에요. 0001-01-01T03:00:00Z보다 이르면 실패해요.
fn since_of(cursor: &Value) -> Result<Option<String>, SourceError> {
    let Some(at) = cursor.as_str().and_then(parse_iso) else {
        return Ok(None);
    };
    match at.checked_sub_signed(CURSOR_OVERLAP) {
        Some(back) if back.date() >= PY_DATE_MIN => Ok(Some(back.format(ISO).to_string())),
        _ => Err(SourceError::Other(
            "a cursor in state.json is too early to go back from".to_string(),
        )),
    }
}

/// 재조정할 때예요: 재조정이 없었거나, 마지막 것이 `RECONCILE_EVERY`보다 오래됐어요.
fn due(reconciled_at: Option<&Value>, now: &str) -> bool {
    let then = reconciled_at.and_then(Value::as_str).and_then(parse_iso);
    match (parse_iso(now), then) {
        (Some(now), Some(then)) => now - then > RECONCILE_EVERY,
        _ => true,
    }
}

/// Python `datetime.strptime`이 ISO 형식(`%Y-%m-%dT%H:%M:%SZ`)으로 만드는 정규식이에요(대소문자 무시, 숫자는
/// ASCII만).
static PY_ISO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^([0-9]{4})-(1[0-2]|0[1-9]|[1-9])-(3[01]|[12][0-9]|0[1-9]|[1-9]| [1-9])",
        r"[Tt](2[0-3]|[01][0-9]|[0-9]):([0-5][0-9]|[0-9]):(6[01]|[0-5][0-9]|[0-9])[Zz]$"
    ))
    .unwrap()
});

/// ISO 시각을 읽어요(모듈 설명). 읽을 수 없으면 None이에요.
fn parse_iso(s: &str) -> Option<NaiveDateTime> {
    let m = PY_ISO.captures(s)?;
    let num = |i: usize| m[i].trim_start_matches(' ').parse::<u32>().ok();
    let year: i32 = m[1].parse().ok()?;
    if year < 1 {
        return None; // 1년보다 앞은 없어요
    }
    NaiveDate::from_ymd_opt(year, num(2)?, num(3)?)?.and_hms_opt(num(4)?, num(5)?, num(6)?)
}

/// 가장 늦은 `updated_at`이에요(글자 순서 = 시간 순서). 없거나 모두 비었으면 None이에요.
fn top(xs: &[Value]) -> Result<Option<String>, SourceError> {
    let mut best: Option<&str> = None;
    for x in xs {
        match x.get("updated_at") {
            Some(Value::String(at)) => {
                if best.is_none_or(|b| at.as_str() > b) {
                    best = Some(at);
                }
            }
            Some(v) if py::truthy(v) => {
                return Err(SourceError::Json(
                    "an updated_at in GitHub's list is not a time".to_string(),
                ));
            }
            _ => {} // 없거나 거짓 값이면 빈 글로 봐요
        }
    }
    Ok(best.filter(|b| !b.is_empty()).map(str::to_string))
}

/// 나열된 이슈의 번호예요: 0 이상의 정수가 아니면 오류예요.
fn issue_number(it: &Value) -> Result<u64, SourceError> {
    it.get("number")
        .and_then(Value::as_u64)
        .ok_or_else(|| SourceError::Json("an issue in GitHub's list has no number".to_string()))
}

fn bad_comment(n: u64) -> SourceError {
    SourceError::Json(format!("a comment of #{n} has no usable id"))
}

/// 목록을 돌아요: 거짓 값은 빈 목록이고, 목록이 아닌 참 값은 오류예요.
fn py_list(n: u64, v: Option<&Value>) -> Result<&[Value], SourceError> {
    match v {
        Some(Value::Array(xs)) => Ok(xs),
        Some(v) if py::truthy(v) => Err(SourceError::Json(format!(
            "the stored item #{n} has a comment list that is not a list"
        ))),
        _ => Ok(&[]),
    }
}

/// 집합에 넣을 값의 열쇠예요(JSON 글자). 목록·객체는 오류예요.
fn hashable(n: u64, v: Option<&Value>) -> Result<String, SourceError> {
    match v {
        Some(Value::Array(_) | Value::Object(_)) => Err(bad_comment(n)),
        Some(v) => Ok(v.to_string()),
        None => Ok(Value::Null.to_string()),
    }
}

/// 합치기 전에 봐요: 나열된 이슈마다 저장된 댓글과 목록의 댓글이 목록·객체가 아닌 id를 가진 객체여야 해요.
fn check_merge(
    numbers: &[u64],
    talk: &HashMap<u64, Talk>,
    old: &BTreeMap<u64, Value>,
) -> Result<(), SourceError> {
    for &n in numbers {
        let prev = old.get(&n);
        let seen = talk.get(&n);
        for key in ["comments", "review_comments"] {
            let stored = py_list(n, prev.and_then(|p| p.get(key)))?;
            let listed = match (seen, key) {
                (Some(t), "comments") => t.comments.as_slice(),
                (Some(t), _) => t.review_comments.as_slice(),
                (None, _) => &[],
            };
            for c in stored.iter().chain(listed) {
                let id = c.as_object().and_then(|c| c.get("id"));
                if id.is_none() {
                    return Err(bad_comment(n));
                }
                hashable(n, id)?;
            }
        }
    }
    Ok(())
}

/// 댓글마다 (id, updated_at)이에요. 객체가 아니면 오류예요.
fn have(n: u64, xs: &[Value]) -> Result<HashSet<(String, String)>, SourceError> {
    xs.iter()
        .map(|c| {
            let c = c.as_object().ok_or_else(|| bad_comment(n))?;
            Ok((hashable(n, c.get("id"))?, hashable(n, c.get("updated_at"))?))
        })
        .collect()
}

/// 저장된 항목이 `since` 목록이 그 번호로 가져온 것을 이미 가졌어요. 이슈의 `updated_at`
/// (나열됐으면)과, 나열된 댓글·리뷰 댓글이 모두 같은 `updated_at`으로 있어요.
fn unchanged(
    n: u64,
    prev: Option<&Value>,
    listed: Option<&Value>,
    seen: Option<&Talk>,
) -> Result<bool, SourceError> {
    let Some(prev) = prev else {
        return Ok(false);
    };
    if let Some(listed) = listed
        && listed.get("updated_at").unwrap_or(&Value::Null)
            != prev.get("updated_at").unwrap_or(&Value::Null)
    {
        return Ok(false);
    }
    let nothing = Talk::default();
    let seen = seen.unwrap_or(&nothing);
    for (key, theirs) in [
        ("comments", &seen.comments),
        ("review_comments", &seen.review_comments),
    ] {
        let theirs = have(n, theirs)?;
        let mine = have(n, py_list(n, prev.get(key))?)?;
        if !theirs.is_subset(&mine) {
            return Ok(false); // 하나라도 다르면 여기서 멈춰요
        }
    }
    Ok(true)
}

/// 증분에서 물을 PR이에요. 문서가 바뀐 것, 열린 것 모두(리뷰는 `updated_at`을 옮기지 않을 수
/// 있어요), 추가 정보가 모자란 것이에요. `kind`가 없는 저장된 항목은 오류예요.
fn wanted(
    items: &BTreeMap<u64, Value>,
    changed: &BTreeSet<u64>,
) -> Result<BTreeSet<u64>, SourceError> {
    let mut out = BTreeSet::new();
    for (&n, it) in items {
        let Some(kind) = it.get("kind") else {
            return Err(SourceError::Json(format!(
                "the stored item #{n} has no kind"
            )));
        };
        if kind != "pull" {
            continue;
        }
        let open = it["issue"].get("state").and_then(Value::as_str) == Some("open");
        if changed.contains(&n) || open || incomplete(it) {
            out.insert(n);
        }
    }
    Ok(out)
}

/// 이슈들이 센 댓글(`comments`, 없거나 거짓 값이면 0)을 모두 더해 0이에요. 정수는 정수로, 소수가 나오면 그때부터
/// 소수로 차례로 더해요(`true`는 1). 수가 아닌 참 값은 오류예요.
fn no_comments_counted(issues: &[Value]) -> Result<bool, SourceError> {
    enum Sum {
        Int(i128),
        Float(f64),
    }
    let mut sum = Sum::Int(0);
    for it in issues {
        let Some(v) = it.get("comments").filter(|v| py::truthy(v)) else {
            continue; // `or 0`
        };
        let x = match v {
            Value::Bool(_) => Sum::Int(1),
            Value::Number(x) => match (x.as_i64(), x.as_u64()) {
                (Some(i), _) => Sum::Int(i128::from(i)),
                (_, Some(u)) => Sum::Int(i128::from(u)),
                _ => Sum::Float(x.as_f64().unwrap_or(f64::NAN)),
            },
            _ => {
                return Err(SourceError::Json(
                    "an issue in GitHub's list has a comment count that is not a number"
                        .to_string(),
                ));
            }
        };
        sum = match (sum, x) {
            (Sum::Int(a), Sum::Int(b)) => Sum::Int(a + b),
            (Sum::Int(a), Sum::Float(b)) => Sum::Float(a as f64 + b),
            (Sum::Float(a), Sum::Int(b)) => Sum::Float(a + b as f64),
            (Sum::Float(a), Sum::Float(b)) => Sum::Float(a + b),
        };
    }
    Ok(match sum {
        Sum::Int(s) => s == 0,
        Sum::Float(f) => f == 0.0,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// 기대값은 예전 Python 엔진이 만든 고정 값이에요(2026-10-26). None은 믿을 커서가 없음, "RAISE"는 실패예요.
    #[test]
    fn since_matches_python() {
        let cases: [(Value, Option<&str>); 30] = [
            (json!("2026-10-19T00:00:00Z"), Some("2026-10-18T21:00:00Z")),
            (json!("2026-1-2T3:4:5Z"), Some("2026-01-02T00:04:05Z")),
            (json!("2026-10- 5T01:00:00Z"), Some("2026-10-04T22:00:00Z")),
            (json!("2026-10-05t01:00:00z"), Some("2026-10-04T22:00:00Z")),
            (json!("2026-02-30T00:00:00Z"), None),
            (json!("2026-10-10T00:00:60Z"), None),
            (json!("2026-10-10T00:00:61Z"), None),
            (json!("2026-10-10T24:00:00Z"), None),
            (json!(" 2026-10-10T00:00:00Z"), None),
            (json!("2026-10-10T00:00:00Z "), None),
            (json!("2026-10-10T00:00:00"), None),
            (json!("26-10-10T00:00:00Z"), None),
            (json!("0001-01-01T02:00:00Z"), Some("RAISE")),
            (json!("0001-01-01T03:00:00Z"), Some("0001-01-01T00:00:00Z")),
            (json!("0999-05-05T05:00:00Z"), Some("0999-05-05T02:00:00Z")),
            (json!("0010-01-01T03:00:00Z"), Some("0010-01-01T00:00:00Z")),
            (json!("2026-10-10T00:00:00.5Z"), None),
            (json!("2024-02-29T01:00:00Z"), Some("2024-02-28T22:00:00Z")),
            (json!("2026-1-01T00:00:00Z"), Some("2025-12-31T21:00:00Z")),
            (json!("2026-001-01T00:00:00Z"), None),
            (json!("2026-10-10T00:00:00Z\n"), None),
            (json!("0000-01-01T00:00:00Z"), None),
            (json!("+026-10-10T00:00:00Z"), None),
            (json!("2026-10-10 00:00:00Z"), None),
            (json!("2026-10-1T00:00:00Z"), Some("2026-09-30T21:00:00Z")),
            (json!("2026-10-  5T01:00:00Z"), None),
            (json!("2026-10-10T0:0:0Z"), Some("2026-10-09T21:00:00Z")),
            (json!(5), None),
            (json!(null), None),
            // 전각 숫자는 받지 않아요. 숫자는 ASCII만이에요(모듈 설명)
            (json!("２０２６-10-10T00:00:00Z"), None),
        ];
        for (cursor, want) in cases {
            match (since_of(&cursor), want) {
                (Err(_), Some("RAISE")) => {}
                (Ok(got), want) if got.as_deref() == want => {}
                (got, want) => panic!("{cursor}: got {got:?}, Python {want:?}"),
            }
        }
    }

    /// 기대값도 같은 엔진으로 만든 고정 값이에요(지금 시각은 "2026-10-26T00:00:00Z").
    #[test]
    fn due_matches_python() {
        let now = "2026-10-26T00:00:00Z";
        let cases = [
            (json!("2026-10-19T00:00:00Z"), false), // 꼭 7일은 지나지 않았어요
            (json!("2026-10-18T23:59:59Z"), true),
            (json!("2026-10-19T00:00:01Z"), false),
            (json!("2026-1-2T3:4:5Z"), true),
            (json!("2026-10-19t00:00:00z"), false),
            (json!(null), true),
            (json!(5), true),
            (json!("x"), true),
            (json!("2026-10-30T00:00:00Z"), false),
        ];
        for (reconciled, want) in cases {
            assert_eq!(due(Some(&reconciled), now), want, "{reconciled}");
        }
        assert!(due(None, now));
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(CURSOR_OVERLAP, TimeDelta::hours(3));
        assert_eq!(RECONCILE_EVERY, TimeDelta::days(7));
    }
}
