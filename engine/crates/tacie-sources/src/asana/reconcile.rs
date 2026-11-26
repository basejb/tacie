//! Weekly reconcile and the 30-minute check (docs/32 6절 "빼는 것", 10-1절 ⑥⑦⑮, 10-2절. 10-1 and 10-2 win).
//!
//! **Reconcile** (`reconcile`): runs every 7 days (`RECONCILE_DAYS`) after the initial fetch finishes. Right after the
//! initial fetch the raw data is fresh, so it does not ask and records that moment as the first reconcile. It runs
//! within one fetch's budget (`Budget`); when that fills up it saves where to resume (`reconcile_next`,
//! `<단계>:<마지막으로 끝낸 것>`) and returns `Later`. The stages, in order:
//! 1. `projects`: fetches `GET /projects?archived=false|true` (gids only) to the end for each open workspace. Only
//!    workspaces where both lists were fetched to the end are compared (an incomplete list never deletes). Projects
//!    missing from those lists and projects of workspaces gone from `users/me` are "missing". A 403 from `/projects`
//!    means the workspace is blocked, so it is deleted at once.
//! 2. `tasks`: fetches only gids for each list and adds them to the seen gids (`reconcile_seen`). Per project
//!    `GET /tasks?project=&opt_fields=gid,modified_at&limit=100&completed_since=<2년 전>`, per saved parent
//!    `GET /tasks/{gid}/subtasks?opt_fields=gid`, and per workspace `assignee=me`. Saves after finishing each list.
//! 3. `check`: asks about saved tasks that are in no list, parents first (by depth), with `GET /tasks/{gid}` through
//!    `/batch` (10 at a time). This is item-by-item evidence, so it deletes right away without the delete rule
//!    (10-2절 치명 3): 404 or 403 deletes; 200 is checked again against the current `memberships`. It keeps the task
//!    and fixes `memberships` (saved projects only) if at least one project is visible, it is assigned to me, its
//!    parent appeared in a list, or it is a subtask chosen to stay. Otherwise it deletes. Deleting a task also
//!    deletes its subtasks from the saved copy. 429 and 5xx (and any other response) are asked again by the next
//!    reconcile (nothing deleted). After all are asked, tasks older than 2 years are deleted.
//! 4. `comments`: refetches comments of tasks changed in the last 90 days (editing a comment leaves `modified_at`
//!    unchanged, 10-1절 ⑥).
//!
//! - **Delete rule** (things missing from a list but not checked one by one: projects missing from a complete
//!   `/projects` list, projects of workspaces gone from `users/me`): if fewer than 5 are missing or 20% or less of the
//!   saved copy, they are deleted at once. Above that, an item is deleted when it is missing twice in a row (`gone`,
//!   like Atlassian `STRIKES = 2`). If it comes back, the count is cleared.
//! - Muted workspaces are not called and their raw data is not touched. Muted projects are reconciled (they are
//!   left out only when building chunks).
//!
//! **30-minute check** (`changed`): only asks whether anything changed. Projects with a task changed in the last 30
//! days are asked 40 at a time (`hot_next`), the rest in turns of 50 (`probe_next`), with
//! `GET /tasks?project=&modified_since=<커서>&limit=1&opt_fields=gid`. Each workspace's `assignee=me` is asked the
//! same way. In total up to 90 calls and 20 seconds. During the initial fetch, an unfinished round or a due reconcile,
//! it does not ask and answers "changed".
//!
//! Content (titles, bodies) is never put in logs or errors.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use crate::asana::api::{AsanaApi, Reply};
use crate::asana::fetch::{End, LIMIT, comments, list_pages, open_workspaces, wipe_workspace};
use crate::asana::state::{
    Proj, Run, State, Step, WINDOW_DAYS, iso, item_ids, load_state, parse, read_item, save_state,
    state_lock,
};
use crate::error::{Kind, SourceError};
use crate::gdrive::fetch::safe;

/// 재조정 간격(일)이에요.
pub(crate) const RECONCILE_DAYS: i64 = 7;
/// 두 번 연달아 빠지면 지워요.
const STRIKES: u8 = 2;
/// 재조정에서 댓글을 다시 받는 범위(일)예요.
const COMMENT_DAYS: i64 = 90;
/// 30분 확인이 한 번에 돌아가며 묻는 프로젝트 수예요.
const PROBE_PROJECTS: usize = 50;
/// 30분 확인 한 번에 묻는 최근 프로젝트 수예요. 넘으면 다음 확인이 이어요(`hot_next`).
const HOT_PROBE: usize = 40;
/// 30분 확인 한 번에 보내는 요청의 끝이에요(`assignee=me`도 세요).
const PROBE_MAX: usize = HOT_PROBE + PROBE_PROJECTS;
/// 30분 확인 한 번이 쓰는 시간의 끝이에요.
pub(crate) const CHECK_TIME: std::time::Duration = std::time::Duration::from_secs(20);
/// 30분 확인이 매번 묻는 프로젝트: 최근 이만큼(일) 안에 task가 바뀌었어요.
const HOT_DAYS: i64 = 30;
/// `/batch` 한 번의 GET 수예요(`api::BATCH_SIZE`와 같아요).
const BATCH: usize = 10;
/// 재조정의 `/batch` GET 칸이에요. `assignee`·`parent`는 다시 판단하는 데 써요.
const CHECK_FIELDS: &str = "memberships.project.gid,modified_at,assignee.gid,parent.gid";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Projects,
    Tasks,
    Check,
    Comments,
}

const PHASES: [Phase; 4] = [Phase::Projects, Phase::Tasks, Phase::Check, Phase::Comments];

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Projects => "projects",
            Phase::Tasks => "tasks",
            Phase::Check => "check",
            Phase::Comments => "comments",
        }
    }

    fn of(name: &str) -> Option<Phase> {
        PHASES.into_iter().find(|p| p.name() == name)
    }
}

/// 재조정할 때가 됐는지예요(처음 받기를 끝냈고, 이어 할 자리가 있거나 지난 재조정에서 7일이 지났어요).
pub(crate) fn reconcile_due(s: &State, now: DateTime<Utc>) -> bool {
    if !s.initial_done {
        return false;
    }
    if s.reconcile_next.is_some() {
        return true;
    }
    match s.reconciled_at.as_deref().and_then(parse) {
        Some(t) => now - t >= Duration::days(RECONCILE_DAYS),
        None => true,
    }
}

/// 재조정이에요. 때가 아니면 아무것도 하지 않아요.
pub(crate) fn reconcile(run: &mut Run) -> Result<Step, SourceError> {
    if !reconcile_due(&run.state, run.now) {
        return Ok(Step::Done);
    }
    if run.state.reconcile_next.is_none() && run.state.reconciled_at.is_none() {
        // 처음 받기를 막 끝내 원본이 새것이에요. 그때를 첫 재조정으로 적어요(묻지 않아요).
        run.state.reconciled_at = Some(iso(run.now));
        run.save_upkeep()?;
        return Ok(Step::Done);
    }
    let (start, after) = match run.state.reconcile_next.clone() {
        Some(at) => {
            let (p, a) = at.split_once(':').unwrap_or((at.as_str(), ""));
            (Phase::of(p).unwrap_or(Phase::Projects), a.to_string())
        }
        None => {
            begin(run);
            mark(run, Phase::Projects, "")?;
            (Phase::Projects, String::new())
        }
    };
    for phase in PHASES.into_iter().skip_while(|p| *p != start) {
        let after = if phase == start { after.as_str() } else { "" };
        let step = match phase {
            Phase::Projects => projects_phase(run)?,
            Phase::Tasks => tasks_phase(run, after)?,
            Phase::Check => check_phase(run, after)?,
            Phase::Comments => comments_phase(run, after)?,
        };
        if step == Step::Later {
            run.save_upkeep()?;
            return Ok(Step::Later);
        }
    }
    run.state.reconcile_next = None;
    run.state.reconcile_seen.clear();
    run.state.reconcile_direct.clear();
    run.state.reconciled_at = Some(iso(run.now));
    run.save_upkeep()?;
    Ok(Step::Done)
}

