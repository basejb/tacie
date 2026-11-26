//! Refetching a whole document.
//!
//! Refetches the issue (the one the list brought, or asked separately if absent), all its comments, and for a PR all
//! inline review comments, and writes them as one item (only if different). Extra PR info is kept as saved. A single
//! document's comment list can also come back short, so it asks once more if the answer missed comments the `since`
//! list brought, has fewer than GitHub's count, or, with no count, comes back empty while saved data exists. If it is
//! still so, it merges the saved ones, the list's ones and the answer by id (the answer wins, then the list): it
//! neither deletes what was saved nor drops what came in the list.
//!
//! These answers are errors: an issue answer that is not an object, a comment with no id among those checked when in
//! doubt (including saved ones when merging), and a `comments` count that is neither a number nor null (only when
//! that count is read). Ids are compared as JSON text (same as `items::merge`: `1` and `1.0` differ).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use serde_json::Value;

use super::GitHub;
pub use super::items::{Talk, by_number};
use super::items::{item, item_path, list, merge, save_item};
use crate::error::SourceError;
use crate::py;
use crate::store::{self, Stored};

/// 번호 n의 저장된 항목(어느 종류든)이에요. 없거나 항목이 아니면 None이에요.
fn stored_item(raw: &Path, n: u64) -> std::io::Result<Option<Value>> {
    for kind in ["pull", "issue"] {
        if let Stored::Value(it) = store::read_json(&item_path(raw, kind, n))?
            && it.get("issue").is_some_and(Value::is_object)
        {
            return Ok(Some(it));
        }
    }
    Ok(None)
}

/// 댓글의 id(JSON 글자)예요. 없으면 오류예요.
fn id_of(n: u64, c: &Value) -> Result<String, SourceError> {
    c.get("id")
        .map(Value::to_string)
        .ok_or_else(|| SourceError::Json(format!("a comment of #{n} has no id")))
}

/// 댓글 id 모음이에요.
fn ids_of(n: u64, comments: &[Value]) -> Result<HashSet<String>, SourceError> {
    comments.iter().map(|c| id_of(n, c)).collect()
}

/// GitHub이 센 댓글 수예요(받은 댓글이 이보다 적으면 짧게 온 것이에요). `true`·`false`는 1·0으로 셈하고, 수도
/// 참·거짓도 아니면 오류예요.
fn count_of(n: u64, v: &Value) -> Result<f64, SourceError> {
    match v {
        Value::Number(x) => Ok(x.as_f64().unwrap_or(f64::NAN)),
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        _ => Err(SourceError::Json(format!(
            "GitHub's answer for #{n} has a bad comments count"
        ))),
    }
}

/// 문서 하나의 모든 댓글이에요. `count`는 GitHub이 센 수예요(None과 null은 셈이 없어요).
fn all_comments(
    gh: &GitHub,
    n: u64,
    path: &str,
    stored: &[Value],
    seen: &[Value],
    count: Option<&Value>,
) -> Result<Vec<Value>, SourceError> {
    let doubted = |got: &[Value]| -> Result<bool, SourceError> {
        let ids = ids_of(n, got)?;
        // 처음 빠진 것에서 멈추고 수는 보지 않아요
        for c in seen {
            if !ids.contains(&id_of(n, c)?) {
                return Ok(true);
            }
        }
        Ok(match count {
            Some(v) if !v.is_null() => (got.len() as f64) < count_of(n, v)?,
            _ => got.is_empty() && !stored.is_empty(),
        })
    };
    let got = gh.rest(path, None, 1)?; // 댓글이 없는 것도 보통의 답이에요
    if !doubted(&got)? {
        return Ok(got);
    }
    let got = gh.rest(path, None, 1)?;
    if !doubted(&got)? {
        return Ok(got);
    }
    eprintln!("github #{n}: a comment list came back short, stored and listed comments kept");
    // 합치기 전에 모든 id를 읽어요(없으면 오류예요)
    ids_of(n, stored)?;
    ids_of(n, seen)?;
    Ok(merge(&merge(stored, seen), &got))
}

/// 이 이슈·PR을 통째로 다시 받아요: 이슈(`listed[n]`이 참이면 그것, 아니면 따로 물어요), 그 댓글 전부, PR이면 인라인
/// 리뷰 댓글 전부예요. PR 추가 정보는 저장된 항목의 것을 둬요. `since`: 번호 → `since` 목록이 가져온 댓글·리뷰 댓글로,
/// 짧은 답이 이것을 빠뜨리면 안 돼요. (쓴 번호, `numbers` 가운데 PR 번호)예요. 번호는 작은 것부터 해요.
///
/// 쓰는 항목의 번호는 이슈 답의 `number`예요(따로 물은 이슈가 옮겨져 다른 번호로 오면 그 번호의
/// 파일이에요). 쓴 번호에는 물은 번호를 넣어요.
pub fn refetch_documents(
    gh: &GitHub,
    repo: &str,
    raw: &Path,
    numbers: &BTreeSet<u64>,
    listed: &HashMap<u64, Value>,
    since: Option<&HashMap<u64, Talk>>,
) -> Result<(BTreeSet<u64>, BTreeSet<u64>), SourceError> {
    let (mut written, mut prs) = (BTreeSet::new(), BTreeSet::new());
    let nothing = Talk::default();
    for &n in numbers {
        let prev = stored_item(raw, n)?;
        let seen = since.and_then(|s| s.get(&n)).unwrap_or(&nothing);
        // 목록이 가져온 이슈, 없으면 따로 물어요
        let issue = match listed.get(&n).filter(|it| py::truthy(it)) {
            Some(it) => it.clone(),
            None => gh.json(&format!("repos/{repo}/issues/{n}"), 3)?,
        };
        if !issue.is_object() {
            return Err(SourceError::Json(format!(
                "GitHub's answer for #{n} is not an issue"
            )));
        }
        let said = all_comments(
            gh,
            n,
            &format!("repos/{repo}/issues/{n}/comments"),
            list(prev.as_ref(), "comments"),
            &seen.comments,
            issue.get("comments"),
        )?;
        let mut inline = Vec::new();
        if issue.get("pull_request").is_some() {
            prs.insert(n);
            // 리뷰 댓글은 GitHub이 세지 않아요
            inline = all_comments(
                gh,
                n,
                &format!("repos/{repo}/pulls/{n}/comments"),
                list(prev.as_ref(), "review_comments"),
                &seen.review_comments,
                None,
            )?;
        }
        let extras = prev
            .as_ref()
            .and_then(|p| p.get("extras"))
            .cloned()
            .unwrap_or(Value::Null);
        if save_item(raw, &item(&issue, said, inline, extras))? {
            written.insert(n);
        }
    }
    Ok((written, prs))
}
