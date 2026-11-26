//! Privacy reporting (docs/31 1-6절, 6절, 10-2절. 10-2 wins).
//!
//! - **When**: once `reported_at + report_every` (7 days by default, following the `Cycle-Period` header) has passed.
//!   If it fell behind while off for a long time, it runs before fetching on the first sync after reconnecting. The
//!   caller checks `report_due` first and then calls `report`.
//! - **What**: collects, from every item file, `accounts`, the `accountId` of person objects and `attrs.id` of ADF
//!   mentions (objects in Jira descriptions and comments, JSON text in `atlas_doc_format.value` for Confluence bodies
//!   and comments). It scans item content again so files written before fetches put mentions into `accounts` are not
//!   missed.
//! - **How**: sends `{accountId, updatedAt}` to `POST app/report-accounts/` in batches of 90. `updatedAt` is the latest
//!   time an item mentioning that person was fetched (`fetched_at`). 204 means nothing to do.
//!   - `closed`: in every item file that mentions the person, the name becomes `탈퇴한 사용자` (person objects'
//!     `displayName`, mentions' `attrs.text`) and the accountId is removed (`accounts`, person objects'
//!     `accountId`/`emailAddress`/`avatarUrls`, mentions' `attrs.id`, and fields whose value is that id, such as
//!     Confluence `authorId`/`ownerId`).
//!   - `updated`: does not refetch items; it only refetches names through `rest/api/3/user/bulk` (90 people at a time,
//!     2 points per person) and replaces them. Sites without Jira keep no person names in the raw data (Confluence v2
//!     gives only `authorId`), so it does not fetch.
//!   - Each batch of 90 is rewritten as soon as its answer arrives (if a later batch stops, earlier batches' closed
//!     accounts are already removed).
//!   - Closed ids are kept in `state.closed`. When fetches and reconciles write item files (`Run::write`) they remove
//!     these people, so refetching does not bring them back.
//!   - Text anchored by an inline comment (`quote`) and names written as plain text in a body cannot be changed. It
//!     cannot tell whether they are names or ordinary text. Only person objects and mentions are changed.
//! - Reports (`report-accounts`) are not Jira or Confluence points, so they are not recorded in the ledger. The user
//!   API is recorded in the ledger (outside the budget).
//! - Reports run even while paused by the hourly limit (they are outside the points). Only the user API is skipped,
//!   and then `reported_at` stays as it is so the next sync does it again. If the user API hits the limit midway, the
//!   pause is saved and `reported_at` stays as it is (`closed` replacement gives the same result however many times it
//!   runs).
//!
//! Names and ids are never put in logs or errors.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Duration, Utc};
use serde_json::{Map, Value, json};

use crate::atlassian::adf;
use crate::atlassian::state::{Run, State, iso, item_ids, json_of, read_item};
use crate::error::{Kind, SourceError};
use crate::gdrive::fetch::parse;

/// 보고 간격의 기본값(초, 7일)이에요.
pub(crate) const REPORT_EVERY: u64 = 604_800;
/// 한 번에 보고하는 계정 수예요(Atlassian 상한).
const BATCH: usize = 90;
/// 탈퇴한 사람의 이름 자리에 두는 말이에요.
pub(crate) const CLOSED_NAME: &str = "탈퇴한 사용자";
/// 글 파일 종류예요.
const KINDS: [&str; 2] = ["jira", "page"];

/// 보고할 때가 됐는지예요. 한 번도 안 했으면 늘 때예요(보고할 사람이 없으면 `report`가 아무것도 부르지 않아요).
pub(crate) fn report_due(state: &State, now: DateTime<Utc>) -> bool {
    let every = i64::try_from(state.report_every.unwrap_or(REPORT_EVERY)).unwrap_or(i64::MAX);
    match state.reported_at.as_deref().and_then(parse) {
        None => true,
        Some(t) => now >= t + Duration::seconds(every.min(i64::from(u32::MAX))),
    }
}