/// 새 재조정을 시작해요: 본 gid를 비우고, 파일이 없는 것의 빠짐 횟수를 지워요(다른 길로 지운 것이에요).
fn begin(run: &mut Run) {
    run.state.reconcile_seen.clear();
    run.state.reconcile_direct.clear();
    let held: BTreeSet<String> = ["task", "project"]
        .iter()
        .flat_map(|k| {
            item_ids(run.dir, k)
                .into_iter()
                .map(move |id| format!("{k}-{id}"))
        })
        .collect();
    run.state.gone.retain(|k, _| held.contains(k));
}

/// 다음 단계로 넘어가는 자리를 적어요.
fn mark(run: &mut Run, phase: Phase, at: &str) -> Result<(), SourceError> {
    run.state.reconcile_next = Some(format!("{}:{at}", phase.name()));
    run.save_upkeep()
}

/// 빠진 것을 지우는 규칙이에요. `missed`는 이번에 빠진 것(저장본 `held`개 중)이고, 빠짐 횟수는 이미 올렸어요.
/// 지울 것을 돌려줘요(그 횟수는 지워요).
fn settle(run: &mut Run, kind: &str, missed: &[String], held: usize) -> Vec<String> {
    let small = missed.len() < 5 || missed.len() * 5 <= held;
    let mut out = Vec::new();
    for id in missed {
        let key = format!("{kind}-{id}");
        let strikes = run.state.gone.get(&key).copied().unwrap_or(0);
        if small || strikes >= STRIKES {
            run.state.gone.remove(&key);
            out.push(id.clone());
        }
    }
    out
}

fn strike(run: &mut Run, kind: &str, id: &str) {
    let n = run.state.gone.entry(format!("{kind}-{id}")).or_insert(0);
    *n = n.saturating_add(1);
}

/// 1단계예요. 목록은 짧아서(워크스페이스마다 두세 번) 몫이 다 차면 다음에 처음부터 다시 받아요. 빠짐 횟수는 모든
/// 목록을 받은 뒤 한꺼번에 올려요.
fn projects_phase(run: &mut Run) -> Result<Step, SourceError> {
    run.refresh_mutes();
    let present: BTreeSet<String> = run.state.workspaces.iter().map(|w| w.gid.clone()).collect();
    // 끝까지 받은 워크스페이스와 그 안의 프로젝트예요.
    let mut complete: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for ws in open_workspaces(run) {
        let mut whole = true;
        for archived in ["false", "true"] {
            let q = [
                ("workspace", ws.clone()),
                ("archived", archived.to_string()),
                ("limit", LIMIT.to_string()),
                ("opt_fields", "gid".to_string()),
            ];
            let end = list_pages(run, "projects", "projects", &q, |_, items, _| {
                seen.extend(gids(items));
                Ok(())
            })?;
            match end {
                End::Done(_) => {}
                End::Later => return Ok(Step::Later),
                End::Status(403) => {
                    run.save_upkeep()?;
                    wipe_workspace(run, &ws)?;
                    whole = false;
                    break;
                }
                End::Status(_) => {
                    whole = false;
                    break;
                }
            }
        }
        if whole {
            complete.insert(ws);
        }
    }
    // 견줄 프로젝트: 끝까지 받은 워크스페이스의 것과 `users/me`에서 빠진 워크스페이스의 것이에요.
    let held: Vec<String> = run
        .state
        .projects
        .iter()
        .filter(|(_, p)| complete.contains(&p.ws) || !present.contains(&p.ws))
        .map(|(g, _)| g.clone())
        .collect();
    let missed: Vec<String> = held
        .iter()
        .filter(|g| !seen.contains(*g))
        .cloned()
        .collect();
    for g in &held {
        if seen.contains(g) {
            run.state.gone.remove(&format!("project-{g}"));
        } else {
            strike(run, "project", g);
        }
    }
    for g in settle(run, "project", &missed, held.len()) {
        run.remove("project", &g)?;
        run.state.projects.remove(&g);
    }
    mark(run, Phase::Tasks, "")?;
    Ok(Step::Done)
}

fn gids(items: &[Value]) -> impl Iterator<Item = String> + '_ {
    items
        .iter()
        .filter_map(|t| t["gid"].as_str().filter(|g| safe(g)).map(str::to_string))
}

/// 재조정이 다루는 저장된 task예요(정렬): 끈 워크스페이스의 것은 빼요(`users/me`에서 빠진 워크스페이스의 것은 넣어요).
fn held_tasks(run: &Run) -> Vec<(String, Value)> {
    let present: BTreeSet<&str> = run
        .state
        .workspaces
        .iter()
        .map(|w| w.gid.as_str())
        .collect();
    item_ids(run.dir, "task")
        .into_iter()
        .filter_map(|g| {
            let v = read_item(run.dir, "task", &g)?;
            let ws = v["workspace"].as_str().unwrap_or("");
            let muted = present.contains(ws) && run.state.muted_workspaces.iter().any(|m| m == ws);
            (!muted).then_some((g, v))
        })
        .collect()
}

/// 2단계의 목록들이에요: `me:<ws>`, `parent:<gid>`, `project:<gid>`(정렬해서 이어 할 자리로 써요).
fn listings(run: &Run) -> Vec<String> {
    let open = open_workspaces(run);
    let mut out: BTreeSet<String> = BTreeSet::new();
    for ws in &open {
        out.insert(format!("me:{ws}"));
    }
    for (g, p) in &run.state.projects {
        if open.contains(&p.ws) {
            out.insert(format!("project:{g}"));
        }
    }
    let held = held_tasks(run);
    let stored: BTreeSet<&str> = held.iter().map(|(g, _)| g.as_str()).collect();
    for (_, v) in &held {
        let ws_open = v["workspace"]
            .as_str()
            .is_some_and(|w| open.iter().any(|o| o == w));
        if let Some(p) = v["parents"]
            .as_array()
            .and_then(|c| c.last())
            .and_then(|p| p["gid"].as_str())
            && ws_open
            && stored.contains(p)
        {
            out.insert(format!("parent:{p}"));
        }
    }
    out.into_iter().collect()
}

/// 2단계예요. 목록 하나를 끝낼 때마다 이을 자리와 본 gid를 저장해요. 2xx가 아닌 목록은 덜 온 것이라 건너뛰어요(그
/// 안의 task는 3단계가 하나씩 물어요).
fn tasks_phase(run: &mut Run, after: &str) -> Result<Step, SourceError> {
    let window = iso(run.now - Duration::days(WINDOW_DAYS));
    for key in listings(run).into_iter().filter(|k| k.as_str() > after) {
        let (kind, gid) = key.split_once(':').unwrap_or(("", ""));
        let (path, q): (String, Vec<(&str, String)>) = match kind {
            "project" => (
                "tasks".into(),
                vec![
                    ("project", gid.to_string()),
                    ("opt_fields", "gid,modified_at".to_string()),
                    ("limit", LIMIT.to_string()),
                    ("completed_since", window.clone()),
                ],
            ),
            "parent" => (
                format!("tasks/{gid}/subtasks"),
                vec![
                    ("opt_fields", "gid".to_string()),
                    ("limit", LIMIT.to_string()),
                ],
            ),
            _ => (
                "tasks".into(),
                vec![
                    ("assignee", "me".to_string()),
                    ("workspace", gid.to_string()),
                    ("opt_fields", "gid,modified_at".to_string()),
                    ("limit", LIMIT.to_string()),
                    ("completed_since", window.clone()),
                ],
            ),
        };
        let end = list_pages(run, &path, "tasks", &q, |run, items, _| {
            let found: Vec<String> = gids(items).collect();
            if kind != "parent" {
                run.state.reconcile_direct.extend(found.iter().cloned());
            }
            run.state.reconcile_seen.extend(found);
            Ok(())
        })?;
        if matches!(end, End::Later) {
            return Ok(Step::Later);
        }
        mark(run, Phase::Tasks, &key)?;
    }
    mark(run, Phase::Check, "")?;
    Ok(Step::Done)
}

fn too_old(v: &Value, edge: DateTime<Utc>) -> bool {
    v["modified_at"]
        .as_str()
        .and_then(parse)
        .is_some_and(|t| t < edge)
}

