//! A fake Asana (`AsanaApi`): holds workspaces, projects, tasks, comments and users in memory. Fetch and reconcile
//! tests share it. It mimics only as much as needed; widen it when the next piece of work needs more.
//!
//! - Every call is recorded in `calls()` as `"GET <path>?<query>"` (the query in the order given; no `?` when there is
//!   none), and GETs inside `/batch` as `"BATCH <path>"`.
//! - Order: `fail_next(status)` (once only; 401 is a `Key` error, 429 `Busy`, 5xx `Down`, anything else a `Reply`) →
//!   routes.
//! - Clock: the time set by `set_now` is `Reply.date` and the default for tasks' `modified_at` and comments'
//!   `created_at` (`FakeAsana::new` starts at 2026-11-12T00:00:00Z).
//! - Routes: `users/me`, `users/<gid>`, `workspaces`, `projects?workspace=&archived=` (`archived` defaults to false),
//!   `projects/<gid>`, `tasks?project=&modified_since=&completed_since=&limit=&offset=` (top-level tasks and `homed`
//!   subtasks in the project), `tasks?assignee=me&workspace=`, `tasks/<gid>`, `tasks/<gid>/stories`,
//!   `tasks/<gid>/subtasks`.
//!   Lists are cut at `limit` (default 100) and give `next_page.offset` when more remain. Project and task lists of a
//!   blocked workspace (`block_workspace`) are 403, and deleted or missing tasks are 404 `Not a recognized ID`.
//!   Requests using an offset given before `expire_offsets()` are 400.
//!   `completed_since` returns only unfinished tasks and finished tasks changed after that time.
//!   A project made private (`make_private`) drops out of lists, and its project and task lists are 403.
//!   `tasks/<gid>`, `stories` and `subtasks` are also 403 for tasks whose projects are all private and for their
//!   subtasks (which follow the parent).

use std::collections::BTreeSet;
use std::sync::Mutex;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};

use crate::asana::api::{AsanaApi, Reply, batch_action};
use crate::error::{Kind, SourceError};

/// 가짜 글이에요. `Task::new("1", "제목").in_project("p1")`처럼 이어서 만들어요.
#[derive(Debug, Clone)]
pub struct Task {
    pub gid: String,
    pub name: String,
    pub notes: String,
    pub completed: bool,
    /// None이면 만들 때의 가짜 시계예요.
    pub modified_at: Option<DateTime<Utc>>,
    pub parent: Option<String>,
    pub projects: Vec<String>,
    pub workspace: Option<String>,
    pub assignee: Option<String>,
    /// None이면 딸린 글 수를 세요.
    pub num_subtasks: Option<u64>,
    /// 하위 task인데 프로젝트에도 넣어서 그 프로젝트 목록에 나와요.
    pub homed: bool,
}

impl Task {
    pub fn new(gid: &str, name: &str) -> Task {
        Task {
            gid: gid.into(),
            name: name.into(),
            notes: String::new(),
            completed: false,
            modified_at: None,
            parent: None,
            projects: Vec::new(),
            workspace: None,
            assignee: None,
            num_subtasks: None,
            homed: false,
        }
    }
    pub fn notes(mut self, n: &str) -> Task {
        self.notes = n.into();
        self
    }
    pub fn in_project(mut self, p: &str) -> Task {
        self.projects.push(p.into());
        self
    }
    pub fn completed(mut self) -> Task {
        self.completed = true;
        self
    }
    pub fn modified(mut self, t: DateTime<Utc>) -> Task {
        self.modified_at = Some(t);
        self
    }
    pub fn parent(mut self, p: &str) -> Task {
        self.parent = Some(p.into());
        self
    }
    pub fn workspace(mut self, w: &str) -> Task {
        self.workspace = Some(w.into());
        self
    }
    /// 하위 task를 `in_project`의 목록에도 나오게 해요(Asana에서 하위 task를 프로젝트에 넣은 것).
    pub fn homed(mut self) -> Task {
        self.homed = true;
        self
    }
    pub fn assignee_me(mut self) -> Task {
        self.assignee = Some("me".into());
        self
    }
}

