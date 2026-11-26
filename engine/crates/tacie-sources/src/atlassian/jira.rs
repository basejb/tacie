//! Fetches Jira issues (docs/31 6절, 10-2절. 10-2 wins over 6).
//!
//! - **Initial fetch (newest first)**: `updated < "<floor 분 + 1분>" AND updated >= -730d ORDER BY updated DESC,
//!   key DESC`. JQL times are at second 0 of the minute, so `<= "floor 분"` would miss seconds 1–59 of that
//!   minute. That is why it asks up to the start of the next minute.
//!   Jira sorts `updated` down to milliseconds. `floor`/`floor_key` are the lowest item seen so far (full `updated`,
//!   key) and only move down. Every item equal to or above (`floor`, `floor_key`) is already held, so dropping only
//!   checks held ids. Saves every page. When the hourly budget (`Points`, initial fetch only) is less than the next
//!   page's estimate (`reserve`), it saves and returns `Later`.
//!   No `nextPageToken` (the 2-year end) means done.
//!   When the minute of `floor` changes, it asks again with the new `floor` (so no item is missed if items above
//!   change during the fetch and the list shifts).
//!   When the minute stays the same (many items in one minute, a bulk edit), it does not ask again but moves on with
//!   the same query's `nextPageToken`. That way it does not page that minute from the start over and over. When it
//!   leaves a minute it paged by token (or reaches the end), it pages that minute's query once more from the top
//!   without a token. While paging, items above may change and shift the list, so items at the page edge can be
//!   missed.
//!   Held items are dropped, so the only cost is paging that minute once more.
//! - `head` is the `updated` of the first (newest) item of the initial fetch. Items changed after it are picked up by
//!   the change fetch.
//! - **Change fetch**: `updated >= "<head − 15분>" ORDER BY updated ASC`. This runs outside the budget but is recorded
//!   in the ledger. Items whose `updated` equals the saved copy are not rewritten. If a fetched item's UTC offset
//!   differs from `head`'s (daylight saving changed), the first query is redone with a 60-minute overlap. It runs
//!   whenever `head` exists, even if the initial fetch is not finished. Paging works as in the initial fetch: if the
//!   query built from the new `head` equals the current query it uses `nextPageToken`, otherwise it asks again.
//! - Ledger: 1 per request, 1 per issue or comment, 2 per person (reporter, assignee, comment author and editor).
//!   Counting the same person more than once is fine (overcounting is the safe side).
//! - **JQL times**: minute strings in JQL are read in the user's Jira time zone. There is no time zone database
//!   (chrono-tz), so only a few zones without daylight saving (`ZONES`) have a known offset by name; for the rest it
//!   uses the offset in the `updated` strings Jira returns (Jira appends the user's zone offset, e.g. `+0900`).
//!   `myself.timeZone` is fetched once and kept in `state.time_zone`.
//! - If comments are incomplete (`comment.total` > the number fetched), only that issue fetches the rest through
//!   `issue/<key>/comment`.
//! - On the hourly limit (429 quota) it saves `paused_until` as the lift time + 0–15 minutes and returns `Later`
//!   (`Busy` in `run.halted`). While paused it calls neither the initial fetch nor the change fetch.
//! - Muted projects are fetched too and kept in the raw data. They are left out only when building chunks.
//!
//! Content (titles, bodies) is never put in logs or errors.

use std::collections::{BTreeSet, HashSet};

use chrono::{DateTime, Duration, FixedOffset};
use serde_json::{Value, json};

use crate::atlassian::adf;
use crate::atlassian::state::{Run, Step, WINDOW_DAYS, iso, item_ids, json_of, read_item};
use crate::error::SourceError;
use crate::gdrive::fetch::safe;

/// 받는 칸이에요.
const FIELDS: &str = "summary,description,status,assignee,reporter,labels,priority,project,issuetype,parent,created,updated,resolutiondate,comment";
/// 한 쪽의 이슈 수예요.
const PAGE: usize = 100;
/// 다음 쪽을 받기 전에 남아 있어야 할 가장 작은 포인트예요: 요청 1 + 이슈 100 + 이슈마다 사람 둘(2점씩).
/// 지난 쪽이 더 들었으면 그만큼을 남겨요(붙어 온 댓글과 댓글을 더 받는 요청까지 들어 있어요).
const RESERVE: u32 = 1 + PAGE as u32 + 2 * 2 * PAGE as u32;
/// 바뀐 것 받기의 겹침(분)이에요. 서머타임이 바뀐 때는 `DST_OVERLAP`이에요.
const OVERLAP: i64 = 15;
const DST_OVERLAP: i64 = 60;
/// 서머타임이 없어 이름만으로 시차를 아는 시간대예요(초). 그 밖은 `updated` 글자의 시차를 써요.
const ZONES: &[(&str, i32)] = &[
    ("UTC", 0),
    ("Etc/UTC", 0),
    ("GMT", 0),
    ("Etc/GMT", 0),
    ("Asia/Seoul", 9 * 3600),
    ("Asia/Tokyo", 9 * 3600),
    ("Asia/Shanghai", 8 * 3600),
    ("Asia/Hong_Kong", 8 * 3600),
    ("Asia/Taipei", 8 * 3600),
    ("Asia/Singapore", 8 * 3600),
    ("Asia/Kolkata", 5 * 3600 + 1800),
];

/// Jira의 `updated`(`2026-11-12T10:30:00.000+0900`)를 읽어요.
pub(crate) fn parse_updated(s: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%z")
        .or_else(|_| DateTime::parse_from_rfc3339(s))
        .ok()
}

