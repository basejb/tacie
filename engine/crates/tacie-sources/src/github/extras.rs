//! Extra PR info: head and base branches, merge time, and all changed files and reviews,
//! fetched with GraphQL. PRs are asked `PR_BATCH` at a time in one query, and files and reviews beyond one page
//! (`NESTED_PAGE`) are asked separately for just that PR and that connection. Query text must not change down to the
//! byte (tests compare it with fixed values). `GitHub::graphql` sends each query
//! (the 3 retries live there too).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use serde_json::{Map, Value};
use tacie_index::pyjson;

use super::GitHub;
use super::items::{load_items, save_item};
use crate::error::SourceError;
use crate::py;

/// 한 GraphQL 질의에 묻는 PR 수예요.
pub const PR_BATCH: usize = 20;
/// 한 쪽에 받는 파일·리뷰 수예요.
pub const NESTED_PAGE: usize = 100;
const QUERY_HEAD: &str =
    "query($owner: String!, $name: String!) { repository(owner: $owner, name: $name) {";

/// 바뀐 파일 연결의 질의 조각이에요.
fn files_query(after: &str) -> String {
    format!(
        "files(first: {NESTED_PAGE}{after}) {{ totalCount pageInfo {{ hasNextPage endCursor }} nodes {{ path }} }}"
    )
}

/// 리뷰 연결의 질의 조각이에요.
fn reviews_query(after: &str) -> String {
    format!(
        "reviews(first: {NESTED_PAGE}{after}) {{ totalCount pageInfo {{ hasNextPage endCursor }} \
         nodes {{ author {{ login }} body state submittedAt url }} }}"
    )
}

fn connection_query(connection: &str, after: &str) -> String {
    match connection {
        "files" => files_query(after),
        _ => reviews_query(after),
    }
}

