//! One fetch (docs/32 6절, 10-1절, 10-2절). A round goes in this order.
//!
//! 1. Workspaces from `GET /users/me` → `GET /projects?archived=false|true` for each workspace. Muted workspaces are
//!    not called. A 403 from `/projects` means an admin blocked it, so that workspace's raw data is deleted at once
//!    (`blocked`).
//! 2. Task lists: first projects that have a cursor (changes only), then projects not fetched yet (last 2 years).
//!    Completed tasks are fetched too (`completed_since=2년 전`). Each page is written to the raw data and the
//!    state is saved. The cursor moves to the first page's response `Date` − 15 minutes, and only after the whole list
//!    has been fetched. When the offset expires (400) that project is fetched again from the start (nothing deleted).
//! 3. My tasks outside projects, with `assignee=me` (same cursor rule, per workspace).
//! 4. Comments: fetches `stories` for changed tasks in descending `modified_at` order and keeps only `comment`.
//!    Saves every 50.
//! 5. Subtasks: for changed tasks with `num_subtasks > 0`, fetches `subtasks` (up to depth 5). Fetched subtasks go
//!    back to step 4.
//! 6. Unknown people in mentions (`https://app.asana.com/0/<사용자 gid>/`) are fetched once each with `GET /users/{gid}`.
//!
//! Each request spends one unit of the budget (`Budget`, 3,000 requests or 10 minutes). When it runs out, the state is
//! saved and the run ends with `more`, so the next run continues the same round (projects already fully fetched in
//! this round are not fetched again). Item content is never put in logs or errors.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::asana::api::AsanaApi;
use crate::asana::reconcile;
use crate::asana::state::{
    Run, Step, WINDOW_DAYS, Ws, iso, item_ids, load_state, parse, read_item, state_lock,
};
use crate::error::{Kind, SourceError};
use crate::gdrive::fetch::{Budget, safe};

/// 앱이 넘긴 새 토큰으로도 401인 요청의 실패 문구예요. 이번 동기화에서 Asana만 건너뛰어요.
pub(crate) const REFUSED_FRESH: &str = "asana: refused with a fresh token";

/// task를 받을 때의 칸이에요. granular scope 밖의 칸(맞춤 필드, 첨부)을 넣으면 요청 전체가 403이에요(10-1절 ⑭).
pub(crate) const TASK_FIELDS: &str = "name,notes,completed,completed_at,modified_at,assignee.name,due_on,permalink_url,num_subtasks,memberships.project.gid,memberships.project.name,memberships.section.name,parent.gid,parent.name";
const PROJECT_FIELDS: &str = "name,notes,modified_at,archived,team.name,permalink_url";
const STORY_FIELDS: &str = "type,text,created_by.gid,created_by.name,created_at";
/// 목록 한 쪽의 크기예요. `limit`이 없으면 `next_page`가 오지 않아요.
pub(crate) const LIMIT: &str = "100";
/// 커서를 Asana 시각보다 이만큼 앞에 둬요.
const OVERLAP_MINUTES: i64 = 15;
/// 하위 task를 내려가는 깊이예요.
const MAX_DEPTH: usize = 5;
/// 댓글·하위 task 단계에서 상태를 저장하는 간격(task 수)이에요.
const SAVE_EVERY: usize = 50;
/// 한 목록이 끝난 offset으로 다시 시작하는 횟수예요.
const MAX_RESTARTS: u32 = 3;

/// 동기화 한 번의 결과예요.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fetched {
    /// 몫이나 한도로 다 받지 못했어요(다음 동기화가 이어요).
    pub more: bool,
    /// 새 토큰으로도 거절돼서 이번엔 건너뛰었어요.
    pub skipped: bool,
}

fn refused_fresh(e: &SourceError) -> bool {
    matches!(e, SourceError::Service { kind: Kind::Down, message } if message == REFUSED_FRESH)
}

/// `AsanaHttp`가 429를 너무 많이 받아 멈춘 것인지예요(연결이 바뀐 `Busy`와 달라요).
fn rate_limited(e: &SourceError) -> bool {
    matches!(e, SourceError::Service { kind: Kind::Busy, message } if message == "asana: rate limited")
}

/// `raw`를 새로 받아요. 몫은 보통 3,000번·10분이고(`state::BUDGET_REQUESTS`, `BATCH_TIME`) 테스트는 작게 줘요.
pub(crate) fn fetch_budget(
    api: &dyn AsanaApi,
    raw: &Path,
    now: DateTime<Utc>,
    user_gid: &str,
    still: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(usize, usize),
    budget: Budget,
) -> Result<Fetched, SourceError> {
    let state = {
        let _held = state_lock();
        load_state(raw)
    };
    if state.user_gid.as_deref().is_some_and(|u| u != user_gid) {
        return Err(SourceError::service(
            Kind::Busy,
            "asana: the saved items are another user's",
        ));
    }
    let mut run = Run {
        api,
        dir: raw,
        user_gid,
        now,
        still,
        state,
        progress,
        budget,
    };
    // 이번 바퀴를 다 받았을 때만 재조정해요(때가 됐고 몫이 남았으면). 재조정은 몫이 다 차면 `Later`로 멈춰 이어요.
    let step = round(&mut run).and_then(|step| match step {
        Step::Done => reconcile::reconcile(&mut run),
        later => Ok(later),
    });
    match step {
        Ok(step) => {
            run.state.fetched_at = Some(iso(now));
            run.save()?;
            Ok(Fetched {
                more: step == Step::Later,
                skipped: false,
            })
        }
        Err(e) if refused_fresh(&e) => {
            let _ = run.save();
            Ok(Fetched {
                more: false,
                skipped: true,
            })
        }
        Err(e) if rate_limited(&e) => {
            run.save()?;
            Ok(Fetched {
                more: true,
                skipped: false,
            })
        }
        Err(e) => {
            // 받은 데까지는 남겨요(연결이 바뀌었으면 `save`가 쓰지 않아요).
            let _ = run.save();
            Err(e)
        }
    }
}

/// 한 바퀴의 한 단계예요.
type Phase = fn(&mut Run) -> Result<Step, SourceError>;

/// 한 바퀴를 이어 가요. 모듈 설명의 차례예요.
fn round(run: &mut Run) -> Result<Step, SourceError> {
    if run.state.round.is_none() {
        run.state.round = Some(iso(run.now));
        run.state.round_listed = false;
    }
    if !run.state.round_listed {
        if list_workspaces(run)? == Step::Later {
            return Ok(Step::Later);
        }
        run.state.round_listed = true;
        run.save()?;
    }
    let steps: [Phase; 4] = [list_projects, list_mine, comments_and_subtasks, users];
    for step in steps {
        if step(run)? == Step::Later {
            return Ok(Step::Later);
        }
    }
    run.state.round = None;
    run.state.round_listed = false;
    run.state.initial_done = true;
    Ok(Step::Done)
}

