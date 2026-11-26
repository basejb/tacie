//! Fetching from Linear (docs/18 9절, 16-1절).
//!
//! - Fetches teams (`teams`) to the end, and fetches issues and documents one page at a time in ascending `updatedAt`
//!   order, saving the cursor **every page**. Comments and project updates have no `sort`, so they are fetched in
//!   `SLICE_DAYS` time slices and saved per slice. Even if it stops on a limit, next time it resumes from where it
//!   stopped (`OVERLAP_SECS` before the cursor).
//! - Canceled and team conditions are not put on the Linear query: with them, issues canceled or moved later would
//!   not show up in incremental fetches and could not be deleted. After fetching, canceled or trashed items are
//!   deleted, and so are items of teams gone from the visible teams (nothing is deleted if the team list is empty or
//!   too many teams vanished).
//! - Rewriting an issue keeps its saved comments. Issues and updates whose comment threads have not been fetched yet
//!   (`thread: false`) fetch the whole thread: the initial fetch, un-canceling, restoring from the trash, moving to a
//!   visible team, and an old issue edited today.
//! - Every 7 days it fetches only ids again and matches them with the saved copy (there is no API that reports
//!   deletions). If too many are missing, nothing is deleted.
//! - When `still()` is false (the connection changed) it writes no more files and stops with `Busy`.
//!
//! Content (titles, bodies) is never put in logs or errors.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{Kind, SourceError};
use crate::linear::api::{LinearApi, unknown_field};
use crate::store::{Stored, read_json, write_json};

pub(crate) const WINDOW_DAYS: i64 = 730;
pub(crate) const OVERLAP_SECS: i64 = 3 * 3600;
pub(crate) const RECONCILE_DAYS: i64 = 7;
pub(crate) const PAGE: usize = 50;
pub(crate) const SLICE_DAYS: i64 = 30;
pub(crate) const STATE: &str = "state.json";
pub(crate) const ITEMS: &str = "items";

const TEAMS: &str = "query LinearTeams($first: Int!, $after: String) { teams(first: $first, after: $after) { nodes { id name key } pageInfo { hasNextPage endCursor } } }";
const ISSUES: &str = "query LinearIssues($first: Int!, $after: String, $since: DateTimeOrDuration!) { issues(first: $first, after: $after, includeArchived: true, sort: [{updatedAt: {order: Ascending}}], filter: {updatedAt: {gt: $since}}) { nodes { id identifier title description url state { name type } assignee { name } creator { name } labels(first: 10) { nodes { name } } priorityLabel project { name } team { id name key } parent { identifier } createdAt updatedAt completedAt archivedAt trashed attachments(first: 10) { nodes { title url } } } pageInfo { hasNextPage endCursor } } }";
const DOCUMENTS: &str = "query LinearDocuments($first: Int!, $after: String, $since: DateTimeOrDuration!) { documents(first: $first, after: $after, includeArchived: true, sort: [{updatedAt: {order: Ascending}}], filter: {updatedAt: {gt: $since}}) { nodes { id title content url updatedAt trashed team { id } issue { id } project { name teams(first: 5) { nodes { id } } } } pageInfo { hasNextPage endCursor } } }";
const UPDATES: &str = "query LinearUpdates($first: Int!, $after: String, $since: DateTimeOrDuration!, $until: DateTimeOrDuration!) { projectUpdates(first: $first, after: $after, includeArchived: true, filter: {updatedAt: {gt: $since, lte: $until}}) { nodes { id body health url createdAt updatedAt user { name } project { name teams(first: 5) { nodes { id } } } } pageInfo { hasNextPage endCursor } } }";
const COMMENTS: &str = "query LinearComments($first: Int!, $after: String, $since: DateTimeOrDuration!, $until: DateTimeOrDuration!) { comments(first: $first, after: $after, includeArchived: true, filter: {updatedAt: {gt: $since, lte: $until}}) { nodes { id body url createdAt updatedAt user { name } issue { id } projectUpdate { id } parent { id } } pageInfo { hasNextPage endCursor } } }";
const THREADS: &str = "query LinearThreads($first: Int!, $after: String, $ids: [ID!]!) { comments(first: $first, after: $after, includeArchived: true, filter: {or: [{issue: {id: {in: $ids}}}, {projectUpdate: {id: {in: $ids}}}]}) { nodes { id body url createdAt updatedAt user { name } issue { id } projectUpdate { id } parent { id } } pageInfo { hasNextPage endCursor } } }";
const ISSUE_IDS: &str = "query LinearIssueIds($first: Int!, $after: String, $since: DateTimeOrDuration!) { issues(first: $first, after: $after, includeArchived: true, filter: {updatedAt: {gt: $since}}) { nodes { id } pageInfo { hasNextPage endCursor } } }";
const DOCUMENT_IDS: &str = "query LinearDocumentIds($first: Int!, $after: String, $since: DateTimeOrDuration!) { documents(first: $first, after: $after, includeArchived: true, filter: {updatedAt: {gt: $since}}) { nodes { id } pageInfo { hasNextPage endCursor } } }";
const UPDATE_IDS: &str = "query LinearUpdateIds($first: Int!, $after: String, $since: DateTimeOrDuration!) { projectUpdates(first: $first, after: $after, includeArchived: true, filter: {updatedAt: {gt: $since}}) { nodes { id } pageInfo { hasNextPage endCursor } } }";
const COMMENT_IDS: &str = "query LinearCommentIds($first: Int!, $after: String, $since: DateTimeOrDuration!) { comments(first: $first, after: $after, includeArchived: true, filter: {updatedAt: {gt: $since}}) { nodes { id } pageInfo { hasNextPage endCursor } } }";

