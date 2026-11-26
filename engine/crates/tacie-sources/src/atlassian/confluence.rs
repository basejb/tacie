//! Fetches Confluence pages, blog posts and comments (docs/31 6절, 10-2절. 10-2 wins over 6).
//!
//! - There are four lists: `pages` (`status=current,archived`), `blogposts` (`current`), `footer-comments`,
//!   `inline-comments`. All fetch the whole site with `sort=-modified-date` (newest first),
//!   `body-format=atlas_doc_format` and `limit=100`. The modified time is `version.createdAt`. For the next page only
//!   the `cursor=` in `_links.next` is read (the rest of that URL is not followed).
//! - **Initial fetch**: v2 lists have no parameter to filter by modified time, so it cannot ask from the resume point
//!   (`floor`). Instead it resumes by cursor and drops items it already has. When the hourly budget (`Points`) is
//!   less than one page's estimate (`RESERVE`), it saves and returns `Later`. Reaching items older than 2 years or the
//!   end of the list marks that list `done`.
//!   Each list's next-page cursor (`Cursor.next`) is saved every page, so the next fetch resumes there (it does not
//!   page from the top again). Items that moved above the resume point changed after `head`, so the change fetch
//!   (`head − 15분`) picks them up. If the list rejects the saved cursor (400), it is cleared and the list is paged
//!   once more from the top. Comment lists stop at the same 2-year end, so comments older than 2 years on recent items
//!   are deliberately not fetched.
//!   `head` is the newest modified time seen on the first page of the initial fetch.
//! - **Change fetch**: pages each list newest first and stops at the first item whose modified time is not later than
//!   `head − 15분`. `head` moves up only after paging to that end (if it stops midway, it looks again from the same
//!   point next time). For pages and blog posts it also pages the `status=trashed,deleted` list to the same point and
//!   deletes those files. This runs outside the budget but is recorded in the ledger.
//! - **Files**: one Confluence item (page or blog post) is one `items/page-<id>.json`:
//!   `{id, kind: "page"|"blogpost", page, comments, accounts, fetched_at}`. Comments go into their item's file by
//!   `pageId`/`blogPostId` (`type: "footer"|"inline"`; an inline comment keeps `properties.inlineOriginalSelection`
//!   as `quote`). If the item has not been fetched (a new comment on an item older than 2 years), the comments sit in
//!   a file with `page: null`. Comments are merged by id, and refetching the item leaves the comments as they are.
//!   Items written in this fetch have their comments replaced with the comments seen this time only when that kind's
//!   comment list was paged from start to end (deleted comments drop out). A resumed list was not seen from the start,
//!   so it does not replace.
//! - Ledger: 1 per request and 1 per item or comment. v2 lists do not embed person objects (only the `authorId`
//!   string), so there are no 2-point entries.
//! - Products: if `state.products.confluence` is off, it does not call. If the first request of this fetch is a 404,
//!   the site is taken to have no Confluence; it is turned off and the fetch ends without an error.
//! - The hourly limit (429 quota) is the same as Jira: it stops until the lift time + 0–15 minutes and returns
//!   `Later`.
//!
//! Content (titles, bodies) is never put in logs or errors.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use crate::atlassian::adf;
use crate::atlassian::state::{
    Cursor, Products, Run, State, Step, WINDOW_DAYS, iso, item_ids, json_of, read_item,
};
use crate::error::SourceError;
use crate::gdrive::api::Got;
use crate::gdrive::fetch::{parse, safe};

/// 한 쪽의 글 수예요.
const PAGE: usize = 100;
/// 다음 쪽을 받기 전에 남아 있어야 할 포인트예요: 요청 1 + 글 100.
const RESERVE: u32 = 1 + PAGE as u32;
/// 바뀐 것 받기의 겹침(분)이에요.
const OVERLAP: i64 = 15;
/// 글 파일 종류예요(`items/page-<id>.json`, 블로그도 같아요).
const KIND: &str = "page";

