//! Reconcile (every 7 days) and the 30-minute check (docs/31 1-5절, 6절, 10-2절. 10-2 wins).
//!
//! **Reconcile** (`reconcile`): runs every 7 days (`RECONCILE_DAYS`) after the initial fetch finishes. Right after the
//! initial fetch the raw data is fresh, so it does not ask and records that moment as the first reconcile. It runs
//! within the hourly budget (`Points`); when short, it saves where to resume (`reconcile_next`,
//! `<단계>:<마지막으로 끝낸 id>`) and returns `Later`. `reconciled_at` is written only when everything is done.
//! The stages, in order:
//! 1. `jira`: asks about saved issue ids with `issue/bulkfetch` (`fields: ["updated"]`, 1,000 at a time).
//! 2. `pages`/`blogs`: asks about saved page and blog ids with `pages?id=…`/`blogposts?id=…` (250 at a time).
//! 3. `comments`: for every held item, refetches that item's comment lists (`pages/<id>/footer-comments`,
//!    `pages/<id>/inline-comments`, `blogposts/<id>/footer-comments`, `blogposts/<id>/inline-comments`) from start to
//!    end and replaces the item's comments wholesale (deleted comments drop out). Lists attached to an item are taken
//!    to return only top-level comments, so for comments that had replies in the saved copy (comments pointed to by a
//!    `parentCommentId`) it fetches more replies through `footer-comments/<id>/children`/
//!    `inline-comments/<id>/children` (replies to replies too). New replies not in the saved copy are picked up by the
//!    change fetch (the site-wide lists).
//!
//! - **Deleting missing items**: each sent id that does not come back adds one to `gone[<file name>]`. After a stage
//!   has been asked to the end, if fewer than 5 are missing or 20% or less of the saved copy, they are deleted at once.
//!   If more than 20% (5 or more) go missing at once, it does not delete then; an id missing twice in a row is deleted
//!   even above 20% (losing access to one big project crosses 20% every time). A returning id goes back to 0. Counts
//!   for turned-off products are cleared.
//! - Comment-only files (`page: null`, new comments on items older than 2 years) are deleted at once if their item
//!   does not come back. They are also deleted if no comments remain after refetching comments.
//! - Items older than 2 years are deleted (issues by `updated`, items by modified time, comments by each comment's
//!   modified time).
//! - Muted projects and spaces are reconciled too (they are left out only when building chunks). Turned-off products
//!   skip their stage.
//! - Ledger: 1 per request + 1 per returned issue, item or comment.
//!
//! **30-minute check** (`changed`): writes nothing and only asks whether anything changed (outside the budget, not
//! recorded in the ledger).
//! - Jira: asks `updated > "<head 분>" AND project not in (<끈 프로젝트>) ORDER BY updated DESC` with
//!   `maxResults: 1`, `fields: updated`. JQL times are per minute, so the `head` issue itself can match; the returned
//!   issue's `updated` is compared with `head` down to milliseconds.
//! - Confluence: for each of the four lists (pages, blog posts, footer comments, inline comments), compares the
//!   modified time of the first entry of `sort=-modified-date&limit=1` with that list's `head`.
//! - If any product has an unfinished initial fetch, it does not ask and answers "changed" (there is more to fetch).
//!   If paused by the limit, it answers "not changed".
//!
//! Content (titles, bodies) is never put in logs or errors.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use crate::atlassian::api::AtlassianApi;
use crate::atlassian::confluence::{comments_of, modified, put, sid};
use crate::atlassian::jira::{jql_minute, parse_updated};
use crate::atlassian::state::{
    Run, State, Step, WINDOW_DAYS, iso, item_ids, json_of, load_state, read_item, state_lock,
};
use crate::error::{Kind, SourceError};
use crate::gdrive::api::Got;
use crate::gdrive::fetch::{parse, safe};

/// 재조정 간격(일)이에요.
pub(crate) const RECONCILE_DAYS: i64 = 7;
/// `bulkfetch` 한 번의 id 수예요.
const JIRA_BATCH: usize = 1_000;
/// `pages?id=`·`blogposts?id=` 한 번의 id 수예요.
const WIKI_BATCH: usize = 250;
/// 글에 딸린 댓글 목록 한 쪽의 수예요.
const COMMENT_PAGE: usize = 100;
/// 글 하나의 댓글을 다시 받기 전에 남아 있어야 할 포인트예요: 목록 둘 × (요청 1 + 댓글 한 쪽).
const COMMENT_RESERVE: u32 = 2 * (1 + COMMENT_PAGE as u32);
/// 두 번 연달아 빠지면 지워요.
const STRIKES: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Jira,
    Pages,
    Blogs,
    Comments,
}

const PHASES: [Phase; 4] = [Phase::Jira, Phase::Pages, Phase::Blogs, Phase::Comments];

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Jira => "jira",
            Phase::Pages => "pages",
            Phase::Blogs => "blogs",
            Phase::Comments => "comments",
        }
    }

    fn of(name: &str) -> Option<Phase> {
        PHASES.into_iter().find(|p| p.name() == name)
    }

    /// 이 단계가 다루는 글 파일 종류예요.
    fn file_kind(self) -> &'static str {
        if self == Phase::Jira { "jira" } else { "page" }
    }

    fn on(self, s: &State) -> bool {
        match self {
            Phase::Jira => s.products.jira,
            _ => s.products.confluence,
        }
    }
}

/// 처음 받기를 끝냈는지예요(켜진 제품마다). 켜진 제품이 없으면 거짓이에요.
fn first_fetch_done(s: &State) -> bool {
    let p = s.products;
    if !p.jira && !p.confluence {
        return false;
    }
    (!p.jira || s.jira.done)
        && (!p.confluence
            || [&s.pages, &s.blogs, &s.footer, &s.inline]
                .iter()
                .all(|c| c.done))
}

