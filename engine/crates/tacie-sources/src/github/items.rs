//! GitHub raw files and sync state.
//!
//! One issue or PR is one `items/{issue|pull}-{번호}.json`: the issue, comments, inline review comments and extra PR
//! info are in one file. The state is `state.json`. Only changed items are rewritten.
//!
//! Odd raw data does not raise errors (the signatures have no errors). Issues with no number, comments whose URL number
//! cannot be read and comments with no id are skipped, and if `state.json` cannot be opened (a folder, or permissions)
//! it leaves one line on stderr and uses an empty state. The number in a file name is ASCII digits and must fit in a
//! u64. Raw data written by Tacie never has these cases.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::store::{self, Stored};

/// 동기화하는 목록이에요(커서가 하나씩 있어요).
pub const STREAMS: [&str; 3] = ["issues", "comments", "review_comments"];

static ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(issue|pull)-([0-9]+)\.json$").unwrap());

/// 이슈("issue")나 PR("pull") 하나를 두는 곳이에요.
pub fn item_path(raw: &Path, kind: &str, number: u64) -> PathBuf {
    raw.join("items").join(format!("{kind}-{number}.json"))
}

/// (번호 → 항목, 파일을 읽을 수 없는 번호)예요. 읽을 수 없는 파일은 다시 쓰일 때까지 건너뛰어요(다음 전체 받기, 증분에서
/// 그 번호가 다시 나오거나 재조정). 이름은 정렬해서 읽어요: 같은 번호의 두 파일은 뒤의 것(`pull`)이 이겨요.
pub fn load_items(raw: &Path) -> std::io::Result<(BTreeMap<u64, Value>, BTreeSet<u64>)> {
    let (mut items, mut bad) = (BTreeMap::new(), BTreeSet::new());
    let dir = raw.join("items");
    if !dir.is_dir() {
        return Ok((items, bad));
    }
    let mut names: Vec<String> = Vec::new();
    for entry in fs::read_dir(&dir)? {
        if let Ok(name) = entry?.file_name().into_string() {
            names.push(name);
        }
    }
    names.sort();
    for name in names {
        let Some(m) = ITEM.captures(&name) else {
            continue;
        };
        let Ok(number) = m[2].parse::<u64>() else {
            continue;
        };
        let item = match store::read_json(&dir.join(&name))? {
            Stored::Value(v) if v.get("issue").is_some_and(Value::is_object) => Some(v),
            Stored::Unreadable => None, // store가 이미 한 줄 남겼어요
            _ => {
                // JSON이지만 항목이 아니에요(또는 읽는 사이에 사라졌어요)
                eprintln!("github: unreadable {name}");
                None
            }
        };
        match item {
            Some(item) => {
                items.insert(number, item);
            }
            None => {
                bad.insert(number);
            }
        }
    }
    Ok((items, bad))
}

fn kind_and_number(item: &Value) -> std::io::Result<(&str, u64)> {
    let kind = item.get("kind").and_then(Value::as_str);
    let number = item.get("number").and_then(Value::as_u64);
    match (kind, number) {
        (Some(kind @ ("issue" | "pull")), Some(number)) => Ok((kind, number)),
        _ => Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "github: an item needs kind issue or pull and a number",
        )),
    }
}

/// 저장된 것과 다를 때만 써요. 썼으면 true예요. 이슈가 PR이 되면(또는 거꾸로) 다른 종류의 파일을 지워서 번호마다
/// 파일 하나예요. 같음은 키 순서를 보지 않아요.
pub fn save_item(raw: &Path, item: &Value) -> std::io::Result<bool> {
    let (kind, number) = kind_and_number(item)?;
    let file = item_path(raw, kind, number);
    if let Stored::Value(old) = store::read_json(&file)?
        && old == *item
    {
        return Ok(false);
    }
    store::write_json(&file, item, None)?;
    let other = if kind == "pull" { "issue" } else { "pull" };
    match fs::remove_file(item_path(raw, other, number)) {
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    Ok(true)
}

/// 저장된 이슈나 PR을 지워요(어느 종류든). 있었으면 true예요.
pub fn delete_item(raw: &Path, number: u64) -> std::io::Result<bool> {
    let mut gone = false;
    for kind in ["issue", "pull"] {
        let file = item_path(raw, kind, number);
        if file.exists() {
            fs::remove_file(file)?;
            gone = true;
        }
    }
    Ok(gone)
}

/// 문서 하나에 달린 댓글과 인라인 리뷰 댓글이에요.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Talk {
    pub comments: Vec<Value>,
    pub review_comments: Vec<Value>,
}