/// `accessible-resources`에서 이 사이트(`cloud`)의 제품이에요. 같은 사이트가 제품마다 한 줄씩 오니(docs/31 10-1절)
/// 같은 id의 줄을 합쳐 봐요.
pub(crate) fn products(resources: &Value, cloud: &str) -> Products {
    let scopes: HashSet<&str> = resources
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["id"].as_str() == Some(cloud))
        .flat_map(|r| r["scopes"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    Products {
        jira: scopes.contains("read:jira-work"),
        confluence: scopes.contains("read:page:confluence"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum List {
    Pages,
    Blogs,
    Footer,
    Inline,
}

const LISTS: [List; 4] = [List::Pages, List::Blogs, List::Footer, List::Inline];

impl List {
    fn path(self) -> &'static str {
        match self {
            List::Pages => "pages",
            List::Blogs => "blogposts",
            List::Footer => "footer-comments",
            List::Inline => "inline-comments",
        }
    }

    /// 받는 상태예요. 댓글은 기본(`current`)이에요.
    fn status(self) -> Option<&'static str> {
        match self {
            List::Pages => Some("current,archived"),
            List::Blogs => Some("current"),
            List::Footer | List::Inline => None,
        }
    }

    /// 댓글의 `type`이에요. 글 목록이면 None이에요.
    fn comment_type(self) -> Option<&'static str> {
        match self {
            List::Footer => Some("footer"),
            List::Inline => Some("inline"),
            List::Pages | List::Blogs => None,
        }
    }

    fn cursor(self, s: &State) -> &Cursor {
        match self {
            List::Pages => &s.pages,
            List::Blogs => &s.blogs,
            List::Footer => &s.footer,
            List::Inline => &s.inline,
        }
    }

    fn cursor_mut(self, s: &mut State) -> &mut Cursor {
        match self {
            List::Pages => &mut s.pages,
            List::Blogs => &mut s.blogs,
            List::Footer => &mut s.footer,
            List::Inline => &mut s.inline,
        }
    }
}

/// 받기 한 번 동안 들고 다니는 것이에요.
#[derive(Default)]
struct Ctx {
    /// 이번 받기에서 Confluence를 부른 수예요(첫 요청의 404만 "Confluence 없음"으로 봐요).
    calls: usize,
    /// 이번 받기에서 쓴 글 id예요.
    fresh: HashSet<String>,
    /// 이번 받기에서 쓴 글·댓글 수와 넘긴 글·댓글 수예요(진행률).
    written: usize,
    seen: usize,
}

/// 한 목록을 넘긴 결과예요.
enum Flow {
    Next,
    Later,
    /// 첫 요청이 404예요: 이 사이트에 Confluence가 없어요.
    Off,
}

/// 목록 한 쪽이에요.
enum Listed {
    Page(Vec<Value>, Option<String>),
    /// 이번 받기의 첫 요청이 404예요.
    Missing,
    /// 넘긴 `cursor`를 받지 않았어요(400). 저장한 커서가 오래됐을 때예요.
    BadCursor,
}

fn list(
    run: &mut Run,
    ctx: &mut Ctx,
    l: List,
    status: Option<&str>,
    cursor: Option<&str>,
) -> Result<Listed, SourceError> {
    run.guard()?;
    let mut q = vec![
        ("sort", "-modified-date".to_string()),
        ("body-format", "atlas_doc_format".to_string()),
        ("limit", PAGE.to_string()),
    ];
    if let Some(s) = status {
        q.push(("status", s.to_string()));
    }
    if let Some(c) = cursor {
        q.push(("cursor", c.to_string()));
    }
    let path = format!("ex/confluence/{}/wiki/api/v2/{}", run.cloud, l.path());
    let got = run.api.get(&path, &q)?;
    let first = ctx.calls == 0;
    ctx.calls += 1;
    if first && matches!(got, Got::Fail { status: 404, .. }) {
        return Ok(Listed::Missing);
    }
    if cursor.is_some() && matches!(got, Got::Fail { status: 400, .. }) {
        run.state.points.charge(run.now, 1, 0, 0);
        return Ok(Listed::BadCursor);
    }
    let v = json_of(got, "confluence list")?;
    let items = v["results"].as_array().cloned().unwrap_or_default();
    let objects = u32::try_from(items.len()).unwrap_or(u32::MAX);
    run.state.points.charge(run.now, 1, objects, 0);
    let next = v["_links"]["next"].as_str().and_then(next_cursor);
    ctx.seen += items.len();
    Ok(Listed::Page(items, next))
}

/// 목록이 방금 받은 커서를 받지 않았어요(`Down`).
fn refused_cursor() -> SourceError {
    SourceError::service(
        crate::error::Kind::Down,
        "atlassian: confluence list refused its cursor",
    )
}

/// `_links.next`(예: `/wiki/api/v2/pages?limit=100&cursor=abc`)의 `cursor`예요.
fn next_cursor(link: &str) -> Option<String> {
    let (_, query) = link.split_once('?')?;
    form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "cursor")
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
}

/// 글자나 숫자로 온 id예요.
pub(crate) fn sid(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 수정 시각(`version.createdAt`)이에요.
pub(crate) fn modified(item: &Value) -> Option<DateTime<Utc>> {
    item["version"]["createdAt"].as_str().and_then(parse)
}

/// 개인정보 보고에 쓸 accountId예요: 글쓴이, 주인, 고친 사람, 댓글 작성자, 본문·댓글의 멘션.
pub(crate) fn accounts(page: &Value, comments: &[Value]) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for item in std::iter::once(page).chain(comments) {
        for v in [
            &item["authorId"],
            &item["ownerId"],
            &item["version"]["authorId"],
        ] {
            if let Some(id) = v.as_str().filter(|s| !s.is_empty()) {
                ids.insert(id.to_string());
            }
        }
        if let Some(doc) = adf::parse_doc(&item["body"]["atlas_doc_format"]["value"]) {
            ids.extend(adf::mentions(&doc).into_iter().map(|(id, _)| id));
        }
    }
    ids.into_iter().collect()
}

