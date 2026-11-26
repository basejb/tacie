//! Weekly reconcile.
//!
//! After an incremental fetch (once a week) it matches the saved documents against GitHub's full lists. Saved numbers
//! not in the issue list are asked about one by one and deleted only when GitHub confirms: 410 (deleted), 404
//! (hidden), or an answer with a different `repository_url` (moved). New numbers, changed numbers (`updated_at`),
//! unreadable numbers, and numbers whose comment and review comment ids differ from the full comment lists are
//! refetched whole. It never deletes ids one by one. If there is saved data but a list comes back empty, or more than
//! `MISSING_LIMIT` (and `MISSING_FLOOR`) are missing, it does not trust that list: that stage does nothing, states the
//! reason in `skipped`, and leaves `reconciled_at` as it is so the next refresh tries again. If a number asked about
//! separately hits the rate limit, it is a busy error (fetch takes it as "try again later"). The summary line on
//! stderr has only counts and reasons (reasons contain only counts and list names).
//!
//! Rules:
//! - This repo's URL (`home`) is built from the state's `full_name` only when that is a non-empty string; otherwise
//!   from the name asked for. Turning a non-string `full_name` into text would make every separately asked number
//!   look moved and get deleted. Tacie writes only strings.
//! - `home` is compared with `to_lowercase`. Repo names and GitHub URLs are ASCII.
//! - These lists are `Json` errors: an issue with no number (only integers 0 or above count as numbers), a comment with
//!   no id, a comment whose number cannot be read from its URL (the rule of `items::number_of`: ASCII digits with
//!   surrounding whitespace removed). This error happens after the separately asked numbers have been deleted.
//! - Ids are compared as JSON text (`1` and `1.0` differ).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::Hash;
use std::path::Path;

use serde_json::{Map, Value};

use super::items::{Talk, by_number, delete_item, list, load_items, number_of};
use super::refetch::refetch_documents;
use super::{GitHub, now_iso, rate_limited};
use crate::error::{Kind, SourceError};

/// 저장된 번호(또는 id) 가운데 전체 목록에 없는 것이 이보다 많으면 그 목록을 믿지 않고 아무것도 지우지 않아요.
pub const MISSING_LIMIT: f64 = 0.2;
/// 그래도 이만큼까지는 늘 하나씩 따로 물어요(작은 저장소; 지우기는 하나씩 확인해요).
pub const MISSING_FLOOR: usize = 5;

/// 재조정의 결과예요.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// 지운 번호예요(작은 것부터).
    pub removed: Vec<u64>,
    /// 다시 받아 바뀐 번호예요(작은 것부터).
    pub refetched: Vec<u64>,
    /// 믿지 않은 목록의 이유예요(`; `로 이어요). 있으면 `reconciled_at`은 그대로예요.
    pub skipped: Option<String>,
}

/// 전체 목록(`got`: 그것이 가져온 것)을 저장된 것과 견줘 믿지 않는 이유예요: 저장된 것이 있는데
/// 비어 왔거나, `MISSING_LIMIT`보다 (그리고 `MISSING_FLOOR`보다) 많이 빠졌어요.
fn untrusted<T: Eq + Hash>(what: &str, stored: &HashSet<T>, got: &HashSet<T>) -> Option<String> {
    if !stored.is_empty() && got.is_empty() {
        return Some(format!("the {what} list came back empty"));
    }
    let missing = stored.iter().filter(|x| !got.contains(x)).count();
    let limit = (MISSING_FLOOR as f64).max(MISSING_LIMIT * stored.len() as f64);
    if missing as f64 > limit {
        return Some(format!(
            "{missing} of {} stored {what}s missing from the list",
            stored.len()
        ));
    }
    None
}

/// 댓글 id 모음이에요: id가 없으면 오류예요.
fn id_set(comments: &[Value]) -> Result<HashSet<String>, SourceError> {
    comments
        .iter()
        .map(|c| {
            c.get("id")
                .map(Value::to_string)
                .ok_or_else(|| SourceError::Json("a comment has no id".to_string()))
        })
        .collect()
}

/// 댓글을 문서 번호별로 모아요: 주소에서 번호를 읽을 수 없는 댓글이 있으면 오류예요(`fetch`도 써요).
pub(super) fn talk_of(said: &[Value], inline: &[Value]) -> Result<HashMap<u64, Talk>, SourceError> {
    for (comments, key) in [(said, "issue_url"), (inline, "pull_request_url")] {
        if comments.iter().any(|c| number_of(c.get(key)).is_none()) {
            return Err(SourceError::Json(format!(
                "a comment in GitHub's list has no number in its {key}"
            )));
        }
    }
    Ok(by_number(said, inline))
}

/// 키의 값이에요(없음과 null은 같아요).
fn field<'a>(x: &'a Value, key: &str) -> &'a Value {
    x.get(key).unwrap_or(&Value::Null)
}