/// 글 파일 안의 모든 객체를 돌며 `f`를 불러요. Confluence 본문처럼 JSON 글자로 든 ADF(`value`)는 풀어서 돌고,
/// 바뀌었으면 다시 글자로 둬요. 무언가 바뀌었는지를 돌려줘요.
fn visit(v: &mut Value, f: &mut dyn FnMut(&mut Map<String, Value>) -> bool) -> bool {
    match v {
        Value::Array(a) => {
            let mut changed = false;
            for x in a {
                changed |= visit(x, f);
            }
            changed
        }
        Value::Object(m) => {
            let mut changed = f(m);
            for (k, x) in m.iter_mut() {
                if k == "value"
                    && matches!(x, Value::String(s) if s.trim_start().starts_with('{'))
                    && let Some(mut doc) = adf::parse_doc(x)
                {
                    if visit(&mut doc, f) {
                        *x = Value::String(doc.to_string());
                        changed = true;
                    }
                } else {
                    changed |= visit(x, f);
                }
            }
            changed
        }
        _ => false,
    }
}

fn is_mention(m: &Map<String, Value>) -> bool {
    m.get("type").and_then(Value::as_str) == Some("mention")
}

/// 글 파일에 나온 사람의 accountId예요: `accounts`, 사람 객체의 `accountId`, 멘션의 `attrs.id`.
pub(crate) fn ids_of(item: &Value) -> BTreeSet<String> {
    let mut ids: BTreeSet<String> = item["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let mut copy = item.clone();
    visit(&mut copy, &mut |m| {
        if let Some(id) = m.get("accountId").and_then(Value::as_str) {
            ids.insert(id.to_string());
        }
        if is_mention(m)
            && let Some(id) = m
                .get("attrs")
                .and_then(|a| a.get("id"))
                .and_then(Value::as_str)
        {
            ids.insert(id.to_string());
        }
        false
    });
    ids.retain(|s| !s.is_empty());
    ids
}

/// 탈퇴한 사람 `id`를 글 파일에서 지워요: 이름은 `탈퇴한 사용자`로, accountId는 빼요. 바뀌었는지를 돌려줘요.
pub(crate) fn forget(item: &mut Value, id: &str) -> bool {
    let mut changed = false;
    if let Some(a) = item.get_mut("accounts").and_then(Value::as_array_mut) {
        let before = a.len();
        a.retain(|x| x.as_str() != Some(id));
        changed |= a.len() != before;
    }
    changed |= visit(item, &mut |m| {
        let mut hit = false;
        if is_mention(m)
            && let Some(attrs) = m.get_mut("attrs").and_then(Value::as_object_mut)
            && attrs.get("id").and_then(Value::as_str) == Some(id)
        {
            attrs.remove("id");
            attrs.insert("text".into(), json!(format!("@{CLOSED_NAME}")));
            hit = true;
        }
        if m.get("accountId").and_then(Value::as_str) == Some(id) {
            m.insert("displayName".into(), json!(CLOSED_NAME));
            for k in ["accountId", "emailAddress", "avatarUrls"] {
                m.remove(k);
            }
            hit = true;
        }
        // Confluence의 `authorId`·`ownerId`처럼 값이 그 사람의 id인 칸이에요.
        let keys: Vec<String> = m
            .iter()
            .filter(|(_, v)| v.as_str() == Some(id))
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            m.remove(&k);
            hit = true;
        }
        hit
    });
    changed
}

/// 사람 `id`의 이름을 `name`으로 바꿔요(사람 객체의 `displayName`, 멘션의 `attrs.text`). 바뀌었는지를 돌려줘요.
pub(crate) fn rename(item: &mut Value, id: &str, name: &str) -> bool {
    visit(item, &mut |m| {
        let mut hit = false;
        if is_mention(m)
            && let Some(attrs) = m.get_mut("attrs").and_then(Value::as_object_mut)
            && attrs.get("id").and_then(Value::as_str) == Some(id)
        {
            let text = json!(format!("@{name}"));
            if attrs.get("text") != Some(&text) {
                attrs.insert("text".into(), text);
                hit = true;
            }
        }
        if m.get("accountId").and_then(Value::as_str) == Some(id)
            && m.get("displayName").and_then(Value::as_str) != Some(name)
        {
            m.insert("displayName".into(), json!(name));
            hit = true;
        }
        hit
    })
}

