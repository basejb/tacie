//! Repo check and change check.
//!
//! `probe_repo` asks about the repo before listing anything and fixes the state (`lost`, `repo_id`, `full_name`).
//! `changed` asks for the newest single item of each list with the saved ETag, only to see whether anything comes
//! after the cursor (a 304 does not use the rate limit). Neither prints text from the bodies GitHub returns.
//!
//! Rules for odd shapes (shapes that neither the state Tacie writes nor GitHub's answers have):
//! - If the saved `lost` is not 0, null or false and also not an integer, it is a `Json` error on a 404/410 or a
//!   different id (where `lost` is read).
//! - When adding one to `lost` on a 404/410, `i64::MAX` is left as it is (it does not overflow and panic). The closed
//!   decision (`lost >= LOST_AT`) stays the same.
//! - If GitHub gives `true` as `id`, it is down.
//! - Comparison with the saved `repo_id` is JSON value equality: a saved `1.0` and an answer of `1` differ.
//! - `changed`: if the saved etag is not a string, it is not sent.

use serde_json::{Map, Value};

use super::{GitHub, rate_limited};
use crate::error::{Kind, SourceError};
use crate::py;

/// 저장소에 404·410이 이만큼 이어지면(또는 id가 바뀌면 한 번에) 그 문서를 검색에서 빼요.
pub const LOST_AT: u64 = 2;

/// 목록마다 가장 새 항목 하나만 묻는 길이에요(`repos/{repo}/` 뒤). 확인(check)이 목록 대신 GitHub에 물어요.
pub const PROBES: [(&str, &str); 3] = [
    (
        "issues",
        "issues?state=all&sort=updated&direction=desc&per_page=1",
    ),
    (
        "comments",
        "issues/comments?sort=updated&direction=desc&per_page=1",
    ),
    (
        "review_comments",
        "pulls/comments?sort=updated&direction=desc&per_page=1",
    ),
];

/// 상태의 `lost`예요. 없거나 거짓 값은 0이고, 그 밖에 정수가 아니면 오류예요.
fn lost_of(state: &Map<String, Value>) -> Result<i64, SourceError> {
    match state.get("lost") {
        Some(v) if py::truthy(v) => v.as_i64().ok_or_else(|| {
            SourceError::Json("state.json's lost is not a whole number".to_string())
        }),
        _ => Ok(0),
    }
}

/// 무엇보다 먼저 저장소를 묻고 `state`를 고쳐요.
///
/// - 200: 있어요. 그 id(처음 본 것을 지켜요)와 지금 이름을 두고 `lost = 0`, true예요.
/// - 404·410: 사라졌거나 숨었다는 답이 하나 더 이어졌어요(`lost` + 1, `i64::MAX`에서 멈춰요), false예요.
/// - 저장된 것과 다른 id의 200: 그 이름이 이제 다른 저장소라 바로 닫아요(`lost = max(lost, LOST_AT)`), false예요.
/// - 그 밖의 답은 오류이고 `state`는 그대로예요(저장된 것을 써요): 호출 제한은 상태 코드와 상관없이 busy, 401·403은
///   key, 429는 busy, 나머지는 down이에요. 정수 id가 없는 200은 down이에요.
pub fn probe_repo(
    gh: &GitHub,
    repo: &str,
    state: &mut Map<String, Value>,
) -> Result<bool, SourceError> {
    let (status, headers, body) = gh.status(&format!("repos/{repo}"), &[], None)?;
    if status == 404 || status == 410 {
        let lost = lost_of(state)?.saturating_add(1); // 저장본의 수가 아무리 커도 넘치지 않아요
        state.insert("lost".into(), Value::from(lost));
        eprintln!("github: {repo} answered {status} ({lost} in a row)");
        return Ok(false);
    }
    if status != 200 {
        let kind = if rate_limited(status, &headers, body.as_ref()) {
            Kind::Busy
        } else {
            match status {
                401 | 403 => Kind::Key,
                429 => Kind::Busy,
                _ => Kind::Down,
            }
        };
        return Err(SourceError::service(
            kind,
            format!("GitHub answered {status} for the repo"),
        ));
    }
    let answer = body.as_ref().and_then(Value::as_object);
    let id = match answer.and_then(|b| b.get("id")) {
        Some(id @ Value::Number(n)) if n.is_i64() || n.is_u64() => id.clone(),
        _ => {
            return Err(SourceError::service(
                Kind::Down,
                "GitHub's answer for the repo has no id",
            ));
        }
    };
    if let Some(kept) = state.get("repo_id").filter(|v| !v.is_null())
        && *kept != id
    {
        let lost = lost_of(state)?.max(LOST_AT as i64);
        state.insert("lost".into(), Value::from(lost));
        eprintln!(
            "github: {repo} is another repo now (its id changed); its issues and PRs are left out of search"
        );
        return Ok(false);
    }
    // 답의 `full_name`, 없거나 거짓 값이면 물은 이름이에요
    let full_name = answer
        .and_then(|b| b.get("full_name"))
        .filter(|v| py::truthy(v))
        .cloned()
        .unwrap_or_else(|| Value::from(repo));
    // 있는 키는 제자리에서 바꿔요
    state.insert("repo_id".into(), id);
    state.insert("full_name".into(), full_name);
    state.insert("lost".into(), Value::from(0));
    Ok(true)
}