/// 재조정할 때가 됐는지예요(이어 할 자리가 있거나 지난 재조정에서 7일이 지났어요).
pub(crate) fn reconcile_due(s: &State, now: DateTime<Utc>) -> bool {
    if !first_fetch_done(s) {
        return false;
    }
    if s.reconcile_next.is_some() {
        return true;
    }
    match s.reconciled_at.as_deref().and_then(parse) {
        Some(t) => now - t >= Duration::days(RECONCILE_DAYS),
        // 처음 받기를 막 끝냈어요: `reconcile`이 그때를 첫 재조정으로 적어요.
        None => true,
    }
}

/// 재조정이에요. 때가 아니면 아무것도 하지 않아요.
pub(crate) fn reconcile(run: &mut Run) -> Result<Step, SourceError> {
    if !reconcile_due(&run.state, run.now) {
        return Ok(Step::Done);
    }
    if run.state.reconcile_next.is_none() && run.state.reconciled_at.is_none() {
        // 처음 받기가 막 끝나 원본이 새것이에요. 그때를 첫 재조정으로 적어요(묻지 않아요).
        run.state.reconciled_at = Some(iso(run.now));
        run.save_upkeep()?;
        return Ok(Step::Done);
    }
    if run.paused() {
        return Ok(Step::Later);
    }
    match reconcile_all(run) {
        Err(e) => run.pause_on_quota(e),
        ok => ok,
    }
}

fn reconcile_all(run: &mut Run) -> Result<Step, SourceError> {
    let (start, after) = match run.state.reconcile_next.as_deref() {
        Some(at) => {
            let (p, a) = at.split_once(':').unwrap_or((at, ""));
            (Phase::of(p).unwrap_or(Phase::Jira), a.to_string())
        }
        None => {
            forget_stale_strikes(run);
            (Phase::Jira, String::new())
        }
    };
    for phase in PHASES.into_iter().skip_while(|p| *p != start) {
        let after = if phase == start { after.as_str() } else { "" };
        if !phase.on(&run.state) {
            // 꺼진 제품의 빠짐 횟수는 이어 세지 않아요.
            let prefix = format!("{}-", phase.file_kind());
            if phase != Phase::Comments {
                run.state.gone.retain(|k, _| !k.starts_with(&prefix));
            }
            continue;
        }
        let step = match phase {
            Phase::Comments => comments_phase(run, after)?,
            _ => ids_phase(run, phase, after)?,
        };
        if step == Step::Later {
            return Ok(Step::Later);
        }
    }
    run.state.reconcile_next = None;
    run.state.reconciled_at = Some(iso(run.now));
    run.save_upkeep()?;
    Ok(Step::Done)
}

/// 파일이 없는 글의 빠짐 횟수를 지워요(다른 길로 지운 글이에요).
fn forget_stale_strikes(run: &mut Run) {
    let held: HashSet<String> = ["jira", "page"]
        .iter()
        .flat_map(|k| {
            item_ids(run.dir, k)
                .into_iter()
                .map(move |id| format!("{k}-{id}"))
        })
        .collect();
    run.state.gone.retain(|k, _| held.contains(k));
}

/// 단계가 다루는 저장된 id예요(정렬). 페이지·블로그 단계는 파일의 `kind`로 나눠요.
fn held(run: &Run, phase: Phase) -> Vec<String> {
    let ids = item_ids(run.dir, phase.file_kind());
    let want = match phase {
        Phase::Pages => "page",
        Phase::Blogs => "blogpost",
        Phase::Jira | Phase::Comments => return ids,
    };
    ids.into_iter()
        .filter(|id| {
            read_item(run.dir, "page", id)
                .is_some_and(|f| f["kind"].as_str().unwrap_or("page") == want)
        })
        .collect()
}

/// 저장한 id를 물어 돌아온 id와 그 수정 시각이에요. 장부에 적어요.
fn ask(
    run: &mut Run,
    phase: Phase,
    ids: &[String],
) -> Result<HashMap<String, Option<DateTime<Utc>>>, SourceError> {
    run.guard()?;
    let mut out = HashMap::new();
    if phase == Phase::Jira {
        let path = format!("ex/jira/{}/rest/api/3/issue/bulkfetch", run.cloud);
        let a = run.api.post(
            &path,
            &json!({"issueIdsOrKeys": ids, "fields": ["updated"]}),
        )?;
        if a.status != 200 {
            return Err(SourceError::service(
                Kind::Down,
                format!("atlassian: jira bulkfetch failed {}", a.status),
            ));
        }
        let v: Value = serde_json::from_slice(&a.body).map_err(|_| {
            SourceError::service(Kind::Down, "atlassian: jira bulkfetch is not JSON")
        })?;
        let issues = v["issues"].as_array().cloned().unwrap_or_default();
        run.state.points.charge(
            run.now,
            1,
            u32::try_from(issues.len()).unwrap_or(u32::MAX),
            0,
        );
        for i in issues {
            if let Some(id) = i["id"].as_str() {
                let t = i["fields"]["updated"]
                    .as_str()
                    .and_then(parse_updated)
                    .map(|t| t.with_timezone(&Utc));
                out.insert(id.to_string(), t);
            }
        }
        return Ok(out);
    }
    let (list, status) = if phase == Phase::Pages {
        ("pages", "current,archived")
    } else {
        ("blogposts", "current")
    };
    let path = format!("ex/confluence/{}/wiki/api/v2/{list}", run.cloud);
    let mut cursor: Option<String> = None;
    loop {
        let mut q = vec![
            ("id", ids.join(",")),
            ("status", status.to_string()),
            ("limit", WIKI_BATCH.to_string()),
        ];
        if let Some(c) = &cursor {
            q.push(("cursor", c.clone()));
        }
        let v = json_of(run.api.get(&path, &q)?, "confluence id check")?;
        let items = v["results"].as_array().cloned().unwrap_or_default();
        run.state.points.charge(
            run.now,
            1,
            u32::try_from(items.len()).unwrap_or(u32::MAX),
            0,
        );
        for item in &items {
            if let Some(id) = sid(&item["id"]) {
                out.insert(id, modified(item));
            }
        }
        match v["_links"]["next"].as_str().and_then(next_cursor) {
            Some(c) if !items.is_empty() => cursor = Some(c),
            _ => return Ok(out),
        }
    }
}