/// 저장된 문서를 GitHub의 전체 목록과 맞춰요. 모듈 설명을 보세요. 건너뛴 것이 없을 때만
/// `state["reconciled_at"] = started`(없거나 비었으면 지금)예요.
pub fn reconcile(
    gh: &GitHub,
    repo: &str,
    raw: &Path,
    state: &mut Map<String, Value>,
    started: Option<&str>,
) -> Result<Reconciled, SourceError> {
    let started = started
        .filter(|s| !s.is_empty())
        .map_or_else(now_iso, str::to_string);
    let mut listed: HashMap<u64, Value> = HashMap::new();
    for it in gh.rest(&format!("repos/{repo}/issues?state=all"), None, 3)? {
        let Some(n) = it.get("number").and_then(Value::as_u64) else {
            return Err(SourceError::Json(
                "an issue in GitHub's list has no number".to_string(),
            ));
        };
        listed.insert(n, it); // 같은 번호가 다시 오면 나중 것이에요
    }
    let said = gh.rest(&format!("repos/{repo}/issues/comments"), None, 3)?;
    let inline = gh.rest(&format!("repos/{repo}/pulls/comments"), None, 3)?;
    let (items, bad) = load_items(raw)?;
    let stored: HashSet<u64> = items.keys().chain(&bad).copied().collect();
    let listed_numbers: HashSet<u64> = listed.keys().copied().collect();
    let (mut skipped, mut removed, mut here) = (Vec::new(), Vec::new(), HashMap::new());
    if let Some(why) = untrusted("issue", &stored, &listed_numbers) {
        skipped.push(why);
    } else {
        // 상태의 `full_name`, 비지 않은 글이 아니면 물은 이름이에요
        let name = match state.get("full_name") {
            Some(Value::String(s)) if !s.is_empty() => s.as_str(),
            _ => repo,
        };
        let home = format!("https://api.github.com/repos/{name}").to_lowercase();
        let missing: BTreeSet<u64> = stored.difference(&listed_numbers).copied().collect();
        for n in missing {
            let (status, headers, body) =
                gh.status(&format!("repos/{repo}/issues/{n}"), &[], None)?;
            if rate_limited(status, &headers, body.as_ref()) {
                // 나머지도 거절될 거예요: fetch가 다음에 모두 다시 해요
                return Err(SourceError::service(
                    Kind::Busy,
                    "GitHub's rate limit stopped the reconcile",
                ));
            }
            let place = body
                .as_ref()
                .and_then(|b| b.get("repository_url"))
                .and_then(Value::as_str);
            let moved = status == 200 && place.is_some_and(|w| w.to_lowercase() != home);
            if status == 404 || status == 410 || moved {
                delete_item(raw, n)?;
                removed.push(n);
            } else if status == 200
                && place.is_some()
                && let Some(body) = body
            {
                here.insert(n, body); // 아직 이 저장소예요. 그 밖의 답은 저장된 것을 그대로 둬요
            }
        }
    }
    // GitHub이 지금 이 저장소에 있다고 하는 번호와 그 이슈예요
    let mut known = listed;
    known.extend(here);
    let mut todo: BTreeSet<u64> = known
        .iter()
        .filter(|&(n, it)| {
            items
                .get(n)
                .is_none_or(|old| field(it, "updated_at") != field(old, "updated_at"))
        })
        .map(|(n, _)| *n)
        .collect();
    let gone: HashSet<u64> = removed.iter().copied().collect();
    let kept: BTreeMap<u64, &Value> = items
        .iter()
        .filter(|(n, _)| !gone.contains(n))
        .map(|(n, it)| (*n, it))
        .collect();
    let talk = talk_of(&said, &inline)?;
    for (key, what, got) in [
        ("comments", "comment", &said),
        ("review_comments", "review comment", &inline),
    ] {
        let mut ids: BTreeMap<u64, HashSet<String>> = BTreeMap::new();
        for (&n, it) in &kept {
            ids.insert(n, id_set(list(Some(it), key))?);
        }
        let all: HashSet<String> = ids.values().flatten().cloned().collect();
        if let Some(why) = untrusted(what, &all, &id_set(got)?) {
            skipped.push(why);
            continue;
        }
        for (n, mine) in &ids {
            if !known.contains_key(n) {
                continue;
            }
            let theirs = match (talk.get(n), key) {
                (Some(t), "comments") => id_set(&t.comments)?,
                (Some(t), _) => id_set(&t.review_comments)?,
                (None, _) => HashSet::new(),
            };
            if *mine != theirs {
                todo.insert(*n);
            }
        }
    }
    let (written, _) = refetch_documents(gh, repo, raw, &todo, &known, None)?;
    let reason = (!skipped.is_empty()).then(|| skipped.join("; "));
    if reason.is_none() {
        state.insert("reconciled_at".into(), Value::from(started));
    }
    eprintln!(
        "github reconcile: {} removed, {} downloaded again, {} changed{}",
        removed.len(),
        todo.len(),
        written.len(),
        reason
            .as_deref()
            .map_or(String::new(), |r| format!("; not trusted: {r}"))
    );
    Ok(Reconciled {
        removed,
        refetched: written.into_iter().collect(),
        skipped: reason,
    })
}