/// `state.json`이에요.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct State {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub cursors: BTreeMap<String, String>,
    #[serde(default)]
    pub teams: Vec<Value>,
    #[serde(default)]
    pub muted: Vec<String>,
    #[serde(default)]
    pub lost_since: Option<String>,
    #[serde(default)]
    pub reconciled_at: Option<String>,
    #[serde(default)]
    pub fetched_at: Option<String>,
}

/// `state.json`, `items/`와 지우기를 함께 막는 자물쇠예요(받기, 앱의 연결·끈 팀, 갱신의 `lost_since`가 같은 파일을
/// 읽고 써요). 엔진에 Linear 원본은 하나라 프로세스에 하나예요.
static STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn state_lock() -> std::sync::MutexGuard<'static, ()> {
    STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn iso(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

pub(crate) fn load_state(dir: &Path) -> State {
    match read_json(&dir.join(STATE)) {
        Ok(Stored::Value(v)) => serde_json::from_value(v).unwrap_or_default(),
        _ => State::default(),
    }
}

pub(crate) fn save_state(dir: &Path, s: &State) -> Result<(), SourceError> {
    fs::create_dir_all(dir)?;
    let mut s = s.clone();
    s.version = 1;
    write_json(
        &dir.join(STATE),
        &serde_json::to_value(&s).map_err(|e| SourceError::Json(e.to_string()))?,
        None,
    )?;
    Ok(())
}

/// Linear id는 UUID예요. 그 밖의 글자가 있으면 파일 이름으로 쓰지 않아요.
fn safe(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn item_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(ITEMS).join(format!("{name}.json"))
}

pub(crate) fn read_item(dir: &Path, name: &str) -> Option<Value> {
    match read_json(&item_path(dir, name)) {
        Ok(Stored::Value(v)) => Some(v),
        _ => None,
    }
}

/// 저장된 항목 이름(`issue-…`, `doc-…`, `update-…`)이에요.
pub(crate) fn item_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir.join(ITEMS))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

/// 시간 조각 하나의 노드를 저장하는 함수예요.
type PutMany<'b> = &'b dyn Fn(&Run<'_>, Vec<Value>) -> Result<(), SourceError>;

/// 시간을 잘라 받는 목록이에요: (커서 이름, 질의, 답의 뿌리).
type List<'b> = (&'b str, &'b str, &'b str);

struct Run<'a> {
    api: &'a dyn LinearApi,
    dir: &'a Path,
    still: &'a dyn Fn() -> bool,
    /// 지금 연결의 워크스페이스예요. 저장본이 다른 워크스페이스면 쓰지 않아요.
    workspace: Option<&'a str>,
}

impl Run<'_> {
    fn guard(&self) -> Result<(), SourceError> {
        if (self.still)() {
            Ok(())
        } else {
            Err(SourceError::service(
                Kind::Busy,
                "linear: connection changed",
            ))
        }
    }

    fn write(&self, name: &str, v: &Value) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        fs::create_dir_all(self.dir.join(ITEMS))?;
        write_json(&item_path(self.dir, name), v, None)?;
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        match fs::remove_file(item_path(self.dir, name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// 받기의 상태를 써요. 끈 팀과 `lost_since`는 다른 쪽(앱의 연결, 갱신)이 받는 도중에 바꿀 수 있어서 디스크의 것을
    /// 그대로 둬요.
    fn save(&self, s: &State) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        let disk = load_state(self.dir);
        if let (Some(ours), Some(theirs)) = (self.workspace, disk.workspace_id.as_deref())
            && ours != theirs
        {
            return Err(SourceError::service(
                Kind::Busy,
                "linear: the saved items are another workspace's",
            ));
        }
        let mut s = s.clone();
        if s.workspace_id.is_none() {
            s.workspace_id = disk
                .workspace_id
                .clone()
                .or(self.workspace.map(str::to_string));
        }
        s.muted = disk.muted;
        s.lost_since = disk.lost_since;
        save_state(self.dir, &s)
    }

    /// 질의 하나예요. 모르는 칸이면 그 칸을 빼고 한 번 더 물어요.
    fn ask(&self, query: &str, vars: &Value) -> Result<Value, SourceError> {
        let mut q = query.to_string();
        for _ in 0..4 {
            let r = self.api.query(&q, vars)?;
            if let Some(d) = r.data {
                return Ok(d);
            }
            let errors = Value::Array(r.errors);
            match unknown_field(&errors).and_then(|f| strip_field(&q, &f).map(|s| (f, s))) {
                Some((f, stripped)) => {
                    eprintln!("linear: dropped unknown field {f}");
                    q = stripped;
                }
                None => {
                    let codes: Vec<&str> = errors
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|e| e["extensions"]["code"].as_str())
                                .collect()
                        })
                        .unwrap_or_default();
                    return Err(SourceError::service(
                        Kind::Down,
                        format!("linear: query failed {codes:?}"),
                    ));
                }
            }
        }
        Err(SourceError::service(Kind::Down, "linear: query failed"))
    }

    /// 한 목록을 끝까지 쪽마다 받아 `each`에 넘겨요.
    fn pages(
        &self,
        query: &str,
        root: &str,
        vars: Value,
        each: &mut dyn FnMut(Vec<Value>) -> Result<(), SourceError>,
    ) -> Result<(), SourceError> {
        let mut after: Option<String> = None;
        loop {
            let mut v = vars.clone();
            v["first"] = json!(PAGE);
            v["after"] = after.clone().map_or(Value::Null, Value::String);
            let d = self.ask(query, &v)?;
            let conn = &d[root];
            let nodes = conn["nodes"].as_array().cloned().unwrap_or_default();
            each(nodes)?;
            if conn["pageInfo"]["hasNextPage"].as_bool() != Some(true) {
                return Ok(());
            }
            after = conn["pageInfo"]["endCursor"].as_str().map(str::to_string);
            if after.is_none() {
                return Ok(());
            }
        }
    }

    fn all(&self, query: &str, root: &str, vars: Value) -> Result<Vec<Value>, SourceError> {
        let mut out = Vec::new();
        self.pages(query, root, vars, &mut |n| {
            out.extend(n);
            Ok(())
        })?;
        Ok(out)
    }
}