/// JQL의 분 글자(`yyyy/MM/dd HH:mm`)예요. `zone`이 `ZONES`에 있으면 그 시차, 아니면 `t`의 시차로 써요.
pub(crate) fn jql_minute(t: DateTime<FixedOffset>, zone: Option<&str>) -> String {
    let offset = zone
        .and_then(|z| ZONES.iter().find(|(name, _)| *name == z))
        .and_then(|(_, secs)| FixedOffset::east_opt(*secs))
        .unwrap_or(*t.offset());
    t.with_timezone(&offset)
        .format("%Y/%m/%d %H:%M")
        .to_string()
}

/// 바뀐 것 받기의 겹침(분)이에요. 받은 `updated`의 시차가 `head`의 시차와 다르면 서머타임이 바뀐 거라 60분이에요.
pub(crate) fn overlap(head: &str, seen: &[&str]) -> i64 {
    let Some(h) = parse_updated(head) else {
        return OVERLAP;
    };
    let changed = seen
        .iter()
        .filter_map(|s| parse_updated(s))
        .any(|t| t.offset() != h.offset());
    if changed { DST_OVERLAP } else { OVERLAP }
}

/// `ENG-12` → (`ENG`, 12)예요. Jira의 `key` 정렬과 같게 견줘요.
fn key_order(key: &str) -> (String, u64) {
    match key.rsplit_once('-') {
        Some((p, n)) => (p.to_string(), n.parse().unwrap_or(0)),
        None => (key.to_string(), 0),
    }
}

fn updated_of(issue: &Value) -> Option<&str> {
    issue["fields"]["updated"].as_str()
}

fn jira_path(run: &Run, rest: &str) -> String {
    format!("ex/jira/{}/rest/api/3/{rest}", run.cloud)
}

fn get_json(run: &Run, rest: &str, q: &[(&str, String)], what: &str) -> Result<Value, SourceError> {
    json_of(run.api.get(&jira_path(run, rest), q)?, what)
}

/// `myself.timeZone`을 한 번 받아 둬요. 없으면 빈 글자로 둬서 다시 묻지 않아요.
fn ensure_zone(run: &mut Run) -> Result<(), SourceError> {
    if run.state.time_zone.is_some() {
        return Ok(());
    }
    let me = get_json(run, "myself", &[], "jira myself")?;
    run.state.points.charge(run.now, 1, 0, 0);
    run.state.time_zone = Some(me["timeZone"].as_str().unwrap_or("").to_string());
    Ok(())
}