#[derive(Debug, Clone)]
struct Project {
    gid: String,
    workspace: String,
    name: String,
    archived: bool,
}

#[derive(Debug, Clone)]
struct Story {
    gid: String,
    task: String,
    /// `comment` 또는 `system`이에요.
    kind: String,
    author: Option<String>,
    text: String,
    at: DateTime<Utc>,
}

struct State {
    now: DateTime<Utc>,
    me: String,
    workspaces: Vec<(String, String)>,
    users: Vec<Value>,
    projects: Vec<Project>,
    tasks: Vec<(Task, DateTime<Utc>)>,
    stories: Vec<Story>,
    deleted: BTreeSet<String>,
    blocked: BTreeSet<String>,
    private: BTreeSet<String>,
    fail: Vec<u16>,
    /// 지금 쪽 번호표의 세대예요. `expire_offsets`가 올려요.
    epoch: u64,
    /// 이 세대보다 오래된 번호표는 400이에요.
    min_gen: u64,
}

pub struct FakeAsana {
    s: Mutex<State>,
    calls: Mutex<Vec<String>>,
}

impl Default for FakeAsana {
    fn default() -> Self {
        Self::new()
    }
}

fn iso(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn ok(v: Value, now: DateTime<Utc>) -> Reply {
    Reply {
        status: 200,
        body: serde_json::to_vec(&v).unwrap_or_default(),
        date: Some(now),
    }
}

fn err(status: u16, message: &str, now: DateTime<Utc>) -> Reply {
    Reply {
        status,
        body: serde_json::to_vec(&json!({"errors": [{"message": message}]})).unwrap_or_default(),
        date: Some(now),
    }
}

impl FakeAsana {
    pub fn new() -> FakeAsana {
        FakeAsana {
            s: Mutex::new(State {
                now: DateTime::parse_from_rfc3339("2026-11-12T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                me: "me-1".into(),
                workspaces: Vec::new(),
                users: Vec::new(),
                projects: Vec::new(),
                tasks: Vec::new(),
                stories: Vec::new(),
                deleted: BTreeSet::new(),
                blocked: BTreeSet::new(),
                private: BTreeSet::new(),
                fail: Vec::new(),
                epoch: 0,
                min_gen: 0,
            }),
            calls: Mutex::default(),
        }
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.s.lock().unwrap()
    }

    pub fn set_now(&self, t: DateTime<Utc>) {
        self.st().now = t;
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.st().now
    }

    /// 가짜 시계를 `secs`초 앞으로 보내요.
    pub fn advance(&self, secs: i64) {
        let mut s = self.st();
        s.now += chrono::Duration::seconds(secs);
    }

    pub fn workspace(&self, gid: &str, name: &str) {
        self.st().workspaces.push((gid.into(), name.into()));
    }

    pub fn project(&self, gid: &str, workspace: &str, name: &str) {
        self.st().projects.push(Project {
            gid: gid.into(),
            workspace: workspace.into(),
            name: name.into(),
            archived: false,
        });
    }

    pub fn archive_project(&self, gid: &str, archived: bool) {
        for p in self.st().projects.iter_mut().filter(|p| p.gid == gid) {
            p.archived = archived;
        }
    }

    /// 프로젝트를 지워요(이후 `projects/<gid>`와 목록에서 사라져요).
    pub fn remove_project(&self, gid: &str) {
        self.st().projects.retain(|p| p.gid != gid);
    }

    pub fn user(&self, gid: &str, name: &str, email: &str) {
        self.st()
            .users
            .push(json!({"gid": gid, "name": name, "email": email, "resource_type": "user"}));
    }

    /// 글을 더해요. `modified_at`이 없으면 지금 가짜 시계예요.
    pub fn task(&self, t: Task) {
        let mut s = self.st();
        let at = t.modified_at.unwrap_or(s.now);
        s.tasks.retain(|(x, _)| x.gid != t.gid);
        s.tasks.push((t, at));
    }

    /// 글을 고치고 `modified_at`을 지금 가짜 시계(`modified_at`을 직접 바꿨으면 그 값)로 올려요.
    pub fn edit_task(&self, gid: &str, f: impl FnOnce(&mut Task)) {
        let mut s = self.st();
        let now = s.now;
        if let Some((t, at)) = s.tasks.iter_mut().find(|(t, _)| t.gid == gid) {
            let before = t.modified_at;
            f(t);
            *at = if t.modified_at != before {
                t.modified_at.unwrap_or(now)
            } else {
                now
            };
        }
    }

    /// 글의 `modified_at`만 지금으로 올려요(딸린 글·댓글이 바뀐 것처럼).
    pub fn touch(&self, gid: &str) {
        self.edit_task(gid, |_| {});
    }

    pub fn delete_task(&self, gid: &str) {
        let mut s = self.st();
        s.deleted.insert(gid.into());
        s.tasks.retain(|(t, _)| t.gid != gid);
    }

    pub fn remove_from_project(&self, task: &str, project: &str) {
        self.edit_task(task, |t| t.projects.retain(|p| p != project));
    }

    /// 댓글(`comment`)이나 시스템 글(`system`)을 더해요.
    pub fn story(&self, task: &str, gid: &str, kind: &str, author: Option<&str>, text: &str) {
        let mut s = self.st();
        let at = s.now;
        s.stories.push(Story {
            gid: gid.into(),
            task: task.into(),
            kind: kind.into(),
            author: author.map(str::to_string),
            text: text.into(),
            at,
        });
    }

    /// 이 워크스페이스의 프로젝트·글 목록은 403이에요.
    pub fn block_workspace(&self, gid: &str) {
        self.st().blocked.insert(gid.into());
    }

    /// 워크스페이스가 `users/me`에서 빠져요(나갔거나 내보내졌어요).
    pub fn remove_workspace(&self, gid: &str) {
        self.st().workspaces.retain(|(g, _)| g != gid);
    }

    /// 프로젝트를 나는 볼 수 없게 바꿔요(다른 사람이 비공개로 돌렸어요).
    pub fn make_private(&self, gid: &str) {
        self.st().private.insert(gid.into());
    }

    pub fn unblock_workspace(&self, gid: &str) {
        self.st().blocked.remove(gid);
    }

    /// 다음 호출 하나가 이 상태로 답해요(여러 번 부르면 차례로).
    pub fn fail_next(&self, status: u16) {
        self.st().fail.push(status);
    }

    /// 지금까지 준 `offset`이 모두 만료돼요(15분 지난 것처럼 400). 이 뒤에 새로 시작한 목록의 offset은 멀쩡해요.
    pub fn expire_offsets(&self) {
        let mut s = self.st();
        s.epoch += 1;
        s.min_gen = s.epoch;
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub fn clear_calls(&self) {
        self.calls.lock().unwrap().clear();
    }

    fn task_json(s: &State, t: &Task, at: DateTime<Utc>) -> Value {
        let subs = s
            .tasks
            .iter()
            .filter(|(x, _)| x.parent.as_deref() == Some(&t.gid))
            .count() as u64;
        let memberships: Vec<Value> = t
            .projects
            .iter()
            .map(|p| {
                let name = s
                    .projects
                    .iter()
                    .find(|x| &x.gid == p)
                    .map(|x| x.name.clone())
                    .unwrap_or_default();
                json!({"project": {"gid": p, "name": name, "resource_type": "project"}})
            })
            .collect();
        let ws = Self::workspace_of(s, t);
        json!({
            "gid": t.gid, "resource_type": "task", "name": t.name, "notes": t.notes,
            "completed": t.completed, "modified_at": iso(at), "created_at": iso(at),
            "parent": t.parent.as_ref().map(|p| {
                let name = s.tasks.iter().find(|(x, _)| &x.gid == p).map(|(x, _)| x.name.clone());
                json!({"gid": p, "name": name})
            }),
            "num_subtasks": t.num_subtasks.unwrap_or(subs),
            "memberships": memberships,
            "assignee": t.assignee.as_ref().map(|a| json!({"gid": if a == "me" { s.me.clone() } else { a.clone() }})),
            "workspace": ws.map(|w| json!({"gid": w})),
        })
    }

    /// 내가 볼 수 없는 글이에요: 프로젝트가 모두 비공개이거나, 하위 task인데 부모를 볼 수 없어요(Asana는 하위 task가
    /// 부모를 따라요).
    fn hidden(s: &State, t: &Task) -> bool {
        let mut cur = t;
        for _ in 0..16 {
            if !cur.projects.is_empty() && cur.projects.iter().all(|p| s.private.contains(p)) {
                return true;
            }
            match cur
                .parent
                .as_ref()
                .and_then(|p| s.tasks.iter().find(|(x, _)| &x.gid == p))
            {
                Some((parent, _)) => cur = parent,
                None => return false,
            }
        }
        false
    }

    /// 글의 워크스페이스예요: 직접 정했으면 그것, 아니면 첫 프로젝트의 것이에요.
    fn workspace_of(s: &State, t: &Task) -> Option<String> {
        t.workspace.clone().or_else(|| {
            t.projects
                .iter()
                .find_map(|p| s.projects.iter().find(|x| &x.gid == p))
                .map(|p| p.workspace.clone())
        })
    }

    /// 쪽 나누기: `offset`(시작 번호 글자)부터 `limit`개와 `next_page`예요.
    fn paged(s: &State, items: Vec<Value>, q: &[(String, String)], now: DateTime<Utc>) -> Reply {
        let limit = get(q, "limit")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(100)
            .max(1);
        let start = get(q, "offset")
            .and_then(|v| v.rsplit('-').next().and_then(|n| n.parse::<usize>().ok()))
            .unwrap_or(0);
        let end = (start + limit).min(items.len());
        let page = items.get(start..end).unwrap_or(&[]).to_vec();
        let next = if end < items.len() {
            json!({"offset": format!("off-{}-{end}", s.epoch), "path": "", "uri": ""})
        } else {
            Value::Null
        };
        ok(json!({"data": page, "next_page": next}), now)
    }

    fn route(&self, path: &str, q: &[(String, String)]) -> Result<Reply, SourceError> {
        let mut s = self.st();
        let now = s.now;
        if !s.fail.is_empty() {
            let code = s.fail.remove(0);
            return match code {
                401 => Err(SourceError::service(
                    Kind::Key,
                    "asana: the connection was refused",
                )),
                429 => Err(SourceError::service(Kind::Busy, "asana: rate limited")),
                c if c >= 500 => Err(SourceError::service(
                    Kind::Down,
                    "asana: the server kept failing",
                )),
                c => Ok(err(c, "forced", now)),
            };
        }
        if let Some(o) = get(q, "offset") {
            let epoch = o.split('-').nth(1).and_then(|g| g.parse::<u64>().ok());
            if epoch.is_none_or(|g| g < s.min_gen) {
                return Ok(err(400, "offset: Your offset token has expired", now));
            }
        }
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        let blocked = |s: &State, ws: Option<&str>| ws.is_some_and(|w| s.blocked.contains(w));
        match parts.as_slice() {
            ["users", "me"] => {
                let ws: Vec<Value> = s
                    .workspaces
                    .iter()
                    .map(|(g, n)| json!({"gid": g, "name": n, "resource_type": "workspace"}))
                    .collect();
                Ok(ok(
                    json!({"data": {"gid": s.me, "name": "Me", "email": "me@example.com", "workspaces": ws}}),
                    now,
                ))
            }
            ["users", gid] => Ok(match s.users.iter().find(|u| u["gid"] == *gid) {
                Some(u) => ok(json!({"data": u}), now),
                None => err(404, "Not a recognized ID", now),
            }),
            ["workspaces"] => {
                let ws: Vec<Value> = s
                    .workspaces
                    .iter()
                    .map(|(g, n)| json!({"gid": g, "name": n, "resource_type": "workspace"}))
                    .collect();
                Ok(Self::paged(&s, ws, q, now))
            }
            ["projects"] => {
                let ws = get(q, "workspace");
                if blocked(&s, ws) {
                    return Ok(err(403, "blocked", now));
                }
                let archived = get(q, "archived") == Some("true");
                let items: Vec<Value> = s
                    .projects
                    .iter()
                    .filter(|p| {
                        ws.is_none_or(|w| p.workspace == w)
                            && p.archived == archived
                            && !s.private.contains(&p.gid)
                    })
                    .map(|p| json!({"gid": p.gid, "name": p.name, "archived": p.archived, "resource_type": "project"}))
                    .collect();
                Ok(Self::paged(&s, items, q, now))
            }
            ["projects", gid] => Ok(match s.projects.iter().find(|p| p.gid == *gid) {
                Some(p) if !blocked(&s, Some(&p.workspace)) && !s.private.contains(&p.gid) => ok(
                    json!({"data": {"gid": p.gid, "name": p.name, "archived": p.archived,
                        "workspace": {"gid": p.workspace}}}),
                    now,
                ),
                Some(_) => err(403, "blocked", now),
                None => err(404, "Not a recognized ID", now),
            }),
            ["tasks"] => {
                let since = |k: &str| {
                    get(q, k).and_then(|v| {
                        if v == "now" {
                            Some(now)
                        } else {
                            DateTime::parse_from_rfc3339(v)
                                .ok()
                                .map(|d| d.with_timezone(&Utc))
                        }
                    })
                };
                let (modified, completed) = (since("modified_since"), since("completed_since"));
                let mut items = Vec::new();
                let rows: Vec<(Task, DateTime<Utc>)> = s.tasks.clone();
                if let Some(p) = get(q, "project") {
                    match s.projects.iter().find(|x| x.gid == p) {
                        Some(pr)
                            if blocked(&s, Some(&pr.workspace)) || s.private.contains(&pr.gid) =>
                        {
                            return Ok(err(403, "blocked", now));
                        }
                        None => return Ok(err(404, "Not a recognized ID", now)),
                        _ => {}
                    }
                    for (t, at) in &rows {
                        if (t.parent.is_none() || t.homed)
                            && t.projects.iter().any(|x| x == p)
                            && fresh(t, *at, modified, completed)
                        {
                            items.push(Self::task_json(&s, t, *at));
                        }
                    }
                } else if get(q, "assignee").is_some() {
                    let ws = get(q, "workspace");
                    if blocked(&s, ws) {
                        return Ok(err(403, "blocked", now));
                    }
                    for (t, at) in &rows {
                        if t.assignee.is_some()
                            && ws.is_none_or(|w| Self::workspace_of(&s, t).as_deref() == Some(w))
                            && fresh(t, *at, modified, completed)
                        {
                            items.push(Self::task_json(&s, t, *at));
                        }
                    }
                } else {
                    return Ok(err(400, "project or assignee required", now));
                }
                Ok(Self::paged(&s, items, q, now))
            }
            ["tasks", gid] => Ok(match s.tasks.iter().find(|(t, _)| t.gid == *gid) {
                Some((t, at)) => {
                    let ws = Self::workspace_of(&s, t);
                    if blocked(&s, ws.as_deref()) || Self::hidden(&s, t) {
                        err(403, "blocked", now)
                    } else {
                        ok(json!({"data": Self::task_json(&s, t, *at)}), now)
                    }
                }
                None => err(404, "Not a recognized ID", now),
            }),
            ["tasks", gid, "stories"] => {
                match s.tasks.iter().find(|(t, _)| t.gid == *gid) {
                    None => return Ok(err(404, "Not a recognized ID", now)),
                    Some((t, _)) if Self::hidden(&s, t) => return Ok(err(403, "private", now)),
                    _ => {}
                }
                let items: Vec<Value> = s
                    .stories
                    .iter()
                    .filter(|x| x.task == *gid)
                    .map(|x| {
                        let by = x.author.as_ref().map(|a| {
                            let name = s
                                .users
                                .iter()
                                .find(|u| u["gid"] == a.as_str())
                                .and_then(|u| u["name"].as_str())
                                .unwrap_or("");
                            json!({"gid": a, "name": name})
                        });
                        json!({"gid": x.gid, "type": x.kind, "resource_subtype": if x.kind == "comment" { "comment_added" } else { "assigned" },
                            "text": x.text, "created_at": iso(x.at), "created_by": by})
                    })
                    .collect();
                Ok(Self::paged(&s, items, q, now))
            }
            ["tasks", gid, "subtasks"] => {
                match s.tasks.iter().find(|(t, _)| t.gid == *gid) {
                    None => return Ok(err(404, "Not a recognized ID", now)),
                    Some((t, _)) if Self::hidden(&s, t) => return Ok(err(403, "private", now)),
                    _ => {}
                }
                let items: Vec<Value> = s
                    .tasks
                    .iter()
                    .filter(|(t, _)| t.parent.as_deref() == Some(gid))
                    .map(|(t, at)| Self::task_json(&s, t, *at))
                    .collect();
                Ok(Self::paged(&s, items, q, now))
            }
            _ => Ok(err(404, "Not a recognized path", now)),
        }
    }
}

/// `modified_since`와 `completed_since`를 통과하는 글인지예요.
fn fresh(
    t: &Task,
    at: DateTime<Utc>,
    modified: Option<DateTime<Utc>>,
    completed: Option<DateTime<Utc>>,
) -> bool {
    modified.is_none_or(|m| at >= m)
        && (completed.is_none() || !t.completed || completed.is_some_and(|c| at >= c))
}

fn get<'a>(q: &'a [(String, String)], k: &str) -> Option<&'a str> {
    q.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str())
}