fn next_cursor(link: &str) -> Option<String> {
    let (_, query) = link.split_once('?')?;
    form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "cursor")
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
}

/// 1–2단계예요: 저장한 id를 묶음마다 물어 빠진 것을 세고, 2년보다 오래된 것을 지워요. 단계를 끝까지 물은 뒤
/// 빠진 것을 지워요(`settle`).
fn ids_phase(run: &mut Run, phase: Phase, after: &str) -> Result<Step, SourceError> {
    let batch = if phase == Phase::Jira {
        JIRA_BATCH
    } else {
        WIKI_BATCH
    };
    let edge = run.now - Duration::days(WINDOW_DAYS);
    let kind = phase.file_kind();
    let todo: Vec<String> = held(run, phase)
        .into_iter()
        .filter(|id| id.as_str() > after)
        .collect();
    let total = todo.len();
    for (n, chunk) in todo.chunks(batch).enumerate() {
        let need = 1 + u32::try_from(chunk.len()).unwrap_or(u32::MAX);
        if run.state.points.left(run.now) < need {
            run.save_upkeep()?;
            return Ok(Step::Later);
        }
        let found = ask(run, phase, chunk)?;
        for id in chunk {
            let key = format!("{kind}-{id}");
            match found.get(id) {
                Some(t) => {
                    run.state.gone.remove(&key);
                    // 돌아온 글이 2년보다 오래됐으면 지워요. 댓글만 든 파일은 댓글 단계가 댓글 시각으로 봐요.
                    let orphan = phase != Phase::Jira && orphan(run, id);
                    if !orphan && t.is_some_and(|t| t < edge) {
                        run.remove(kind, id)?;
                    }
                }
                None if phase != Phase::Jira && orphan(run, id) => {
                    run.state.gone.remove(&key);
                    run.remove(kind, id)?;
                }
                None => {
                    let strikes = run.state.gone.entry(key).or_insert(0);
                    *strikes = strikes.saturating_add(1);
                }
            }
        }
        let last = chunk.last().map(String::as_str).unwrap_or(after);
        run.state.reconcile_next = Some(format!("{}:{last}", phase.name()));
        (run.progress)((n * batch + chunk.len()).min(total), total);
        run.save_upkeep()?;
    }
    settle(run, phase)?;
    Ok(Step::Done)
}

/// 단계를 끝까지 물은 뒤 빠진 것을 지워요. 이 단계의 저장본 중 `gone`에 남은 id가 이번에 빠진 것이에요(돌아온 id는
/// 지웠고, 지난 재조정의 남은 횟수는 이번에 다시 물어 올리거나 지웠어요). 적게 빠졌으면(5개보다 적거나 20% 이하)
/// 바로, 많이 빠졌으면 두 번 연달아 빠진 것만 지워요.
fn settle(run: &mut Run, phase: Phase) -> Result<(), SourceError> {
    let kind = phase.file_kind();
    let held = held(run, phase);
    let missed: Vec<(String, u8)> = held
        .iter()
        .filter_map(|id| {
            run.state
                .gone
                .get(&format!("{kind}-{id}"))
                .map(|n| (id.clone(), *n))
        })
        .collect();
    let small = missed.len() < 5 || missed.len() * 5 <= held.len();
    for (id, strikes) in missed {
        if small || strikes >= STRIKES {
            run.state.gone.remove(&format!("{kind}-{id}"));
            run.remove(kind, &id)?;
        }
    }
    run.save_upkeep()
}

/// 글 본문 없이 댓글만 든 파일인지예요.
fn orphan(run: &Run, id: &str) -> bool {
    read_item(run.dir, "page", id).is_some_and(|f| !f["page"].is_object())
}

/// 3단계예요: 가진 글마다 댓글을 처음부터 끝까지 다시 받아 바꿔 써요.
fn comments_phase(run: &mut Run, after: &str) -> Result<Step, SourceError> {
    let todo: Vec<String> = item_ids(run.dir, "page")
        .into_iter()
        .filter(|id| id.as_str() > after)
        .collect();
    let total = todo.len();
    for (n, id) in todo.iter().enumerate() {
        if run.state.points.left(run.now) < COMMENT_RESERVE {
            run.save_upkeep()?;
            return Ok(Step::Later);
        }
        relist(run, id)?;
        run.state.reconcile_next = Some(format!("{}:{id}", Phase::Comments.name()));
        (run.progress)(n + 1, total);
        run.save_upkeep()?;
    }
    Ok(Step::Done)
}