/// URL의 마지막 `/` 뒤의 번호예요(`…/issues/12` → 12). Python `int()`처럼 앞뒤 공백과 `+` 하나는 받아요.
pub(crate) fn number_of(url: Option<&Value>) -> Option<u64> {
    let (_, last) = url?.as_str()?.rsplit_once('/')?;
    crate::py::strip(last).parse().ok()
}

/// 번호 → 댓글(issue_url로)과 인라인 리뷰 댓글(pull_request_url로)이에요. 받은 순서예요. 번호를 읽을 수 없는 댓글은
/// 건너뛰어요.
pub fn by_number(comments: &[Value], review_comments: &[Value]) -> HashMap<u64, Talk> {
    let mut talk: HashMap<u64, Talk> = HashMap::new();
    for c in comments {
        if let Some(n) = number_of(c.get("issue_url")) {
            talk.entry(n).or_default().comments.push(c.clone());
        }
    }
    for c in review_comments {
        if let Some(n) = number_of(c.get("pull_request_url")) {
            talk.entry(n).or_default().review_comments.push(c.clone());
        }
    }
    talk
}

/// id로 합쳐요. 처음 나온 자리에 마지막 값이에요(앞의 것 다음 새것, 같은 id는 새것이 이겨요). id가 없는 것은
/// 건너뛰어요.
pub(crate) fn merge(old: &[Value], new: &[Value]) -> Vec<Value> {
    let mut seen = Map::new(); // 순서를 지키는 dict예요(preserve_order)
    for x in old.iter().chain(new) {
        if let Some(id) = x.get("id") {
            seen.insert(id.to_string(), x.clone());
        }
    }
    seen.into_iter().map(|(_, x)| x).collect()
}

/// 저장할 항목 하나예요. 키 순서: number, kind, updated_at, issue, comments, review_comments, extras.
pub(crate) fn item(
    issue: &Value,
    comments: Vec<Value>,
    review_comments: Vec<Value>,
    extras: Value,
) -> Value {
    let kind = if issue.get("pull_request").is_some() {
        "pull"
    } else {
        "issue"
    };
    let mut m = Map::new();
    m.insert(
        "number".into(),
        issue.get("number").cloned().unwrap_or(Value::Null),
    );
    m.insert("kind".into(), Value::from(kind));
    m.insert(
        "updated_at".into(),
        issue.get("updated_at").cloned().unwrap_or(Value::Null),
    );
    m.insert("issue".into(), issue.clone());
    m.insert("comments".into(), Value::Array(comments));
    m.insert("review_comments".into(), Value::Array(review_comments));
    m.insert("extras".into(), extras);
    Value::Object(m)
}

/// `x`의 `key` 목록이에요(없거나 목록이 아니면 비어 있어요).
pub(crate) fn list<'a>(x: Option<&'a Value>, key: &str) -> &'a [Value] {
    x.and_then(|x| x.get(key))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// 나열된 이슈마다 항목이에요(issues 순서): 댓글(issue_url로)과 리뷰 댓글(pull_request_url로)을 붙여요. 댓글은 저장된