pub(crate) fn comments_of(file: &Value) -> Vec<Value> {
    file["comments"].as_array().cloned().unwrap_or_default()
}

/// 글 파일을 써요(`accounts`·`fetched_at`을 새로 채워요).
pub(crate) fn put(
    run: &Run,
    id: &str,
    kind: &str,
    page: Value,
    comments: Vec<Value>,
) -> Result<(), SourceError> {
    let file = json!({
        "id": id,
        "kind": kind,
        "accounts": accounts(&page, &comments),
        "page": page,
        "comments": comments,
        "fetched_at": iso(run.now),
    });
    run.write(KIND, id, &file)
}

/// 페이지·블로그 하나를 써요. 저장된 댓글은 그대로 둬요. 저장본과 판(`version`)이 같으면 쓰지 않아요.
fn put_page(run: &mut Run, ctx: &mut Ctx, l: List, item: &Value) -> Result<(), SourceError> {
    let Some(id) = sid(&item["id"]).filter(|i| safe(i)) else {
        return Ok(());
    };
    let old = read_item(run.dir, KIND, &id);
    if old
        .as_ref()
        .is_some_and(|o| o["page"]["version"] == item["version"])
    {
        return Ok(());
    }
    let kind = if l == List::Blogs { "blogpost" } else { "page" };
    let comments = old.as_ref().map(comments_of).unwrap_or_default();
    put(run, &id, kind, item.clone(), comments)?;
    ctx.fresh.insert(id);
    ctx.written += 1;
    Ok(())
}

/// 댓글이 붙은 글(id, 종류)이에요.
fn parent(c: &Value) -> Option<(String, &'static str)> {
    sid(&c["pageId"])
        .map(|p| (p, "page"))
        .or_else(|| sid(&c["blogPostId"]).map(|b| (b, "blogpost")))
        .filter(|(p, _)| safe(p))
}

/// 댓글 하나를 그 글의 파일에 id로 합쳐요. 그대로면 쓰지 않아요. 돌려주는 값은 붙은 글 id예요.
fn put_comment(run: &mut Run, l: List, c: &Value) -> Result<Option<String>, SourceError> {
    let (Some((pid, kind)), Some(cid), Some(ty)) = (parent(c), sid(&c["id"]), l.comment_type())
    else {
        return Ok(None);
    };
    let mut entry = c.clone();
    entry["type"] = json!(ty);
    if l == List::Inline {
        entry["quote"] = c["properties"]["inlineOriginalSelection"].clone();
    }
    let old = read_item(run.dir, KIND, &pid);
    let mut comments = old.as_ref().map(comments_of).unwrap_or_default();
    match comments
        .iter_mut()
        .find(|x| sid(&x["id"]).as_deref() == Some(&cid))
    {
        Some(x) if *x == entry => return Ok(Some(pid)),
        Some(x) => *x = entry,
        None => comments.push(entry),
    }
    comments.sort_by_key(|x| (modified(x), sid(&x["id"])));
    let page = old.as_ref().map_or(Value::Null, |o| o["page"].clone());
    let kind = old
        .as_ref()
        .and_then(|o| o["kind"].as_str())
        .unwrap_or(kind)
        .to_string();
    put(run, &pid, &kind, page, comments)?;
    Ok(Some(pid))
}

/// 이번 받기에서 쓴 글의 `ty` 댓글을 이번에 본 것(`seen`)으로 바꿔 써요. 그 목록을 처음부터 끝까지 넘겼을 때만 불러요.
fn prune(
    run: &mut Run,
    ctx: &Ctx,
    ty: &str,
    seen: &HashMap<String, HashSet<String>>,
) -> Result<(), SourceError> {
    let none = HashSet::new();
    for pid in &ctx.fresh {
        let Some(file) = read_item(run.dir, KIND, pid) else {
            continue;
        };
        let keep = seen.get(pid).unwrap_or(&none);
        let comments = comments_of(&file);
        let kept: Vec<Value> = comments
            .iter()
            .filter(|c| {
                c["type"].as_str() != Some(ty) || sid(&c["id"]).is_some_and(|i| keep.contains(&i))
            })
            .cloned()
            .collect();
        if kept.len() != comments.len() {
            let kind = file["kind"].as_str().unwrap_or("page").to_string();
            put(run, pid, &kind, file["page"].clone(), kept)?;
        }
    }
    Ok(())
}

/// 글 본문까지 가진 글 id예요(댓글만 있는 파일은 빼요).
fn held_pages(run: &Run) -> HashSet<String> {
    item_ids(run.dir, KIND)
        .into_iter()
        .filter(|id| read_item(run.dir, KIND, id).is_some_and(|f| f["page"].is_object()))
        .collect()
}

/// 이 사이트에 Confluence가 없어요: 끄고 저장한 뒤 오류 없이 끝내요.
fn turn_off(run: &mut Run) -> Result<Step, SourceError> {
    let mut p = run.state.products;
    p.confluence = false;
    run.state.products = p;
    run.save_products(p)?;
    Ok(Step::Done)
}