/// 3단계의 차례 열쇠예요: 부모가 자식보다 먼저 오게 깊이(`parents` 길이) 다음 gid예요.
fn check_key(gid: &str, v: &Value) -> String {
    let depth = v["parents"].as_array().map_or(0, Vec::len);
    format!("{depth:02}/{gid}")
}

/// 3단계예요: 어느 목록에도 없던 task를 부모부터(깊이 순서) `/batch`로 물어요. GET 하나마다 몫을 하나 써요. 하나씩 본
/// 증거라 빠진 것은 바로 지우고(하위 task도 같이), 남는 것은 본 gid에 더해 그 하위 task가 남을 수 있게 해요. 다 물으면
/// 2년보다 오래된 task를 지워요.
fn check_phase(run: &mut Run, after: &str) -> Result<Step, SourceError> {
    let edge = run.now - Duration::days(WINDOW_DAYS);
    let held = held_tasks(run);
    let below = descendants(&held);
    let mut todo: Vec<(String, String)> = held
        .into_iter()
        .filter(|(g, v)| !run.state.reconcile_seen.contains(g) && !too_old(v, edge))
        .map(|(g, v)| (check_key(&g, &v), g))
        .filter(|(k, _)| k.as_str() > after)
        .collect();
    todo.sort();
    let mut rest = todo.into_iter().peekable();
    while rest.peek().is_some() {
        // 앞 묶음에서 부모와 함께 지운 하위 task는 묻지 않아요.
        let mut chunk: Vec<(String, String, Value)> = Vec::new();
        let mut spent = false;
        while chunk.len() < BATCH {
            if rest.peek().is_none() {
                break;
            }
            if run.budget.spent() {
                spent = true;
                break;
            }
            let Some((key, gid)) = rest.next() else { break };
            if let Some(v) = read_item(run.dir, "task", &gid) {
                run.budget.take();
                chunk.push((key, gid, v));
            }
        }
        if chunk.is_empty() {
            if spent {
                return Ok(Step::Later);
            }
            break;
        }
        run.guard()?;
        let gets: Vec<String> = chunk
            .iter()
            .map(|(_, g, _)| format!("tasks/{g}?opt_fields={CHECK_FIELDS}"))
            .collect();
        let replies = run.api.batch(&gets)?;
        for ((_, gid, stored), reply) in chunk.iter().zip(replies) {
            judge(run, &below, gid, stored, reply)?;
        }
        let last = chunk.last().map(|(k, _, _)| k.clone()).unwrap_or_default();
        mark(run, Phase::Check, &last)?;
        if spent {
            return Ok(Step::Later);
        }
    }
    for (gid, v) in held_tasks(run) {
        if too_old(&v, edge) {
            run.remove("task", &gid)?;
            forget_task(run, &gid);
        }
    }
    mark(run, Phase::Comments, "")?;
    Ok(Step::Done)
}

/// 프로젝트가 보이는지예요: 저장한 프로젝트이고 그 워크스페이스가 `users/me`에 있고 막히지 않았어요.
fn visible_project(s: &State, gid: &str) -> bool {
    s.projects
        .get(gid)
        .is_some_and(|p| s.workspaces.iter().any(|w| w.gid == p.ws && !w.blocked))
}

/// `/batch` 답 하나로 task 하나를 판단해요. 404·403과 보이는 곳이 없는 200은 바로 지워요(docs/32 10-2절 치명 3).
fn judge(
    run: &mut Run,
    below: &Below,
    gid: &str,
    stored: &Value,
    reply: Reply,
) -> Result<(), SourceError> {
    if read_item(run.dir, "task", gid).is_none() {
        // 같은 묶음의 부모와 함께 지웠어요.
        return Ok(());
    }
    match reply.status {
        404 | 403 => drop_task(run, below, gid),
        200..=299 => {
            let Ok(v) = reply.json("task check") else {
                return Ok(());
            };
            let t = &v["data"];
            let projects: Vec<String> = t["memberships"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["project"]["gid"].as_str().map(str::to_string))
                .collect();
            let ws = stored["workspace"].as_str().unwrap_or("");
            let ws_present = run
                .state
                .workspaces
                .iter()
                .any(|w| w.gid == ws && !w.blocked);
            let mine = ws_present && t["assignee"]["gid"].as_str() == Some(run.user_gid);
            // 하위 task는 부모를 따라요: 부모가 목록에 나왔거나 이번에 남기로 했어야 해요.
            let under_parent = ws_present
                && t["parent"]["gid"]
                    .as_str()
                    .is_some_and(|p| run.state.reconcile_seen.contains(p));
            let seen_project = projects.iter().any(|p| visible_project(&run.state, p));
            if seen_project || mine || under_parent {
                run.state.reconcile_seen.insert(gid.to_string());
                run.state.reconcile_direct.insert(gid.to_string());
                fix_memberships(run, gid, &projects)
            } else {
                drop_task(run, below, gid)
            }
        }
        // 한도(429)·서버 오류(5xx)·그 밖의 답은 다음 재조정이 다시 물어요. 그 하위 task도 남게 본 것으로 둬요.
        _ => {
            run.state.reconcile_seen.insert(gid.to_string());
            run.state.reconcile_direct.insert(gid.to_string());
            Ok(())
        }
    }
}

/// 저장본의 `memberships`를 지금 프로젝트에 맞춰요. 저장한 프로젝트만 남기고, 남은 프로젝트의 칸(이름, 섹션)은 그대로
/// 두고, 새 프로젝트는 저장한 이름으로 더해요.
fn fix_memberships(run: &mut Run, gid: &str, projects: &[String]) -> Result<(), SourceError> {
    let Some(mut item) = read_item(run.dir, "task", gid) else {
        return Ok(());
    };
    let old: Vec<Value> = item["memberships"].as_array().cloned().unwrap_or_default();
    let fresh: Vec<Value> = projects
        .iter()
        .filter_map(|p| {
            let known = run.state.projects.get(p)?;
            Some(
                old.iter()
                    .find(|m| m["project"]["gid"].as_str() == Some(p))
                    .cloned()
                    .unwrap_or_else(|| json!({"project": {"gid": p, "name": known.name}})),
            )
        })
        .collect();
    if fresh == old {
        return Ok(());
    }
    item["memberships"] = Value::Array(fresh);
    run.write("task", gid, &item)
}

/// task 하나와 그 밑의 하위 task(저장본의 `parents`에 그 gid가 든 것)를 지워요.
/// 조상 gid → 그 밑의 저장된 하위 task(gid, 그 조상 바로 밑부터 바로 위 부모까지의 gid)예요. 재조정 3단계에서 한 번
/// 만들어요.
type Below = BTreeMap<String, Vec<(String, Vec<String>)>>;