/// 저장된 모든 글 파일이에요: (종류, id).
fn files(run: &Run) -> Vec<(&'static str, String)> {
    KINDS
        .iter()
        .flat_map(|k| item_ids(run.dir, k).into_iter().map(move |id| (*k, id)))
        .collect()
}

/// `ids`가 나온 글 파일마다 `change`를 불러 바뀐 것만 다시 써요.
fn rewrite(
    run: &Run,
    ids: &BTreeSet<String>,
    change: &dyn Fn(&mut Value, &str) -> bool,
) -> Result<(), SourceError> {
    if ids.is_empty() {
        return Ok(());
    }
    for (kind, fid) in files(run) {
        let Some(mut item) = read_item(run.dir, kind, &fid) else {
            continue;
        };
        let present = ids_of(&item);
        let mut changed = false;
        for id in ids.iter().filter(|i| present.contains(*i)) {
            changed |= change(&mut item, id);
        }
        if changed {
            run.write(kind, &fid, &item)?;
        }
    }
    Ok(())
}

/// 보고 답의 (closed, updated) 계정이에요.
fn statuses(body: &[u8]) -> Result<(BTreeSet<String>, BTreeSet<String>), SourceError> {
    let mut closed = BTreeSet::new();
    let mut updated = BTreeSet::new();
    if body.is_empty() {
        return Ok((closed, updated));
    }
    let v: Value = serde_json::from_slice(body)
        .map_err(|_| SourceError::service(Kind::Down, "atlassian: report answer is not JSON"))?;
    for a in v["accounts"].as_array().into_iter().flatten() {
        let Some(id) = a["accountId"].as_str().filter(|s| !s.is_empty()) else {
            continue;
        };
        match a["status"].as_str() {
            Some("closed") => {
                closed.insert(id.to_string());
            }
            Some("updated") => {
                updated.insert(id.to_string());
            }
            _ => {}
        }
    }
    Ok((closed, updated))
}

/// 개인정보 보고예요. 때가 아니어도 부르면 해요(때는 부르는 쪽이 `report_due`로 봐요).
pub(crate) fn report(run: &mut Run) -> Result<(), SourceError> {
    match report_all(run) {
        Err(e) => run.pause_on_quota(e).map(|_| ()),
        ok => ok,
    }
}