/// 처음 받기예요(목록마다 맨 위부터, 예산 안에서).
pub(crate) fn initial(run: &mut Run) -> Result<Step, SourceError> {
    if !run.state.products.confluence || LISTS.iter().all(|l| l.cursor(&run.state).done) {
        return Ok(Step::Done);
    }
    if run.paused() {
        return Ok(Step::Later);
    }
    match initial_lists(run) {
        Err(e) => run.pause_on_quota(e),
        ok => ok,
    }
}

fn initial_lists(run: &mut Run) -> Result<Step, SourceError> {
    let mut ctx = Ctx::default();
    let mut held = held_pages(run);
    for l in LISTS {
        if l.cursor(&run.state).done {
            continue;
        }
        match initial_list(run, &mut ctx, l, &mut held)? {
            Flow::Next => {}
            Flow::Later => return Ok(Step::Later),
            Flow::Off => return turn_off(run),
        }
    }
    Ok(Step::Done)
}

fn initial_list(
    run: &mut Run,
    ctx: &mut Ctx,
    l: List,
    held: &mut HashSet<String>,
) -> Result<Flow, SourceError> {
    let edge = run.now - Duration::days(WINDOW_DAYS);
    // 지난번에 멈춘 쪽부터 이어요. 그 자리보다 위로 올라간 글은 `head` 뒤에 바뀐 글이라 바뀐 것 받기가 가져가요.
    let mut cursor: Option<String> = l.cursor(&run.state).next.clone();
    // 맨 위부터 넘겼는지예요. 그때만 이번에 쓴 글의 댓글을 이번에 본 것으로 바꿔 써요.
    let mut from_top = cursor.is_none();
    let mut restarted = false;
    // 이번에 쓴 글마다 이번에 본 댓글 id예요(목록 끝까지 넘기면 그걸로 바꿔 써요).
    let mut seen: HashMap<String, HashSet<String>> = HashMap::new();
    loop {
        if run.state.points.left(run.now) < RESERVE {
            run.save()?;
            return Ok(Flow::Later);
        }
        let (items, next) = match list(run, ctx, l, l.status(), cursor.as_deref())? {
            Listed::Page(items, next) => (items, next),
            Listed::Missing => return Ok(Flow::Off),
            // 저장한 커서를 받지 않으면 지우고 맨 위부터 한 번 다시 넘겨요.
            Listed::BadCursor if !restarted => {
                restarted = true;
                from_top = true;
                cursor = None;
                l.cursor_mut(&mut run.state).next = None;
                continue;
            }
            Listed::BadCursor => {
                return Err(refused_cursor());
            }
        };
        let mut old = false;
        for item in &items {
            let Some(t) = modified(item) else { continue };
            if t < edge {
                old = true;
                break;
            }
            let c = l.cursor_mut(&mut run.state);
            if c.head.is_none() {
                c.head = Some(iso(t));
            }
            if c.floor.as_deref().and_then(parse).is_none_or(|f| t < f) {
                c.floor = Some(iso(t));
            }
            if l.comment_type().is_some() {
                let cid = sid(&item["id"]);
                if let Some(pid) = put_comment(run, l, item)?
                    && ctx.fresh.contains(&pid)
                    && let Some(cid) = cid
                {
                    seen.entry(pid).or_default().insert(cid);
                }
            } else if let Some(id) = sid(&item["id"])
                && !held.contains(&id)
            {
                put_page(run, ctx, l, item)?;
                held.insert(id);
            }
        }
        (run.progress)(ctx.written, ctx.seen);
        run.state.fetched_at = Some(iso(run.now));
        let Some(next) = next.filter(|_| !old) else {
            let c = l.cursor_mut(&mut run.state);
            c.done = true;
            c.next = None;
            if from_top && let Some(ty) = l.comment_type() {
                prune(run, ctx, ty, &seen)?;
            }
            run.save()?;
            return Ok(Flow::Next);
        };
        l.cursor_mut(&mut run.state).next = Some(next.clone());
        cursor = Some(next);
        run.save()?;
    }
}

/// 바뀐 것 받기예요(예산 밖, 장부에는 적어요).
pub(crate) fn changes(run: &mut Run) -> Result<Step, SourceError> {
    if !run.state.products.confluence {
        return Ok(Step::Done);
    }
    if run.paused() {
        return Ok(Step::Later);
    }
    match change_lists(run) {
        Err(e) => run.pause_on_quota(e),
        ok => ok,
    }
}

/// 바뀐 것을 받는 기준이에요: `head − 15분`. `head`가 없으면 처음 받기가 정하고(None), 처음 받기가 빈 목록으로
/// 끝났으면 2년 전부터예요.
fn since(run: &Run, l: List) -> Option<DateTime<Utc>> {
    let c = l.cursor(&run.state);
    match c.head.as_deref().and_then(parse) {
        Some(h) => Some(h - Duration::minutes(OVERLAP)),
        None if c.done => Some(run.now - Duration::days(WINDOW_DAYS)),
        None => None,
    }
}