/// 질의 글에서 칸 하나(인자와 `{ … }`까지)를 빼요.
pub(crate) fn strip_field(query: &str, field: &str) -> Option<String> {
    let bytes = query.as_bytes();
    let mut from = 0;
    while let Some(off) = query[from..].find(field) {
        let start = from + off;
        let end = start + field.len();
        let word = |i: usize| {
            bytes
                .get(i)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        };
        if (start > 0 && word(start - 1)) || word(end) {
            from = end;
            continue;
        }
        let mut i = end;
        let skip_ws = |mut i: usize| {
            while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
                i += 1;
            }
            i
        };
        for (open, close) in [(b'(', b')'), (b'{', b'}')] {
            let j = skip_ws(i);
            if bytes.get(j) == Some(&open) {
                let mut depth = 0;
                let mut k = j;
                while k < bytes.len() {
                    if bytes[k] == open {
                        depth += 1;
                    } else if bytes[k] == close {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    k += 1;
                }
                i = k + 1;
            }
        }
        return Some(format!(
            "{}{}",
            &query[..start],
            &query[i.min(query.len())..]
        ));
    }
    None
}

fn since(s: &State, list: &str, floor: DateTime<Utc>) -> DateTime<Utc> {
    s.cursors
        .get(list)
        .and_then(|c| parse(c))
        .map(|c| c - Duration::seconds(OVERLAP_SECS))
        .map_or(floor, |c| c.max(floor))
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter()
        .try_fold(v, |v, k| v.get(*k))
        .and_then(Value::as_str)
}

/// 항목이 속한 팀 id들이에요. 비었으면 팀을 모르는 항목이에요(지우지 않아요).
pub(crate) fn teams_of(dir: &Path, name: &str, v: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(t) = str_at(v, &["team", "id"]) {
        out.push(t.to_string());
    }
    if let Some(ts) = v["project"]["teams"]["nodes"].as_array() {
        out.extend(
            ts.iter()
                .filter_map(|t| t["id"].as_str().map(str::to_string)),
        );
    }
    if out.is_empty()
        && name.starts_with("doc-")
        && let Some(i) = str_at(v, &["issue", "id"]).filter(|i| safe(i))
        && let Some(issue) = read_item(dir, &format!("issue-{i}"))
    {
        return teams_of(dir, "issue", &issue);
    }
    out
}

fn merge_comment(list: &mut Vec<Value>, c: Value) {
    let id = c["id"].clone();
    match list.iter_mut().find(|x| x["id"] == id) {
        Some(x) => *x = c,
        None => list.push(c),
    }
    list.sort_by(|a, b| a["createdAt"].as_str().cmp(&b["createdAt"].as_str()));
}