fn window(run: &Run) -> String {
    iso(run.now - chrono::Duration::days(WINDOW_DAYS))
}

/// 이 바퀴에서 이미 끝낸 목록인지예요(`listed_at`이 바퀴 시작 뒤예요).
fn done_this_round(run: &Run, listed_at: Option<&str>) -> bool {
    let round = run.state.round.as_deref().and_then(parse);
    match (round, listed_at.and_then(parse)) {
        (Some(r), Some(l)) => l >= r,
        _ => false,
    }
}

/// 받는 워크스페이스예요: 끄지 않았고 막히지 않았어요.
pub(crate) fn open_workspaces(run: &Run) -> Vec<String> {
    run.state.open_workspaces()
}

/// 목록 하나의 끝이에요.
pub(crate) enum End {
    /// 끝까지 받았어요: 첫 쪽 답의 `Date`예요.
    Done(Option<DateTime<Utc>>),
    /// 몫이 다 찼어요.
    Later,
    /// 2xx가 아닌 답이에요(403, 404 …).
    Status(u16),
}

/// 쪽을 끝까지 받아요. 쪽마다 `on_page(run, 글, 첫 쪽인지)`예요. 끝난 offset(400)이면 처음부터 다시 받아요
/// (`MAX_RESTARTS`번까지). 다시 받을 때 첫 쪽이 다시 `true`로 와요.
pub(crate) fn list_pages<'a>(
    run: &mut Run<'a>,
    path: &str,
    what: &str,
    base: &[(&str, String)],
    mut on_page: impl FnMut(&mut Run<'a>, &[Value], bool) -> Result<(), SourceError>,
) -> Result<End, SourceError> {
    let mut restarts = 0;
    let mut first_date: Option<Option<DateTime<Utc>>> = None;
    'outer: loop {
        let mut offset: Option<String> = None;
        loop {
            let mut q = base.to_vec();
            if let Some(o) = &offset {
                q.push(("offset", o.clone()));
            }
            let Some(r) = run.get(path, &q)? else {
                return Ok(End::Later);
            };
            if r.status == 400 && offset.is_some() {
                if restarts >= MAX_RESTARTS {
                    return Err(SourceError::service(
                        Kind::Down,
                        format!("asana: {what} offsets kept expiring"),
                    ));
                }
                restarts += 1;
                continue 'outer;
            }
            if !(200..300).contains(&r.status) {
                return Ok(End::Status(r.status));
            }
            let date = r.date;
            first_date.get_or_insert(date);
            let v = r.json(what)?;
            let items = v["data"].as_array().cloned().unwrap_or_default();
            on_page(run, &items, offset.is_none())?;
            match v["next_page"]["offset"].as_str() {
                Some(o) if !o.is_empty() => offset = Some(o.to_string()),
                _ => return Ok(End::Done(first_date.flatten())),
            }
        }
    }
}

/// 새 커서예요: Asana 시각 − 15분. `Date`가 없으면 None이라 부르는 쪽이 옛 커서를 둬요(Mac 시계는 쓰지 않아요).
fn cursor_of(date: Option<DateTime<Utc>>) -> Option<String> {
    date.map(|d| iso(d - chrono::Duration::minutes(OVERLAP_MINUTES)))
}

/// 워크스페이스 단위 목록이 403일 때 그 워크스페이스가 막혔는지 `/projects`로 다시 봐요. 막혔으면 바로 지워요.
/// 몫이 다 찼으면 None이에요.
fn blocked_now(run: &mut Run, ws: &str) -> Result<Option<bool>, SourceError> {
    let q = [
        ("workspace", ws.to_string()),
        ("limit", "1".to_string()),
        ("opt_fields", "name".to_string()),
    ];
    let Some(r) = run.get("projects", &q)? else {
        return Ok(None);
    };
    if r.status == 403 {
        wipe_workspace(run, ws)?;
        return Ok(Some(true));
    }
    Ok(Some(false))
}

/// 이 task를 받아도 되는지예요: 원본의 워크스페이스가 끄지 않았고 막히지 않았어요. 원본이 없으면 참이에요(부르는 쪽이
/// 그냥 목록에서 빼요).
fn open_item(run: &Run, open: &[String], gid: &str) -> bool {
    read_item(run.dir, "task", gid).is_none_or(|v| {
        v["workspace"]
            .as_str()
            .is_some_and(|w| open.iter().any(|o| o == w))
    })
}

/// 1. 워크스페이스와 프로젝트 목록이에요.
fn list_workspaces(run: &mut Run) -> Result<Step, SourceError> {
    let q = [("opt_fields", "workspaces.name".to_string())];
    let Some(r) = run.get("users/me", &q)? else {
        return Ok(Step::Later);
    };
    let me = r.json("users/me")?;
    let fresh: Vec<Ws> = me["data"]["workspaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let gid = w["gid"].as_str().filter(|g| safe(g))?;
            let old = run.state.workspaces.iter().find(|x| x.gid == gid);
            Some(Ws {
                gid: gid.to_string(),
                name: w["name"].as_str().unwrap_or("").to_string(),
                ..old.cloned().unwrap_or_default()
            })
        })
        .collect();
    run.state.workspaces = fresh;
    let all: Vec<String> = run.state.workspaces.iter().map(|w| w.gid.clone()).collect();
    for ws in all {
        if run.state.muted_workspaces.contains(&ws) {
            continue;
        }
        for archived in ["false", "true"] {
            let q = [
                ("workspace", ws.clone()),
                ("archived", archived.to_string()),
                ("limit", LIMIT.to_string()),
                ("opt_fields", PROJECT_FIELDS.to_string()),
            ];
            let end = list_pages(run, "projects", "projects", &q, |run, items, _| {
                for p in items {
                    put_project(run, p, &ws)?;
                }
                Ok(())
            })?;
            match end {
                End::Done(_) => set_blocked(run, &ws, false),
                End::Later => return Ok(Step::Later),
                End::Status(403) => {
                    wipe_workspace(run, &ws)?;
                    break;
                }
                End::Status(s) => {
                    return Err(SourceError::service(
                        Kind::Down,
                        format!("asana: projects failed {s}"),
                    ));
                }
            }
        }
    }
    Ok(Step::Done)
}

fn set_blocked(run: &mut Run, ws: &str, blocked: bool) {
    if let Some(w) = run.state.workspaces.iter_mut().find(|w| w.gid == ws) {
        w.blocked = blocked;
    }
}