fn change_lists(run: &mut Run) -> Result<Step, SourceError> {
    let mut ctx = Ctx::default();
    for l in LISTS {
        let Some(since) = since(run, l) else { continue };
        if let Flow::Off = change_list(run, &mut ctx, l, since)? {
            return turn_off(run);
        }
        if l.comment_type().is_none() {
            remove_gone(run, &mut ctx, l, since)?;
        }
    }
    Ok(Step::Done)
}

fn change_list(
    run: &mut Run,
    ctx: &mut Ctx,
    l: List,
    since: DateTime<Utc>,
) -> Result<Flow, SourceError> {
    let mut cursor: Option<String> = None;
    let mut newest: Option<DateTime<Utc>> = None;
    loop {
        let (items, next) = match list(run, ctx, l, l.status(), cursor.as_deref())? {
            Listed::Page(items, next) => (items, next),
            Listed::Missing => return Ok(Flow::Off),
            Listed::BadCursor => return Err(refused_cursor()),
        };
        let mut reached = false;
        for item in &items {
            let Some(t) = modified(item) else { continue };
            if t <= since {
                reached = true;
                break;
            }
            newest = newest.max(Some(t));
            if l.comment_type().is_some() {
                put_comment(run, l, item)?;
            } else {
                put_page(run, ctx, l, item)?;
            }
        }
        (run.progress)(ctx.written, ctx.seen);
        run.state.fetched_at = Some(iso(run.now));
        let Some(next) = next.filter(|_| !reached) else {
            // 끝까지 넘긴 뒤에만 `head`를 올려요(최근 것부터라 도중에 올리면 아래쪽을 놓쳐요).
            let c = l.cursor_mut(&mut run.state);
            if let Some(n) = newest
                && c.head.as_deref().and_then(parse).is_none_or(|h| n > h)
            {
                c.head = Some(iso(n));
            }
            run.save()?;
            return Ok(Flow::Next);
        };
        cursor = Some(next);
        run.save()?;
    }
}