/// `path?a=1&b=2`를 길과 쿼리로 나눠요.
fn split(p: &str) -> (String, Vec<(String, String)>) {
    match p.split_once('?') {
        Some((path, q)) => (
            path.to_string(),
            form_urlencoded::parse(q.as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect(),
        ),
        None => (p.to_string(), Vec::new()),
    }
}

impl AsanaApi for FakeAsana {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply, SourceError> {
        let q: Vec<(String, String)> = query
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        let mut line = format!("GET {}", path.trim_start_matches('/'));
        if !q.is_empty() {
            let j: Vec<String> = q.iter().map(|(k, v)| format!("{k}={v}")).collect();
            line.push('?');
            line.push_str(&j.join("&"));
        }
        self.calls.lock().unwrap().push(line);
        self.route(path, &q)
    }

    fn batch(&self, gets: &[String]) -> Result<Vec<Reply>, SourceError> {
        let mut out = Vec::new();
        for g in gets {
            self.calls
                .lock()
                .unwrap()
                .push(format!("BATCH {}", g.trim_start_matches('/')));
            batch_action(g)?;
            let (path, _) = split(g);
            out.push(self.route(&path, &[])?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake() -> FakeAsana {
        let f = FakeAsana::new();
        f.workspace("w1", "acme");
        f.project("p1", "w1", "로드맵");
        f.project("p2", "w1", "옛날");
        f.archive_project("p2", true);
        for i in 1..=5 {
            f.task(Task::new(&i.to_string(), "글").in_project("p1"));
        }
        f.task(Task::new("9", "하위").parent("1").in_project("p1"));
        f
    }

    fn data(r: Reply) -> Value {
        r.json("x").unwrap()
    }

    #[test]
    fn lists_projects_by_archived_flag() {
        let f = fake();
        let q = [("workspace", "w1".to_string())];
        assert_eq!(
            data(f.get("/projects", &q).unwrap())["data"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let q = [
            ("workspace", "w1".to_string()),
            ("archived", "true".to_string()),
        ];
        assert_eq!(data(f.get("projects", &q).unwrap())["data"][0]["gid"], "p2");
    }

    #[test]
    fn pages_tasks_with_offsets_and_top_level_only() {
        let f = fake();
        let q = |o: Option<&str>| {
            let mut v = vec![("project", "p1".to_string()), ("limit", "2".to_string())];
            if let Some(o) = o {
                v.push(("offset", o.to_string()));
            }
            v
        };
        let a = data(f.get("tasks", &q(None)).unwrap());
        assert_eq!(a["data"].as_array().unwrap().len(), 2);
        let off = a["next_page"]["offset"].as_str().unwrap().to_string();
        let b = data(f.get("tasks", &q(Some(&off))).unwrap());
        let c_off = b["next_page"]["offset"].as_str().unwrap().to_string();
        let c = data(f.get("tasks", &q(Some(&c_off))).unwrap());
        assert_eq!(c["data"].as_array().unwrap().len(), 1);
        assert!(c["next_page"].is_null());
        f.expire_offsets();
        assert_eq!(f.get("tasks", &q(Some(&off))).unwrap().status, 400);
        // 새로 시작한 목록은 둘째 쪽도 읽혀요.
        let a = data(f.get("tasks", &q(None)).unwrap());
        let off2 = a["next_page"]["offset"].as_str().unwrap().to_string();
        assert_eq!(f.get("tasks", &q(Some(&off2))).unwrap().status, 200);
    }

    #[test]
    fn filters_by_modified_and_completed_since() {
        let f = fake();
        let t0 = f.now();
        f.advance(3600);
        f.edit_task("2", |t| t.completed = true);
        let since = |k: &str, v: DateTime<Utc>| {
            data(
                f.get("tasks", &[("project", "p1".into()), (k, iso(v))])
                    .unwrap(),
            )["data"]
                .as_array()
                .unwrap()
                .len()
        };
        assert_eq!(
            since("modified_since", t0 + chrono::Duration::seconds(1)),
            1
        );
        assert_eq!(since("modified_since", t0), 5);
        // 끝난 글(2)은 completed_since가 그 뒤여야 보여요.
        assert_eq!(
            since("completed_since", f.now() + chrono::Duration::seconds(1)),
            4
        );
        assert_eq!(since("completed_since", t0), 5);
    }

    #[test]
    fn deleted_blocked_and_forced_failures() {
        let f = fake();
        f.delete_task("3");
        assert_eq!(f.get("tasks/3", &[]).unwrap().status, 404);
        f.block_workspace("w1");
        assert_eq!(
            f.get("projects", &[("workspace", "w1".into())])
                .unwrap()
                .status,
            403
        );
        f.fail_next(503);
        assert_eq!(f.get("users/me", &[]).unwrap_err().kind(), Kind::Down);
        f.fail_next(401);
        assert_eq!(f.get("users/me", &[]).unwrap_err().kind(), Kind::Key);
        f.fail_next(400);
        assert_eq!(f.get("users/me", &[]).unwrap().status, 400);
    }

    #[test]
    fn stories_subtasks_batch_and_calls() {
        let f = fake();
        f.user("u1", "지윤", "j@x.com");
        f.story("1", "s1", "comment", Some("u1"), "좋아요");
        f.story("1", "s2", "system", None, "assigned");
        let st = data(f.get("tasks/1/stories", &[]).unwrap());
        assert_eq!(st["data"].as_array().unwrap().len(), 2);
        assert_eq!(st["data"][0]["created_by"]["name"], "지윤");
        let sub = data(f.get("tasks/1/subtasks", &[]).unwrap());
        assert_eq!(sub["data"][0]["gid"], "9");
        assert_eq!(
            data(f.get("tasks/1", &[]).unwrap())["data"]["num_subtasks"],
            1
        );
        assert!(f.batch(&["/tasks?limit=5".into()]).is_err());
        let rs = f
            .batch(&["/tasks/2?opt_fields=name".into(), "/tasks/404".into()])
            .unwrap();
        assert_eq!((rs[0].status, rs[1].status), (200, 404));
        assert_eq!(f.calls().last().unwrap(), "BATCH tasks/404");
        assert!(f.get("tasks/2", &[]).unwrap().date.is_some());
        f.remove_from_project("2", "p1");
        assert!(
            data(f.get("tasks/2", &[]).unwrap())["data"]["memberships"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