/// 사람 객체 수예요(2점씩): 이 객체들의 `field` 칸마다 하나.
fn people(items: &[Value], fields: &[&str]) -> u32 {
    let n = items
        .iter()
        .map(|i| fields.iter().filter(|f| i[**f].is_object()).count())
        .sum::<usize>();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 검색 한 쪽이에요: 이슈와 다음 쪽 토큰. 장부에 적어요(요청 1 + 이슈와 붙어 온 댓글 수 + 사람마다 2).
fn search(
    run: &mut Run,
    jql: &str,
    token: Option<&str>,
) -> Result<(Vec<Value>, Option<String>), SourceError> {
    let mut q = vec![
        ("jql", jql.to_string()),
        ("fields", FIELDS.to_string()),
        ("maxResults", PAGE.to_string()),
    ];
    if let Some(t) = token {
        q.push(("nextPageToken", t.to_string()));
    }
    let v = get_json(run, "search/jql", &q, "jira search")?;
    let issues = v["issues"].as_array().cloned().unwrap_or_default();
    let embedded: Vec<Value> = issues
        .iter()
        .flat_map(|i| {
            i["fields"]["comment"]["comments"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .collect();
    let fields: Vec<Value> = issues.iter().map(|i| i["fields"].clone()).collect();
    let objects = u32::try_from(issues.len() + embedded.len()).unwrap_or(u32::MAX);
    let users = people(&fields, &["reporter", "assignee"])
        .saturating_add(people(&embedded, &["author", "updateAuthor"]));
    run.state.points.charge(run.now, 1, objects, users);
    let next = v["nextPageToken"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    Ok((issues, next))
}

/// 이슈의 댓글을 처음부터 다 받아요.
fn all_comments(run: &mut Run, issue: &str) -> Result<Vec<Value>, SourceError> {
    let mut out: Vec<Value> = Vec::new();
    loop {
        let q = [
            ("startAt", out.len().to_string()),
            ("maxResults", PAGE.to_string()),
        ];
        let v = get_json(run, &format!("issue/{issue}/comment"), &q, "jira comments")?;
        let page = v["comments"].as_array().cloned().unwrap_or_default();
        let objects = u32::try_from(page.len()).unwrap_or(u32::MAX);
        let users = people(&page, &["author", "updateAuthor"]);
        run.state.points.charge(run.now, 1, objects, users);
        let total = v["total"].as_u64().unwrap_or(0) as usize;
        let empty = page.is_empty();
        out.extend(page);
        if empty || out.len() >= total {
            return Ok(out);
        }
    }
}

/// 개인정보 보고에 쓸 accountId예요: 보고자, 담당자, 댓글 작성자, 설명·댓글의 멘션.
fn accounts(fields: &Value, comments: &[Value]) -> Vec<String> {
    let mut ids = BTreeSet::new();
    let mut person = |p: &Value| {
        if let Some(id) = p["accountId"].as_str().filter(|s| !s.is_empty()) {
            ids.insert(id.to_string());
        }
    };
    person(&fields["reporter"]);
    person(&fields["assignee"]);
    for c in comments {
        person(&c["author"]);
        person(&c["updateAuthor"]);
    }
    let docs = std::iter::once(&fields["description"]).chain(comments.iter().map(|c| &c["body"]));
    for d in docs {
        if let Some(doc) = adf::parse_doc(d) {
            ids.extend(adf::mentions(&doc).into_iter().map(|(id, _)| id));
        }
    }
    ids.into_iter().collect()
}

/// key가 주소에 넣어도 되는 글자인지예요(영문·숫자·`-`·`_`).
fn plain_key(key: &str) -> bool {
    safe(key)
}

/// 이슈 하나를 파일로 써요: `{id, key, project, fields, comments, accounts, fetched_at}`.
fn store(run: &mut Run, issue: &Value) -> Result<(), SourceError> {
    let id = issue["id"].as_str().unwrap_or("");
    let key = issue["key"].as_str().unwrap_or("");
    let mut fields = issue["fields"].clone();
    let comment = fields
        .as_object_mut()
        .and_then(|m| m.remove("comment"))
        .unwrap_or(Value::Null);
    let mut comments = comment["comments"].as_array().cloned().unwrap_or_default();
    let total = comment["total"].as_u64().unwrap_or(0) as usize;
    if total > comments.len() {
        let which = if plain_key(key) { key } else { id };
        comments = all_comments(run, which)?;
    }
    let project = &fields["project"];
    let item = json!({
        "id": id,
        "key": key,
        "project": {"id": project["id"], "key": project["key"], "name": project["name"]},
        "accounts": accounts(&fields, &comments),
        "fields": fields,
        "comments": comments,
        "fetched_at": iso(run.now),
    });
    run.write("jira", id, &item)
}

/// 처음 받기의 물음이에요: `floor`(Jira `updated` 글자)의 다음 분 앞까지, 없으면 맨 위부터예요.
fn below_jql(floor: Option<&str>, zone: Option<&str>) -> String {
    let window = format!("updated >= -{WINDOW_DAYS}d");
    let below = floor
        .and_then(parse_updated)
        .map(|t| {
            format!(
                "updated < \"{}\" AND ",
                jql_minute(t + Duration::minutes(1), zone)
            )
        })
        .unwrap_or_default();
    format!("{below}{window} ORDER BY updated DESC, key DESC")
}

fn initial_jql(run: &Run) -> String {
    below_jql(
        run.state.jira.floor.as_deref(),
        run.state.time_zone.as_deref(),
    )
}

/// 처음 받기에서 (`updated`, key)가 `floor`·`floor_key`보다 아래(목록에서 뒤)인지예요. `floor`는 이걸로만 내려가요.
fn below_floor(run: &Run, issue: &Value) -> bool {
    let cursor = &run.state.jira;
    let (Some(floor), Some(floor_key)) = (
        cursor.floor.as_deref().and_then(parse_updated),
        cursor.floor_key.as_deref(),
    ) else {
        return true;
    };
    let Some(t) = updated_of(issue).and_then(parse_updated) else {
        return false;
    };
    (t, key_order(issue["key"].as_str().unwrap_or(""))) < (floor, key_order(floor_key))
}

/// 처음 받기에서 지금 넘기는 물음이에요.
struct Walk {
    jql: String,
    token: Option<String>,
    /// 이 물음을 `nextPageToken`으로 넘긴 적이 있어요(같은 분에 글이 많았어요).
    followed: bool,
    /// 토큰으로 넘긴 분을 맨 위부터 한 번 더 보는 중이에요.
    recheck: bool,
}

impl Walk {
    fn new(jql: String) -> Walk {
        Walk {
            jql,
            token: None,
            followed: false,
            recheck: false,
        }
    }
}

/// 처음 받기예요(최근 것부터, 예산 안에서).
pub(crate) fn initial(run: &mut Run) -> Result<Step, SourceError> {
    if run.state.jira.done {
        return Ok(Step::Done);
    }
    if run.paused() {
        return Ok(Step::Later);
    }
    match initial_pages(run) {
        Err(e) => run.pause_on_quota(e),
        ok => ok,
    }
}

fn initial_pages(run: &mut Run) -> Result<Step, SourceError> {
    ensure_zone(run)?;
    // 가진 글이에요. `floor`·`floor_key`와 그 앞(먼저 본 글)은 모두 여기 있어서, 버리는 건 이것만 봐요. 그래야 토큰으로
    // 넘기다 목록이 밀려 빠진 글을 다시 볼 때(`recheck`) 주워요.
    let mut held: HashSet<String> = item_ids(run.dir, "jira").into_iter().collect();
    let mut written = 0usize;
    let mut walk = Walk::new(initial_jql(run));
    let mut reserve = RESERVE;
    loop {
        let before = run.state.points.left(run.now);
        if before < reserve {
            run.save()?;
            return Ok(Step::Later);
        }
        run.guard()?;
        let (issues, next) = search(run, &walk.jql, walk.token.as_deref())?;
        let mut fresh = 0usize;
        let mut last: Option<String> = None;
        for issue in &issues {
            let id = issue["id"].as_str().unwrap_or("");
            let Some(up) = updated_of(issue).filter(|u| parse_updated(u).is_some()) else {
                continue;
            };
            last = Some(up.to_string());
            if run.state.jira.head.is_none() {
                run.state.jira.head = Some(up.to_string());
            }
            // `floor`는 내려가기만 해요(다시 보는 동안 위쪽 글로 올라가지 않아요).
            if below_floor(run, issue) {
                run.state.jira.floor = Some(up.to_string());
                run.state.jira.floor_key = issue["key"].as_str().map(str::to_string);
            }
            if !safe(id) || held.contains(id) {
                continue;
            }
            store(run, issue)?;
            held.insert(id.to_string());
            fresh += 1;
        }
        written += fresh;
        // 지난 쪽이 쓴 만큼을 다음 쪽에도 남겨요(댓글을 더 받은 요청까지 들어 있어요).
        reserve = RESERVE.max(before.saturating_sub(run.state.points.left(run.now)));
        (run.progress)(written, written);
        run.state.fetched_at = Some(iso(run.now));
        let next_jql = initial_jql(run);
        // 다시 보는 중에는 쪽의 끝이 아직 그 분 안이면 계속 넘겨요.
        let inside = last
            .as_deref()
            .is_some_and(|l| below_jql(Some(l), run.state.time_zone.as_deref()) == walk.jql);
        walk = match next {
            Some(token) if walk.recheck && inside => Walk {
                token: Some(token),
                ..walk
            },
            Some(_) if walk.recheck => Walk::new(next_jql),
            // `floor`의 분이 그대로예요: 같은 물음을 토큰으로 넘겨요(그 분을 처음부터 되풀이하지 않아요).
            Some(token) if next_jql == walk.jql => Walk {
                token: Some(token),
                followed: true,
                ..walk
            },
            // 토큰으로 넘긴 분을 벗어나거나 끝에 닿았어요. 넘기는 동안 위쪽 글이 바뀌어 목록이 밀렸으면 쪽 끝의 글이
            // 빠졌을 수 있어서, 그 분을 맨 위부터 한 번 더 봐요(가진 글은 버려요).
            _ if walk.followed && !walk.recheck => Walk {
                token: None,
                followed: false,
                recheck: true,
                ..walk
            },
            Some(_) => Walk::new(next_jql),
            None => {
                run.state.jira.done = true;
                run.save()?;
                return Ok(Step::Done);
            }
        };
        run.save()?;
    }
}

/// 바뀐 것 받기예요(예산 밖, 장부에는 적어요).
pub(crate) fn changes(run: &mut Run) -> Result<Step, SourceError> {
    if run.paused() {
        return Ok(Step::Later);
    }
    match change_pages(run) {
        Err(e) => run.pause_on_quota(e),
        ok => ok,
    }
}

/// 바뀐 것 받기의 물음이에요: `head − minutes`부터, `head`가 없으면 2년 안 전부예요.
fn changes_jql(run: &Run, minutes: i64) -> String {
    match run.state.jira.head.as_deref().and_then(parse_updated) {
        Some(h) => format!(
            "updated >= \"{}\" ORDER BY updated ASC",
            jql_minute(
                h - Duration::minutes(minutes),
                run.state.time_zone.as_deref()
            )
        ),
        None => format!("updated >= -{WINDOW_DAYS}d ORDER BY updated ASC"),
    }
}

fn change_pages(run: &mut Run) -> Result<Step, SourceError> {
    // 처음 받기가 아직 첫 쪽도 받지 않았으면 그쪽이 `head`를 정해요.
    if run.state.jira.head.is_none() && !run.state.jira.done {
        return Ok(Step::Done);
    }
    ensure_zone(run)?;
    let first_head = run.state.jira.head.clone();
    let mut minutes = OVERLAP;
    let mut first = true;
    let mut written = 0usize;
    let mut follow: Option<(String, String)> = None;
    loop {
        run.guard()?;
        let (jql, token) = match follow.take() {
            Some((jql, token)) => (jql, Some(token)),
            None => (changes_jql(run, if first { minutes } else { 0 }), None),
        };
        let (issues, next) = search(run, &jql, token.as_deref())?;
        if first
            && minutes == OVERLAP
            && let Some(h) = first_head.as_deref()
        {
            let seen: Vec<&str> = issues.iter().filter_map(updated_of).collect();
            if overlap(h, &seen) == DST_OVERLAP {
                minutes = DST_OVERLAP;
                continue;
            }
        }
        first = false;
        let mut fresh = 0usize;
        for issue in &issues {
            let id = issue["id"].as_str().unwrap_or("");
            let Some(up) = updated_of(issue) else {
                continue;
            };
            let Some(t) = parse_updated(up) else { continue };
            if !safe(id) {
                continue;
            }
            let stored = read_item(run.dir, "jira", id);
            let current = stored
                .as_ref()
                .and_then(|s| s["fields"]["updated"].as_str())
                .is_some_and(|s| s == up);
            if !current {
                store(run, issue)?;
                fresh += 1;
            }
            let later = run
                .state
                .jira
                .head
                .as_deref()
                .and_then(parse_updated)
                .is_none_or(|h| t > h);
            if later {
                run.state.jira.head = Some(up.to_string());
            }
        }
        written += fresh;
        (run.progress)(written, written);
        run.state.fetched_at = Some(iso(run.now));
        let Some(next) = next else {
            run.save()?;
            return Ok(Step::Done);
        };
        if changes_jql(run, 0) == jql {
            follow = Some((jql, next));
        }
        run.save()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    use chrono::{Duration, Timelike, Utc};
    use serde_json::{Value, json};

    use crate::atlassian::api::{POINTS_PER_HOUR, Points};
    use crate::atlassian::state::{State, item_ids, load_state, read_item, save_state};
    use crate::error::Kind;
    use crate::testing::atlassian::FakeAtlassian;

    /// 지금보다 하루 앞의 정분이에요. 가짜의 `-730d`가 진짜 시계를 써서 시각을 지금 가까이에 둬요.
    fn base() -> DateTime<Utc> {
        let t = Utc::now() - Duration::days(1);
        t.with_second(0).unwrap().with_nanosecond(0).unwrap()
    }

    fn stamp(t: DateTime<Utc>) -> String {
        t.format("%Y-%m-%dT%H:%M:%S%.3f+0000").to_string()
    }

    fn issue(n: u32, updated: &str) -> Value {
        json!({
            "id": n.to_string(),
            "key": format!("A-{n}"),
            "fields": {
                "summary": format!("issue {n}"),
                "updated": updated,
                "project": {"id": "100", "key": "A", "name": "Alpha"},
                "comment": {"comments": [], "total": 0, "maxResults": 100, "startAt": 0}
            }
        })
    }

    /// `n`개를 `A-1`(가장 최근, `base − 1분`)부터 1분씩 앞으로 둬요.
    fn seed(f: &FakeAtlassian, n: u32) {
        let b = base();
        *f.jira.lock().unwrap() = (1..=n)
            .map(|i| issue(i, &stamp(b - Duration::minutes(i64::from(i)))))
            .collect();
    }

    /// 이 시간에 한 쪽만 받을 만큼 남긴 장부예요: 첫 쪽의 어림값(`RESERVE`, 501)은 넘고, 한 쪽(101)을 쓰고 나면
    /// 모자라요.
    fn one_page(now: DateTime<Utc>) -> Points {
        Points {
            used: POINTS_PER_HOUR - 600,
            hour: now.timestamp() / 3600,
        }
    }

    struct Out {
        step: Result<Step, SourceError>,
        halted: Option<SourceError>,
    }

    fn go(
        f: &FakeAtlassian,
        dir: &Path,
        still: &dyn Fn() -> bool,
        prep: impl FnOnce(&mut State),
        what: fn(&mut Run) -> Result<Step, SourceError>,
    ) -> Out {
        let mut state = load_state(dir);
        prep(&mut state);
        let mut progress = |_: usize, _: usize| {};
        let jitter = || 600;
        let mut run = Run {
            api: f,
            dir,
            cloud: "cloud-1",
            now: Utc::now(),
            still,
            state,
            progress: &mut progress,
            jitter: &jitter,
            halted: None,
        };
        let step = what(&mut run);
        Out {
            step,
            halted: run.halted,
        }
    }

    fn always() -> bool {
        true
    }

    fn minute_of(s: &str) -> DateTime<Utc> {
        let t = parse_updated(s).unwrap().with_timezone(&Utc);
        t.with_second(0).unwrap().with_nanosecond(0).unwrap()
    }

    #[test]
    fn first_fetch_takes_newest_first_and_stops_at_the_budget() {
        let f = FakeAtlassian::new();
        seed(&f, 250);
        let d = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let out = go(&f, d.path(), &always, |s| s.points = one_page(now), initial);
        assert_eq!(out.step.unwrap(), Step::Later);
        let ids: BTreeSet<u32> = item_ids(d.path(), "jira")
            .iter()
            .map(|i| i.parse().unwrap())
            .collect();
        assert_eq!(ids, (1..=100).collect());
        let s = load_state(d.path());
        assert!(!s.jira.done);
        assert_eq!(
            minute_of(s.jira.floor.as_deref().unwrap()),
            base() - Duration::minutes(100)
        );
        assert_eq!(s.jira.floor_key.as_deref(), Some("A-100"));
        assert_eq!(
            minute_of(s.jira.head.as_deref().unwrap()),
            base() - Duration::minutes(1)
        );
        assert_eq!(s.cloud.as_deref(), Some("cloud-1"));
        let item = read_item(d.path(), "jira", "1").unwrap();
        assert_eq!(item["key"], "A-1");
        assert_eq!(item["project"]["key"], "A");
        assert!(item["fields"].get("comment").is_none());
        assert!(item["fetched_at"].is_string());
    }

    #[test]
    fn the_next_run_carries_on_below_the_floor_without_gaps() {
        let f = FakeAtlassian::new();
        seed(&f, 102);
        // A-100과 A-101이 같은 분이에요. key 내림차순이라 A-101이 첫 쪽의 끝(100번째), A-100이 다음 쪽 처음이에요.
        {
            let mut j = f.jira.lock().unwrap();
            let same = j[99]["fields"]["updated"].clone();
            j[100]["fields"]["updated"] = same;
        }
        let d = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let out = go(&f, d.path(), &always, |s| s.points = one_page(now), initial);
        assert_eq!(out.step.unwrap(), Step::Later);
        assert_eq!(item_ids(d.path(), "jira").len(), 100);
        assert!(read_item(d.path(), "jira", "100").is_none());
        assert_eq!(
            load_state(d.path()).jira.floor_key.as_deref(),
            Some("A-101")
        );

        let out = go(
            &f,
            d.path(),
            &always,
            |s| s.points = Points::default(),
            initial,
        );
        assert_eq!(out.step.unwrap(), Step::Done);
        assert_eq!(item_ids(d.path(), "jira").len(), 102);
        assert!(read_item(d.path(), "jira", "100").is_some());
        assert!(load_state(d.path()).jira.done);
    }

    fn keyed(id: &str, key: &str, updated: &str) -> Value {
        let mut v = issue(0, updated);
        v["id"] = json!(id);
        v["key"] = json!(key);
        v
    }

    fn searches(f: &FakeAtlassian) -> usize {
        f.calls()
            .iter()
            .filter(|c| c.ends_with("search/jql"))
            .count()
    }

    #[test]
    fn issues_in_one_minute_are_ordered_by_the_millisecond_across_a_page_cut() {
        let f = FakeAtlassian::new();
        seed(&f, 99);
        // 한 분 안의 세 글이에요. Jira는 밀리초로 정렬해서 key 순서와 엇갈려요: C-5(40초), C-1(20초), C-9(5초).
        // 첫 쪽은 C-5에서 끊겨요. 남은 C-1·C-9는 그 분의 1초 뒤라 `<= "그 분"`이면 빠지고, key만 견주면 C-9가 빠져요.
        let m = base() - Duration::minutes(200);
        f.jira.lock().unwrap().extend([
            keyed("905", "C-5", &stamp(m + Duration::seconds(40))),
            keyed("901", "C-1", &stamp(m + Duration::seconds(20))),
            keyed("909", "C-9", &stamp(m + Duration::seconds(5))),
        ]);
        let d = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let out = go(&f, d.path(), &always, |s| s.points = one_page(now), initial);
        assert_eq!(out.step.unwrap(), Step::Later);
        assert!(read_item(d.path(), "jira", "905").is_some());
        assert_eq!(load_state(d.path()).jira.floor_key.as_deref(), Some("C-5"));

        let out = go(
            &f,
            d.path(),
            &always,
            |s| s.points = Points::default(),
            initial,
        );
        assert_eq!(out.step.unwrap(), Step::Done);
        assert_eq!(item_ids(d.path(), "jira").len(), 102);
        assert!(read_item(d.path(), "jira", "901").is_some());
        assert!(read_item(d.path(), "jira", "909").is_some());
    }

    #[test]
    fn a_bulk_edited_minute_is_paged_through_not_from_its_top_each_time() {
        let f = FakeAtlassian::new();
        // 한꺼번에 고친 300개가 한 분 안에 100ms씩 떨어져 있어요.
        let m = base() - Duration::minutes(30);
        *f.jira.lock().unwrap() = (1..=300)
            .map(|i: i64| {
                keyed(
                    &i.to_string(),
                    &format!("B-{i}"),
                    &stamp(m + Duration::milliseconds(i * 100)),
                )
            })
            .collect();
        let d = tempfile::tempdir().unwrap();
        let mut runs = 0;
        loop {
            runs += 1;
            assert!(
                runs <= 3,
                "the first fetch should end within a few hours of budget"
            );
            let out = go(
                &f,
                d.path(),
                &always,
                |s| s.points = Points::default(),
                initial,
            );
            if out.step.unwrap() == Step::Done {
                break;
            }
        }
        assert_eq!(item_ids(d.path(), "jira").len(), 300);
        // 첫 쪽, 새 floor로 다시 묻기, 토큰으로 두 쪽, 그 분을 맨 위부터 한 번 더(세 쪽)예요. 분을 쪽마다 처음부터
        // 되풀이하지 않아요(되풀이하면 쪽 수의 제곱으로 늘어요).
        assert!(searches(&f) <= 7, "searches: {}", searches(&f));
    }

    #[test]
    fn an_issue_skipped_by_a_shifting_token_walk_is_picked_up_on_the_recheck() {
        let f = FakeAtlassian::new();
        let m = base() - Duration::minutes(30);
        *f.jira.lock().unwrap() = (1..=300)
            .map(|i: i64| {
                keyed(
                    &i.to_string(),
                    &format!("B-{i}"),
                    &stamp(m + Duration::milliseconds(i * 100)),
                )
            })
            .collect();
        let d = tempfile::tempdir().unwrap();
        // 두 번째 검색(그 분을 맨 위부터, 모두 가진 글) 뒤, 토큰으로 넘기기 전에 이미 지난 B-300이 바뀌어 범위 밖으로
        // 올라가요. 목록이 하나씩 당겨져서 토큰의 다음 쪽은 B-200을 건너뛰어요.
        let bumped = std::sync::atomic::AtomicBool::new(false);
        let still = || {
            if searches(&f) == 2 && !bumped.swap(true, std::sync::atomic::Ordering::SeqCst) {
                f.jira.lock().unwrap()[299]["fields"]["updated"] = json!(stamp(base()));
            }
            true
        };
        let out = go(
            &f,
            d.path(),
            &still,
            |s| s.points = Points::default(),
            initial,
        );
        assert_eq!(out.step.unwrap(), Step::Done);
        assert!(bumped.load(std::sync::atomic::Ordering::SeqCst));
        assert!(read_item(d.path(), "jira", "200").is_some());
        assert_eq!(item_ids(d.path(), "jira").len(), 300);
    }

    #[test]
    fn people_cost_two_points_each() {
        let f = FakeAtlassian::new();
        let mut one = issue(1, &stamp(base()));
        one["fields"]["reporter"] = json!({"accountId": "u1"});
        one["fields"]["assignee"] = json!({"accountId": "u2"});
        one["fields"]["comment"] = json!({
            "comments": [{"id": "c1", "author": {"accountId": "u1"}, "updateAuthor": {"accountId": "u3"}}],
            "total": 1
        });
        *f.jira.lock().unwrap() = vec![one];
        let d = tempfile::tempdir().unwrap();
        let out = go(
            &f,
            d.path(),
            &always,
            |s| s.time_zone = Some("UTC".into()),
            initial,
        );
        assert_eq!(out.step.unwrap(), Step::Done);
        // 요청 1 + 이슈 1 + 댓글 1 + 사람 4 × 2.
        assert_eq!(load_state(d.path()).points.used, 1 + 1 + 1 + 4 * 2);
    }

    #[test]
    fn save_keeps_what_other_parts_own() {
        let f = FakeAtlassian::new();
        seed(&f, 1);
        let d = tempfile::tempdir().unwrap();
        let mut s = State::default();
        s.products.jira = true;
        s.reported_at = Some("2026-11-01T00:00:00.000Z".into());
        s.report_every = Some(604_800);
        s.reconciled_at = Some("2026-11-02T00:00:00.000Z".into());
        s.gone.insert("7".into(), 1);
        s.muted_spaces = vec!["DOCS".into()];
        s.lost_since = Some("2026-11-03T00:00:00.000Z".into());
        save_state(d.path(), &s).unwrap();
        let out = go(&f, d.path(), &always, |s| *s = State::default(), initial);
        assert_eq!(out.step.unwrap(), Step::Done);
        let back = load_state(d.path());
        assert!(back.products.jira);
        assert_eq!(back.reported_at, s.reported_at);
        assert_eq!(back.report_every, Some(604_800));
        assert_eq!(back.reconciled_at, s.reconciled_at);
        assert_eq!(back.gone.get("7"), Some(&1));
        assert_eq!(back.muted_spaces, ["DOCS"]);
        assert_eq!(back.lost_since, s.lost_since);
        assert!(back.jira.done);
    }

    #[test]
    fn an_issue_updated_during_the_first_fetch_is_picked_up_by_changes() {
        let f = FakeAtlassian::new();
        seed(&f, 150);
        let d = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let out = go(&f, d.path(), &always, |s| s.points = one_page(now), initial);
        assert_eq!(out.step.unwrap(), Step::Later);
        assert!(read_item(d.path(), "jira", "120").is_none());

        // 아직 받지 않은 A-120과 이미 받은 A-5가 처음 받기 도중에 바뀌어요.
        let later = stamp(base() + Duration::minutes(5));
        let latest = stamp(base() + Duration::minutes(6));
        {
            let mut j = f.jira.lock().unwrap();
            j[119]["fields"]["updated"] = json!(later);
            j[4]["fields"]["updated"] = json!(latest);
            j[4]["fields"]["summary"] = json!("renamed");
        }
        let out = go(&f, d.path(), &always, |_| {}, changes);
        assert_eq!(out.step.unwrap(), Step::Done);
        assert_eq!(
            read_item(d.path(), "jira", "120").unwrap()["fields"]["updated"],
            json!(later)
        );
        assert_eq!(
            read_item(d.path(), "jira", "5").unwrap()["fields"]["summary"],
            "renamed"
        );
        assert_eq!(
            load_state(d.path()).jira.head.as_deref(),
            Some(latest.as_str())
        );

        // 처음 받기를 마저 하면 나머지가 들어오고 A-120은 새것 그대로예요.
        let out = go(
            &f,
            d.path(),
            &always,
            |s| s.points = Points::default(),
            initial,
        );
        assert_eq!(out.step.unwrap(), Step::Done);
        assert_eq!(item_ids(d.path(), "jira").len(), 150);
        assert_eq!(
            read_item(d.path(), "jira", "120").unwrap()["fields"]["updated"],
            json!(later)
        );
    }

    #[test]
    fn long_comment_lists_are_completed() {
        let f = FakeAtlassian::new();
        let mut one = issue(1, &stamp(base()));
        let doc = |text: &str, mention: Option<&str>| {
            let mut content = vec![json!({"type": "text", "text": text})];
            if let Some(id) = mention {
                content.push(json!({"type": "mention", "attrs": {"id": id, "text": "@Lee"}}));
            }
            json!({"type": "doc", "version": 1, "content": [{"type": "paragraph", "content": content}]})
        };
        let comment = |id: &str, author: &str, mention: Option<&str>| json!({"issue": "A-1", "id": id, "author": {"accountId": author}, "body": doc(id, mention)});
        one["fields"]["reporter"] = json!({"accountId": "u-rep"});
        one["fields"]["assignee"] = json!({"accountId": "u-asg"});
        one["fields"]["comment"] = json!({
            "comments": [comment("c1", "u1", None)], "total": 3, "maxResults": 1, "startAt": 0
        });
        *f.jira.lock().unwrap() = vec![one];
        *f.comments.lock().unwrap() = vec![
            comment("c1", "u1", None),
            comment("c2", "u2", Some("u-men")),
            comment("c3", "u1", None),
        ];
        let d = tempfile::tempdir().unwrap();
        let out = go(&f, d.path(), &always, |_| {}, initial);
        assert_eq!(out.step.unwrap(), Step::Done);
        let item = read_item(d.path(), "jira", "1").unwrap();
        let ids: Vec<&str> = item["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["c1", "c2", "c3"]);
        assert_eq!(
            item["accounts"],
            json!(["u-asg", "u-men", "u-rep", "u1", "u2"])
        );
        assert!(
            f.calls()
                .iter()
                .any(|c| c == "GET ex/jira/cloud-1/rest/api/3/issue/A-1/comment")
        );
    }

    #[test]
    fn muted_projects_stay_on_disk_but_are_skipped_later() {
        let f = FakeAtlassian::new();
        seed(&f, 3);
        f.jira.lock().unwrap()[1]["fields"]["project"] =
            json!({"id": "200", "key": "B", "name": "Beta"});
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            go(&f, d.path(), &always, |_| {}, initial).step.unwrap(),
            Step::Done
        );
        // 앱이 B를 꺼요(받는 쪽의 상태는 그 전 것이에요).
        let mut s = load_state(d.path());
        s.muted_projects = vec!["B".into()];
        save_state(d.path(), &s).unwrap();
        f.jira.lock().unwrap()[1]["fields"]["updated"] =
            json!(stamp(base() + Duration::minutes(3)));
        let out = go(&f, d.path(), &always, |s| s.muted_projects.clear(), changes);
        assert_eq!(out.step.unwrap(), Step::Done);
        // 원본은 남고(조각을 만들 때 빼요), 끈 목록은 디스크 것 그대로예요.
        assert!(read_item(d.path(), "jira", "2").is_some());
        assert_eq!(load_state(d.path()).muted_projects, ["B"]);
    }

    #[test]
    fn a_changed_connection_stops_writing() {
        let f = FakeAtlassian::new();
        seed(&f, 250);
        let d = tempfile::tempdir().unwrap();
        let searches = || {
            f.calls()
                .iter()
                .filter(|c| c.ends_with("search/jql"))
                .count()
        };
        let still = || searches() < 2;
        let out = go(&f, d.path(), &still, |_| {}, initial);
        assert_eq!(out.step.unwrap_err().kind(), Kind::Busy);
        assert_eq!(item_ids(d.path(), "jira").len(), 100);
    }

    #[test]
    fn quota_pause_saves_paused_until_with_jitter() {
        let f = FakeAtlassian::new();
        seed(&f, 10);
        let reset = base() + Duration::days(1) + Duration::hours(1);
        *f.quota.lock().unwrap() = Some(crate::gdrive::fetch::iso(reset));
        let d = tempfile::tempdir().unwrap();
        let out = go(
            &f,
            d.path(),
            &always,
            |s| s.time_zone = Some("UTC".into()),
            initial,
        );
        assert_eq!(out.step.unwrap(), Step::Later);
        assert_eq!(out.halted.unwrap().kind(), Kind::Busy);
        let s = load_state(d.path());
        let until = crate::gdrive::fetch::parse(s.paused_until.as_deref().unwrap()).unwrap();
        assert!(until >= reset && until <= reset + Duration::minutes(15));
        assert_eq!(until, reset + Duration::seconds(600));
        assert!(item_ids(d.path(), "jira").is_empty());

        // 멈춘 동안에는 부르지 않아요(바뀐 것 받기도요).
        let calls = f.calls().len();
        let out = go(&f, d.path(), &always, |_| {}, initial);
        assert_eq!(out.step.unwrap(), Step::Later);
        assert_eq!(out.halted.unwrap().kind(), Kind::Busy);
        let out = go(&f, d.path(), &always, |_| {}, changes);
        assert_eq!(out.step.unwrap(), Step::Later);
        assert_eq!(f.calls().len(), calls);
    }

    #[test]
    fn jql_minutes_are_in_the_jira_zone() {
        let t = |s: &str| parse_updated(s).unwrap();
        assert_eq!(
            jql_minute(t("2026-11-12T01:30:45.000+0000"), Some("Asia/Seoul")),
            "2026/11/12 10:30"
        );
        assert_eq!(
            jql_minute(t("2026-11-11T20:05:00.000-0500"), Some("Asia/Seoul")),
            "2026/11/12 10:05"
        );
        // 표에 없는 시간대는 Jira가 준 글자의 시차를 써요.
        assert_eq!(
            jql_minute(t("2026-11-12T10:30:00.000+0900"), None),
            "2026/11/12 10:30"
        );
        assert_eq!(
            jql_minute(t("2026-11-01T01:30:00.000-0400"), Some("America/New_York")),
            "2026/11/01 01:30"
        );
    }

    #[test]
    fn the_overlap_is_an_hour_when_the_offset_changed() {
        let head = "2026-11-01T01:30:00.000-0400";
        assert_eq!(overlap(head, &["2026-11-01T01:40:00.000-0400"]), 15);
        assert_eq!(overlap(head, &[]), 15);
        assert_eq!(
            overlap(
                head,
                &[
                    "2026-11-01T01:40:00.000-0400",
                    "2026-11-01T01:10:00.000-0500"
                ]
            ),
            60
        );
    }

    #[test]
    fn a_changed_offset_widens_the_changes_window_to_an_hour() {
        let f = FakeAtlassian::new();
        let b = base();
        // 한 시간 안쪽(40분 전)에 바뀐 글이에요. 15분 겹침으로는 안 보이고, 새 시차가 보이면 60분으로 다시 물어요.
        let mut old = issue(1, &stamp(b - Duration::minutes(40)));
        old["fields"]["updated"] = json!(stamp(b - Duration::minutes(40)));
        let newer = issue(
            2,
            &(b + Duration::hours(1) + Duration::minutes(5))
                .with_timezone(&FixedOffset::east_opt(3600).unwrap())
                .format("%Y-%m-%dT%H:%M:%S%.3f%z")
                .to_string(),
        );
        *f.jira.lock().unwrap() = vec![old, newer];
        let d = tempfile::tempdir().unwrap();
        let head = stamp(b);
        let out = go(
            &f,
            d.path(),
            &always,
            |s| {
                s.jira.head = Some(head.clone());
                s.time_zone = Some(String::new());
            },
            changes,
        );
        assert_eq!(out.step.unwrap(), Step::Done);
        let searches = f
            .calls()
            .iter()
            .filter(|c| c.ends_with("search/jql"))
            .count();
        assert_eq!(searches, 2);
        assert!(read_item(d.path(), "jira", "1").is_some());
        assert!(read_item(d.path(), "jira", "2").is_some());
    }
}