fn put_project(run: &mut Run, p: &Value, ws: &str) -> Result<(), SourceError> {
    let Some(gid) = p["gid"].as_str().filter(|g| safe(g)) else {
        return Ok(());
    };
    let entry = run.state.projects.entry(gid.to_string()).or_default();
    entry.ws = ws.to_string();
    entry.name = p["name"].as_str().unwrap_or("").to_string();
    entry.archived = p["archived"].as_bool().unwrap_or(false);
    let mut v = p.clone();
    v["workspace"] = json!(ws);
    run.write("project", gid, &v)
}

/// 관리자가 막은 워크스페이스예요: 그 워크스페이스의 task·프로젝트 원본을 바로 지우고 받지 않아요.
pub(crate) fn wipe_workspace(run: &mut Run, ws: &str) -> Result<(), SourceError> {
    for gid in item_ids(run.dir, "task") {
        let mine =
            read_item(run.dir, "task", &gid).is_some_and(|v| v["workspace"].as_str() == Some(ws));
        if mine {
            run.remove("task", &gid)?;
            run.state.pending_comments.remove(&gid);
            run.state.pending_subtasks.remove(&gid);
        }
    }
    let projects: Vec<String> = run
        .state
        .projects
        .iter()
        .filter(|(_, p)| p.ws == ws)
        .map(|(g, _)| g.clone())
        .collect();
    for gid in projects {
        run.remove("project", &gid)?;
        run.state.projects.remove(&gid);
    }
    run.state.pending_users.retain(|_, w| w != ws);
    if let Some(w) = run.state.workspaces.iter_mut().find(|w| w.gid == ws) {
        w.blocked = true;
        w.me_cursor = None;
        w.me_listed_at = None;
    }
    run.save()
}

/// task 하나를 원본에 써요. 전에 받은 댓글은 새 댓글을 받을 때까지 둬요. 댓글(과 하위 task)을 받을 목록에 넣어요.
/// `parents`가 비었는데 하위 task예요(프로젝트에도 넣은 하위 task가 프로젝트 목록에 나왔어요): 저장된 부모 줄을 두고,
/// 없으면 바로 위 부모 하나로 둬요. `subtask`(하위 task 목록에서 왔어요)면 `modified_at`이 그대로일 때 댓글을 다시
/// 받지 않아요(부모가 바뀔 때마다 모든 하위 task의 댓글을 받지 않게요).
fn put_task(
    run: &mut Run,
    t: &Value,
    ws: &str,
    parents: &[Value],
    subtask: bool,
) -> Result<(), SourceError> {
    let Some(gid) = t["gid"].as_str().filter(|g| safe(g)) else {
        return Ok(());
    };
    if !t.is_object() {
        return Ok(());
    }
    let old = read_item(run.dir, "task", gid);
    let stories = old
        .as_ref()
        .map(|o| o["stories"].clone())
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([]));
    let mut chain = parents.to_vec();
    if chain.is_empty()
        && let Some(p) = t["parent"]["gid"].as_str()
    {
        chain = old
            .as_ref()
            .and_then(|o| o["parents"].as_array().cloned())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| vec![json!({"gid": p, "name": t["parent"]["name"]})]);
    }
    let same = old
        .as_ref()
        .is_some_and(|o| o["modified_at"].is_string() && o["modified_at"] == t["modified_at"]);
    let depth = chain.len();
    let mut v = t.clone();
    v["workspace"] = json!(ws);
    v["parents"] = Value::Array(chain);
    v["stories"] = stories;
    run.write("task", gid, &v)?;
    if !(subtask && same) {
        run.state.pending_comments.insert(gid.to_string());
    }
    if t["num_subtasks"].as_u64().unwrap_or(0) > 0 {
        if depth < MAX_DEPTH {
            run.state.pending_subtasks.insert(gid.to_string());
        } else {
            eprintln!("asana: subtasks deeper than {MAX_DEPTH} levels are skipped");
        }
    }
    note_mentions(run, t["notes"].as_str().unwrap_or(""), ws);
    Ok(())
}

/// 글의 멘션 중 이름을 모르는 사람을 받을 목록에 넣어요. 여러 워크스페이스에 나오면 열린 워크스페이스를 남겨요.
fn note_mentions(run: &mut Run, text: &str, ws: &str) {
    let open = open_workspaces(run);
    for (_, _, gid) in crate::asana::docs::mention_spans(text) {
        if run.state.users.contains_key(gid) {
            continue;
        }
        let keep = run
            .state
            .pending_users
            .get(gid)
            .is_some_and(|w| open.contains(w));
        if !keep {
            run.state
                .pending_users
                .insert(gid.to_string(), ws.to_string());
        }
    }
}

/// task 목록의 쿼리예요.
fn task_query<'q>(
    run: &Run,
    mut q: Vec<(&'q str, String)>,
    cursor: Option<&str>,
) -> Vec<(&'q str, String)> {
    let window = window(run);
    q.extend([
        (
            "modified_since",
            cursor.map_or(window.clone(), str::to_string),
        ),
        ("completed_since", window),
        ("limit", LIMIT.to_string()),
        ("opt_fields", TASK_FIELDS.to_string()),
    ]);
    q
}

/// 2. 프로젝트마다 task 목록이에요: 바뀐 것만 받는 프로젝트 먼저, 그다음 처음 받는 프로젝트예요.
fn list_projects(run: &mut Run) -> Result<Step, SourceError> {
    let open = open_workspaces(run);
    let mut order: Vec<(bool, String)> = run
        .state
        .projects
        .iter()
        .filter(|(_, p)| open.contains(&p.ws) && !done_this_round(run, p.listed_at.as_deref()))
        .map(|(g, p)| (p.cursor.is_none(), g.clone()))
        .collect();
    order.sort();
    for (_, gid) in order {
        if list_project(run, &gid)? == Step::Later {
            return Ok(Step::Later);
        }
    }
    Ok(Step::Done)
}

fn list_project(run: &mut Run, gid: &str) -> Result<Step, SourceError> {
    let Some(p) = run.state.projects.get(gid).cloned() else {
        return Ok(Step::Done);
    };
    let q = task_query(run, vec![("project", gid.to_string())], p.cursor.as_deref());
    let end = list_pages(run, "tasks", "tasks", &q, |run, items, _| {
        for t in items {
            put_task(run, t, &p.ws, &[], false)?;
            let at = t["modified_at"].as_str().map(str::to_string);
            if let Some(entry) = run.state.projects.get_mut(gid)
                && at.is_some()
                && at > entry.changed_at
            {
                entry.changed_at = at;
            }
        }
        run.save()
    })?;
    match end {
        End::Later => Ok(Step::Later),
        End::Done(date) => {
            let cursor = cursor_of(date);
            let now = iso(run.now);
            if let Some(entry) = run.state.projects.get_mut(gid) {
                if cursor.is_some() {
                    entry.cursor = cursor;
                }
                entry.listed_at = Some(now);
            }
            run.save()?;
            Ok(Step::Done)
        }
        // 워크스페이스가 막혔으면 바로 지워요. 아니면 지웠거나 볼 수 없게 된 프로젝트라 지우는 건 재조정이 해요.
        End::Status(403) => match blocked_now(run, &p.ws)? {
            None => Ok(Step::Later),
            Some(_) => Ok(Step::Done),
        },
        End::Status(_) => Ok(Step::Done),
    }
}