/// 댓글 목록 하나를 끝까지 받아요(장부: 요청 1 + 댓글마다 1). 404면 None이에요(그 사이 없어졌어요).
fn list_all(run: &mut Run, path: &str) -> Result<Option<Vec<Value>>, SourceError> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        run.guard()?;
        let mut q = vec![
            ("body-format", "atlas_doc_format".to_string()),
            ("limit", COMMENT_PAGE.to_string()),
        ];
        if let Some(c) = &cursor {
            q.push(("cursor", c.clone()));
        }
        let got = run.api.get(path, &q)?;
        if matches!(got, Got::Fail { status: 404, .. }) {
            run.state.points.charge(run.now, 1, 0, 0);
            return Ok(None);
        }
        let v = json_of(got, "confluence comments")?;
        let items = v["results"].as_array().cloned().unwrap_or_default();
        run.state.points.charge(
            run.now,
            1,
            u32::try_from(items.len()).unwrap_or(u32::MAX),
            0,
        );
        let more = !items.is_empty();
        out.extend(items);
        match v["_links"]["next"].as_str().and_then(next_cursor) {
            Some(c) if more => cursor = Some(c),
            _ => return Ok(Some(out)),
        }
    }
}

/// 글 하나의 댓글을 다시 받아 바꿔 써요. 그대로면 쓰지 않아요.
fn relist(run: &mut Run, id: &str) -> Result<(), SourceError> {
    let Some(file) = read_item(run.dir, "page", id) else {
        return Ok(());
    };
    let kind = file["kind"].as_str().unwrap_or("page").to_string();
    let base = if kind == "blogpost" {
        "blogposts"
    } else {
        "pages"
    };
    // 저장본에서 답글이 달려 있던 댓글이에요. 이 댓글들만 답글 목록을 더 물어요.
    let parents: HashSet<String> = comments_of(&file)
        .iter()
        .filter_map(|c| sid(&c["parentCommentId"]))
        .collect();
    let edge = run.now - Duration::days(WINDOW_DAYS);
    let mut comments = Vec::new();
    for (list, ty) in [("footer-comments", "footer"), ("inline-comments", "inline")] {
        let path = format!("ex/confluence/{}/wiki/api/v2/{base}/{id}/{list}", run.cloud);
        let Some(mut todo) = list_all(run, &path)? else {
            // 그 사이 글이 없어졌어요. 다음 재조정의 id 단계가 다뤄요.
            return Ok(());
        };
        let mut asked = HashSet::new();
        while let Some(c) = todo.pop() {
            if let Some(cid) = sid(&c["id"])
                && parents.contains(&cid)
                && safe(&cid)
                && asked.insert(cid.clone())
            {
                let path = format!(
                    "ex/confluence/{}/wiki/api/v2/{list}/{cid}/children",
                    run.cloud
                );
                todo.extend(list_all(run, &path)?.unwrap_or_default());
            }
            if modified(&c).is_some_and(|t| t < edge) {
                continue;
            }
            let mut entry = c.clone();
            entry["type"] = json!(ty);
            if ty == "inline" {
                entry["quote"] = c["properties"]["inlineOriginalSelection"].clone();
            }
            comments.push(entry);
        }
    }
    comments.sort_by_key(|x| (modified(x), sid(&x["id"])));
    // 글에 딸린 목록이 답글까지 준다면 답글 목록과 겹쳐요. 같은 id는 하나만 둬요.
    let mut ids = HashSet::new();
    comments.retain(|c| ids.insert((c["type"].clone(), sid(&c["id"]))));
    let page = file["page"].clone();
    if !page.is_object() && comments.is_empty() {
        run.state.gone.remove(&format!("page-{id}"));
        return run.remove("page", id);
    }
    if comments == comments_of(&file) {
        return Ok(());
    }
    put(run, id, &kind, page, comments)
}