/// 항목의 것과 id로 합쳐요: 전체 목록이 짧게 오는 때가 있었고(2026-10-23), 짧은 목록이 저장한 것을 지우면 안 돼요.
/// PR 추가 정보는 `old`에서 가져와요. 같은 번호가 다시 나오면 자리는 처음, 값은 나중이에요.
pub fn build_items(
    issues: &[Value],
    comments: &[Value],
    review_comments: &[Value],
    old: &BTreeMap<u64, Value>,
) -> Vec<(u64, Value)> {
    let talk = by_number(comments, review_comments);
    let mut out: Vec<(u64, Value)> = Vec::new();
    let mut at: HashMap<u64, usize> = HashMap::new();
    for it in issues {
        let Some(n) = it.get("number").and_then(Value::as_u64) else {
            continue;
        };
        // 저장된 항목이 비었거나 객체가 아니면 없는 것과 같아요
        let prev = old
            .get(&n)
            .filter(|p| p.as_object().is_some_and(|m| !m.is_empty()));
        let seen = talk.get(&n);
        let said = merge(
            list(prev, "comments"),
            seen.map_or(&[], |t| t.comments.as_slice()),
        );
        let inline = merge(
            list(prev, "review_comments"),
            seen.map_or(&[], |t| t.review_comments.as_slice()),
        );
        let extras = prev
            .and_then(|p| p.get("extras"))
            .cloned()
            .unwrap_or(Value::Null);
        let value = item(it, said, inline, extras);
        match at.get(&n) {
            Some(&i) => out[i].1 = value,
            None => {
                at.insert(n, out.len());
                out.push((n, value));
            }
        }
    }
    out
}

/// 빈 상태예요. 키 순서도 고정이에요.
pub fn blank_state() -> Map<String, Value> {
    let cursors: Map<String, Value> = STREAMS
        .iter()
        .map(|s| (s.to_string(), Value::Null))
        .collect();
    let mut m = Map::new();
    m.insert("version".into(), Value::from(2));
    m.insert("repo".into(), Value::Null);
    m.insert("fetched_at".into(), Value::Null);
    m.insert("cursors".into(), Value::Object(cursors));
    m.insert("etags".into(), Value::Object(Map::new()));
    m.insert("reconciled_at".into(), Value::Null);
    m.insert("repo_id".into(), Value::Null);
    m.insert("full_name".into(), Value::Null);
    m.insert("lost".into(), Value::from(0));
    m
}

/// `version`이 2예요(2.0도 같아요).
fn is_version_2(v: Option<&Value>) -> bool {
    v.and_then(Value::as_f64) == Some(2.0)
}

/// 동기화 상태예요. 없거나, 읽을 수 없거나, 옛 모양(version 2가 아님)이면 빈 상태라 다음 받기가 모두 나열해요.
/// 아는 키만 빈 상태의 순서로 가져오고, 커서는 빈 커서 위에 저장된 것을 덮어요. 파일이 있는데 열 수 없으면(폴더,
/// 권한) stderr에 `github: state.json unreadable (<IO 오류 종류>)` 한 줄을 남기고 빈 상태예요.
pub fn load_state(raw: &Path) -> Map<String, Value> {
    let mut out = blank_state();
    let s = match store::read_json(&raw.join("state.json")) {
        Ok(Stored::Value(Value::Object(s))) => s,
        Ok(_) => return out,
        Err(e) => {
            eprintln!("github: state.json unreadable ({:?})", e.kind());
            return out;
        }
    };
    if !is_version_2(s.get("version")) {
        return out;
    }
    for (k, v) in out.iter_mut() {
        if let Some(saved) = s.get(k) {
            *v = saved.clone();
        }
    }
    let Value::Object(mut cursors) = blank_state()["cursors"].clone() else {
        unreachable!("blank cursors are an object")
    };
    if let Some(Value::Object(saved)) = s.get("cursors") {
        for (k, v) in saved {
            cursors.insert(k.clone(), v.clone());
        }
    }
    out.insert("cursors".into(), Value::Object(cursors));
    out
}

pub fn save_state(raw: &Path, state: &Map<String, Value>) -> std::io::Result<()> {
    store::write_json(&raw.join("state.json"), &Value::Object(state.clone()), None)
}