/// 3. 워크스페이스마다 `assignee=me`예요. 하위 task는 부모를 따라 받으니 빼요.
fn list_mine(run: &mut Run) -> Result<Step, SourceError> {
    for ws in open_workspaces(run) {
        let Some(w) = run.state.workspaces.iter().find(|w| w.gid == ws).cloned() else {
            continue;
        };
        if done_this_round(run, w.me_listed_at.as_deref()) {
            continue;
        }
        let q = task_query(
            run,
            vec![("assignee", "me".to_string()), ("workspace", ws.clone())],
            w.me_cursor.as_deref(),
        );
        let end = list_pages(run, "tasks", "tasks", &q, |run, items, _| {
            for t in items.iter().filter(|t| t["parent"]["gid"].is_null()) {
                put_task(run, t, &ws, &[], false)?;
            }
            run.save()
        })?;
        match end {
            End::Later => return Ok(Step::Later),
            End::Done(date) => {
                let cursor = cursor_of(date);
                let now = iso(run.now);
                if let Some(w) = run.state.workspaces.iter_mut().find(|w| w.gid == ws) {
                    if cursor.is_some() {
                        w.me_cursor = cursor;
                    }
                    w.me_listed_at = Some(now);
                }
                run.save()?;
            }
            End::Status(403) => {
                if blocked_now(run, &ws)?.is_none() {
                    return Ok(Step::Later);
                }
            }
            End::Status(_) => {}
        }
    }
    Ok(Step::Done)
}

/// 4·5. 댓글과 하위 task예요. 하위 task가 새로 들어오면 그 댓글을 다시 받아요.
fn comments_and_subtasks(run: &mut Run) -> Result<Step, SourceError> {
    loop {
        if comments(run)? == Step::Later {
            return Ok(Step::Later);
        }
        match subtasks(run)? {
            Some(Step::Later) => return Ok(Step::Later),
            // 받은 하위 task가 없으면 끝이에요(끈 워크스페이스의 것은 남겨 둬요).
            None => return Ok(Step::Done),
            Some(Step::Done) => {}
        }
    }
}

/// 댓글 단계예요. 끈(또는 막힌) 워크스페이스의 task는 부르지 않고 목록에 남겨 둬요(다시 켜면 받아요).
pub(crate) fn comments(run: &mut Run) -> Result<Step, SourceError> {
    run.refresh_mutes();
    let mut order: Vec<(String, String)> = run
        .state
        .pending_comments
        .iter()
        .map(|g| {
            let at = read_item(run.dir, "task", g)
                .and_then(|v| v["modified_at"].as_str().map(str::to_string))
                .unwrap_or_default();
            (at, g.clone())
        })
        .collect();
    order.sort_by(|a, b| b.cmp(a));
    let total = order.len();
    for (i, (_, gid)) in order.into_iter().enumerate() {
        if i > 0 && i % SAVE_EVERY == 0 {
            run.refresh_mutes();
        }
        if !open_item(run, &open_workspaces(run), &gid) {
            continue;
        }
        if let Some(mut item) = read_item(run.dir, "task", &gid) {
            let ws = item["workspace"].as_str().unwrap_or("").to_string();
            let q = [
                ("limit", LIMIT.to_string()),
                ("opt_fields", STORY_FIELDS.to_string()),
            ];
            let mut kept: Vec<Value> = Vec::new();
            let path = format!("tasks/{gid}/stories");
            let end = list_pages(run, &path, "stories", &q, |run, items, first| {
                if first {
                    kept.clear();
                }
                for s in items.iter().filter(|s| s["type"] == "comment") {
                    let by = &s["created_by"];
                    if let (Some(g), Some(n)) = (by["gid"].as_str(), by["name"].as_str())
                        && !n.is_empty()
                    {
                        run.state.users.insert(g.to_string(), n.to_string());
                        run.state.pending_users.remove(g);
                    }
                    note_mentions(run, s["text"].as_str().unwrap_or(""), &ws);
                    kept.push(json!({
                        "gid": s["gid"], "text": s["text"], "created_at": s["created_at"],
                        "created_by": s["created_by"],
                    }));
                }
                Ok(())
            })?;
            match end {
                End::Later => {
                    run.save()?;
                    return Ok(Step::Later);
                }
                End::Done(_) => {
                    item["stories"] = Value::Array(kept);
                    run.write("task", &gid, &item)?;
                }
                // 지웠거나 볼 수 없게 된 task예요. 지우는 건 재조정이 해요.
                End::Status(_) => {}
            }
        }
        run.state.pending_comments.remove(&gid);
        (run.progress)(i + 1, total);
        if (i + 1) % SAVE_EVERY == 0 {
            run.save()?;
        }
    }
    run.save()?;
    Ok(Step::Done)
}

/// 하위 task 단계예요. 하나도 받지 않았으면(남은 것이 모두 끈 워크스페이스의 것이면) None이에요.
fn subtasks(run: &mut Run) -> Result<Option<Step>, SourceError> {
    run.refresh_mutes();
    let mut n = 0;
    let mut held = std::collections::BTreeSet::new();
    loop {
        let open = open_workspaces(run);
        let next = run
            .state
            .pending_subtasks
            .iter()
            .find(|g| !held.contains(*g))
            .cloned();
        let Some(gid) = next else { break };
        if !open_item(run, &open, &gid) {
            held.insert(gid);
            continue;
        }
        if let Some(parent) = read_item(run.dir, "task", &gid) {
            let mut chain = parent["parents"].as_array().cloned().unwrap_or_default();
            chain.push(json!({"gid": gid, "name": parent["name"]}));
            let ws = parent["workspace"].as_str().unwrap_or("").to_string();
            let q = [
                ("limit", LIMIT.to_string()),
                ("opt_fields", TASK_FIELDS.to_string()),
            ];
            let path = format!("tasks/{gid}/subtasks");
            let end = list_pages(run, &path, "subtasks", &q, |run, items, _| {
                for t in items {
                    put_task(run, t, &ws, &chain, true)?;
                }
                Ok(())
            })?;
            if matches!(end, End::Later) {
                run.save()?;
                return Ok(Some(Step::Later));
            }
        }
        run.state.pending_subtasks.remove(&gid);
        n += 1;
        if n % SAVE_EVERY == 0 {
            run.save()?;
        }
    }
    run.save()?;
    Ok((n > 0).then_some(Step::Done))
}