fn report_all(run: &mut Run) -> Result<(), SourceError> {
    // 사람마다 그 사람이 나온 글을 가장 늦게 받은 때예요.
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (kind, fid) in files(run) {
        let Some(item) = read_item(run.dir, kind, &fid) else {
            continue;
        };
        let at = item["fetched_at"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| iso(run.now));
        for id in ids_of(&item) {
            let e = seen.entry(id).or_insert_with(|| at.clone());
            if parse(&at) > parse(e) {
                *e = at.clone();
            }
        }
    }
    if seen.is_empty() {
        // 보고할 사람이 없어요. 처음이면 `reported_at`을 두지 않아 글이 생기면 바로 보고해요.
        if run.state.reported_at.is_some() {
            run.state.reported_at = Some(iso(run.now));
            run.save_upkeep()?;
        }
        return Ok(());
    }
    let all: Vec<(String, String)> = seen.into_iter().collect();
    // 한도로 멈춰 있어 이름을 다시 받지 못한 묶음이 있었는지예요.
    let mut skipped = false;
    for chunk in all.chunks(BATCH) {
        run.guard()?;
        let accounts: Vec<Value> = chunk
            .iter()
            .map(|(id, at)| json!({"accountId": id, "updatedAt": at}))
            .collect();
        let a = run
            .api
            .post("app/report-accounts/", &json!({"accounts": accounts}))?;
        if let Some(p) = a.cycle_period.filter(|p| *p > 0) {
            run.state.report_every = Some(p);
        }
        let (closed, mut updated) = match a.status {
            204 => continue,
            200 => statuses(&a.body)?,
            s => {
                return Err(SourceError::service(
                    Kind::Down,
                    format!("atlassian: report-accounts failed {s}"),
                ));
            }
        };
        if !closed.is_empty() {
            run.state.closed.extend(closed.iter().cloned());
            // Confluence 이름 캐시에서도 지워요.
            run.state.names.retain(|id, _| !closed.contains(id));
            rewrite(run, &closed, &|item, id| forget(item, id))?;
            run.save_upkeep()?;
        }
        updated.retain(|id| !closed.contains(id));
        if !run.state.products.jira || updated.is_empty() {
            continue;
        }
        if run.paused() {
            skipped = true;
            continue;
        }
        let names = user_names(run, &updated)?;
        // Confluence 이름 캐시에 있는 사람도 새 이름으로 바꿔요.
        for (id, name) in &names {
            if let Some(n) = run.state.names.get_mut(id) {
                n.clone_from(name);
            }
        }
        rewrite(run, &updated, &|item, id| match names.get(id) {
            Some(name) => rename(item, id, name),
            None => false,
        })?;
    }
    if !skipped {
        run.state.reported_at = Some(iso(run.now));
    }
    run.save_upkeep()
}