/// GitHub에 `state`의 커서 뒤의 것이 있는지예요. 목록마다 한 번씩 물어요.
///
/// 목록마다 저장된 ETag(`state["etags"][목록]["etag"]`)를 보내요. 304는 가장 새 항목이 저장된 top 그대로라는 뜻이고
/// 호출 제한을 쓰지 않아요. 200은 가장 새 `updated_at`(빈 목록은 null)과 새 ETag를 가져와요. 어느 목록의 top이든
/// 커서보다 뒤면 바뀐 거예요. (바뀜, 200으로 답한 목록의 `{목록: {"etag", "top"}}` 저장할 것)을 돌려주고, 어느 하나라도
/// 실패하거나(토큰이 없음, 오프라인, 너무 느림, 연결이 끊김) 다른 답이면 `(None, {})`예요. 아무것도 쓰지 않아요.
pub fn changed(
    gh: &GitHub,
    repo: &str,
    state: &Map<String, Value>,
) -> (Option<bool>, Map<String, Value>) {
    let empty = Map::new();
    let saved = state
        .get("etags")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let cursors = state
        .get("cursors")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let (mut found, mut etags) = (false, Map::new());
    for (stream, probe) in PROBES {
        let prev = saved
            .get(stream)
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let etag = prev
            .get("etag")
            .and_then(Value::as_str)
            .filter(|e| !e.is_empty());
        let headers: Vec<(&str, &str)> = etag.map(|e| ("If-None-Match", e)).into_iter().collect();
        let Ok((status, names, body)) = gh.status(
            &format!("repos/{repo}/{probe}"),
            &headers,
            Some(gh.timeouts().check),
        ) else {
            return (None, Map::new());
        };
        let top = match status {
            304 => prev.get("top").cloned().unwrap_or(Value::Null),
            200 => {
                let first = match &body {
                    Some(Value::Array(items)) => items.first(),
                    _ => None,
                };
                let top = first
                    .and_then(Value::as_object)
                    .and_then(|f| f.get("updated_at"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let mut fresh = Map::new();
                fresh.insert(
                    "etag".into(),
                    names
                        .get("etag")
                        .map_or(Value::Null, |e| Value::from(e.as_str())),
                );
                fresh.insert("top".into(), top.clone());
                etags.insert(stream.to_string(), Value::Object(fresh));
                top
            }
            _ => return (None, Map::new()),
        };
        // ISO 글자라 글자 순서가 시간 순서예요(UTF-8 바이트 순서 = 코드 포인트 순서)
        let cursor = cursors.get(stream).and_then(Value::as_str).unwrap_or("");
        if let Value::String(top) = &top
            && top.as_str() > cursor
        {
            found = true;
        }
    }
    (Some(found), etags)
}