/// 6단계, 멘션에 나온 모르는 사람의 이름이에요. 못 받은 사람(404·403)은 빈 이름으로 둬서 다시 묻지 않아요.
/// 끈(또는 막힌) 워크스페이스에서 나온 사람은 묻지 않고 남겨 둬요.
fn users(run: &mut Run) -> Result<Step, SourceError> {
    run.refresh_mutes();
    let open = open_workspaces(run);
    let todo: Vec<String> = run
        .state
        .pending_users
        .iter()
        .filter(|(_, w)| open.contains(w))
        .map(|(g, _)| g.clone())
        .collect();
    for gid in todo {
        if !run.state.users.contains_key(&gid) {
            let q = [("opt_fields", "name".to_string())];
            let Some(r) = run.get(&format!("users/{gid}"), &q)? else {
                run.save()?;
                return Ok(Step::Later);
            };
            let name = if (200..300).contains(&r.status) {
                r.json("users")?["data"]["name"]
                    .as_str()
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            };
            run.state.users.insert(gid.clone(), name);
        }
        run.state.pending_users.remove(&gid);
    }
    run.save()?;
    Ok(Step::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asana::api::Reply;
    use crate::asana::state::{State, iso, item_ids, read_item, save_state, write_item};
    use crate::gdrive::fetch::BATCH_TIME;
    use crate::testing::asana::{FakeAsana, Task};
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicBool, Ordering};

    fn run(f: &dyn AsanaApi, dir: &Path, now: DateTime<Utc>, n: usize) -> Fetched {
        fetch_budget(
            f,
            dir,
            now,
            "me-1",
            &|| true,
            &mut |_, _| {},
            Budget::new(n, BATCH_TIME),
        )
        .unwrap()
    }

    fn full(f: &FakeAsana, dir: &Path) -> Fetched {
        run(f, dir, f.now(), 10_000)
    }

    fn item(dir: &Path, gid: &str) -> Value {
        read_item(dir, "task", gid).unwrap_or_else(|| panic!("task {gid} is not stored"))
    }

    fn state(dir: &Path) -> State {
        load_state(dir)
    }

    fn ago(f: &FakeAsana, secs: i64) -> DateTime<Utc> {
        f.now() - chrono::Duration::seconds(secs)
    }

    fn base() -> FakeAsana {
        let f = FakeAsana::new();
        f.workspace("w1", "acme");
        f.project("p1", "w1", "로드맵");
        f
    }

    fn calls_with(f: &FakeAsana, part: &str) -> Vec<String> {
        f.calls().into_iter().filter(|c| c.contains(part)).collect()
    }

    fn texts(v: &Value) -> Vec<String> {
        v["stories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["text"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn first_sync_lists_tasks_then_comments_newest_first() {
        let f = base();
        f.project("p2", "w1", "결제");
        f.task(
            Task::new("1", "가")
                .in_project("p1")
                .modified(ago(&f, 3 * 86_400)),
        );
        f.task(
            Task::new("2", "나")
                .in_project("p1")
                .modified(ago(&f, 86_400)),
        );
        f.task(
            Task::new("3", "다")
                .in_project("p1")
                .modified(ago(&f, 2 * 86_400)),
        );
        f.task(
            Task::new("4", "라")
                .in_project("p2")
                .modified(ago(&f, 5 * 3600)),
        );
        for g in ["1", "2", "3", "4"] {
            f.story(g, &format!("s{g}"), "comment", None, &format!("댓글 {g}"));
        }
        let dir = d();
        assert_eq!(full(&f, dir.path()), Fetched::default());
        let calls = f.calls();
        let last_list = calls
            .iter()
            .rposition(|c| c.starts_with("GET tasks?project="))
            .unwrap();
        let first_story = calls.iter().position(|c| c.contains("/stories")).unwrap();
        assert!(last_list < first_story, "{calls:#?}");
        let order: Vec<&str> = calls
            .iter()
            .filter(|c| c.contains("/stories"))
            .map(|c| c.split('/').nth(1).unwrap())
            .collect();
        assert_eq!(order, ["4", "2", "3", "1"]);
        assert_eq!(texts(&item(dir.path(), "2")), ["댓글 2"]);
        let q = &calls_with(&f, "tasks?project=p1")[0];
        assert!(q.contains("limit=100"), "{q}");
        assert!(q.contains("opt_fields=name,notes,completed"), "{q}");
        assert!(state(dir.path()).initial_done);
    }

    fn d() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn budget_stops_and_the_next_run_resumes_without_relisting() {
        let f = base();
        f.project("p2", "w1", "둘");
        f.project("p3", "w1", "셋");
        for (g, p) in [("1", "p1"), ("2", "p2"), ("3", "p3")] {
            f.task(Task::new(g, "글").in_project(p));
        }
        let dir = d();
        // users/me, projects ×2, tasks p1, tasks p2 → 몫이 다 찼어요.
        let out = run(&f, dir.path(), f.now(), 5);
        assert!(out.more);
        assert!(!state(dir.path()).initial_done);
        assert!(read_item(dir.path(), "task", "2").is_some());
        assert!(read_item(dir.path(), "task", "3").is_none());
        f.clear_calls();
        let out = full(&f, dir.path());
        assert!(!out.more);
        let calls = f.calls();
        assert!(
            calls.iter().all(|c| !c.starts_with("GET projects")),
            "{calls:#?}"
        );
        assert!(
            calls.iter().all(|c| !c.starts_with("GET users/me")),
            "{calls:#?}"
        );
        assert!(calls_with(&f, "project=p1").is_empty(), "{calls:#?}");
        assert!(calls_with(&f, "project=p2").is_empty(), "{calls:#?}");
        assert_eq!(calls_with(&f, "project=p3").len(), 1);
        assert_eq!(calls_with(&f, "/stories").len(), 3);
        assert_eq!(item_ids(dir.path(), "task"), ["1", "2", "3"]);
        assert!(state(dir.path()).initial_done);
    }

    #[test]
    fn completed_tasks_and_archived_projects_are_kept() {
        let f = base();
        f.project("p2", "w1", "옛날");
        f.archive_project("p2", true);
        f.task(Task::new("1", "끝냄").in_project("p1").completed());
        f.task(Task::new("2", "보관").in_project("p2"));
        let dir = d();
        full(&f, dir.path());
        assert_eq!(item(dir.path(), "1")["completed"], true);
        assert_eq!(item(dir.path(), "2")["name"], "보관");
        let s = state(dir.path());
        assert!(s.projects["p2"].archived);
        assert!(!s.projects["p1"].archived);
        assert_eq!(item_ids(dir.path(), "project"), ["p1", "p2"]);
        assert!(calls_with(&f, "tasks?project=p1")[0].contains("completed_since="));
    }

    #[test]
    fn system_stories_are_dropped() {
        let f = base();
        f.user("u1", "지윤", "j@x.com");
        f.task(Task::new("1", "글").in_project("p1"));
        f.story("1", "s1", "system", None, "Tacie assigned to you");
        f.story("1", "s2", "comment", Some("u1"), "좋아요");
        let dir = d();
        full(&f, dir.path());
        let v = item(dir.path(), "1");
        assert_eq!(texts(&v), ["좋아요"]);
        assert_eq!(v["stories"][0]["created_by"]["name"], "지윤");
    }

    #[test]
    fn multi_homed_task_is_stored_once() {
        let f = base();
        f.project("p2", "w1", "둘");
        f.task(Task::new("1", "둘 다").in_project("p1").in_project("p2"));
        let dir = d();
        full(&f, dir.path());
        assert_eq!(item_ids(dir.path(), "task"), ["1"]);
        assert_eq!(calls_with(&f, "tasks/1/stories").len(), 1);
        assert_eq!(
            item(dir.path(), "1")["memberships"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn incremental_uses_the_date_header_minus_15_minutes() {
        let f = base();
        f.task(
            Task::new("1", "옛 글")
                .in_project("p1")
                .modified(ago(&f, 86_400)),
        );
        f.task(
            Task::new("2", "고칠 글")
                .in_project("p1")
                .modified(ago(&f, 86_400)),
        );
        let dir = d();
        let asana_now = f.now();
        // Mac 시계가 한 시간 빨라도 커서는 Asana 시각이에요.
        let mac = asana_now + chrono::Duration::hours(1);
        run(&f, dir.path(), mac, 10_000);
        let cursor = iso(asana_now - chrono::Duration::minutes(15));
        assert_eq!(
            state(dir.path()).projects["p1"].cursor.as_deref(),
            Some(cursor.as_str())
        );
        f.advance(3600);
        f.edit_task("2", |t| t.notes = "고침".into());
        f.clear_calls();
        run(&f, dir.path(), mac + chrono::Duration::hours(1), 10_000);
        let listed = calls_with(&f, "tasks?project=p1");
        assert_eq!(listed.len(), 1);
        assert!(
            listed[0].contains(&format!("modified_since={cursor}")),
            "{listed:?}"
        );
        assert_eq!(
            calls_with(&f, "/stories"),
            [
                "GET tasks/2/stories?limit=100&opt_fields=type,text,created_by.gid,created_by.name,created_at"
            ]
        );
        assert_eq!(item(dir.path(), "2")["notes"], "고침");
        let next = iso(f.now() - chrono::Duration::minutes(15));
        assert_eq!(
            state(dir.path()).projects["p1"].cursor.as_deref(),
            Some(next.as_str())
        );
    }

    /// 첫 쪽을 준 뒤 offset을 모두 끝내는 가짜예요(15분이 지난 것처럼).
    struct Expiring<'a> {
        f: &'a FakeAsana,
        done: AtomicBool,
    }

    impl AsanaApi for Expiring<'_> {
        fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply, SourceError> {
            let r = self.f.get(path, query)?;
            let first_page = path == "tasks" && !query.iter().any(|(k, _)| *k == "offset");
            if first_page && !self.done.swap(true, Ordering::SeqCst) {
                self.f.expire_offsets();
            }
            Ok(r)
        }
        fn batch(&self, gets: &[String]) -> Result<Vec<Reply>, SourceError> {
            self.f.batch(gets)
        }
    }

    #[test]
    fn expired_offset_restarts_that_project() {
        let f = base();
        for i in 0..250 {
            f.task(Task::new(&format!("t{i}"), "글").in_project("p1"));
        }
        let dir = d();
        write_item(
            dir.path(),
            "task",
            "kept",
            &json!({"gid": "kept", "workspace": "w1"}),
        )
        .unwrap();
        let api = Expiring {
            f: &f,
            done: AtomicBool::new(false),
        };
        let out = run(&api, dir.path(), f.now(), 10_000);
        assert!(!out.more);
        let firsts: Vec<String> = calls_with(&f, "tasks?project=p1")
            .into_iter()
            .filter(|c| !c.contains("offset="))
            .collect();
        assert_eq!(firsts.len(), 2, "the listing restarts once");
        assert_eq!(item_ids(dir.path(), "task").len(), 251);
        assert!(state(dir.path()).projects["p1"].cursor.is_some());
    }

    #[test]
    fn subtasks_are_fetched_recursively_to_depth_5_with_their_comments() {
        let f = base();
        f.task(Task::new("1", "부모").in_project("p1"));
        let mut parent = "1".to_string();
        for i in 1..=6 {
            let g = format!("s{i}");
            f.task(Task::new(&g, &format!("하위 {i}")).parent(&parent));
            parent = g;
        }
        f.story("s3", "c3", "comment", None, "하위 댓글");
        let dir = d();
        full(&f, dir.path());
        for i in 1..=5 {
            let v = item(dir.path(), &format!("s{i}"));
            assert_eq!(v["parents"].as_array().unwrap().len(), i);
            assert_eq!(v["workspace"], "w1");
        }
        let s3 = item(dir.path(), "s3");
        assert_eq!(s3["parents"][0], json!({"gid": "1", "name": "부모"}));
        assert_eq!(s3["parents"][2]["gid"], "s2");
        assert_eq!(texts(&s3), ["하위 댓글"]);
        assert!(read_item(dir.path(), "task", "s6").is_none());
        assert!(calls_with(&f, "tasks/s5/subtasks").is_empty());
        assert_eq!(calls_with(&f, "tasks/s4/subtasks").len(), 1);
        assert!(state(dir.path()).pending_subtasks.is_empty());
    }

    #[test]
    fn a_new_subtask_arrives_when_the_parent_changes() {
        let f = base();
        f.task(Task::new("1", "부모").in_project("p1"));
        let dir = d();
        full(&f, dir.path());
        f.advance(3600);
        f.task(Task::new("n1", "새 하위").parent("1"));
        f.touch("1");
        full(&f, dir.path());
        assert_eq!(item(dir.path(), "n1")["parents"][0]["gid"], "1");
    }

    #[test]
    fn muted_workspace_is_never_fetched() {
        let f = base();
        f.workspace("w2", "남의 회사");
        f.project("q1", "w2", "비밀");
        f.task(Task::new("1", "우리").in_project("p1"));
        f.task(Task::new("2", "남의").in_project("q1"));
        f.task(Task::new("3", "남의 내 일").workspace("w2").assignee_me());
        let dir = d();
        save_state(
            dir.path(),
            &State {
                muted_workspaces: vec!["w2".into()],
                ..State::default()
            },
        )
        .unwrap();
        full(&f, dir.path());
        let calls = f.calls();
        assert!(
            calls.iter().all(|c| !c.contains("w2") && !c.contains("q1")),
            "{calls:#?}"
        );
        assert_eq!(item_ids(dir.path(), "task"), ["1"]);
        assert_eq!(state(dir.path()).muted_workspaces, ["w2"]);
    }

    #[test]
    fn blocked_workspace_is_wiped_now() {
        let f = base();
        f.workspace("w2", "다른");
        f.project("q1", "w2", "남는");
        f.task(Task::new("1", "글").in_project("p1"));
        f.task(Task::new("s1", "하위").parent("1"));
        f.task(Task::new("2", "남는 글").in_project("q1"));
        let dir = d();
        full(&f, dir.path());
        assert_eq!(item_ids(dir.path(), "task"), ["1", "2", "s1"]);
        f.block_workspace("w1");
        f.advance(3600);
        full(&f, dir.path());
        assert_eq!(item_ids(dir.path(), "task"), ["2"]);
        assert_eq!(item_ids(dir.path(), "project"), ["q1"]);
        let s = state(dir.path());
        assert!(s.workspaces.iter().any(|w| w.gid == "w1" && w.blocked));
        assert!(!s.projects.contains_key("p1"));
        assert!(s.pending_comments.is_empty());
    }

    #[test]
    fn assignee_me_tasks_outside_projects_are_stored() {
        let f = base();
        f.task(Task::new("m1", "내 일").workspace("w1").assignee_me());
        let dir = d();
        full(&f, dir.path());
        let v = item(dir.path(), "m1");
        assert_eq!(v["workspace"], "w1");
        assert_eq!(v["parents"], json!([]));
        let mine = calls_with(&f, "tasks?assignee=me&workspace=w1");
        assert_eq!(mine.len(), 1);
        assert!(mine[0].contains("limit=100"));
        assert!(state(dir.path()).workspaces[0].me_cursor.is_some());
    }

    #[test]
    fn mention_gids_become_names_once() {
        let f = base();
        f.user("1209", "민호", "m@x.com");
        f.task(
            Task::new("1", "글").in_project("p1").notes(
                "https://app.asana.com/0/1209/ 확인 부탁해요. https://app.asana.com/0/555/777",
            ),
        );
        f.task(Task::new("2", "글").in_project("p1"));
        f.story(
            "2",
            "c1",
            "comment",
            None,
            "https://app.asana.com/0/1209/ 보셨어요?",
        );
        let dir = d();
        full(&f, dir.path());
        assert_eq!(calls_with(&f, "GET users/1209").len(), 1);
        assert!(calls_with(&f, "users/555").is_empty());
        assert_eq!(
            state(dir.path()).users.get("1209").map(String::as_str),
            Some("민호")
        );
        f.advance(3600);
        f.touch("1");
        f.clear_calls();
        full(&f, dir.path());
        assert!(calls_with(&f, "GET users/1209").is_empty());
        assert!(state(dir.path()).pending_users.is_empty());
    }

    fn mute(dir: &Path, ws: &str) {
        let mut s = load_state(dir);
        s.muted_workspaces.push(ws.into());
        save_state(dir, &s).unwrap();
    }

    #[test]
    fn workspace_muted_mid_round_gets_no_comment_subtask_or_user_calls() {
        let f = base();
        f.workspace("w2", "남의 회사");
        f.project("q1", "w2", "비밀");
        f.user("1209", "민호", "m@x.com");
        f.task(Task::new("1", "우리").in_project("p1"));
        f.task(
            Task::new("2", "남의")
                .in_project("q1")
                .notes("https://app.asana.com/0/1209/ 봐 주세요"),
        );
        f.task(Task::new("s2", "남의 하위").parent("2"));
        let dir = d();
        // users/me, projects ×4, tasks p1, tasks q1, me ×2 → 몫이 다 찼어요(댓글 전).
        assert!(run(&f, dir.path(), f.now(), 9).more);
        assert!(calls_with(&f, "/stories").is_empty());
        mute(dir.path(), "w2");
        f.clear_calls();
        let out = full(&f, dir.path());
        assert!(!out.more);
        let calls = f.calls();
        assert!(
            calls.iter().all(|c| !c.contains("tasks/2/")
                && !c.contains("tasks/s2")
                && !c.contains("users/")),
            "{calls:#?}"
        );
        assert_eq!(calls_with(&f, "tasks/1/stories").len(), 1);
        let s = state(dir.path());
        // 다시 켜면 받을 수 있게 남겨 둬요.
        assert!(s.pending_comments.contains("2"));
        assert!(s.pending_users.contains_key("1209"));
    }

    /// 다음 바퀴를 몫 3(users/me, projects ×2)으로 시작해 목록만 받고 멈춰요.
    fn list_only(f: &FakeAsana, dir: &Path) {
        f.advance(3600);
        assert!(run(f, dir, f.now(), 3).more);
    }

    #[test]
    fn a_403_on_a_project_listing_rechecks_and_wipes_the_workspace() {
        let f = base();
        f.workspace("w2", "다른");
        f.project("q1", "w2", "남는");
        f.task(Task::new("1", "글").in_project("p1"));
        f.task(Task::new("2", "남는 글").in_project("q1"));
        let dir = d();
        full(&f, dir.path());
        f.advance(3600);
        assert!(run(&f, dir.path(), f.now(), 5).more);
        f.block_workspace("w1");
        f.clear_calls();
        full(&f, dir.path());
        assert_eq!(calls_with(&f, "projects?workspace=w1&limit=1").len(), 1);
        assert!(calls_with(&f, "assignee=me&workspace=w1").is_empty());
        assert_eq!(item_ids(dir.path(), "task"), ["2"]);
        assert_eq!(item_ids(dir.path(), "project"), ["q1"]);
        let s = state(dir.path());
        assert!(s.workspaces.iter().any(|w| w.gid == "w1" && w.blocked));
        assert!(!s.workspaces.iter().any(|w| w.gid == "w2" && w.blocked));
    }

    #[test]
    fn a_403_on_assignee_me_rechecks_and_wipes_the_workspace() {
        let f = FakeAsana::new();
        f.workspace("w1", "acme");
        f.task(Task::new("m1", "내 일").workspace("w1").assignee_me());
        let dir = d();
        full(&f, dir.path());
        assert_eq!(item_ids(dir.path(), "task"), ["m1"]);
        list_only(&f, dir.path());
        f.block_workspace("w1");
        f.clear_calls();
        full(&f, dir.path());
        assert_eq!(calls_with(&f, "projects?workspace=w1&limit=1").len(), 1);
        assert!(item_ids(dir.path(), "task").is_empty());
        assert!(state(dir.path()).workspaces[0].blocked);
    }

    #[test]
    fn budget_inside_a_multi_page_listing_keeps_the_old_cursor() {
        let f = base();
        for i in 0..150 {
            f.task(
                Task::new(&format!("t{i}"), "글")
                    .in_project("p1")
                    .modified(ago(&f, 86_400)),
            );
        }
        let dir = d();
        full(&f, dir.path());
        let old = state(dir.path()).projects["p1"].cursor.clone().unwrap();
        f.advance(3600);
        for i in 0..150 {
            f.touch(&format!("t{i}"));
        }
        // users/me, projects ×2, 첫 쪽 → 둘째 쪽 전에 멈춰요.
        assert!(run(&f, dir.path(), f.now(), 4).more);
        assert_eq!(
            state(dir.path()).projects["p1"].cursor.as_deref(),
            Some(old.as_str())
        );
        f.clear_calls();
        assert!(!full(&f, dir.path()).more);
        let firsts: Vec<String> = calls_with(&f, "tasks?project=p1")
            .into_iter()
            .filter(|c| !c.contains("offset="))
            .collect();
        assert_eq!(firsts.len(), 1);
        assert!(
            firsts[0].contains(&format!("modified_since={old}")),
            "{firsts:?}"
        );
        assert_ne!(
            state(dir.path()).projects["p1"].cursor.as_deref(),
            Some(old.as_str())
        );
    }

    #[test]
    fn budget_inside_the_comment_phase_resumes_the_rest() {
        let f = base();
        for g in ["1", "2", "3"] {
            f.task(Task::new(g, "글").in_project("p1"));
            f.story(g, &format!("c{g}"), "comment", None, &format!("댓글 {g}"));
        }
        let dir = d();
        // users/me, projects ×2, tasks p1, me, stories 하나.
        assert!(run(&f, dir.path(), f.now(), 6).more);
        assert_eq!(calls_with(&f, "/stories").len(), 1);
        assert_eq!(state(dir.path()).pending_comments.len(), 2);
        f.clear_calls();
        assert!(!full(&f, dir.path()).more);
        assert_eq!(calls_with(&f, "/stories").len(), 2);
        assert!(calls_with(&f, "GET tasks?").is_empty());
        for g in ["1", "2", "3"] {
            assert_eq!(texts(&item(dir.path(), g)), [format!("댓글 {g}")]);
        }
    }

    #[test]
    fn unchanged_subtasks_do_not_refetch_comments() {
        let f = base();
        f.task(Task::new("1", "부모").in_project("p1"));
        f.task(Task::new("s1", "하위").parent("1"));
        let dir = d();
        full(&f, dir.path());
        f.advance(3600);
        f.touch("1");
        f.clear_calls();
        full(&f, dir.path());
        assert_eq!(calls_with(&f, "tasks/1/subtasks").len(), 1);
        assert!(calls_with(&f, "tasks/s1/stories").is_empty());
        assert_eq!(calls_with(&f, "tasks/1/stories").len(), 1);
    }

    #[test]
    fn a_subtask_in_a_project_listing_keeps_its_parent_chain() {
        let f = base();
        f.task(Task::new("1", "부모").in_project("p1"));
        f.task(Task::new("s1", "하위").parent("1"));
        f.task(
            Task::new("s2", "손주")
                .parent("s1")
                .in_project("p1")
                .homed(),
        );
        let dir = d();
        full(&f, dir.path());
        let chain = |d: &Path| item(d, "s2")["parents"].clone();
        assert_eq!(
            chain(dir.path()),
            json!([{"gid": "1", "name": "부모"}, {"gid": "s1", "name": "하위"}])
        );
        f.advance(3600);
        f.touch("s2");
        full(&f, dir.path());
        assert_eq!(chain(dir.path()).as_array().unwrap().len(), 2);
        // 저장본이 없으면 바로 위 부모 하나예요.
        let fresh = d();
        let g = FakeAsana::new();
        g.workspace("w1", "acme");
        g.project("p1", "w1", "로드맵");
        g.task(Task::new("x", "남의 부모"));
        g.task(
            Task::new("y", "보이는 하위")
                .parent("x")
                .in_project("p1")
                .homed(),
        );
        full(&g, fresh.path());
        assert_eq!(
            item(fresh.path(), "y")["parents"],
            json!([{"gid": "x", "name": "남의 부모"}])
        );
    }

    #[test]
    fn a_mention_followed_by_hangul_is_a_mention() {
        let f = base();
        f.user("1209", "민호", "m@x.com");
        f.user("77", "지윤", "j@x.com");
        f.task(
            Task::new("1", "글")
                .in_project("p1")
                .notes("https://app.asana.com/0/1209/님 확인, https://app.asana.com/0/77/_ 그리고 https://app.asana.com/0/88/list"),
        );
        let dir = d();
        full(&f, dir.path());
        assert_eq!(calls_with(&f, "GET users/1209").len(), 1);
        assert_eq!(calls_with(&f, "GET users/77").len(), 1);
        assert!(calls_with(&f, "users/88").is_empty());
    }

    /// `Date` 머리가 없는 답이에요.
    struct NoDate<'a>(&'a FakeAsana);

    impl AsanaApi for NoDate<'_> {
        fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply, SourceError> {
            let mut r = self.0.get(path, query)?;
            r.date = None;
            Ok(r)
        }
        fn batch(&self, gets: &[String]) -> Result<Vec<Reply>, SourceError> {
            self.0.batch(gets)
        }
    }

    #[test]
    fn a_missing_date_header_keeps_the_old_cursor() {
        let f = base();
        f.task(Task::new("1", "글").in_project("p1"));
        let dir = d();
        run(&NoDate(&f), dir.path(), f.now(), 10_000);
        let s = state(dir.path());
        assert_eq!(s.projects["p1"].cursor, None);
        assert!(s.projects["p1"].listed_at.is_some());
        assert_eq!(s.workspaces[0].me_cursor, None);
        f.advance(60);
        full(&f, dir.path());
        let old = state(dir.path()).projects["p1"].cursor.clone();
        assert!(old.is_some());
        f.advance(3600);
        run(
            &NoDate(&f),
            dir.path(),
            f.now() + chrono::Duration::hours(5),
            10_000,
        );
        assert_eq!(state(dir.path()).projects["p1"].cursor, old);
    }
}