/// PR 여럿을 한 번에 묻는 질의예요: 별명 `p{n}`마다 한 PR이고 `" "`로 이어요.
fn batch_query(batch: &[u64]) -> String {
    let body = batch
        .iter()
        .map(|n| {
            format!(
                "p{n}: pullRequest(number: {n}) {{ number headRefName baseRefName mergedAt {} {} }}",
                files_query(""),
                reviews_query("")
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("{QUERY_HEAD} {body} }} }}")
}

/// 한 PR의 한 연결의 다음 쪽이에요. `after`는 커서의 JSON 글자예요.
fn follow_query(n: u64, connection: &str, after: &str) -> String {
    format!(
        "{QUERY_HEAD} p{n}: pullRequest(number: {n}) {{ {} }} }} }}",
        connection_query(connection, &format!(", after: {after}"))
    )
}

/// Python `json.dumps(v)` 모양이에요(`ensure_ascii=True`): 0x7f 이상은 `\uXXXX`(소문자, BMP 밖은 서로게이트 쌍)예요.
fn dumps_ascii(v: &Value) -> String {
    let mut out = String::new();
    for c in pyjson::to_string(v, None).chars() {
        if (c as u32) < 0x7f {
            out.push(c);
            continue;
        }
        let mut units = [0u16; 2];
        for unit in c.encode_utf16(&mut units) {
            out.push_str(&format!("\\u{unit:04x}"));
        }
    }
    out
}

fn bad(n: u64, what: &str) -> SourceError {
    SourceError::Json(format!("GitHub's answer for #{n} has a bad {what}"))
}

/// 객체의 키 값이에요. 객체가 아니거나 키가 없으면 오류예요.
fn field<'a>(n: u64, v: &'a Value, key: &str, what: &str) -> Result<&'a Value, SourceError> {
    v.as_object()
        .and_then(|m| m.get(key))
        .ok_or_else(|| bad(n, what))
}

fn nodes_of(n: u64, page: &Value, connection: &str) -> Result<Vec<Value>, SourceError> {
    field(n, page, "nodes", connection)?
        .as_array()
        .cloned()
        .ok_or_else(|| bad(n, connection))
}

/// `node[connection]`의 모든 쪽이에요: GitHub이 다음 쪽이 있다고 하는 동안(totalCount로 멈추지
/// 않아요) 이 PR과 이 연결만 이어서 물어요. 앞에서 물은 커서가 다시 오면 멈춰요(끝나지 않으니까요; 그 PR은 모자란
/// 것으로 남아 다음 받기에 다시 물어요). 다음 쪽에서 GitHub이 그 PR을 null로 답하면 None이에요. 돌려주는 것은
/// `{"totalCount", "nodes"}`예요(pageInfo는 빠져요).
fn rest_of(
    gh: &GitHub,
    connection: &str,
    owner: &str,
    name: &str,
    n: u64,
    node: &Map<String, Value>,
) -> Result<Option<Value>, SourceError> {
    let first = node.get(connection).ok_or_else(|| bad(n, connection))?;
    let mut nodes = nodes_of(n, first, connection)?;
    let mut info = field(n, first, "pageInfo", connection)?.clone();
    let mut asked: HashSet<String> = HashSet::new();
    loop {
        let more = py::truthy(field(n, &info, "hasNextPage", connection)?);
        let cursor = info.get("endCursor").cloned().unwrap_or(Value::Null);
        if !more || !py::truthy(&cursor) {
            break;
        }
        let after = dumps_ascii(&cursor);
        if !asked.insert(after.clone()) {
            eprintln!(
                "github #{n}: GitHub gave the same {connection} cursor again, paging stopped"
            );
            break;
        }
        let got = gh.graphql(&follow_query(n, connection, &after), owner, name)?;
        let page = match got.get(&format!("p{n}")) {
            None | Some(Value::Null) => return Ok(None),
            Some(pr) => field(n, pr, connection, connection)?,
        };
        nodes.extend(nodes_of(n, page, connection)?);
        info = field(n, page, "pageInfo", connection)?.clone();
    }
    let total = field(n, first, "totalCount", connection)?.clone();
    let mut out = Map::new();
    out.insert("totalCount".into(), total);
    out.insert("nodes".into(), Value::Array(nodes));
    Ok(Some(Value::Object(out)))
}

/// 저장소 이름을 owner와 name으로 나눠요: `/`가 꼭 하나예요.
fn owner_and_name(repo: &str) -> Result<(&str, &str), SourceError> {
    match repo.split_once('/') {
        Some((owner, name)) if !name.contains('/') => Ok((owner, name)),
        _ => Err(SourceError::Other("repo must be owner/name".to_string())),
    }
}

/// 번호 → 그 PR의 head·base 브랜치, 머지 시각, 모든 파일과 리뷰(pageInfo는 빼요)예요. GitHub이 null로 답한 PR은
/// None이에요. 번호는 정렬하고 겹친 것은 한 번 물어요.
pub fn pr_extras(
    gh: &GitHub,
    repo: &str,
    numbers: &[u64],
) -> Result<BTreeMap<u64, Option<Value>>, SourceError> {
    let (owner, name) = owner_and_name(repo)?;
    let numbers: Vec<u64> = numbers
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut out = BTreeMap::new();
    for batch in numbers.chunks(PR_BATCH) {
        let got = gh.graphql(&batch_query(batch), owner, name)?;
        for &n in batch {
            let mut node = match got.get(&format!("p{n}")) {
                None | Some(Value::Null) => None,
                Some(Value::Object(m)) => Some(m.clone()),
                Some(_) => return Err(bad(n, "pullRequest")),
            };
            for connection in ["files", "reviews"] {
                let Some(m) = node.as_mut() else { break };
                match rest_of(gh, connection, owner, name, n, m)? {
                    Some(all) => {
                        m.insert(connection.to_string(), all); // 그 자리에서 바꿔요(키 순서 그대로)
                    }
                    None => node = None,
                }
            }
            out.insert(n, node.map(Value::Object));
        }
    }
    Ok(out)
}

/// 목록·객체·글의 길이예요. 없거나 그 밖의 값이면 0이에요.
fn len_of(v: Option<&Value>) -> usize {
    match v {
        Some(Value::Array(a)) => a.len(),
        Some(Value::Object(m)) => m.len(),
        Some(Value::String(s)) => s.chars().count(),
        _ => 0,
    }
}

/// 저장된 추가 정보가 없거나, 리뷰를 세기 전에 저장했거나, GitHub이 센 것보다 파일·리뷰가 적어요(옛 질의는 60개와
/// 30개에서 잘랐어요). totalCount가 수가 아니면 모자란 것으로 봐요(다시 물으면 고쳐져요).
pub fn incomplete(item: &Value) -> bool {
    let Some(x) = item.get("extras").and_then(Value::as_object) else {
        return true;
    };
    let (Some(files), Some(reviews)) = (
        x.get("files").and_then(Value::as_object),
        x.get("reviews").and_then(Value::as_object),
    ) else {
        return true;
    };
    if !reviews.contains_key("totalCount") {
        return true;
    }
    [files, reviews].iter().any(|page| {
        let have = len_of(page.get("nodes").filter(|v| py::truthy(v))) as f64;
        match page.get("totalCount") {
            None => false, // totalCount가 없으면 0으로 봐요
            Some(total) => total.as_f64().is_none_or(|total| have < total),
        }
    })
}

/// 저장된 PR 가운데 `numbers`에 든 것을 묻고, 추가 정보가 다른 것만 써요. GitHub이 null로 답한 PR은 저장된 것을
/// 그대로 둬요. 쓴 번호예요.
pub fn update_extras(
    gh: &GitHub,
    repo: &str,
    raw: &Path,
    numbers: &BTreeSet<u64>,
) -> Result<BTreeSet<u64>, SourceError> {
    let (mut items, _) = load_items(raw)?;
    let asked: Vec<u64> = numbers
        .iter()
        .copied()
        .filter(|n| {
            items
                .get(n)
                .is_some_and(|it| it.get("kind").and_then(Value::as_str) == Some("pull"))
        })
        .collect();
    let mut written = BTreeSet::new();
    if asked.is_empty() {
        return Ok(written);
    }
    for (n, x) in pr_extras(gh, repo, &asked)? {
        let (Some(x), Some(Value::Object(item))) = (x, items.get_mut(&n)) else {
            continue;
        };
        if item.get("extras") == Some(&x) {
            continue;
        }
        item.insert("extras".to_string(), x);
        if save_item(raw, &Value::Object(item.clone()))? {
            written.insert(n);
        }
    }
    Ok(written)
}