/// `rest/api/3/user/bulk`로 받은 이름이에요(90명씩). 장부에 적어요: 요청 1 + 사람마다 2.
pub(crate) fn user_names(
    run: &mut Run,
    ids: &BTreeSet<String>,
) -> Result<HashMap<String, String>, SourceError> {
    let ids: Vec<&String> = ids.iter().collect();
    let mut out = HashMap::new();
    let path = format!("ex/jira/{}/rest/api/3/user/bulk", run.cloud);
    for chunk in ids.chunks(BATCH) {
        run.guard()?;
        let mut q: Vec<(&str, String)> = vec![("maxResults", BATCH.to_string())];
        q.extend(chunk.iter().map(|id| ("accountId", (*id).clone())));
        let v = json_of(run.api.get(&path, &q)?, "jira user bulk")?;
        let users = v["values"].as_array().cloned().unwrap_or_default();
        run.state.points.charge(
            run.now,
            1,
            0,
            u32::try_from(users.len()).unwrap_or(u32::MAX),
        );
        for u in users {
            if let (Some(id), Some(name)) = (u["accountId"].as_str(), u["displayName"].as_str()) {
                out.insert(id.to_string(), name.to_string());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::atlassian::state::{Step, load_state, save_state, write_item};
    use crate::testing::atlassian::FakeAtlassian;

    fn go<T>(f: &FakeAtlassian, dir: &Path, what: impl FnOnce(&mut Run) -> T) -> T {
        let state = load_state(dir);
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

    fn mention(id: &str, name: &str) -> Value {
        json!({"type": "mention", "attrs": {"id": id, "text": format!("@{name}")}})
    }

    fn doc(nodes: Vec<Value>) -> Value {
        json!({"type": "doc", "version": 1, "content": [{"type": "paragraph", "content": nodes}]})
    }

    fn person(id: &str, name: &str) -> Value {
        json!({"accountId": id, "displayName": name, "emailAddress": "x@example.com"})
    }

    /// 탈퇴한 `u-gone`이 보고자, 댓글 작성자, 설명의 멘션으로 나오는 이슈예요.
    fn issue() -> Value {
        json!({
            "id": "10", "key": "A-1", "project": {"key": "A"},
            "fields": {
                "summary": "s",
                "reporter": person("u-gone", "지윤"),
                "assignee": person("u-stay", "민호"),
                "description": doc(vec![json!({"type": "text", "text": "ask "}), mention("u-gone", "지윤")]),
                "updated": "2026-11-11T00:00:00.000+0000"
            },
            "comments": [{
                "id": "c1", "author": person("u-gone", "지윤"), "updateAuthor": person("u-gone", "지윤"),
                "body": doc(vec![mention("u-stay", "민호")])
            }],
            "accounts": ["u-gone", "u-stay"],
            "fetched_at": "2026-11-11T00:00:00.000Z"
        })
    }

    /// Confluence 본문은 ADF가 JSON 글자로 들어 있어요.
    fn page() -> Value {
        json!({
            "id": "7", "kind": "page",
            "page": {
                "id": "7", "authorId": "u-gone", "version": {"authorId": "u-stay"},
                "body": {"atlas_doc_format": {"representation": "atlas_doc_format",
                    "value": doc(vec![mention("u-gone", "지윤")]).to_string()}}
            },
            "comments": [],
            // 예전에 쓴 파일이라 멘션이 `accounts`에 없어요.
            "accounts": ["u-stay"],
            "fetched_at": "2026-11-10T00:00:00.000Z"
        })
    }

    fn setup() -> (FakeAtlassian, tempfile::TempDir) {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        let mut s = State {
            cloud: Some("cloud-1".into()),
            ..State::default()
        };
        s.products.jira = true;
        s.products.confluence = true;
        save_state(d.path(), &s).unwrap();
        write_item(d.path(), "jira", "10", &issue()).unwrap();
        write_item(d.path(), "page", "7", &page()).unwrap();
        (f, d)
    }

    #[test]
    fn closed_accounts_become_탈퇴한_사용자_everywhere() {
        let (f, d) = setup();
        f.closed.lock().unwrap().push("u-gone".into());
        go(&f, d.path(), report).unwrap();

        // 멘션에만 나온 사람도(옛 파일) 보고해요.
        let sent: BTreeSet<String> = f.reports.lock().unwrap()[0]["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["accountId"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(sent, BTreeSet::from(["u-gone".into(), "u-stay".into()]));

        let i = read_item(d.path(), "jira", "10").unwrap();
        let text = i.to_string();
        assert!(!text.contains("u-gone"), "{text}");
        assert!(!text.contains("지윤"), "{text}");
        assert_eq!(i["fields"]["reporter"]["displayName"], CLOSED_NAME);
        assert!(i["fields"]["reporter"].get("emailAddress").is_none());
        assert_eq!(i["comments"][0]["author"]["displayName"], CLOSED_NAME);
        assert_eq!(i["comments"][0]["updateAuthor"]["displayName"], CLOSED_NAME);
        let m = &i["fields"]["description"]["content"][0]["content"][1];
        assert_eq!(m["attrs"]["text"], "@탈퇴한 사용자");
        assert!(m["attrs"].get("id").is_none());
        assert_eq!(i["accounts"], json!(["u-stay"]));
        // 남은 사람은 그대로예요.
        assert_eq!(i["fields"]["assignee"]["displayName"], "민호");

        let p = read_item(d.path(), "page", "7").unwrap();
        let text = p.to_string();
        assert!(!text.contains("u-gone"), "{text}");
        assert!(!text.contains("지윤"), "{text}");
        let body = adf::parse_doc(&p["page"]["body"]["atlas_doc_format"]["value"]).unwrap();
        assert_eq!(adf::mentions(&body), vec![]);
        assert!(body.to_string().contains("@탈퇴한 사용자"));

        let s = load_state(d.path());
        assert!(s.reported_at.is_some());
        assert_eq!(s.report_every, Some(604_800));
        assert!(!report_due(&s, Utc::now()));
    }

    #[test]
    fn updated_accounts_get_their_new_name_without_refetching() {
        let (f, d) = setup();
        f.updated.lock().unwrap().push("u-stay".into());
        f.users
            .lock()
            .unwrap()
            .push(json!({"accountId": "u-stay", "displayName": "민호 박"}));
        go(&f, d.path(), report).unwrap();
        let i = read_item(d.path(), "jira", "10").unwrap();
        assert_eq!(i["fields"]["assignee"]["displayName"], "민호 박");
        assert_eq!(
            i["comments"][0]["body"]["content"][0]["content"][0]["attrs"]["text"],
            "@민호 박"
        );
        assert_eq!(i["fields"]["reporter"]["displayName"], "지윤");
        let calls = f.calls();
        assert!(calls.iter().any(|c| c.ends_with("rest/api/3/user/bulk")));
        assert!(!calls.iter().any(|c| c.contains("search/jql")));
        // 요청 1 + 사람 1명(2점)이에요.
        assert_eq!(load_state(d.path()).points.used, 3);
    }

    #[test]
    fn reports_go_ninety_at_a_time_and_follow_the_cycle_period() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        let s = State {
            cloud: Some("cloud-1".into()),
            report_every: Some(60),
            reported_at: Some(iso(Utc::now() - Duration::seconds(61))),
            ..State::default()
        };
        save_state(d.path(), &s).unwrap();
        assert!(report_due(&s, Utc::now()));
        let accounts: Vec<String> = (0..100).map(|i| format!("u{i}")).collect();
        write_item(
            d.path(),
            "jira",
            "1",
            &json!({"id": "1", "accounts": accounts, "fetched_at": iso(Utc::now())}),
        )
        .unwrap();
        go(&f, d.path(), report).unwrap();
        let reports = f.reports.lock().unwrap().clone();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0]["accounts"].as_array().unwrap().len(), 90);
        assert_eq!(reports[1]["accounts"].as_array().unwrap().len(), 10);
        // `Cycle-Period`(가짜는 604800)를 따라요.
        assert_eq!(load_state(d.path()).report_every, Some(604_800));
    }

    #[test]
    fn nothing_to_report_calls_nothing_and_stays_due() {
        let f = FakeAtlassian::new();
        let d = tempfile::tempdir().unwrap();
        let s = State {
            cloud: Some("cloud-1".into()),
            ..State::default()
        };
        save_state(d.path(), &s).unwrap();
        go(&f, d.path(), report).unwrap();
        assert!(f.calls().is_empty());
        assert!(report_due(&load_state(d.path()), Utc::now()));
    }

    #[test]
    fn overdue_report_runs_before_fetching() {
        let (f, d) = setup();
        let mut s = load_state(d.path());
        s.reported_at = Some(iso(Utc::now() - Duration::days(8)));
        s.jira.head = Some("2026-11-11T00:00:00.000+0000".into());
        s.jira.done = true;
        s.time_zone = Some("UTC".into());
        save_state(d.path(), &s).unwrap();
        // 동기화가 하는 순서예요: 밀린 보고가 먼저, 그다음 받기.
        go(&f, d.path(), |run| {
            if report_due(&run.state, run.now) {
                report(run).unwrap();
            }
            assert_eq!(crate::atlassian::jira::changes(run).unwrap(), Step::Done);
        });
        let calls = f.calls();
        let report_at = calls
            .iter()
            .position(|c| c == "POST app/report-accounts/")
            .expect("reported");
        let search_at = calls
            .iter()
            .position(|c| c.ends_with("search/jql"))
            .expect("searched");
        assert!(report_at < search_at, "{calls:?}");
    }

    #[test]
    fn a_report_that_is_not_due_is_not_due() {
        let mut s = State::default();
        let now = Utc::now();
        s.reported_at = Some(iso(now - Duration::days(6)));
        assert!(!report_due(&s, now));
        s.reported_at = Some(iso(now - Duration::days(7)));
        assert!(report_due(&s, now));
    }

    /// 둘째 보고부터 실패하는 가짜예요.
    struct FailsLater {
        inner: FakeAtlassian,
        posts: std::sync::atomic::AtomicUsize,
    }

    impl crate::atlassian::api::AtlassianApi for FailsLater {
        fn get(
            &self,
            path: &str,
            query: &[(&str, String)],
        ) -> Result<crate::gdrive::api::Got, SourceError> {
            self.inner.get(path, query)
        }

        fn post(
            &self,
            path: &str,
            body: &Value,
        ) -> Result<crate::atlassian::api::Answer, SourceError> {
            if self.posts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                return Err(SourceError::service(Kind::Down, "atlassian: down"));
            }
            self.inner.post(path, body)
        }
    }

    #[test]
    fn each_group_is_applied_before_the_next_is_sent() {
        let (_, d) = setup();
        // `u-gone`이 첫 묶음에 들게 이름이 뒤로 가는 사람 100명을 더해요.
        let accounts: Vec<String> = (0..100).map(|i| format!("v{i:03}")).collect();
        write_item(
            d.path(),
            "jira",
            "11",
            &json!({"id": "11", "accounts": accounts, "fetched_at": iso(Utc::now())}),
        )
        .unwrap();
        let f = FailsLater {
            inner: FakeAtlassian::new(),
            posts: std::sync::atomic::AtomicUsize::new(0),
        };
        f.inner.closed.lock().unwrap().push("u-gone".into());
        let state = load_state(d.path());
        let mut progress = |_: usize, _: usize| {};
        let jitter = || 0;
        let still = || true;
        let mut run = Run {
            api: &f,
            dir: d.path(),
            cloud: "cloud-1",
            now: Utc::now(),
            still: &still,
            state,
            progress: &mut progress,
            jitter: &jitter,
            halted: None,
        };
        assert!(report(&mut run).is_err());
        let i = read_item(d.path(), "jira", "10").unwrap();
        assert!(!i.to_string().contains("u-gone"));
        let s = load_state(d.path());
        assert!(s.closed.contains("u-gone"));
        assert!(s.reported_at.is_none());
    }

    #[test]
    fn a_quota_pause_still_reports_but_skips_names() {
        let (f, d) = setup();
        let mut s = load_state(d.path());
        s.paused_until = Some(iso(Utc::now() + Duration::minutes(30)));
        save_state(d.path(), &s).unwrap();
        f.closed.lock().unwrap().push("u-gone".into());
        f.updated.lock().unwrap().push("u-stay".into());
        go(&f, d.path(), report).unwrap();
        let calls = f.calls();
        assert!(calls.contains(&"POST app/report-accounts/".to_string()));
        assert!(!calls.iter().any(|c| c.ends_with("user/bulk")));
        assert!(
            !read_item(d.path(), "jira", "10")
                .unwrap()
                .to_string()
                .contains("u-gone")
        );
        // 이름을 못 받았으니 다음 동기화가 다시 보고해요.
        assert!(load_state(d.path()).reported_at.is_none());
    }

    #[test]
    fn a_closed_person_does_not_come_back_with_a_refetch() {
        let (f, d) = setup();
        f.closed.lock().unwrap().push("u-gone".into());
        go(&f, d.path(), report).unwrap();
        // 받기가 같은 사람이 든 이슈를 다시 받아요.
        let mut s = load_state(d.path());
        s.jira.head = Some("2026-11-11T00:00:00.000+0000".into());
        s.jira.done = true;
        s.time_zone = Some("UTC".into());
        save_state(d.path(), &s).unwrap();
        let mut again = issue();
        again["fields"]["updated"] = json!(
            (Utc::now() - Duration::hours(1))
                .format("%Y-%m-%dT%H:%M:%S%.3f+0000")
                .to_string()
        );
        again["fields"]["comment"] = json!({"comments": again["comments"].clone(), "total": 1});
        f.jira.lock().unwrap().push(again);
        go(&f, d.path(), |run| {
            crate::atlassian::jira::changes(run).unwrap()
        });
        let i = read_item(d.path(), "jira", "10").unwrap();
        assert_ne!(i["fields"]["updated"], "2026-11-11T00:00:00.000+0000");
        let text = i.to_string();
        assert!(!text.contains("u-gone"), "{text}");
        assert!(!text.contains("지윤"), "{text}");
        assert_eq!(i["fields"]["reporter"]["displayName"], CLOSED_NAME);
    }
}