impl Run<'_> {
    fn put_issue(&self, n: Value) -> Result<(), SourceError> {
        let Some(id) = n["id"].as_str().filter(|i| safe(i)).map(str::to_string) else {
            return Ok(());
        };
        let name = format!("issue-{id}");
        if n["trashed"].as_bool() == Some(true)
            || str_at(&n, &["state", "type"]) == Some("canceled")
        {
            return self.remove(&name);
        }
        let old = read_item(self.dir, &name);
        let mut v = n;
        v["comments"] = old.as_ref().map_or(json!([]), |o| o["comments"].clone());
        v["thread"] = json!(
            old.as_ref()
                .is_some_and(|o| o["thread"].as_bool() == Some(true))
        );
        self.write(&name, &v)
    }

    fn put_doc(&self, n: Value) -> Result<(), SourceError> {
        let Some(id) = n["id"].as_str().filter(|i| safe(i)).map(str::to_string) else {
            return Ok(());
        };
        let name = format!("doc-{id}");
        if n["trashed"].as_bool() == Some(true) {
            return self.remove(&name);
        }
        self.write(&name, &n)
    }

    fn put_update(&self, n: Value) -> Result<(), SourceError> {
        let Some(id) = n["id"].as_str().filter(|i| safe(i)).map(str::to_string) else {
            return Ok(());
        };
        let name = format!("update-{id}");
        let old = read_item(self.dir, &name);
        let mut v = n;
        v["comments"] = old.as_ref().map_or(json!([]), |o| o["comments"].clone());
        v["thread"] = json!(
            old.as_ref()
                .is_some_and(|o| o["thread"].as_bool() == Some(true))
        );
        self.write(&name, &v)
    }

    /// 댓글을 그 이슈나 업데이트 파일에 합쳐요. 그 파일이 없으면 버려요.
    fn put_comments(&self, nodes: Vec<Value>) -> Result<(), SourceError> {
        let mut by: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for c in nodes {
            let parent = str_at(&c, &["issue", "id"])
                .map(|i| format!("issue-{i}"))
                .or_else(|| str_at(&c, &["projectUpdate", "id"]).map(|u| format!("update-{u}")));
            if let Some(p) = parent.filter(|p| safe(p)) {
                by.entry(p).or_default().push(c);
            }
        }
        for (name, cs) in by {
            let Some(mut v) = read_item(self.dir, &name) else {
                continue;
            };
            let mut list = v["comments"].as_array().cloned().unwrap_or_default();
            for c in cs {
                merge_comment(&mut list, c);
            }
            v["comments"] = Value::Array(list);
            self.write(&name, &v)?;
        }
        Ok(())
    }

    /// 스레드를 아직 받지 않은 이슈·업데이트의 댓글을 처음부터 받아요.
    fn backfill(&self) -> Result<(), SourceError> {
        let need: Vec<String> = item_names(self.dir)
            .into_iter()
            .filter(|n| n.starts_with("issue-") || n.starts_with("update-"))
            .filter(|n| read_item(self.dir, n).is_some_and(|v| v["thread"].as_bool() != Some(true)))
            .collect();
        for batch in need.chunks(PAGE) {
            let ids: Vec<&str> = batch
                .iter()
                .filter_map(|n| n.split_once('-').map(|x| x.1))
                .collect();
            let comments = self.all(THREADS, "comments", json!({"ids": ids}))?;
            let mut by: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            for c in comments {
                let parent = str_at(&c, &["issue", "id"])
                    .map(|i| format!("issue-{i}"))
                    .or_else(|| {
                        str_at(&c, &["projectUpdate", "id"]).map(|u| format!("update-{u}"))
                    });
                if let Some(p) = parent {
                    by.entry(p).or_default().push(c);
                }
            }
            for name in batch {
                let Some(mut v) = read_item(self.dir, name) else {
                    continue;
                };
                let mut list = v["comments"].as_array().cloned().unwrap_or_default();
                for c in by.remove(name).unwrap_or_default() {
                    merge_comment(&mut list, c);
                }
                v["comments"] = Value::Array(list);
                v["thread"] = json!(true);
                self.write(name, &v)?;
            }
        }
        Ok(())
    }

    /// 쪽마다 커서를 저장하며 정렬된 목록(이슈, 문서)을 받아요.
    fn sorted(
        &self,
        s: &mut State,
        list: &str,
        query: &str,
        root: &str,
        floor: DateTime<Utc>,
        put: &dyn Fn(&Self, Value) -> Result<(), SourceError>,
    ) -> Result<(), SourceError> {
        let from = since(s, list, floor);
        self.pages(query, root, json!({"since": iso(from)}), &mut |nodes| {
            let last = nodes
                .last()
                .and_then(|n| n["updatedAt"].as_str())
                .map(str::to_string);
            for n in nodes {
                put(self, n)?;
            }
            if let Some(l) = last {
                s.cursors.insert(list.to_string(), l);
                self.save(s)?;
            }
            Ok(())
        })
    }

    /// 시간을 잘라 받는 목록(댓글, 업데이트)이에요. 조각마다 커서를 저장해요.
    fn sliced(
        &self,
        s: &mut State,
        (list, query, root): List<'_>,
        floor: DateTime<Utc>,
        now: DateTime<Utc>,
        put: PutMany<'_>,
    ) -> Result<(), SourceError> {
        let mut a = since(s, list, floor);
        while a < now {
            let b = (a + Duration::days(SLICE_DAYS)).min(now);
            let nodes = self.all(query, root, json!({"since": iso(a), "until": iso(b)}))?;
            put(self, nodes)?;
            s.cursors.insert(list.to_string(), iso(b));
            self.save(s)?;
            a = b;
        }
        Ok(())
    }

    /// 항목의 팀이 지금 보이는 팀에 없으면 지워요. 지난 받기가 끊겨 팀 목록만 바뀌었어도 다음 받기가 지워요. 팀 목록이
    /// 비었거나, 항목이 가리키는 팀 중 20%(최소 5개)보다 많이 사라졌으면 아무것도 지우지 않아요.
    fn prune_teams(&self, new: &[Value]) -> Result<(), SourceError> {
        let new: HashSet<String> = new
            .iter()
            .filter_map(|t| t["id"].as_str().map(str::to_string))
            .collect();
        let mut old: HashSet<String> = HashSet::new();
        for name in item_names(self.dir) {
            if let Some(v) = read_item(self.dir, &name) {
                old.extend(teams_of(self.dir, &name, &v));
            }
        }
        let gone = old.difference(&new).count();
        if new.is_empty() || gone as f64 > (old.len() as f64 * 0.2).max(5.0) {
            if gone > 0 {
                eprintln!("linear: {gone} teams vanished, nothing removed");
            }
            return Ok(());
        }
        for name in item_names(self.dir) {
            let Some(v) = read_item(self.dir, &name) else {
                continue;
            };
            let ts = teams_of(self.dir, &name, &v);
            if !ts.is_empty() && !ts.iter().any(|t| new.contains(t)) {
                self.remove(&name)?;
            }
        }
        Ok(())
    }

    /// 7일마다 id를 다시 받아 저장본에 없는 걸 지워요. 너무 많이 빠지면 그 목록은 건너뛰어요.
    fn reconcile(&self, floor: DateTime<Utc>) -> Result<bool, SourceError> {
        let since = json!({"since": iso(floor)});
        let mut clean = true;
        for (prefix, query, root) in [
            ("issue-", ISSUE_IDS, "issues"),
            ("doc-", DOCUMENT_IDS, "documents"),
            ("update-", UPDATE_IDS, "projectUpdates"),
        ] {
            let remote: HashSet<String> = self
                .all(query, root, since.clone())?
                .iter()
                .filter_map(|n| n["id"].as_str().map(str::to_string))
                .collect();
            let stored: Vec<String> = item_names(self.dir)
                .into_iter()
                .filter(|n| n.starts_with(prefix))
                .collect();
            let missing: Vec<&String> = stored
                .iter()
                .filter(|n| !remote.contains(&n[prefix.len()..]))
                .collect();
            if stored.is_empty() {
                continue;
            }
            if remote.is_empty() || missing.len() as f64 > (stored.len() as f64 * 0.2).max(5.0) {
                eprintln!(
                    "linear: reconcile skipped {root}: {} of {} missing",
                    missing.len(),
                    stored.len()
                );
                clean = false;
                continue;
            }
            for n in missing {
                self.remove(n)?;
            }
        }
        // 댓글: 창 안에서 고친 댓글 중 Linear에 없는 것만 빼요(창 밖 옛 댓글은 그대로 둬요).
        let remote: HashSet<String> = self
            .all(COMMENT_IDS, "comments", since)?
            .iter()
            .filter_map(|n| n["id"].as_str().map(str::to_string))
            .collect();
        let floor_s = iso(floor);
        let mut stale = Vec::new();
        for name in item_names(self.dir)
            .into_iter()
            .filter(|n| n.starts_with("issue-") || n.starts_with("update-"))
        {
            let Some(v) = read_item(self.dir, &name) else {
                continue;
            };
            let list = v["comments"].as_array().cloned().unwrap_or_default();
            let kept: Vec<Value> = list
                .iter()
                .filter(|c| {
                    c["updatedAt"]
                        .as_str()
                        .is_none_or(|u| u <= floor_s.as_str())
                        || c["id"].as_str().is_some_and(|i| remote.contains(i))
                })
                .cloned()
                .collect();
            if kept.len() != list.len() {
                stale.push((name, v, kept));
            }
        }
        let gone: usize = stale
            .iter()
            .map(|(_, v, k)| v["comments"].as_array().map_or(0, |a| a.len()) - k.len())
            .sum();
        if gone > 0 && remote.is_empty() {
            return Ok(false);
        }
        for (name, mut v, kept) in stale {
            v["comments"] = Value::Array(kept);
            self.write(&name, &v)?;
        }
        // 창 밖으로 나간 항목이에요.
        for name in item_names(self.dir) {
            let Some(v) = read_item(self.dir, &name) else {
                continue;
            };
            if v["updatedAt"]
                .as_str()
                .is_some_and(|u| u <= floor_s.as_str())
            {
                self.remove(&name)?;
            }
        }
        Ok(clean)
    }
}