fn descendants(held: &[(String, Value)]) -> Below {
    let mut out = Below::new();
    for (gid, v) in held {
        let chain: Vec<String> = v["parents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["gid"].as_str().map(str::to_string))
            .collect();
        for (i, a) in chain.iter().enumerate() {
            out.entry(a.clone())
                .or_default()
                .push((gid.clone(), chain[i + 1..].to_vec()));
        }
    }
    out
}

/// task 하나와 그 밑의 하위 task를 지워요. 이번 재조정의 프로젝트·`assignee=me` 목록에 나왔거나 `/batch`로 남기로 한
/// 하위 task(`reconcile_direct`)와 그 밑은 남겨요(부모의 하위 task 목록에만 나온 것은 부모를 따라 지워요).
fn drop_task(run: &mut Run, below: &Below, gid: &str) -> Result<(), SourceError> {
    let doomed: Vec<String> = below
        .get(gid)
        .into_iter()
        .flatten()
        .filter(|(child, between)| {
            let seen = &run.state.reconcile_direct;
            !seen.contains(child) && !between.iter().any(|g| seen.contains(g))
        })
        .map(|(child, _)| child.clone())
        .collect();
    for g in std::iter::once(gid.to_string()).chain(doomed) {
        run.remove("task", &g)?;
        forget_task(run, &g);
    }
    Ok(())
}

fn forget_task(run: &mut Run, gid: &str) {
    run.state.pending_comments.remove(gid);
    run.state.pending_subtasks.remove(gid);
    run.state.reconcile_seen.remove(gid);
    run.state.reconcile_direct.remove(gid);
    run.state.gone.remove(&format!("task-{gid}"));
}

/// 4단계예요: 최근 90일에 바뀐 task를 댓글 받을 목록에 한 번 넣고(`comments:marked`), 댓글 단계를 돌려요. 몫이 다
/// 차면 다음 받기의 댓글 단계가 이어요.
fn comments_phase(run: &mut Run, after: &str) -> Result<Step, SourceError> {
    if after != "marked" {
        let edge = run.now - Duration::days(COMMENT_DAYS);
        let open = open_workspaces(run);
        let recent: Vec<String> = held_tasks(run)
            .into_iter()
            .filter(|(_, v)| {
                v["workspace"]
                    .as_str()
                    .is_some_and(|w| open.iter().any(|o| o == w))
                    && v["modified_at"]
                        .as_str()
                        .and_then(parse)
                        .is_some_and(|t| t >= edge)
            })
            .map(|(g, _)| g)
            .collect();
        run.state.pending_comments.extend(recent);
        mark(run, Phase::Comments, "marked")?;
    }
    comments(run)
}

/// 30분 확인이에요: 바뀐 게 있는지만 물어요. 돌아가며 묻는 자리(`hot_next`, `probe_next`)만 써요. 한 번에
/// `CHECK_TIME` 안에서 `PROBE_MAX`번까지만 물어요(앱의 `/index/check`가 기다리는 시간 안에 끝나요).
pub fn changed(api: &dyn AsanaApi, raw: &Path, now: DateTime<Utc>) -> Result<bool, SourceError> {
    changed_within(api, raw, now, CHECK_TIME)
}

/// `changed`의 몸통이에요. `limit`이 지나면(또는 부르는 쪽이 시간 끝 `Busy`를 주면) 더 묻지 않고, 그때까지 바뀐 게
/// 없었으면 거짓이에요. 다음 확인이 그 자리부터 이어요.
pub(crate) fn changed_within(
    api: &dyn AsanaApi,
    raw: &Path,
    now: DateTime<Utc>,
    limit: std::time::Duration,
) -> Result<bool, SourceError> {
    let deadline = std::time::Instant::now() + limit;
    let s = {
        let _held = state_lock();
        load_state(raw)
    };
    if !s.initial_done || s.round.is_some() || reconcile_due(&s, now) {
        return Ok(true);
    }
    let open = s.open_workspaces();
    let projects: Vec<(&String, &Proj)> = s
        .projects
        .iter()
        .filter(|(_, p)| open.contains(&p.ws))
        .collect();
    let hot_edge = now - Duration::days(HOT_DAYS);
    let hot: Vec<usize> = projects
        .iter()
        .enumerate()
        .filter(|(_, (_, p))| {
            p.changed_at
                .as_deref()
                .and_then(parse)
                .is_some_and(|t| t >= hot_edge)
        })
        .map(|(i, _)| i)
        .collect();
    let hot_start = if hot.is_empty() {
        0
    } else {
        s.hot_next % hot.len()
    };
    let hot_turn: Vec<usize> = (0..hot.len().min(HOT_PROBE))
        .map(|k| hot[(hot_start + k) % hot.len()])
        .collect();
    let n = projects.len();
    let start = if n == 0 { 0 } else { s.probe_next % n };
    let turn: Vec<usize> = (0..n.min(PROBE_PROJECTS))
        .map(|k| (start + k) % n)
        .collect();
    let me: Vec<(String, String)> = s
        .workspaces
        .iter()
        .filter(|w| open.contains(&w.gid))
        .filter_map(|w| Some((w.gid.clone(), w.me_cursor.clone()?)))
        .collect();
    // 묻는 순서: 최근에 바뀐 프로젝트(40개까지), 워크스페이스마다 `assignee=me`, 돌아가며 묻는 프로젝트(50개까지)예요.
    // 모두 합쳐 `PROBE_MAX`번까지예요.
    #[derive(Clone, Copy)]
    enum Probe {
        Hot(usize),
        Me(usize),
        Turn(usize),
    }
    let mut order: Vec<Probe> = hot_turn.iter().map(|&i| Probe::Hot(i)).collect();
    order.extend((0..me.len()).map(Probe::Me));
    order.extend(turn.iter().map(|&i| Probe::Turn(i)));
    let (mut hot_asked, mut asked, mut sent) = (0usize, 0usize, 0usize);
    let mut hit = false;
    for p in order {
        if sent >= PROBE_MAX || std::time::Instant::now() >= deadline {
            break;
        }
        let q = match p {
            Probe::Hot(i) | Probe::Turn(i) => {
                if let Probe::Hot(_) = p {
                    hot_asked += 1;
                } else {
                    asked += 1;
                    // 이번에 이미 최근 것으로 물었어요
                    if hot_turn.contains(&i) {
                        continue;
                    }
                }
                let (gid, proj) = projects[i];
                vec![
                    ("project", gid.clone()),
                    ("modified_since", probe_cursor(proj, now)),
                    ("limit", "1".to_string()),
                    ("opt_fields", "gid".to_string()),
                ]
            }
            Probe::Me(k) => vec![
                ("assignee", "me".to_string()),
                ("workspace", me[k].0.clone()),
                ("modified_since", me[k].1.clone()),
                ("limit", "1".to_string()),
                ("opt_fields", "gid".to_string()),
            ],
        };
        sent += 1;
        match api.get("tasks", &q) {
            Ok(r) => {
                if any(r) {
                    hit = true;
                    break;
                }
            }
            // 시간 끝이나 한도예요: 여기까지만 묻고 다음 확인이 이어요
            Err(e) if e.kind() == Kind::Busy => {
                match p {
                    Probe::Hot(_) => hot_asked -= 1,
                    Probe::Turn(_) => asked -= 1,
                    Probe::Me(_) => {}
                }
                break;
            }
            Err(e) => return Err(e),
        }
    }
    if hot_asked > 0 || asked > 0 {
        let _held = state_lock();
        let mut disk = load_state(raw);
        if disk.user_gid == s.user_gid {
            if hot_asked > 0 {
                disk.hot_next = (hot_start + hot_asked) % hot.len();
            }
            if asked > 0 {
                disk.probe_next = (start + asked) % n;
            }
            save_state(raw, &disk)?;
        }
    }
    Ok(hit)
}

/// 30분 확인의 기준 시각이에요. 커서가 없는 프로젝트(첫 목록이 404였거나 `Date`가 없었어요)도 다른 프로젝트처럼
/// 물어요: 목록을 끝낸 Mac 시각 − 15분, 그것도 없으면 2년 전이에요. 그래서 "바뀜"이 영영 이어지지 않아요.
fn probe_cursor(p: &Proj, now: DateTime<Utc>) -> String {
    p.cursor.clone().unwrap_or_else(|| {
        p.listed_at
            .as_deref()
            .and_then(parse)
            .map(|t| iso(t - Duration::minutes(15)))
            .unwrap_or_else(|| iso(now - Duration::days(WINDOW_DAYS)))
    })
}

/// 목록 답에 항목이 하나라도 있는지예요. 2xx가 아니면(지웠거나 볼 수 없는 프로젝트) 없는 것으로 봐요.
fn any(r: Reply) -> bool {
    if !(200..300).contains(&r.status) {
        return false;
    }
    r.json("tasks check")
        .ok()
        .and_then(|v| v["data"].as_array().map(|a| !a.is_empty()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asana::fetch::{Fetched, fetch_budget};
    use crate::asana::state::read_item;
    use crate::error::Kind;
    use crate::gdrive::fetch::{BATCH_TIME, Budget};
    use crate::testing::asana::{FakeAsana, Task};
    use std::sync::atomic::{AtomicBool, Ordering};

    const DAY: i64 = 86_400;

    fn d() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn run_with(
        f: &dyn AsanaApi,
        dir: &Path,
        now: DateTime<Utc>,
        n: usize,
    ) -> Result<Fetched, SourceError> {
        fetch_budget(
            f,
            dir,
            now,
            "me-1",
            &|| true,
            &mut |_, _| {},
            Budget::new(n, BATCH_TIME),
        )
    }

    fn full(f: &FakeAsana, dir: &Path) -> Fetched {
        run_with(f, dir, f.now(), 10_000).unwrap()
    }

    /// 처음 받기(첫 재조정으로 적혀요) 뒤 8일을 보내요.
    fn synced_then_a_week(f: &FakeAsana, dir: &Path) {
        full(f, dir);
        let s = load_state(dir);
        assert!(s.initial_done);
        assert!(s.reconciled_at.is_some());
        f.advance(8 * DAY);
    }

    fn tasks(dir: &Path) -> Vec<String> {
        item_ids(dir, "task")
    }

    fn calls_with(f: &FakeAsana, part: &str) -> Vec<String> {
        f.calls().into_iter().filter(|c| c.contains(part)).collect()
    }

    fn base() -> FakeAsana {
        let f = FakeAsana::new();
        f.workspace("w1", "acme");
        f.project("p1", "w1", "로드맵");
        f
    }

    fn many(f: &FakeAsana, n: usize, project: &str) {
        for i in 1..=n {
            f.task(Task::new(&i.to_string(), "글").in_project(project));
        }
    }

    #[test]
    fn first_reconcile_is_recorded_without_asking_then_waits_a_week() {
        let f = base();
        many(&f, 3, "p1");
        let dir = d();
        full(&f, dir.path());
        assert!(f.calls().iter().all(|c| !c.starts_with("BATCH")));
        assert!(calls_with(&f, "opt_fields=gid,modified_at").is_empty());
        let s = load_state(dir.path());
        assert!(!reconcile_due(&s, f.now() + Duration::days(6)));
        assert!(reconcile_due(&s, f.now() + Duration::days(7)));
    }

    #[test]
    fn deleted_task_404_in_batch_is_removed() {
        let f = base();
        many(&f, 6, "p1");
        let dir = d();
        synced_then_a_week(&f, dir.path());
        f.delete_task("3");
        f.clear_calls();
        assert_eq!(full(&f, dir.path()), Fetched::default());
        assert_eq!(tasks(dir.path()), ["1", "2", "4", "5", "6"]);
        let checked = calls_with(&f, "BATCH ");
        assert_eq!(
            checked,
            [
                "BATCH tasks/3?opt_fields=memberships.project.gid,modified_at,assignee.gid,parent.gid"
            ]
        );
        let listed = calls_with(&f, "tasks?project=p1&opt_fields=gid,modified_at");
        assert_eq!(listed.len(), 1);
        assert!(
            listed[0].contains("limit=100&completed_since="),
            "{listed:?}"
        );
        let s = load_state(dir.path());
        assert_eq!(s.reconcile_next, None);
        assert_eq!(s.reconciled_at.as_deref(), Some(iso(f.now()).as_str()));
        assert!(s.reconcile_seen.is_empty());
        assert!(s.gone.is_empty());
    }

    /// 재조정의 `assignee=me` 목록과 하위 task 목록을 404로 답하는 가짜예요(그 task가 `/batch`로 가요).
    struct NoMine<'a>(&'a FakeAsana);

    impl AsanaApi for NoMine<'_> {
        fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply, SourceError> {
            let field = |want: &str| query.iter().any(|(k, v)| *k == "opt_fields" && v == want);
            let mine = (query.iter().any(|(k, _)| *k == "assignee") && field("gid,modified_at"))
                || (path.ends_with("/subtasks") && field("gid"));
            let r = self.0.get(path, query)?;
            if mine {
                return Ok(Reply {
                    status: 404,
                    body: b"{}".to_vec(),
                    date: r.date,
                });
            }
            Ok(r)
        }
        fn batch(&self, gets: &[String]) -> Result<Vec<Reply>, SourceError> {
            self.0.batch(gets)
        }
    }

    #[test]
    fn task_removed_from_its_only_project_is_removed_unless_assigned_to_me() {
        let f = base();
        f.project("p2", "w1", "결제");
        f.task(Task::new("1", "하나만").in_project("p1"));
        f.task(Task::new("2", "둘에").in_project("p1").in_project("p2"));
        f.task(
            Task::new("3", "내 일")
                .in_project("p1")
                .workspace("w1")
                .assignee_me(),
        );
        for g in ["4", "5", "6", "7"] {
            f.task(Task::new(g, "남는 글").in_project("p1"));
        }
        let dir = d();
        synced_then_a_week(&f, dir.path());
        for g in ["1", "2", "3"] {
            f.remove_from_project(g, "p1");
        }
        f.clear_calls();
        let out = run_with(&NoMine(&f), dir.path(), f.now(), 10_000).unwrap();
        assert_eq!(out, Fetched::default());
        assert_eq!(tasks(dir.path()), ["2", "3", "4", "5", "6", "7"]);
        let checked: Vec<String> = calls_with(&f, "BATCH ")
            .into_iter()
            .map(|c| c.split('?').next().unwrap().to_string())
            .collect();
        assert_eq!(checked, ["BATCH tasks/1", "BATCH tasks/3"]);
        // 남은 task는 지금 프로젝트로 고쳐져 있어요.
        let three = read_item(dir.path(), "task", "3").unwrap();
        assert_eq!(three["memberships"], json!([]));
        let two = read_item(dir.path(), "task", "2").unwrap();
        assert_eq!(two["memberships"].as_array().unwrap().len(), 1);
        assert_eq!(two["memberships"][0]["project"]["gid"], "p2");
    }

    /// 프로젝트 `p1`(task 하나)과 빈 프로젝트 다섯(`q1`…`q5`)이에요.
    fn six_projects() -> FakeAsana {
        let f = base();
        for i in 1..=5 {
            f.project(&format!("q{i}"), "w1", "빈 프로젝트");
        }
        f.task(Task::new("1", "글").in_project("p1"));
        f
    }

    #[test]
    fn more_than_20_percent_missing_waits_for_a_second_strike() {
        let f = six_projects();
        let dir = d();
        synced_then_a_week(&f, dir.path());
        for i in 1..=5 {
            f.remove_project(&format!("q{i}"));
        }
        full(&f, dir.path());
        assert_eq!(
            item_ids(dir.path(), "project").len(),
            6,
            "first strike keeps them"
        );
        let s = load_state(dir.path());
        assert_eq!(s.gone.len(), 5);
        assert!(s.gone.values().all(|n| *n == 1));
        assert_eq!(s.projects.len(), 6);
        // 재조정이 아닌 받기는 횟수를 지우지 않아요.
        f.advance(3600);
        full(&f, dir.path());
        assert_eq!(load_state(dir.path()).gone.len(), 5);
        f.advance(8 * DAY);
        full(&f, dir.path());
        assert_eq!(item_ids(dir.path(), "project"), ["p1"]);
        let s = load_state(dir.path());
        assert!(s.gone.is_empty());
        assert_eq!(s.projects.len(), 1);
        assert_eq!(tasks(dir.path()), ["1"]);
    }

    #[test]
    fn a_project_that_comes_back_loses_its_strike() {
        let f = six_projects();
        let dir = d();
        synced_then_a_week(&f, dir.path());
        for i in 1..=5 {
            f.remove_project(&format!("q{i}"));
        }
        full(&f, dir.path());
        assert_eq!(load_state(dir.path()).gone.len(), 5);
        f.advance(8 * DAY);
        for i in 1..=5 {
            f.project(&format!("q{i}"), "w1", "돌아온 프로젝트");
        }
        full(&f, dir.path());
        assert_eq!(item_ids(dir.path(), "project").len(), 6);
        assert!(load_state(dir.path()).gone.is_empty());
    }

    #[test]
    fn batch_403_on_many_tasks_deletes_them_now() {
        let f = base();
        f.project("p2", "w1", "비공개될");
        for i in 1..=4 {
            f.task(Task::new(&format!("a{i}"), "남는 글").in_project("p1"));
        }
        for i in 1..=6 {
            f.task(Task::new(&format!("b{i}"), "숨을 글").in_project("p2"));
        }
        let dir = d();
        synced_then_a_week(&f, dir.path());
        f.make_private("p2");
        f.clear_calls();
        full(&f, dir.path());
        // 60%가 빠져도 하나씩 403을 받았으니 한 번에 지워요.
        assert_eq!(tasks(dir.path()), ["a1", "a2", "a3", "a4"]);
        assert_eq!(calls_with(&f, "BATCH tasks/b").len(), 6);
        assert!(load_state(dir.path()).gone.is_empty());
    }

    #[test]
    fn parent_removed_from_its_only_project_takes_its_subtasks() {
        let f = base();
        f.task(Task::new("1", "부모").in_project("p1"));
        f.task(Task::new("s1", "하위").parent("1"));
        f.task(Task::new("s2", "손주").parent("s1"));
        f.task(Task::new("2", "남는 글").in_project("p1"));
        let dir = d();
        synced_then_a_week(&f, dir.path());
        assert_eq!(tasks(dir.path()), ["1", "2", "s1", "s2"]);
        f.remove_from_project("1", "p1");
        full(&f, dir.path());
        assert_eq!(tasks(dir.path()), ["2"]);
        let s = load_state(dir.path());
        assert!(s.reconcile_seen.is_empty());
        assert!(s.pending_comments.is_empty());
    }

    #[test]
    fn a_subtask_seen_in_my_list_survives_its_deleted_parent() {
        let f = base();
        f.task(Task::new("1", "부모").in_project("p1"));
        f.task(
            Task::new("s1", "내 하위")
                .parent("1")
                .workspace("w1")
                .assignee_me(),
        );
        f.task(Task::new("s2", "남의 하위").parent("1"));
        f.task(Task::new("s3", "내 하위의 하위").parent("s1"));
        f.task(Task::new("2", "남는 글").in_project("p1"));
        let dir = d();
        synced_then_a_week(&f, dir.path());
        assert_eq!(tasks(dir.path()), ["1", "2", "s1", "s2", "s3"]);
        f.remove_from_project("1", "p1");
        f.clear_calls();
        full(&f, dir.path());
        // s1은 내 목록에 나왔으니 남고, 그 밑의 s3도 따라 남아요. s2는 부모와 함께 지워요.
        assert_eq!(tasks(dir.path()), ["2", "s1", "s3"]);
        assert!(calls_with(&f, "BATCH tasks/s").is_empty());
    }

    #[test]
    fn a_private_parent_takes_its_subtasks() {
        let f = base();
        f.project("p2", "w1", "비공개될");
        f.task(Task::new("1", "부모").in_project("p2"));
        f.task(Task::new("s1", "하위").parent("1"));
        f.task(Task::new("2", "남는 글").in_project("p1"));
        let dir = d();
        synced_then_a_week(&f, dir.path());
        f.make_private("p2");
        // Asana에서 하위 task는 부모를 따라 볼 수 없어요.
        assert_eq!(f.get("tasks/s1", &[]).unwrap().status, 403);
        assert_eq!(f.get("tasks/1/subtasks", &[]).unwrap().status, 403);
        full(&f, dir.path());
        assert_eq!(tasks(dir.path()), ["2"]);
    }

    #[test]
    fn a_kept_parent_keeps_its_subtask_checked_in_the_same_reconcile() {
        // 부모가 목록에 나오지 않아도(나에게 맡겨진 것을 `/batch`로 남겼어요) 하위 task가 따라 남아요.
        let f = base();
        f.task(
            Task::new("1", "내 부모")
                .in_project("p1")
                .workspace("w1")
                .assignee_me(),
        );
        f.task(Task::new("s1", "하위").parent("1"));
        let dir = d();
        synced_then_a_week(&f, dir.path());
        f.remove_from_project("1", "p1");
        f.clear_calls();
        let out = run_with(&NoMine(&f), dir.path(), f.now(), 10_000).unwrap();
        assert_eq!(out, Fetched::default());
        assert_eq!(tasks(dir.path()), ["1", "s1"]);
        let checked: Vec<String> = calls_with(&f, "BATCH ")
            .into_iter()
            .map(|c| c.split('?').next().unwrap().to_string())
            .collect();
        assert_eq!(checked, ["BATCH tasks/1", "BATCH tasks/s1"]);
    }

    #[test]
    fn batch_takes_one_budget_per_get() {
        let f = base();
        for i in 0..10 {
            f.task(Task::new(&format!("t{i}"), "글").in_project("p1"));
        }
        let dir = d();
        full(&f, dir.path());
        f.advance(3600);
        full(&f, dir.path());
        f.advance(8 * DAY);
        for i in 0..10 {
            f.remove_from_project(&format!("t{i}"), "p1");
        }
        f.clear_calls();
        // 바퀴 5번(users/me, projects ×2, p1, me), 재조정 목록 4번(projects ×2, me, p1), 남은 3번이 GET 셋이에요.
        assert!(run_with(&f, dir.path(), f.now(), 12).unwrap().more);
        assert_eq!(calls_with(&f, "BATCH ").len(), 3);
        assert_eq!(tasks(dir.path()).len(), 7);
        assert!(
            load_state(dir.path())
                .reconcile_next
                .as_deref()
                .is_some_and(|n| n.starts_with("check:"))
        );
        full(&f, dir.path());
        assert!(tasks(dir.path()).is_empty());
    }

    /// 재조정의 보관 프로젝트 목록(`opt_fields=gid`)이 시간을 넘기는 가짜예요.
    struct CutProjects<'a> {
        f: &'a FakeAsana,
        on: AtomicBool,
    }

    impl AsanaApi for CutProjects<'_> {
        fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply, SourceError> {
            let cut = path == "projects"
                && query.iter().any(|(k, v)| *k == "archived" && v == "true")
                && query.iter().any(|(k, v)| *k == "opt_fields" && v == "gid");
            if cut && self.on.load(Ordering::SeqCst) {
                return Err(SourceError::service(Kind::Network, "asana: timed out"));
            }
            self.f.get(path, query)
        }
        fn batch(&self, gets: &[String]) -> Result<Vec<Reply>, SourceError> {
            self.f.batch(gets)
        }
    }

    #[test]
    fn partial_project_list_deletes_nothing() {
        let f = base();
        f.project("p2", "w1", "결제");
        f.task(Task::new("1", "글").in_project("p1"));
        f.task(Task::new("2", "결제 글").in_project("p2"));
        let dir = d();
        synced_then_a_week(&f, dir.path());
        f.remove_project("p2");
        let api = CutProjects {
            f: &f,
            on: AtomicBool::new(true),
        };
        assert!(run_with(&api, dir.path(), f.now(), 10_000).is_err());
        assert_eq!(item_ids(dir.path(), "project"), ["p1", "p2"]);
        assert_eq!(tasks(dir.path()), ["1", "2"]);
        let s = load_state(dir.path());
        assert!(s.gone.is_empty());
        assert!(s.projects.contains_key("p2"));
        assert!(f.calls().iter().all(|c| !c.starts_with("BATCH")));
        // 목록이 다 오면 그때 지워요.
        api.on.store(false, Ordering::SeqCst);
        run_with(&api, dir.path(), f.now(), 10_000).unwrap();
        assert_eq!(item_ids(dir.path(), "project"), ["p1"]);
        assert_eq!(tasks(dir.path()), ["1"]);
        assert!(!load_state(dir.path()).projects.contains_key("p2"));
    }

    #[test]
    fn project_turned_private_removes_its_tasks() {
        let f = base();
        f.project("p2", "w1", "비공개될");
        f.task(Task::new("1", "글").in_project("p1"));
        f.task(Task::new("2", "숨을 글").in_project("p2"));
        f.task(Task::new("3", "둘에").in_project("p1").in_project("p2"));
        let dir = d();
        synced_then_a_week(&f, dir.path());
        f.make_private("p2");
        full(&f, dir.path());
        assert_eq!(item_ids(dir.path(), "project"), ["p1"]);
        assert_eq!(tasks(dir.path()), ["1", "3"]);
    }

    #[test]
    fn a_workspace_gone_from_users_me_loses_its_items() {
        let f = base();
        f.workspace("w2", "떠난 회사");
        f.project("q1", "w2", "옛 일");
        f.task(Task::new("1", "글").in_project("p1"));
        f.task(Task::new("2", "떠난 글").in_project("q1"));
        f.task(Task::new("3", "떠난 내 일").workspace("w2").assignee_me());
        let dir = d();
        synced_then_a_week(&f, dir.path());
        f.remove_workspace("w2");
        full(&f, dir.path());
        assert_eq!(item_ids(dir.path(), "project"), ["p1"]);
        assert_eq!(tasks(dir.path()), ["1"]);
        // 빠진 워크스페이스는 재조정에서 부르지 않아요.
        assert!(
            f.calls()
                .iter()
                .all(|c| !(c.contains("workspace=w2") && c.contains("opt_fields=gid")))
        );
    }

    #[test]
    fn subtasks_are_reconciled_through_their_parent() {
        let f = base();
        f.task(Task::new("1", "부모").in_project("p1"));
        f.task(Task::new("s1", "하위").parent("1"));
        f.task(Task::new("s2", "지울 하위").parent("1"));
        let dir = d();
        synced_then_a_week(&f, dir.path());
        f.delete_task("s2");
        f.clear_calls();
        full(&f, dir.path());
        assert_eq!(tasks(dir.path()), ["1", "s1"]);
        assert_eq!(
            calls_with(&f, "GET tasks/1/subtasks?opt_fields=gid"),
            ["GET tasks/1/subtasks?opt_fields=gid&limit=100"]
        );
        let checked: Vec<String> = calls_with(&f, "BATCH ")
            .into_iter()
            .map(|c| c.split('?').next().unwrap().to_string())
            .collect();
        assert_eq!(checked, ["BATCH tasks/s2"]);
    }

    #[test]
    fn muted_workspace_items_are_left_alone() {
        let f = base();
        f.workspace("w2", "남의 회사");
        f.project("q1", "w2", "비밀");
        f.task(Task::new("1", "글").in_project("p1"));
        f.task(Task::new("2", "남의 글").in_project("q1"));
        let dir = d();
        synced_then_a_week(&f, dir.path());
        let mut s = load_state(dir.path());
        s.muted_workspaces.push("w2".into());
        save_state(dir.path(), &s).unwrap();
        f.delete_task("2");
        f.clear_calls();
        full(&f, dir.path());
        assert_eq!(tasks(dir.path()), ["1", "2"]);
        assert!(
            f.calls()
                .iter()
                .all(|c| !c.contains("w2") && !c.contains("q1") && !c.contains("tasks/2"))
        );
    }

    #[test]
    fn items_older_than_two_years_are_dropped() {
        let f = base();
        f.task(Task::new("1", "새 글").in_project("p1"));
        f.task(
            Task::new("old", "옛 하위")
                .parent("1")
                .modified(f.now() - Duration::days(800)),
        );
        let dir = d();
        synced_then_a_week(&f, dir.path());
        assert!(read_item(dir.path(), "task", "old").is_some());
        full(&f, dir.path());
        assert_eq!(tasks(dir.path()), ["1"]);
        assert!(f.calls().iter().all(|c| !c.contains("BATCH tasks/old")));
    }

    #[test]
    fn comments_of_recent_tasks_are_refetched() {
        let f = base();
        f.task(
            Task::new("new", "최근")
                .in_project("p1")
                .modified(f.now() - Duration::days(10)),
        );
        f.task(
            Task::new("old", "오래")
                .in_project("p1")
                .modified(f.now() - Duration::days(200)),
        );
        f.story("new", "c1", "comment", None, "첫 댓글");
        f.story("old", "c2", "comment", None, "옛 댓글");
        let dir = d();
        synced_then_a_week(&f, dir.path());
        // 고친 댓글처럼 `modified_at`이 그대로인 채 댓글이 바뀌어요.
        f.story("new", "c3", "comment", None, "고친 댓글");
        f.story("old", "c4", "comment", None, "옛 글의 고친 댓글");
        f.clear_calls();
        full(&f, dir.path());
        assert_eq!(calls_with(&f, "/stories").len(), 1);
        assert_eq!(calls_with(&f, "tasks/new/stories").len(), 1);
        let n = read_item(dir.path(), "task", "new").unwrap();
        assert_eq!(n["stories"].as_array().unwrap().len(), 2);
        let o = read_item(dir.path(), "task", "old").unwrap();
        assert_eq!(o["stories"].as_array().unwrap().len(), 1);
        assert!(load_state(dir.path()).pending_comments.is_empty());
    }

    #[test]
    fn reconcile_stops_at_the_budget_and_resumes() {
        let f = base();
        f.project("p2", "w1", "둘");
        f.project("p3", "w1", "셋");
        for (g, p) in [("1", "p1"), ("2", "p2"), ("3", "p3"), ("4", "p1")] {
            f.task(Task::new(g, "글").in_project(p));
        }
        let dir = d();
        full(&f, dir.path());
        // 한 번 더 받아 커서가 처음 받은 것을 넘게 해요(바퀴가 짧아져요).
        f.advance(3600);
        full(&f, dir.path());
        f.advance(8 * DAY);
        f.delete_task("4");
        f.clear_calls();
        // 한 바퀴(users/me, projects ×2, 프로젝트 셋, me)가 7번이라 재조정에는 두 번씩 남아요.
        let mut paused = 0;
        for _ in 0..20 {
            let out = run_with(&f, dir.path(), f.now(), 9).unwrap();
            if !out.more {
                break;
            }
            paused += usize::from(load_state(dir.path()).reconcile_next.is_some());
            f.advance(60);
        }
        assert!(paused >= 2, "{paused}");
        let s = load_state(dir.path());
        assert_eq!(s.reconcile_next, None);
        assert_eq!(tasks(dir.path()), ["1", "2", "3"]);
        // 목록 단계는 이어서 받아요: 같은 목록을 두 번 받지 않아요.
        assert_eq!(
            calls_with(&f, "project=p1&opt_fields=gid,modified_at").len(),
            1
        );
    }

    #[test]
    fn retryable_batch_replies_keep_the_task() {
        struct Busy<'a>(&'a FakeAsana);
        impl AsanaApi for Busy<'_> {
            fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply, SourceError> {
                self.0.get(path, query)
            }
            fn batch(&self, gets: &[String]) -> Result<Vec<Reply>, SourceError> {
                let mut out = self.0.batch(gets)?;
                for (r, code) in out.iter_mut().zip([429u16, 503]) {
                    r.status = code;
                }
                Ok(out)
            }
        }
        let f = base();
        many(&f, 6, "p1");
        let dir = d();
        synced_then_a_week(&f, dir.path());
        f.delete_task("1");
        f.delete_task("2");
        run_with(&Busy(&f), dir.path(), f.now(), 10_000).unwrap();
        assert_eq!(tasks(dir.path()).len(), 6);
        assert!(load_state(dir.path()).gone.is_empty());
    }

    fn probes(f: &FakeAsana) -> Vec<String> {
        calls_with(f, "GET tasks?project=")
            .into_iter()
            .map(|c| {
                c.split('&')
                    .next()
                    .unwrap()
                    .trim_start_matches("GET tasks?project=")
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn changed_probes_at_most_50_projects_round_robin() {
        let f = base();
        for i in 0..120 {
            f.project(&format!("q{i:03}"), "w1", "프로젝트");
        }
        f.task(Task::new("h", "자주 바뀌는").in_project("q060"));
        let dir = d();
        full(&f, dir.path());
        // 커서가 15분 겹쳐서, 바뀐 뒤 받기를 한 번 더 해야 커서가 그 변경을 넘어요.
        f.advance(3600);
        full(&f, dir.path());
        f.advance(60);
        let name = |i: usize| format!("q{i:03}");
        let mut want_all: Vec<Vec<String>> = Vec::new();
        // 프로젝트는 p1, q000…q119 순서예요(121개). q060은 최근에 바뀌어 매번 물어요.
        let all: Vec<String> = std::iter::once("p1".to_string())
            .chain((0..120).map(name))
            .collect();
        let mut start = 0;
        for _ in 0..3 {
            let turn: Vec<String> = (0..50)
                .map(|k| all[(start + k) % all.len()].clone())
                .collect();
            let mut want = vec!["q060".to_string()];
            want.extend(turn.into_iter().filter(|g| g != "q060"));
            want_all.push(want);
            start = (start + 50) % all.len();
        }
        for want in want_all {
            f.clear_calls();
            assert!(!changed(&f, dir.path(), f.now()).unwrap());
            assert_eq!(probes(&f), want);
        }
        assert_eq!(load_state(dir.path()).probe_next, 150 % 121);
        let q = &calls_with(&f, "GET tasks?project=")[0];
        let cursor = load_state(dir.path()).projects["q060"]
            .cursor
            .clone()
            .unwrap();
        assert_eq!(
            q,
            &format!("GET tasks?project=q060&modified_since={cursor}&limit=1&opt_fields=gid")
        );
    }

    #[test]
    fn changed_is_false_when_nothing_moved() {
        let f = base();
        f.task(Task::new("1", "글").in_project("p1"));
        f.task(Task::new("m", "내 일").workspace("w1").assignee_me());
        let dir = d();
        full(&f, dir.path());
        f.advance(3600);
        // 커서가 15분 겹쳐서 받은 직후의 확인은 방금 받은 것도 걸려요. 한 번 더 받으면 커서가 넘어가요.
        assert!(changed(&f, dir.path(), f.now()).unwrap());
        full(&f, dir.path());
        f.advance(3600);
        f.clear_calls();
        assert!(!changed(&f, dir.path(), f.now()).unwrap());
        assert_eq!(f.calls().len(), 2, "{:#?}", f.calls());
        assert!(
            f.calls()
                .iter()
                .all(|c| c.contains("limit=1&opt_fields=gid"))
        );
        f.touch("m");
        assert!(changed(&f, dir.path(), f.now()).unwrap());
        full(&f, dir.path());
        f.advance(3600);
        full(&f, dir.path());
        f.advance(3600);
        assert!(!changed(&f, dir.path(), f.now()).unwrap());
        f.touch("1");
        assert!(changed(&f, dir.path(), f.now()).unwrap());
        // 재조정할 때가 되면 묻지 않고 바뀜이에요.
        full(&f, dir.path());
        f.clear_calls();
        assert!(changed(&f, dir.path(), f.now() + Duration::days(8)).unwrap());
        assert!(f.calls().is_empty());
    }

    /// `Date` 머리가 없는 답이에요(커서가 생기지 않아요).
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
    fn changed_is_not_stuck_on_a_project_without_a_cursor() {
        let f = base();
        f.project("gone", "w1", "곧 지울");
        f.task(Task::new("1", "글").in_project("p1"));
        let dir = d();
        run_with(&NoDate(&f), dir.path(), f.now(), 10_000).unwrap();
        f.advance(3600);
        run_with(&NoDate(&f), dir.path(), f.now(), 10_000).unwrap();
        let s = load_state(dir.path());
        assert!(s.projects.values().all(|p| p.cursor.is_none()));
        f.remove_project("gone");
        f.advance(60);
        f.clear_calls();
        assert!(!changed(&f, dir.path(), f.now()).unwrap());
        // 목록을 끝낸 시각 − 15분으로 물어요.
        let listed = s.projects["p1"].listed_at.clone().unwrap();
        let since = iso(parse(&listed).unwrap() - Duration::minutes(15));
        assert!(
            calls_with(&f, "project=p1&")[0].contains(&format!("modified_since={since}")),
            "{:#?}",
            f.calls()
        );
        f.touch("1");
        assert!(changed(&f, dir.path(), f.now()).unwrap());
    }

    #[test]
    fn changed_is_true_before_the_first_sync_finishes() {
        let f = base();
        f.task(Task::new("1", "글").in_project("p1"));
        let dir = d();
        assert!(changed(&f, dir.path(), f.now()).unwrap());
        assert!(run_with(&f, dir.path(), f.now(), 2).unwrap().more);
        assert!(changed(&f, dir.path(), f.now()).unwrap());
        assert!(
            f.calls()
                .iter()
                .all(|c| !c.contains("limit=1&opt_fields=gid"))
        );
    }

    /// 최근에 바뀐 프로젝트가 많아도 30분 확인 한 번은 90번(+ 워크스페이스마다 하나)을 넘지 않고, 다음 확인이 다음
    /// 40개를 물어요.
    #[test]
    fn changed_caps_hot_projects_and_rotates() {
        let f = base();
        f.task(Task::new("m", "내 일").workspace("w1").assignee_me());
        for i in 0..300 {
            let p = format!("h{i:03}");
            f.project(&p, "w1", "프로젝트");
            f.task(Task::new(&format!("t{i}"), "글").in_project(&p));
        }
        let dir = d();
        full(&f, dir.path());
        f.advance(3600);
        full(&f, dir.path());
        f.advance(60);
        let s = load_state(dir.path());
        let hot = s
            .projects
            .values()
            .filter(|p| p.changed_at.is_some())
            .count();
        assert_eq!(hot, 300);
        f.clear_calls();
        assert!(!changed(&f, dir.path(), f.now()).unwrap());
        let calls = f.calls();
        assert!(calls.len() <= 90 + 1, "{}", calls.len());
        assert_eq!(calls_with(&f, "assignee=me").len(), 1);
        let first = probes(&f);
        assert_eq!(first[..40], (0..40).map(|i| format!("h{i:03}")).collect::<Vec<_>>()[..]);
        assert_eq!(load_state(dir.path()).hot_next, 40);
        f.clear_calls();
        assert!(!changed(&f, dir.path(), f.now()).unwrap());
        assert!(f.calls().len() <= 91);
        assert_eq!(probes(&f)[0], "h040");
        // 41번째 최근 프로젝트(h045)의 변경은 첫 확인이 묻지 않고(돌아가며 묻는 자리도 비껴요), 다음 확인이 찾아요.
        let mut st = load_state(dir.path());
        st.hot_next = 0;
        st.probe_next = 200;
        save_state(dir.path(), &st).unwrap();
        f.touch("t45");
        f.clear_calls();
        assert!(!changed(&f, dir.path(), f.now()).unwrap());
        assert!(!probes(&f).iter().any(|p| p == "h045"));
        assert!(changed(&f, dir.path(), f.now()).unwrap());
    }

    /// 한 번에 오래 걸리는 답이에요.
    struct Slow<'a>(&'a FakeAsana, std::time::Duration);

    impl AsanaApi for Slow<'_> {
        fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply, SourceError> {
            std::thread::sleep(self.1);
            self.0.get(path, query)
        }
        fn batch(&self, gets: &[String]) -> Result<Vec<Reply>, SourceError> {
            self.0.batch(gets)
        }
    }

    /// 시간 끝이에요(부르는 쪽이 한도를 기다릴 수 없어요).
    struct OutOfTime;

    impl AsanaApi for OutOfTime {
        fn get(&self, _: &str, _: &[(&str, String)]) -> Result<Reply, SourceError> {
            Err(SourceError::service(Kind::Busy, "asana: out of time"))
        }
        fn batch(&self, _: &[String]) -> Result<Vec<Reply>, SourceError> {
            Err(SourceError::service(Kind::Busy, "asana: out of time"))
        }
    }

    /// 시간이 다 되면 거기까지만 묻고 거짓이에요. 다음 확인은 이어서 물어요.
    #[test]
    fn changed_stops_at_the_time_cap_and_carries_on() {
        let f = base();
        for i in 0..120 {
            f.project(&format!("q{i:03}"), "w1", "프로젝트");
        }
        let dir = d();
        full(&f, dir.path());
        f.advance(3600);
        full(&f, dir.path());
        f.advance(60);
        f.clear_calls();
        let slow = Slow(&f, std::time::Duration::from_millis(20));
        let started = std::time::Instant::now();
        // 0.4 s: room for a slow CI runner to ask at least one project (0.1 s asked none on GitHub's macOS runner,
        // 2026-11-26); 50 asks at 20 ms would need a full second, so the upper bound below still means something
        let got = changed_within(&slow, dir.path(), f.now(), std::time::Duration::from_millis(400));
        assert!(!got.unwrap());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        let asked = probes(&f).len();
        assert!((1..50).contains(&asked), "{asked}");
        assert_eq!(load_state(dir.path()).probe_next, asked);
        // 시간 끝 `Busy`는 오류가 아니라 여기까지예요. 자리는 그대로예요.
        assert!(!changed(&OutOfTime, dir.path(), f.now()).unwrap());
        assert_eq!(load_state(dir.path()).probe_next, asked);
        f.clear_calls();
        assert!(!changed(&f, dir.path(), f.now()).unwrap());
        assert_eq!(probes(&f)[0], format!("q{:03}", asked - 1), "{:?}", f.calls());
    }
}