/// 휴지통에 들어가거나 지운 페이지·블로그의 파일을 지워요(`since`까지).
fn remove_gone(
    run: &mut Run,
    ctx: &mut Ctx,
    l: List,
    since: DateTime<Utc>,
) -> Result<(), SourceError> {
    let mut cursor: Option<String> = None;
    loop {
        let (items, next) = match list(run, ctx, l, Some("trashed,deleted"), cursor.as_deref())? {
            Listed::Page(items, next) => (items, next),
            Listed::Missing => return Ok(()),
            Listed::BadCursor => return Err(refused_cursor()),
        };
        let mut reached = false;
        for item in &items {
            if modified(item).is_some_and(|t| t <= since) {
                reached = true;
                break;
            }
            if let Some(id) = sid(&item["id"]) {
                run.remove(KIND, &id)?;
            }
        }
        let Some(next) = next.filter(|_| !reached) else {
            run.save()?;
            return Ok(());
        };
        cursor = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::Ordering;

    use chrono::{DateTime, Duration, Utc};
    use serde_json::{Value, json};

    use crate::atlassian::api::{POINTS_PER_HOUR, Points};
    use crate::atlassian::state::{State, item_ids, load_state, read_item, save_state};
    use crate::testing::atlassian::FakeAtlassian;

    fn at(t: DateTime<Utc>) -> String {
        t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    /// 하루 앞이에요. 2년 창 안에 들게 지금 가까이에 둬요.
    fn base() -> DateTime<Utc> {
        Utc::now() - Duration::days(1)
    }

    fn adf(text: &str) -> Value {
        json!({"representation": "atlas_doc_format", "value": json!({
            "type": "doc", "version": 1,
            "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]
        }).to_string()})
    }

    fn page(id: &str, t: DateTime<Utc>) -> Value {
        json!({
            "id": id, "status": "current", "title": format!("page {id}"), "spaceId": "9",
            "authorId": "u-author",
            "version": {"number": 1, "createdAt": at(t), "authorId": "u-editor"},
            "body": {"atlas_doc_format": adf("body")},
            "_links": {"webui": format!("/spaces/S/pages/{id}")}
        })
    }

    fn blog(id: &str, t: DateTime<Utc>) -> Value {
        let mut b = page(id, t);
        b["title"] = json!(format!("blog {id}"));
        b
    }

    fn comment(id: &str, parent: (&str, &str), t: DateTime<Utc>) -> Value {
        let mut c = json!({
            "id": id, "status": "current",
            "version": {"number": 1, "createdAt": at(t), "authorId": "u-commenter"},
            "body": {"atlas_doc_format": adf("comment")}
        });
        c[parent.0] = json!(parent.1);
        c
    }

    fn on(dir: &Path) {
        let mut s = State::default();
        s.products.confluence = true;
        save_state(dir, &s).unwrap();
    }

    fn go(
        f: &FakeAtlassian,
        dir: &Path,
        prep: impl FnOnce(&mut State),
        what: fn(&mut Run) -> Result<Step, SourceError>,
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
        what(&mut run)
    }

    fn comment_ids(item: &Value) -> Vec<String> {
        item["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_string())
            .collect()
    }

    /// 길 끝(`pages` 등)마다 부른 수예요.
    fn count(f: &FakeAtlassian, list: &str) -> usize {
        f.calls()
            .iter()
            .filter(|c| c.ends_with(&format!("/wiki/api/v2/{list}")))
            .count()
    }

    #[test]
    fn pages_blogs_and_comments_land_in_one_file_per_page() {
        let f = FakeAtlassian::new();
        let b = base();
        let mut p = page("11", b);
        p["body"]["atlas_doc_format"]["value"] = json!(
            json!({
                "type": "doc", "version": 1,
                "content": [{"type": "paragraph", "content": [
                    {"type": "mention", "attrs": {"id": "u-mentioned", "text": "@Kim"}}
                ]}]
            })
            .to_string()
        );
        *f.pages.lock().unwrap() = vec![p, page("12", b - Duration::hours(1))];
        *f.blogs.lock().unwrap() = vec![blog("21", b - Duration::hours(2))];
        *f.footer.lock().unwrap() = vec![
            comment("31", ("pageId", "11"), b),
            comment("32", ("blogPostId", "21"), b),
        ];
        let mut inline = comment("41", ("pageId", "11"), b - Duration::minutes(5));
        inline["properties"] = json!({"inlineOriginalSelection": "the quoted words"});
        *f.inline.lock().unwrap() = vec![inline];
        // 2년보다 오래된 페이지는 받지 않아요.
        f.pages
            .lock()
            .unwrap()
            .push(page("13", Utc::now() - Duration::days(800)));
        let d = tempfile::tempdir().unwrap();
        on(d.path());

        assert_eq!(go(&f, d.path(), |_| {}, initial).unwrap(), Step::Done);
        assert_eq!(item_ids(d.path(), "page"), ["11", "12", "21"]);

        let p = read_item(d.path(), "page", "11").unwrap();
        assert_eq!(p["kind"], "page");
        assert_eq!(p["page"]["title"], "page 11");
        assert_eq!(comment_ids(&p), ["41", "31"]);
        let inline = &p["comments"][0];
        assert_eq!(inline["type"], "inline");
        assert_eq!(inline["quote"], "the quoted words");
        assert_eq!(p["comments"][1]["type"], "footer");
        let accounts: Vec<&str> = p["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        assert_eq!(
            accounts,
            ["u-author", "u-commenter", "u-editor", "u-mentioned"]
        );

        let bl = read_item(d.path(), "page", "21").unwrap();
        assert_eq!(bl["kind"], "blogpost");
        assert_eq!(comment_ids(&bl), ["32"]);

        let s = load_state(d.path());
        for c in [&s.pages, &s.blogs, &s.footer, &s.inline] {
            assert!(c.done);
            assert!(c.head.is_some());
        }
        assert_eq!(
            s.pages
                .head
                .as_deref()
                .and_then(crate::gdrive::fetch::parse),
            Some(crate::gdrive::fetch::parse(&at(b)).unwrap())
        );
        // 쪽 하나에 요청 1 + 글 수예요(2년 넘은 글도 받은 것이라 세요).
        assert_eq!(s.points.used, (1 + 3) + (1 + 1) + (1 + 2) + (1 + 1));
    }

    /// `n`개의 페이지예요. `100`부터 1분씩 앞으로 둬요.
    fn pages(f: &FakeAtlassian, n: i64) {
        let b = base();
        *f.pages.lock().unwrap() = (0..n)
            .map(|i| page(&(100 + i).to_string(), b - Duration::minutes(i)))
            .collect();
    }

    /// 이 시간에 `pages`쪽만 받을 만큼 남긴 장부예요(쪽 하나에 101점, 어림값도 101점).
    fn budget(pages: u32) -> Points {
        Points {
            used: POINTS_PER_HOUR - (101 * pages + 49),
            hour: Utc::now().timestamp() / 3600,
        }
    }

    #[test]
    fn the_first_fetch_stops_at_the_budget_and_resumes_from_its_cursor() {
        let f = FakeAtlassian::new();
        pages(&f, 150);
        let d = tempfile::tempdir().unwrap();
        on(d.path());
        assert_eq!(
            go(&f, d.path(), |s| s.points = budget(1), initial).unwrap(),
            Step::Later
        );
        assert_eq!(item_ids(d.path(), "page").len(), 100);
        let s = load_state(d.path());
        assert!(!s.pages.done);
        assert!(s.pages.head.is_some());
        assert_eq!(s.pages.next.as_deref(), Some("100"));

        f.log.lock().unwrap().clear();
        assert_eq!(
            go(&f, d.path(), |s| s.points = Points::default(), initial).unwrap(),
            Step::Done
        );
        assert_eq!(item_ids(d.path(), "page").len(), 150);
        // 저장한 커서에서 이어요(둘째 쪽 하나만).
        assert_eq!(count(&f, "pages"), 1);
        let s = load_state(d.path());
        assert!(s.pages.done);
        assert!(s.pages.next.is_none());
    }

    #[test]
    fn a_list_bigger_than_an_hour_finishes_over_several_runs_without_repaging() {
        let f = FakeAtlassian::new();
        pages(&f, 950);
        let d = tempfile::tempdir().unwrap();
        on(d.path());
        let mut runs = 0;
        loop {
            runs += 1;
            assert!(runs <= 10, "never finished");
            if go(&f, d.path(), |s| s.points = budget(2), initial).unwrap() == Step::Done {
                break;
            }
        }
        assert_eq!(item_ids(d.path(), "page").len(), 950);
        // 쪽 10개를 시간마다 둘씩 받아요(다섯 번). 여섯 번째에 남은 블로그·댓글 목록을 받아요. 이미 넘긴 쪽을 다시
        // 받지 않아서 요청 수는 쪽 수 그대로예요.
        assert_eq!(runs, 6);
        assert!(count(&f, "pages") <= 10, "{}", count(&f, "pages"));
    }

    #[test]
    fn a_refused_stored_cursor_restarts_from_the_top() {
        let f = FakeAtlassian::new();
        pages(&f, 150);
        let d = tempfile::tempdir().unwrap();
        on(d.path());
        let mut s = load_state(d.path());
        s.pages.next = Some("stale-cursor".into());
        s.pages.head = Some(at(base()));
        save_state(d.path(), &s).unwrap();
        assert_eq!(go(&f, d.path(), |_| {}, initial).unwrap(), Step::Done);
        assert_eq!(item_ids(d.path(), "page").len(), 150);
        // 거절된 요청 1 + 맨 위부터 두 쪽이에요.
        assert_eq!(count(&f, "pages"), 3);
        let s = load_state(d.path());
        assert!(s.pages.done);
        assert!(s.pages.next.is_none());
    }

    #[test]
    fn changes_stop_at_the_cursor() {
        let f = FakeAtlassian::new();
        let b = base();
        // 두 쪽이 넘는 목록이에요. 바뀐 게 없으면 첫 쪽에서 멈춰요.
        *f.pages.lock().unwrap() = (0..150)
            .map(|i| page(&(100 + i).to_string(), b - Duration::hours(i)))
            .collect();
        *f.blogs.lock().unwrap() = vec![blog("21", b)];
        *f.footer.lock().unwrap() = vec![comment("31", ("pageId", "100"), b)];
        *f.inline.lock().unwrap() = vec![comment("41", ("pageId", "101"), b)];
        let d = tempfile::tempdir().unwrap();
        on(d.path());
        assert_eq!(go(&f, d.path(), |_| {}, initial).unwrap(), Step::Done);

        f.log.lock().unwrap().clear();
        assert_eq!(go(&f, d.path(), |_| {}, changes).unwrap(), Step::Done);
        // 페이지·블로그는 지금 목록 + 휴지통·지운 목록이에요.
        assert_eq!(count(&f, "pages"), 2);
        assert_eq!(count(&f, "blogposts"), 2);
        assert_eq!(count(&f, "footer-comments"), 1);
        assert_eq!(count(&f, "inline-comments"), 1);

        // 오래된 페이지 하나가 바뀌어 맨 위로 와요. 새 댓글은 그 페이지에 합쳐지고 옛 댓글은 남아요.
        let later = Utc::now() - Duration::minutes(1);
        {
            let mut pages = f.pages.lock().unwrap();
            let old = pages.iter_mut().find(|p| p["id"] == "140").unwrap();
            old["title"] = json!("renamed");
            old["version"] = json!({"number": 2, "createdAt": at(later), "authorId": "u-editor"});
        }
        f.footer
            .lock()
            .unwrap()
            .push(comment("33", ("pageId", "100"), later));
        assert_eq!(go(&f, d.path(), |_| {}, changes).unwrap(), Step::Done);
        assert_eq!(
            read_item(d.path(), "page", "140").unwrap()["page"]["title"],
            "renamed"
        );
        assert_eq!(
            comment_ids(&read_item(d.path(), "page", "100").unwrap()),
            ["31", "33"]
        );
        let s = load_state(d.path());
        assert_eq!(
            s.pages
                .head
                .as_deref()
                .and_then(crate::gdrive::fetch::parse),
            crate::gdrive::fetch::parse(&at(later))
        );
    }

    #[test]
    fn a_page_written_with_its_whole_comment_list_drops_gone_comments() {
        let f = FakeAtlassian::new();
        let b = base();
        *f.pages.lock().unwrap() = vec![page("11", b), page("12", b)];
        *f.footer.lock().unwrap() = vec![comment("31", ("pageId", "11"), b)];
        let d = tempfile::tempdir().unwrap();
        on(d.path());
        let stale = |pid: &str| {
            let mut c = comment("39", ("pageId", pid), b - Duration::hours(3));
            c["type"] = json!("footer");
            c
        };
        // 11은 댓글만 있는 파일이라 이번에 다시 받아요. 12는 이미 가진 글이라 받지 않아요.
        crate::atlassian::state::write_item(
            d.path(),
            "page",
            "11",
            &json!({"id": "11", "kind": "page", "page": null, "comments": [stale("11")]}),
        )
        .unwrap();
        let mut held = page("12", b);
        held["version"]["number"] = json!(1);
        crate::atlassian::state::write_item(
            d.path(),
            "page",
            "12",
            &json!({"id": "12", "kind": "page", "page": held, "comments": [stale("12")]}),
        )
        .unwrap();

        assert_eq!(go(&f, d.path(), |_| {}, initial).unwrap(), Step::Done);
        // 11은 이번에 글과 댓글 목록을 다 받아서 이번에 본 댓글만 남아요.
        assert_eq!(
            comment_ids(&read_item(d.path(), "page", "11").unwrap()),
            ["31"]
        );
        // 12는 이번에 쓰지 않아서 댓글을 그대로 둬요.
        assert_eq!(
            comment_ids(&read_item(d.path(), "page", "12").unwrap()),
            ["39"]
        );
    }

    #[test]
    fn trashed_and_deleted_are_removed() {
        let f = FakeAtlassian::new();
        let b = base();
        *f.pages.lock().unwrap() = vec![page("11", b), page("12", b - Duration::hours(1))];
        *f.blogs.lock().unwrap() = vec![blog("21", b)];
        let d = tempfile::tempdir().unwrap();
        on(d.path());
        assert_eq!(go(&f, d.path(), |_| {}, initial).unwrap(), Step::Done);
        assert_eq!(item_ids(d.path(), "page"), ["11", "12", "21"]);

        let later = Utc::now() - Duration::minutes(1);
        {
            let mut pages = f.pages.lock().unwrap();
            pages[0]["status"] = json!("trashed");
            pages[0]["version"]["createdAt"] = json!(at(later));
            pages[1]["status"] = json!("deleted");
            pages[1]["version"]["createdAt"] = json!(at(later));
            let mut blogs = f.blogs.lock().unwrap();
            blogs[0]["status"] = json!("trashed");
            blogs[0]["version"]["createdAt"] = json!(at(later));
        }
        assert_eq!(go(&f, d.path(), |_| {}, changes).unwrap(), Step::Done);
        assert!(item_ids(d.path(), "page").is_empty());
    }

    #[test]
    fn a_jira_only_site_never_calls_confluence() {
        let resources = json!([
            {"id": "cloud-1", "url": "https://acme.atlassian.net", "name": "acme",
             "scopes": ["read:jira-work", "read:jira-user"]}
        ]);
        let p = products(&resources, "cloud-1");
        assert!(p.jira);
        assert!(!p.confluence);

        let f = FakeAtlassian::new();
        f.confluence.store(false, Ordering::SeqCst);
        let d = tempfile::tempdir().unwrap();
        let s = State {
            products: p,
            ..State::default()
        };
        save_state(d.path(), &s).unwrap();
        assert_eq!(go(&f, d.path(), |_| {}, initial).unwrap(), Step::Done);
        assert_eq!(go(&f, d.path(), |_| {}, changes).unwrap(), Step::Done);
        assert!(f.calls().is_empty());
    }

    #[test]
    fn a_404_turns_confluence_off_without_an_error() {
        let f = FakeAtlassian::new();
        f.confluence.store(false, Ordering::SeqCst);
        let d = tempfile::tempdir().unwrap();
        on(d.path());
        assert_eq!(go(&f, d.path(), |_| {}, initial).unwrap(), Step::Done);
        assert_eq!(f.calls().len(), 1);
        let s = load_state(d.path());
        assert!(!s.products.confluence);
        assert_eq!(s.cloud.as_deref(), Some("cloud-1"));
        // 다음에는 부르지 않아요.
        assert_eq!(go(&f, d.path(), |_| {}, changes).unwrap(), Step::Done);
        assert_eq!(f.calls().len(), 1);
    }

    #[test]
    fn products_merges_duplicate_rows() {
        let resources = json!([
            {"id": "cloud-1", "url": "https://acme.atlassian.net", "name": "acme",
             "scopes": ["read:jira-work", "read:jira-user"]},
            {"id": "cloud-1", "url": "https://acme.atlassian.net", "name": "acme",
             "scopes": ["read:page:confluence", "read:comment:confluence", "read:label:confluence",
                        "read:space:confluence", "report:personal-data"]},
            {"id": "cloud-2", "scopes": ["read:jira-work"]}
        ]);
        let p = products(&resources, "cloud-1");
        assert!(p.jira && p.confluence);
        let other = products(&resources, "cloud-2");
        assert!(other.jira && !other.confluence);
        assert_eq!(products(&resources, "cloud-9"), Products::default());
    }
}