/// Linear에서 받아 `raw_dir`(state.json, items/)을 고쳐요. `progress(done, total)`은 1000 단위 몫이에요.
pub fn fetch_with(
    api: &dyn LinearApi,
    raw_dir: &Path,
    now: DateTime<Utc>,
    workspace: Option<&str>,
    still: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<(), SourceError> {
    let run = Run {
        api,
        dir: raw_dir,
        still,
        workspace,
    };
    let floor = now - Duration::days(WINDOW_DAYS);
    let mut s = load_state(raw_dir);
    let teams = run.all(TEAMS, "teams", json!({}))?;
    if !teams.is_empty() {
        s.teams = teams.clone();
    }
    run.save(&s)?;
    progress(100, 1000);

    let first_comments = !s.cursors.contains_key("comments");
    run.sorted(&mut s, "issues", ISSUES, "issues", floor, &|r, n| {
        r.put_issue(n)
    })?;
    progress(450, 1000);
    run.sorted(
        &mut s,
        "documents",
        DOCUMENTS,
        "documents",
        floor,
        &|r, n| r.put_doc(n),
    )?;
    progress(550, 1000);
    run.sliced(
        &mut s,
        ("updates", UPDATES, "projectUpdates"),
        floor,
        now,
        &|r, ns| {
            for n in ns {
                r.put_update(n)?;
            }
            Ok(())
        },
    )?;
    progress(650, 1000);
    if first_comments {
        // 처음에는 스레드 받기(`backfill`)가 모든 댓글을 받아요. 시간 조각으로 또 받지 않아요.
        s.cursors.insert("comments".to_string(), iso(now));
        run.save(&s)?;
    } else {
        run.sliced(
            &mut s,
            ("comments", COMMENTS, "comments"),
            floor,
            now,
            &|r, ns| r.put_comments(ns),
        )?;
    }
    progress(800, 1000);
    run.backfill()?;
    progress(900, 1000);
    run.prune_teams(&teams)?;
    let due = s
        .reconciled_at
        .as_deref()
        .and_then(parse)
        .is_none_or(|r| now - r >= Duration::days(RECONCILE_DAYS));
    if due && run.reconcile(floor)? {
        s.reconciled_at = Some(iso(now));
    }
    s.fetched_at = Some(iso(now));
    run.save(&s)?;
    progress(1000, 1000);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linear::api::Reply;
    use chrono::TimeZone;
    use serde_json::json;
    use std::sync::Mutex;

    fn t(days_ago: i64) -> String {
        iso(now() - chrono::Duration::days(days_ago))
    }
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap()
    }

    #[derive(Default)]
    struct Fake {
        teams: Mutex<Vec<Value>>,
        issues: Mutex<Vec<Value>>,
        documents: Mutex<Vec<Value>>,
        updates: Mutex<Vec<Value>>,
        comments: Mutex<Vec<Value>>,
        refuse: bool,
        /// After this many answered calls, every call is busy.
        busy_after: Mutex<Option<usize>>,
        /// A field that does not exist (answers a validation error while the query names it).
        missing: Option<String>,
        calls: Mutex<Vec<(String, Value)>>,
    }

    fn op(q: &str) -> String {
        q.trim_start()
            .strip_prefix("query ")
            .unwrap_or("")
            .split(['(', ' ', '{'])
            .next()
            .unwrap_or("")
            .to_string()
    }
    fn after(a: &str, b: &Value) -> bool {
        b.as_str().is_none_or(|b| a > b)
    }
    fn page(nodes: Vec<Value>, vars: &Value) -> Value {
        let start: usize = vars["after"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let first = vars["first"].as_u64().unwrap_or(50) as usize;
        let end = (start + first).min(nodes.len());
        json!({"nodes": nodes[start.min(end)..end], "pageInfo": {"hasNextPage": end < nodes.len(), "endCursor": end.to_string()}})
    }

    impl LinearApi for Fake {
        fn query(&self, q: &str, vars: &Value) -> Result<Reply, SourceError> {
            let name = op(q);
            self.calls
                .lock()
                .unwrap()
                .push((name.clone(), vars.clone()));
            if self.refuse {
                return Err(SourceError::service(Kind::Key, "linear: refused"));
            }
            {
                let mut b = self.busy_after.lock().unwrap();
                if let Some(n) = b.as_mut() {
                    if *n == 0 {
                        return Err(SourceError::service(Kind::Busy, "linear: rate limited"));
                    }
                    *n -= 1;
                }
            }
            if let Some(f) = &self.missing
                && q.contains(f.as_str())
            {
                return Ok(Reply {
                    data: None,
                    errors: vec![
                        json!({"message": format!("Cannot query field \"{f}\" on type \"Issue\".")}),
                    ],
                });
            }
            let window = |list: &Mutex<Vec<Value>>, sort: bool| {
                let mut v: Vec<Value> = list
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|n| after(n["updatedAt"].as_str().unwrap(), &vars["since"]))
                    .filter(|n| {
                        vars["until"]
                            .as_str()
                            .is_none_or(|u| n["updatedAt"].as_str().unwrap() <= u)
                    })
                    .cloned()
                    .collect();
                if sort {
                    v.sort_by(|a, b| a["updatedAt"].as_str().cmp(&b["updatedAt"].as_str()));
                }
                v
            };
            let data = match name.as_str() {
                "LinearTeams" => json!({"teams": page(self.teams.lock().unwrap().clone(), vars)}),
                "LinearIssues" | "LinearIssueIds" => {
                    json!({"issues": page(window(&self.issues, true), vars)})
                }
                "LinearDocuments" | "LinearDocumentIds" => {
                    json!({"documents": page(window(&self.documents, true), vars)})
                }
                "LinearUpdates" | "LinearUpdateIds" => {
                    json!({"projectUpdates": page(window(&self.updates, false), vars)})
                }
                "LinearComments" | "LinearCommentIds" => {
                    json!({"comments": page(window(&self.comments, false), vars)})
                }
                "LinearThreads" => {
                    let ids: Vec<&str> = vars["ids"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect();
                    let v: Vec<Value> = self
                        .comments
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|c| {
                            ids.contains(&c["issue"]["id"].as_str().unwrap_or("-"))
                                || ids.contains(&c["projectUpdate"]["id"].as_str().unwrap_or("-"))
                        })
                        .cloned()
                        .collect();
                    json!({"comments": page(v, vars)})
                }
                other => panic!("unexpected query {other}"),
            };
            Ok(Reply {
                data: Some(data),
                errors: vec![],
            })
        }
    }

    fn team(id: &str) -> Value {
        json!({"id": id, "name": format!("Team {id}"), "key": id.to_uppercase()})
    }
    fn issue(id: &str, team: &str, updated: &str) -> Value {
        json!({"id": id, "identifier": format!("ENG-{id}"), "title": format!("Issue {id}"), "description": "body",
               "url": format!("https://linear.app/x/issue/ENG-{id}"), "state": {"name": "Done", "type": "completed"},
               "team": {"id": team, "name": "T", "key": "T"}, "createdAt": updated, "updatedAt": updated, "trashed": false})
    }
    fn comment(id: &str, issue: &str, updated: &str) -> Value {
        json!({"id": id, "body": format!("comment {id}"), "user": {"name": "Ann"}, "issue": {"id": issue},
               "url": format!("https://linear.app/x/issue/ENG-{issue}#comment-{id}"), "createdAt": updated, "updatedAt": updated})
    }
    fn fake() -> Fake {
        let f = Fake::default();
        *f.teams.lock().unwrap() = vec![team("t1"), team("t2")];
        *f.issues.lock().unwrap() = vec![issue("1", "t1", &t(10)), issue("2", "t2", &t(5))];
        *f.comments.lock().unwrap() = vec![comment("c1", "1", &t(9)), comment("c2", "1", &t(8))];
        f
    }
    fn run(f: &Fake, dir: &Path) -> Result<(), SourceError> {
        fetch_with(f, dir, now(), None, &|| true, &mut |_, _| {})
    }
    fn item(dir: &Path, name: &str) -> Option<Value> {
        read_item(dir, name)
    }
    fn state(dir: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join("state.json")).unwrap()).unwrap()
    }
    fn sinces(f: &Fake, name: &str) -> Vec<Value> {
        f.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v["since"].clone())
            .collect()
    }

    #[test]
    fn first_fetch_saves_issues_with_their_comments_and_cursors() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        run(&f, d.path()).unwrap();
        let i = item(d.path(), "issue-1").unwrap();
        assert_eq!(i["identifier"], "ENG-1");
        let ids: Vec<&str> = i["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["c1", "c2"]);
        assert!(item(d.path(), "issue-2").is_some());
        let s = state(d.path());
        assert_eq!(s["cursors"]["issues"], t(5));
        assert_eq!(s["teams"].as_array().unwrap().len(), 2);
        assert!(s["cursors"]["comments"].is_string());
    }

    #[test]
    fn the_next_fetch_asks_from_three_hours_before_the_cursor() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        run(&f, d.path()).unwrap();
        f.calls.lock().unwrap().clear();
        run(&f, d.path()).unwrap();
        let want = iso(DateTime::parse_from_rfc3339(&t(5))
            .unwrap()
            .with_timezone(&Utc)
            - chrono::Duration::hours(3));
        assert_eq!(sinces(&f, "LinearIssues")[0], want);
    }

    #[test]
    fn nothing_older_than_two_years_is_asked() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        run(&f, d.path()).unwrap();
        assert_eq!(sinces(&f, "LinearIssues")[0], t(730));
    }

    #[test]
    fn an_issue_that_becomes_canceled_or_trashed_is_removed() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        run(&f, d.path()).unwrap();
        {
            let mut is = f.issues.lock().unwrap();
            is[0]["state"]["type"] = json!("canceled");
            is[0]["updatedAt"] = json!(t(1));
            is[1]["trashed"] = json!(true);
            is[1]["updatedAt"] = json!(t(1));
        }
        run(&f, d.path()).unwrap();
        assert!(item(d.path(), "issue-1").is_none());
        assert!(item(d.path(), "issue-2").is_none());
    }

    #[test]
    fn an_issue_from_a_team_no_longer_visible_is_removed() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        run(&f, d.path()).unwrap();
        f.teams.lock().unwrap().retain(|t| t["id"] == "t1");
        run(&f, d.path()).unwrap();
        assert!(item(d.path(), "issue-1").is_some());
        assert!(item(d.path(), "issue-2").is_none());
    }

    #[test]
    fn an_empty_teams_answer_deletes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        run(&f, d.path()).unwrap();
        f.teams.lock().unwrap().clear();
        run(&f, d.path()).unwrap();
        assert!(item(d.path(), "issue-2").is_some());
    }

    #[test]
    fn teams_are_paged_past_fifty() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        let mut many: Vec<Value> = (0..60).map(|i| team(&format!("x{i}"))).collect();
        many.push(team("t2"));
        many.push(team("t1"));
        *f.teams.lock().unwrap() = many;
        run(&f, d.path()).unwrap();
        assert_eq!(state(d.path())["teams"].as_array().unwrap().len(), 62);
        assert!(item(d.path(), "issue-2").is_some());
    }

    #[test]
    fn comments_for_unknown_issues_are_dropped() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        run(&f, d.path()).unwrap();
        f.comments.lock().unwrap().push(comment("c9", "404", &t(1)));
        run(&f, d.path()).unwrap();
        let all = std::fs::read_dir(d.path().join("items")).unwrap().count();
        assert_eq!(all, 2);
    }

    #[test]
    fn rewriting_an_issue_keeps_its_comments() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        run(&f, d.path()).unwrap();
        f.issues.lock().unwrap()[0]["updatedAt"] = json!(t(1));
        f.issues.lock().unwrap()[0]["title"] = json!("renamed");
        run(&f, d.path()).unwrap();
        let i = item(d.path(), "issue-1").unwrap();
        assert_eq!(i["title"], "renamed");
        assert_eq!(i["comments"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_reopened_issue_gets_its_old_comments() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        run(&f, d.path()).unwrap();
        f.issues.lock().unwrap()[0]["state"]["type"] = json!("canceled");
        f.issues.lock().unwrap()[0]["updatedAt"] = json!(t(2));
        run(&f, d.path()).unwrap();
        assert!(item(d.path(), "issue-1").is_none());
        f.issues.lock().unwrap()[0]["state"]["type"] = json!("started");
        f.issues.lock().unwrap()[0]["updatedAt"] = json!(t(1));
        run(&f, d.path()).unwrap();
        assert_eq!(
            item(d.path(), "issue-1").unwrap()["comments"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn an_issue_older_than_its_discussion_window_gets_its_whole_thread() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        f.comments
            .lock()
            .unwrap()
            .push(comment("old", "2", &t(900)));
        run(&f, d.path()).unwrap();
        let i = item(d.path(), "issue-2").unwrap();
        assert_eq!(i["comments"][0]["id"], "old");
    }

    #[test]
    fn update_comments_land_on_the_update() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        *f.updates.lock().unwrap() = vec![
            json!({"id": "u1", "body": "on track", "health": "onTrack", "updatedAt": t(3), "createdAt": t(3),
            "url": "https://linear.app/x/u1", "user": {"name": "Bo"}, "project": {"name": "P", "teams": {"nodes": [{"id": "t1"}]}}}),
        ];
        run(&f, d.path()).unwrap();
        f.comments.lock().unwrap().push(json!({"id": "uc", "body": "why", "user": {"name": "Ann"}, "issue": null,
            "projectUpdate": {"id": "u1"}, "url": "https://linear.app/x/u1#c", "createdAt": iso(now()), "updatedAt": iso(now())}));
        run(&f, d.path()).unwrap();
        assert_eq!(
            item(d.path(), "update-u1").unwrap()["comments"][0]["id"],
            "uc"
        );
    }

    #[test]
    fn a_busy_answer_keeps_saved_items_and_the_cursor_reached() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        run(&f, d.path()).unwrap();
        f.issues.lock().unwrap()[0]["updatedAt"] = json!(t(1));
        *f.busy_after.lock().unwrap() = Some(1); // teams answers, issues is busy
        let e = run(&f, d.path()).unwrap_err();
        assert_eq!(e.kind(), Kind::Busy);
        assert!(item(d.path(), "issue-2").is_some());
        assert_eq!(state(d.path())["cursors"]["issues"], t(5));
    }

    #[test]
    fn a_first_fetch_cut_short_resumes_from_the_last_page_not_two_years_ago() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        let many: Vec<Value> = (0..120)
            .map(|i| issue(&format!("n{i}"), "t1", &t(600 - i)))
            .collect();
        *f.issues.lock().unwrap() = many;
        *f.busy_after.lock().unwrap() = Some(2); // teams + one issues page
        assert!(run(&f, d.path()).is_err());
        let reached = state(d.path())["cursors"]["issues"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(reached, t(600 - 49));
        *f.busy_after.lock().unwrap() = None;
        f.calls.lock().unwrap().clear();
        run(&f, d.path()).unwrap();
        let want = iso(DateTime::parse_from_rfc3339(&reached)
            .unwrap()
            .with_timezone(&Utc)
            - chrono::Duration::hours(3));
        assert_eq!(sinces(&f, "LinearIssues")[0], want);
        assert_eq!(
            std::fs::read_dir(d.path().join("items")).unwrap().count(),
            120
        );
    }

    #[test]
    fn a_refusal_fails_with_key() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake {
            refuse: true,
            ..fake()
        };
        assert_eq!(run(&f, d.path()).unwrap_err().kind(), Kind::Key);
    }

    #[test]
    fn an_unknown_field_is_dropped_and_asked_again() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake {
            missing: Some("priorityLabel".into()),
            ..fake()
        };
        run(&f, d.path()).unwrap();
        assert!(item(d.path(), "issue-1").is_some());
    }

    #[test]
    fn a_connection_change_mid_fetch_stops_writing() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        let n = std::sync::atomic::AtomicUsize::new(0);
        let still = || n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 1;
        let e = fetch_with(&f, d.path(), now(), None, &still, &mut |_, _| {}).unwrap_err();
        assert_eq!(e.kind(), Kind::Busy);
        assert!(item(d.path(), "issue-1").is_none());
    }

    #[test]
    fn weekly_reconcile_removes_missing_items() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        let many: Vec<Value> = (0..10)
            .map(|i| issue(&format!("r{i}"), "t1", &t(20 + i)))
            .collect();
        f.issues.lock().unwrap().extend(many);
        run(&f, d.path()).unwrap();
        f.issues.lock().unwrap().retain(|i| i["id"] != "r3");
        let later = now() + chrono::Duration::days(8);
        fetch_with(&f, d.path(), later, None, &|| true, &mut |_, _| {}).unwrap();
        assert!(item(d.path(), "issue-r3").is_none());
        assert!(item(d.path(), "issue-r4").is_some());
    }

    #[test]
    fn reconcile_skips_when_too_many_are_missing() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        let many: Vec<Value> = (0..10)
            .map(|i| issue(&format!("r{i}"), "t1", &t(20 + i)))
            .collect();
        f.issues.lock().unwrap().extend(many);
        run(&f, d.path()).unwrap();
        let before = state(d.path())["reconciled_at"].clone();
        f.issues.lock().unwrap().truncate(3);
        let later = now() + chrono::Duration::days(8);
        fetch_with(&f, d.path(), later, None, &|| true, &mut |_, _| {}).unwrap();
        assert!(item(d.path(), "issue-r9").is_some());
        assert_eq!(state(d.path())["reconciled_at"], before);
    }

    #[test]
    fn a_team_lost_while_a_fetch_was_cut_is_still_removed_later() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        run(&f, d.path()).unwrap();
        f.teams.lock().unwrap().retain(|t| t["id"] == "t1");
        *f.busy_after.lock().unwrap() = Some(1); // teams saved, then cut
        assert!(run(&f, d.path()).is_err());
        *f.busy_after.lock().unwrap() = None;
        run(&f, d.path()).unwrap();
        assert!(item(d.path(), "issue-2").is_none());
        assert!(item(d.path(), "issue-1").is_some());
    }

    #[test]
    fn the_first_save_names_the_workspace_and_a_different_one_stops_writing() {
        let d = tempfile::tempdir().unwrap();
        let f = fake();
        *f.busy_after.lock().unwrap() = Some(1);
        let _ = fetch_with(&f, d.path(), now(), Some("w1"), &|| true, &mut |_, _| {});
        assert_eq!(state(d.path())["workspace_id"], "w1");
        *f.busy_after.lock().unwrap() = None;
        let e = fetch_with(&f, d.path(), now(), Some("w2"), &|| true, &mut |_, _| {}).unwrap_err();
        assert_eq!(e.kind(), Kind::Busy);
        assert!(item(d.path(), "issue-1").is_none());
    }
}