/// 30분 확인이에요: 바뀐 게 있는지만 물어요. 아무것도 쓰지 않아요.
pub fn changed(
    api: &dyn AtlassianApi,
    raw: &Path,
    now: DateTime<Utc>,
) -> Result<bool, SourceError> {
    let s = {
        let _held = state_lock();
        load_state(raw)
    };
    let Some(cloud) = s.cloud.as_deref() else {
        return Ok(false);
    };
    if s.paused_until
        .as_deref()
        .and_then(parse)
        .is_some_and(|t| t > now)
    {
        return Ok(false);
    }
    let p = s.products;
    let confluence = [&s.pages, &s.blogs, &s.footer, &s.inline];
    if (p.jira && !s.jira.done) || (p.confluence && confluence.iter().any(|c| !c.done)) {
        return Ok(true);
    }
    if p.jira && jira_changed(api, cloud, &s)? {
        return Ok(true);
    }
    if p.confluence {
        let lists = [
            ("pages", Some("current,archived"), &s.pages),
            ("blogposts", Some("current"), &s.blogs),
            ("footer-comments", None, &s.footer),
            ("inline-comments", None, &s.inline),
        ];
        for (list, status, cursor) in lists {
            let mut q = vec![
                ("sort", "-modified-date".to_string()),
                ("limit", "1".to_string()),
            ];
            if let Some(st) = status {
                q.push(("status", st.to_string()));
            }
            let path = format!("ex/confluence/{cloud}/wiki/api/v2/{list}");
            let got = api.get(&path, &q)?;
            if matches!(got, Got::Fail { status: 404, .. }) {
                continue;
            }
            let v = json_of(got, "confluence check")?;
            let Some(first) = v["results"].as_array().and_then(|a| a.first()) else {
                continue;
            };
            let head = cursor.head.as_deref().and_then(parse);
            if modified(first).is_some_and(|t| head.is_none_or(|h| t > h)) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn jira_changed(api: &dyn AtlassianApi, cloud: &str, s: &State) -> Result<bool, SourceError> {
    let Some(head) = s.jira.head.as_deref().and_then(parse_updated) else {
        // 처음 받기가 빈 채로 끝났어요: 2년 안에 무엇이든 있으면 바뀜이에요.
        return any_issue(api, cloud, s, format!("updated >= -{WINDOW_DAYS}d"));
    };
    let jql = format!("updated > \"{}\"", jql_minute(head, s.time_zone.as_deref()));
    let path = format!("ex/jira/{cloud}/rest/api/3/search/jql");
    let v = json_of(api.get(&path, &search_query(s, jql))?, "jira check")?;
    let newest = v["issues"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|i| i["fields"]["updated"].as_str())
        .and_then(parse_updated);
    Ok(newest.is_some_and(|t| t > head))
}

fn any_issue(
    api: &dyn AtlassianApi,
    cloud: &str,
    s: &State,
    jql: String,
) -> Result<bool, SourceError> {
    let path = format!("ex/jira/{cloud}/rest/api/3/search/jql");
    let v = json_of(api.get(&path, &search_query(s, jql))?, "jira check")?;
    Ok(v["issues"].as_array().is_some_and(|a| !a.is_empty()))
}

/// 끈 프로젝트를 빼고 가장 최근 하나만 묻는 검색이에요.
fn search_query(s: &State, mut jql: String) -> Vec<(&'static str, String)> {
    if !s.muted_projects.is_empty() {
        let list: Vec<String> = s
            .muted_projects
            .iter()
            .map(|p| format!("\"{}\"", p.replace(['"', '\\'], "")))
            .collect();
        jql.push_str(&format!(" AND project not in ({})", list.join(", ")));
    }
    jql.push_str(" ORDER BY updated DESC");
    vec![
        ("jql", jql),
        ("maxResults", "1".to_string()),
        ("fields", "updated".to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::atlassian::api::{POINTS_PER_HOUR, Points};
    use crate::atlassian::state::{save_state, write_item};
    use crate::testing::atlassian::FakeAtlassian;

    fn go(
        f: &FakeAtlassian,
        dir: &Path,
        prep: impl FnOnce(&mut State),
    ) -> Result<Step, SourceError> {
        let mut state = load_state(dir);
        prep(&mut state);
        let mut progress = |_: usize, _: usize| {};
        let jitter = || 0;
        let still = || true;
        let mut run = Run {
            api: f,
            dir,
            cloud: "cloud-1",
            now: Utc::now(),
            still: &still,
            state,
            progress: &mut progress,
            jitter: &jitter,
            halted: None,
        };
        reconcile(&mut run)
    }

    fn stamp(t: DateTime<Utc>) -> String {
        t.format("%Y-%m-%dT%H:%M:%S%.3f+0000").to_string()
    }

    fn at(t: DateTime<Utc>) -> String {
        t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn recent() -> DateTime<Utc> {
        Utc::now() - Duration::days(1)
    }

    fn issue(id: &str) -> Value {
        json!({"id": id, "key": format!("A-{id}"), "fields": {"updated": stamp(recent()), "project": {"key": "A"}}})
    }

    /// 재조정할 때가 된 상태예요(처음 받기 끝, 지난 재조정 8일 전).
    fn due(dir: &Path, jira: bool, confluence: bool) {
        let mut s = State {
            cloud: Some("cloud-1".into()),
            ..State::default()
        };
        s.products.jira = jira;
        s.products.confluence = confluence;
        for c in [
            &mut s.jira,
            &mut s.pages,
            &mut s.blogs,
            &mut s.footer,
            &mut s.inline,
        ] {
            c.done = true;
        }
        s.reconciled_at = Some(iso(Utc::now() - Duration::days(8)));
        save_state(dir, &s).unwrap();
    }

    fn again(dir: &Path) {
        let mut s = load_state(dir);
        s.reconciled_at = Some(iso(Utc::now() - Duration::days(8)));
        save_state(dir, &s).unwrap();
    }

    /// Jira 이슈 `ids`를 가짜와 원본 둘 다에 둬요.
    fn jira(f: &FakeAtlassian, dir: &Path, ids: &[&str]) {
        for id in ids {
            f.jira.lock().unwrap().push(issue(id));
            write_item(
                dir,
                "jira",
                id,
                &json!({"id": id, "fields": {"updated": stamp(recent())}}),
            )
            .unwrap();
        }
    }

    fn drop_issues(f: &FakeAtlassian, ids: &[&str]) {
        f.jira
            .lock()
            .unwrap()
            .retain(|i| !ids.contains(&i["id"].as_str().unwrap()));
    }

    /// `n`개의 이슈 `"10"`, `"11"`, …예요.
    fn many(f: &FakeAtlassian, dir: &Path, n: u32) -> Vec<String> {
        let ids: Vec<String> = (10..10 + n).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        jira(f, dir, &refs);
        ids
    }

    #[test]
    fn a_small_miss_deletes_on_first_sight() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), true, false);
        jira(&f, d.path(), &["1", "2", "3"]);
        drop_issues(&f, &["2"]);
        assert_eq!(go(&f, d.path(), |_| {}).unwrap(), Step::Done);
        // 5개보다 적게 빠졌어요(3개 중 1개, 33%여도요).
        assert_eq!(item_ids(d.path(), "jira"), ["1", "3"]);
        let s = load_state(d.path());
        assert!(s.gone.is_empty());
        assert!(s.reconcile_next.is_none());
        // 다 끝냈으니 재조정 시각이 지금이에요.
        assert!(!reconcile_due(&s, Utc::now()));
        // 요청 1 + 돌아온 이슈 2개예요.
        assert_eq!(s.points.used, 3);

        // 30개 중 6개(20%)도 바로 지워요.
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), true, false);
        many(&f, d.path(), 30);
        drop_issues(&f, &["10", "11", "12", "13", "14", "15"]);
        go(&f, d.path(), |_| {}).unwrap();
        assert_eq!(item_ids(d.path(), "jira").len(), 24);
    }

    #[test]
    fn reconcile_deletes_after_two_misses() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), true, false);
        many(&f, d.path(), 10);
        // 10개 중 5개(50%, 5개 이상)가 빠져요: 처음에는 두고 세기만 해요.
        drop_issues(&f, &["10", "11", "12", "13", "14"]);
        assert_eq!(go(&f, d.path(), |_| {}).unwrap(), Step::Done);
        assert_eq!(item_ids(d.path(), "jira").len(), 10);
        let s = load_state(d.path());
        assert_eq!(s.gone.get("jira-10"), Some(&1));
        assert_eq!(s.gone.len(), 5);

        again(d.path());
        assert_eq!(go(&f, d.path(), |_| {}).unwrap(), Step::Done);
        assert_eq!(item_ids(d.path(), "jira"), ["15", "16", "17", "18", "19"]);
        assert!(load_state(d.path()).gone.is_empty());
    }

    #[test]
    fn an_issue_that_comes_back_starts_over() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), true, false);
        many(&f, d.path(), 10);
        let big = ["10", "11", "12", "13", "14"];
        drop_issues(&f, &big);
        go(&f, d.path(), |_| {}).unwrap();
        for id in big {
            f.jira.lock().unwrap().push(issue(id));
        }
        again(d.path());
        go(&f, d.path(), |_| {}).unwrap();
        assert!(load_state(d.path()).gone.is_empty());
        drop_issues(&f, &big);
        again(d.path());
        go(&f, d.path(), |_| {}).unwrap();
        // 연달아가 아니라서 아직 남아요.
        assert_eq!(item_ids(d.path(), "jira").len(), 10);
    }

    #[test]
    fn a_product_that_is_off_forgets_its_strikes() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), true, true);
        many(&f, d.path(), 10);
        drop_issues(&f, &["10", "11", "12", "13", "14"]);
        go(&f, d.path(), |_| {}).unwrap();
        assert_eq!(load_state(d.path()).gone.len(), 5);
        let mut s = load_state(d.path());
        s.products.jira = false;
        s.reconciled_at = Some(iso(Utc::now() - Duration::days(8)));
        save_state(d.path(), &s).unwrap();
        go(&f, d.path(), |_| {}).unwrap();
        assert!(load_state(d.path()).gone.is_empty());
        assert_eq!(item_ids(d.path(), "jira").len(), 10);
    }

    #[test]
    fn reconcile_guard_holds_on_a_first_big_miss_only() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), true, false);
        let ids: Vec<String> = (10..30).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        jira(&f, d.path(), &refs);
        // 20개 중 6개(30%, 20%와 최소 5개를 둘 다 넘어요)가 한꺼번에 빠져요.
        drop_issues(&f, &["10", "11", "12", "13", "14", "15"]);
        go(&f, d.path(), |_| {}).unwrap();
        assert_eq!(item_ids(d.path(), "jira").len(), 20);
        assert_eq!(load_state(d.path()).gone.len(), 6);
        again(d.path());
        go(&f, d.path(), |_| {}).unwrap();
        assert_eq!(item_ids(d.path(), "jira").len(), 14);
        assert!(!item_ids(d.path(), "jira").contains(&"10".to_string()));
    }

    #[test]
    fn the_first_reconcile_after_the_first_fetch_only_marks_the_time() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), true, false);
        let mut s = load_state(d.path());
        s.reconciled_at = None;
        save_state(d.path(), &s).unwrap();
        jira(&f, d.path(), &["1"]);
        drop_issues(&f, &["1"]);
        assert_eq!(go(&f, d.path(), |_| {}).unwrap(), Step::Done);
        assert!(f.calls().is_empty());
        assert!(load_state(d.path()).reconciled_at.is_some());
        // 처음 받기가 안 끝났으면 하지 않아요.
        let mut s = load_state(d.path());
        s.jira.done = false;
        s.reconciled_at = Some(iso(Utc::now() - Duration::days(30)));
        save_state(d.path(), &s).unwrap();
        assert_eq!(go(&f, d.path(), |_| {}).unwrap(), Step::Done);
        assert!(f.calls().is_empty());
    }

    #[test]
    fn items_older_than_two_years_are_deleted() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), true, false);
        jira(&f, d.path(), &["1", "2"]);
        f.jira.lock().unwrap()[1]["fields"]["updated"] =
            json!(stamp(Utc::now() - Duration::days(800)));
        go(&f, d.path(), |_| {}).unwrap();
        assert_eq!(item_ids(d.path(), "jira"), ["1"]);
    }

    fn adf(text: &str) -> Value {
        json!({"representation": "atlas_doc_format", "value": json!({
            "type": "doc", "version": 1,
            "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]
        }).to_string()})
    }

    fn page(id: &str) -> Value {
        json!({"id": id, "status": "current", "version": {"number": 1, "createdAt": at(recent())},
               "body": {"atlas_doc_format": adf("body")}})
    }

    fn comment(id: &str, pid: &str) -> Value {
        json!({"id": id, "status": "current", "pageId": pid,
               "version": {"number": 1, "createdAt": at(recent()), "authorId": "u1"},
               "body": {"atlas_doc_format": adf("comment")}})
    }

    fn held_page(dir: &Path, id: &str, kind: &str, comments: Vec<Value>) {
        write_item(
            dir,
            "page",
            id,
            &json!({"id": id, "kind": kind, "page": page(id), "comments": comments, "accounts": []}),
        )
        .unwrap();
    }

    fn comment_ids(dir: &Path, id: &str) -> Vec<String> {
        read_item(dir, "page", id).unwrap()["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn pages_and_blogs_lose_missing_ids_the_same_way() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), false, true);
        *f.pages.lock().unwrap() = vec![page("1")];
        *f.blogs.lock().unwrap() = vec![page("5")];
        held_page(d.path(), "1", "page", vec![]);
        held_page(d.path(), "2", "page", vec![]);
        held_page(d.path(), "5", "blogpost", vec![]);
        held_page(d.path(), "6", "blogpost", vec![]);
        // 적게 빠져서 처음에 바로 지워요.
        go(&f, d.path(), |_| {}).unwrap();
        assert_eq!(item_ids(d.path(), "page"), ["1", "5"]);
        let calls = f.calls();
        assert!(calls.iter().any(|c| c.ends_with("wiki/api/v2/pages")));
        assert!(calls.iter().any(|c| c.ends_with("wiki/api/v2/blogposts")));
    }

    #[test]
    fn a_comment_only_file_goes_when_its_page_does_not_come_back() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), false, true);
        *f.pages.lock().unwrap() = vec![page("1")];
        // 2는 받지 않은 옛 글에 달린 댓글만 든 파일이에요. 그 글이 돌아오지 않아요.
        write_item(
            d.path(),
            "page",
            "2",
            &json!({"id": "2", "kind": "page", "page": null, "comments": [comment("c9", "2")]}),
        )
        .unwrap();
        held_page(d.path(), "1", "page", vec![]);
        go(&f, d.path(), |_| {}).unwrap();
        assert_eq!(item_ids(d.path(), "page"), ["1"]);
    }

    #[test]
    fn reconcile_relists_each_pages_comments_and_drops_deleted_ones() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), false, true);
        *f.pages.lock().unwrap() = vec![page("1")];
        *f.blogs.lock().unwrap() = vec![page("5")];
        let mut inline = comment("c2", "1");
        inline["properties"] = json!({"inlineOriginalSelection": "quoted"});
        *f.footer.lock().unwrap() = vec![comment("c1", "1"), {
            let mut c = comment("c5", "x");
            c.as_object_mut().unwrap().remove("pageId");
            c["blogPostId"] = json!("5");
            c
        }];
        *f.inline.lock().unwrap() = vec![inline];
        let stale =
            json!({"id": "c-deleted", "type": "footer", "version": {"createdAt": at(recent())}});
        held_page(d.path(), "1", "page", vec![stale.clone()]);
        held_page(d.path(), "5", "blogpost", vec![stale]);

        assert_eq!(go(&f, d.path(), |_| {}).unwrap(), Step::Done);
        let mut got = comment_ids(d.path(), "1");
        got.sort();
        assert_eq!(got, ["c1", "c2"]);
        assert_eq!(comment_ids(d.path(), "5"), ["c5"]);
        let p = read_item(d.path(), "page", "1").unwrap();
        let inline = p["comments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "c2")
            .unwrap();
        assert_eq!(inline["type"], "inline");
        assert_eq!(inline["quote"], "quoted");
        assert_eq!(p["accounts"], json!(["u1"]));
        let calls = f.calls();
        assert!(calls.iter().any(|c| c.ends_with("pages/1/footer-comments")));
        assert!(calls.iter().any(|c| c.ends_with("pages/1/inline-comments")));
        assert!(
            calls
                .iter()
                .any(|c| c.ends_with("blogposts/5/footer-comments"))
        );
        assert!(
            calls
                .iter()
                .any(|c| c.ends_with("blogposts/5/inline-comments"))
        );
    }

    #[test]
    fn a_reconcile_over_budget_resumes_next_run() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), false, true);
        let ids: Vec<String> = (10..14).map(|i| i.to_string()).collect();
        *f.pages.lock().unwrap() = ids.iter().map(|i| page(i)).collect();
        for id in &ids {
            held_page(d.path(), id, "page", vec![]);
        }
        let before = load_state(d.path()).reconciled_at;
        // id 확인(요청 1 + 글 4)과 글 둘의 댓글(댓글이 없어 글마다 요청 2)만 받을 만큼이에요. 셋째 글 앞에서
        // 남은 포인트가 `COMMENT_RESERVE`보다 적어요.
        let hour = Utc::now().timestamp() / 3600;
        let used = POINTS_PER_HOUR - (COMMENT_RESERVE + 8);
        assert_eq!(
            go(&f, d.path(), |s| s.points = Points { used, hour }).unwrap(),
            Step::Later
        );
        let s = load_state(d.path());
        assert_eq!(s.reconciled_at, before);
        assert_eq!(s.reconcile_next.as_deref(), Some("comments:11"));

        f.log.lock().unwrap().clear();
        assert_eq!(
            go(&f, d.path(), |s| s.points = Points::default()).unwrap(),
            Step::Done
        );
        let s = load_state(d.path());
        assert!(s.reconcile_next.is_none());
        assert_ne!(s.reconciled_at, before);
        // 이어서 남은 글 둘의 댓글만 받아요(id 확인을 다시 하지 않아요).
        let calls = f.calls();
        assert!(!calls.iter().any(|c| c.ends_with("wiki/api/v2/pages")));
        assert!(
            calls
                .iter()
                .any(|c| c.ends_with("pages/12/footer-comments"))
        );
        assert!(!calls.iter().any(|c| c.contains("pages/10/")));
    }

    /// 30분 확인용 상태예요: 처음 받기 끝, 커서는 가짜의 가장 최근 것과 같아요.
    fn checked(dir: &Path, f: &FakeAtlassian) {
        let t = recent();
        let mut a = issue("1");
        a["fields"]["updated"] = json!(stamp(t));
        f.jira.lock().unwrap().push(a);
        let mut b = issue("2");
        b["key"] = json!("B-2");
        b["fields"]["project"] = json!({"key": "B"});
        b["fields"]["updated"] = json!(stamp(t - Duration::hours(1)));
        f.jira.lock().unwrap().push(b);
        let mut p = page("1");
        p["version"]["createdAt"] = json!(at(t));
        *f.pages.lock().unwrap() = vec![p];
        let mut c = comment("c1", "1");
        c["version"]["createdAt"] = json!(at(t));
        *f.footer.lock().unwrap() = vec![c];
        let mut s = State {
            cloud: Some("cloud-1".into()),
            ..State::default()
        };
        s.products.jira = true;
        s.products.confluence = true;
        s.time_zone = Some("UTC".into());
        for c in [
            &mut s.jira,
            &mut s.pages,
            &mut s.blogs,
            &mut s.footer,
            &mut s.inline,
        ] {
            c.done = true;
        }
        s.jira.head = Some(stamp(t));
        s.pages.head = Some(at(t));
        s.footer.head = Some(at(t));
        s.muted_projects = vec!["B".into()];
        save_state(dir, &s).unwrap();
    }

    #[test]
    fn check_ignores_muted_projects_and_unchanged_cursors() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        checked(d.path(), &f);
        let state = std::fs::read(d.path().join("state.json")).unwrap();
        // 커서 자리의 이슈와 글은 "바뀜"이 아니에요.
        assert!(!changed(&f, d.path(), Utc::now()).unwrap());
        // 끈 프로젝트의 이슈가 바뀌어도 아니에요.
        f.jira.lock().unwrap()[1]["fields"]["updated"] = json!(stamp(Utc::now()));
        assert!(!changed(&f, d.path(), Utc::now()).unwrap());
        let search = f
            .calls()
            .into_iter()
            .filter(|c| c.ends_with("search/jql"))
            .count();
        assert_eq!(search, 2);
        // 켜진 프로젝트의 이슈가 바뀌면 바뀜이에요.
        f.jira.lock().unwrap()[0]["fields"]["updated"] = json!(stamp(Utc::now()));
        assert!(changed(&f, d.path(), Utc::now()).unwrap());
        f.jira.lock().unwrap()[0]["fields"]["updated"] = json!(stamp(recent()));
        // 새 꼬리 댓글도 바뀜이에요.
        f.footer.lock().unwrap()[0]["version"]["createdAt"] = json!(at(Utc::now()));
        assert!(changed(&f, d.path(), Utc::now()).unwrap());
        // 아무것도 쓰지 않아요.
        assert_eq!(std::fs::read(d.path().join("state.json")).unwrap(), state);
        assert!(item_ids(d.path(), "jira").is_empty());
    }

    #[test]
    fn check_skips_products_that_are_off_and_answers_early_when_unfinished() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        checked(d.path(), &f);
        let mut s = load_state(d.path());
        s.products.confluence = false;
        save_state(d.path(), &s).unwrap();
        f.footer.lock().unwrap()[0]["version"]["createdAt"] = json!(at(Utc::now()));
        assert!(!changed(&f, d.path(), Utc::now()).unwrap());
        assert!(!f.calls().iter().any(|c| c.contains("/wiki/")));

        f.log.lock().unwrap().clear();
        s.jira.done = false;
        save_state(d.path(), &s).unwrap();
        assert!(changed(&f, d.path(), Utc::now()).unwrap());
        assert!(f.calls().is_empty());

        s.paused_until = Some(iso(Utc::now() + Duration::minutes(10)));
        save_state(d.path(), &s).unwrap();
        assert!(!changed(&f, d.path(), Utc::now()).unwrap());
    }

    #[test]
    fn a_reply_survives_the_weekly_relist() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        due(d.path(), false, true);
        *f.pages.lock().unwrap() = vec![page("1")];
        let mut reply = comment("r1", "1");
        reply["parentCommentId"] = json!("c1");
        let mut deep = comment("r2", "1");
        deep["parentCommentId"] = json!("r1");
        *f.footer.lock().unwrap() = vec![comment("c1", "1"), reply.clone(), deep.clone()];
        let mut stored = vec![comment("c1", "1"), reply, deep];
        for c in &mut stored {
            c["type"] = json!("footer");
        }
        stored.push(
            json!({"id": "c-deleted", "type": "footer", "version": {"createdAt": at(recent())}}),
        );
        held_page(d.path(), "1", "page", stored);

        go(&f, d.path(), |_| {}).unwrap();
        let mut got = comment_ids(d.path(), "1");
        got.sort();
        assert_eq!(got, ["c1", "r1", "r2"]);
        let calls = f.calls();
        assert!(
            calls
                .iter()
                .any(|c| c.ends_with("footer-comments/c1/children"))
        );
        assert!(
            calls
                .iter()
                .any(|c| c.ends_with("footer-comments/r1/children"))
        );
        // 답글이 없던 댓글은 묻지 않아요.
        assert!(
            !calls
                .iter()
                .any(|c| c.ends_with("footer-comments/r2/children"))
        );
    }

    /// 페이지 목록만 404인 가짜예요.
    struct NoPages(FakeAtlassian);

    impl AtlassianApi for NoPages {
        fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Got, SourceError> {
            if path.ends_with("wiki/api/v2/pages") {
                return Ok(Got::Fail {
                    status: 404,
                    reason: String::new(),
                });
            }
            self.0.get(path, query)
        }

        fn post(
            &self,
            path: &str,
            body: &Value,
        ) -> Result<crate::atlassian::api::Answer, SourceError> {
            self.0.post(path, body)
        }
    }

    #[test]
    fn a_404_on_one_list_still_checks_the_others() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        checked(d.path(), &f);
        f.footer.lock().unwrap()[0]["version"]["createdAt"] = json!(at(Utc::now()));
        assert!(changed(&NoPages(f), d.path(), Utc::now()).unwrap());
    }
}
