//! Turns raw Atlassian data into documents and chunks (docs/31 6절).
//!
//! - Jira issue: `jira/<cloudid>/<id>`, header `jira: ENG-123 · 제목` + `meta: Jira · 프로젝트 · 유형 · 상태 · 담당 … ·
//!   10월 27일 수정 · 라벨 …` (the card's byline). The body is the description, then comments (`이름 10월 27일: 본문`),
//!   and each comment starts a new window. Description lines open the issue URL; comment lines open with
//!   `?focusedCommentId=<id>`.
//! - Confluence item (page or blog post): `confluence/<cloudid>/<id>`, header `confluence: 제목` + `meta: Confluence ·
//!   스페이스 · 쓴 사람 · 10월 27일 수정 · 라벨 …`. The body is the item, then comments; an inline comment is preceded by
//!   its anchored text as a `> 원문` line. Confluence v2 gives no person names, so it uses `state.names` (names fetched
//!   with Jira `user/bulk`) and drops the name field when there is none.
//! - Muted projects and spaces (by id or key), items older than 2 years and files with only comments and no item are
//!   left out. The raw data stays as is.
//! - Once `LOST_DAYS` have passed since the connection was lost (`lost_since`), no chunks are produced (at that point
//!   `gather` deletes the raw data too).

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Datelike, Duration, FixedOffset, Utc};
use serde_json::{Value, json};
use tacie_index::RawChunk;

use crate::atlassian::adf;
use crate::atlassian::jira::parse_updated;
use crate::atlassian::state::{State, WINDOW_DAYS, item_ids, load_state, read_item};
use crate::doc::{Doc, windows_at};
use crate::error::SourceError;
use crate::gdrive::fetch::parse;

/// 연결이 끊긴 뒤 이만큼 지나면 Atlassian 자료를 찾지 않고 원본도 지워요.
pub const LOST_DAYS: i64 = 7;
const KST_SECS: i32 = 9 * 3600;

/// Jira(`2026-11-12T10:30:00.000+0900`)나 RFC 3339 시각이에요.
fn when(s: &str) -> Option<DateTime<Utc>> {
    parse(s).or_else(|| parse_updated(s).map(|t| t.with_timezone(&Utc)))
}

fn kst(s: &str) -> Option<DateTime<FixedOffset>> {
    when(s).map(|t| t.with_timezone(&FixedOffset::east_opt(KST_SECS).expect("KST")))
}

/// `10월 27일`이에요.
fn md(s: &str) -> String {
    kst(s).map_or_else(String::new, |t| format!("{}월 {}일", t.month(), t.day()))
}

/// `2026-10-27`이에요.
fn ymd(s: &str) -> String {
    kst(s).map_or_else(
        || s.chars().take(10).collect(),
        |t| t.format("%Y-%m-%d").to_string(),
    )
}

fn s<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    path.iter()
        .try_fold(v, |v, k| v.get(*k))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn join(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn edited(at: &str) -> String {
    let d = md(at);
    if d.is_empty() {
        d
    } else {
        format!("{d} 수정")
    }
}

fn labels(words: &[&str]) -> String {
    if words.is_empty() {
        String::new()
    } else {
        format!("라벨 {}", words.join(", "))
    }
}

/// ADF(객체나 JSON 글자)를 Markdown 줄로 바꿔요.
fn markdown(v: &Value) -> String {
    adf::parse_doc(v)
        .map(|d| adf::to_markdown(&d))
        .unwrap_or_default()
}

/// 사이트 주소 끝의 `/`를 뺀 것이에요.
fn site(st: &State) -> String {
    st.site_url
        .as_deref()
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string()
}

fn focused(url: &str, comment: &str) -> String {
    if comment.is_empty() {
        url.to_string()
    } else {
        format!("{url}?focusedCommentId={comment}")
    }
}

/// 댓글 하나를 본문 줄로 붙여요(첫 줄 앞에 `이름 날짜: `). 창은 그 댓글에서 시작해요.
fn comment_lines(
    body: &mut Vec<(String, String)>,
    starts: &mut Vec<usize>,
    who: &str,
    at: &str,
    quote: &str,
    text: &str,
    url: &str,
) {
    starts.push(body.len());
    for q in quote.lines().filter(|l| !l.trim().is_empty()) {
        body.push((format!("> {q}"), url.to_string()));
    }
    let date = md(at);
    let head = [who, date.as_str()]
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let head = if head.is_empty() {
        String::new()
    } else {
        format!("{head}: ")
    };
    let mut lines = text.lines();
    body.push((
        format!("{head}{}", lines.next().unwrap_or("")),
        url.to_string(),
    ));
    body.extend(lines.map(|l| (l.to_string(), url.to_string())));
}

/// 댓글을 쓴 시각(`at` 칸) 순서로 늘어놔요. 시각이 같거나 없으면 받은 순서예요.
fn by_time<'a>(comments: &'a Value, at: &[&str]) -> Vec<&'a Value> {
    let mut out: Vec<&Value> = comments.as_array().into_iter().flatten().collect();
    out.sort_by_key(|c| when(s(c, at)));
    out
}

/// 본문이 비면 제목 한 줄을 본문으로 둬요(제목만 있는 글도 찾을 수 있게요).
fn doc(
    path: String,
    title: String,
    url: String,
    at: &str,
    head: String,
    body: Vec<(String, String)>,
    starts: &mut Vec<usize>,
) -> Doc {
    let body = if body.iter().all(|(l, _)| l.trim().is_empty()) {
        starts.clear();
        vec![(title.clone(), url.clone())]
    } else {
        body
    };
    Doc {
        path,
        title,
        url,
        author: None,
        at: ymd(at),
        acl: None,
        head,
        body,
    }
}

/// 이슈 파일 하나의 문서와 창 시작 줄이에요.
fn issue_doc(v: &Value, cloud: &str, site_url: &str) -> Option<(Doc, Vec<usize>)> {
    let id = s(v, &["id"]);
    let key = s(v, &["key"]);
    if id.is_empty() {
        return None;
    }
    let f = &v["fields"];
    let title = s(f, &["summary"]).to_string();
    let url = if key.is_empty() {
        site_url.to_string()
    } else {
        format!("{site_url}/browse/{key}")
    };
    let at = s(f, &["updated"]);
    let assignee = s(f, &["assignee", "displayName"]);
    let who = if assignee.is_empty() {
        String::new()
    } else {
        format!("담당 {assignee}")
    };
    let label_words: Vec<&str> = f["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let project = match s(v, &["project", "name"]) {
        "" => s(f, &["project", "name"]),
        p => p,
    };
    let head = format!(
        "jira: {}\nmeta: {}",
        join(&[key, &title]),
        join(&[
            "Jira",
            project,
            s(f, &["issuetype", "name"]),
            s(f, &["status", "name"]),
            &who,
            &edited(at),
            &labels(&label_words),
        ])
    );
    let mut body: Vec<(String, String)> = markdown(&f["description"])
        .lines()
        .map(|l| (l.to_string(), url.clone()))
        .collect();
    let mut starts = Vec::new();
    for c in by_time(&v["comments"], &["created"]) {
        let cid = crate::atlassian::confluence::sid(&c["id"]).unwrap_or_default();
        comment_lines(
            &mut body,
            &mut starts,
            s(c, &["author", "displayName"]),
            s(c, &["created"]),
            "",
            &markdown(&c["body"]),
            &focused(&url, &cid),
        );
    }
    let d = doc(
        format!("jira/{cloud}/{id}"),
        title,
        url,
        at,
        head,
        body,
        &mut starts,
    );
    Some((d, starts))
}

/// Confluence 글 파일 하나의 문서와 창 시작 줄이에요. 글 없이 댓글만 든 파일은 None이에요.
fn page_doc(
    v: &Value,
    cloud: &str,
    site_url: &str,
    space: &str,
    names: &BTreeMap<String, String>,
) -> Option<(Doc, Vec<usize>)> {
    let p = &v["page"];
    if !p.is_object() {
        return None;
    }
    let id = crate::atlassian::confluence::sid(&v["id"])
        .or_else(|| crate::atlassian::confluence::sid(&p["id"]))?;
    let title = s(p, &["title"]).to_string();
    let webui = s(p, &["_links", "webui"]);
    // v2의 `webui`는 `_links.base`(`https://<site>/wiki`) 뒤의 길이라 `/wiki`를 붙여요.
    let url = if webui.is_empty() {
        site_url.to_string()
    } else if webui.starts_with("/wiki/") {
        format!("{site_url}{webui}")
    } else {
        format!("{site_url}/wiki{webui}")
    };
    let at = s(p, &["version", "createdAt"]);
    let name_of = |id: &str| names.get(id).map_or("", String::as_str);
    let author = name_of(s(p, &["authorId"]));
    let label_words: Vec<&str> = p["labels"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["name"].as_str())
        .collect();
    let head = format!(
        "confluence: {title}\nmeta: {}",
        join(&[
            "Confluence",
            space,
            author,
            &edited(at),
            &labels(&label_words)
        ])
    );
    let mut body: Vec<(String, String)> = markdown(&p["body"]["atlas_doc_format"]["value"])
        .lines()
        .map(|l| (l.to_string(), url.clone()))
        .collect();
    let mut starts = Vec::new();
    for c in by_time(&v["comments"], &["version", "createdAt"]) {
        let cid = crate::atlassian::confluence::sid(&c["id"]).unwrap_or_default();
        let who = match s(c, &["version", "authorId"]) {
            "" => name_of(s(c, &["authorId"])),
            a => name_of(a),
        };
        comment_lines(
            &mut body,
            &mut starts,
            who,
            s(c, &["version", "createdAt"]),
            c["quote"].as_str().unwrap_or(""),
            &markdown(&c["body"]["atlas_doc_format"]["value"]),
            &focused(&url, &cid),
        );
    }
    let d = doc(
        format!("confluence/{cloud}/{id}"),
        title,
        url,
        at,
        head,
        body,
        &mut starts,
    );
    Some((d, starts))
}

/// 이슈의 프로젝트(id, key, 이름)예요.
pub(crate) fn project_of(v: &Value) -> (String, String, String) {
    let p = if v["project"].is_object() {
        &v["project"]
    } else {
        &v["fields"]["project"]
    };
    let id = crate::atlassian::confluence::sid(&p["id"]).unwrap_or_default();
    (id, s(p, &["key"]).to_string(), s(p, &["name"]).to_string())
}

fn muted(list: &[String], id: &str, key: &str) -> bool {
    list.iter()
        .any(|m| (!id.is_empty() && m == id) || (!key.is_empty() && m == key))
}

/// 연결이 끊긴 지 `LOST_DAYS`가 지났는지예요.
pub(crate) fn expired(st: &State, now: DateTime<Utc>) -> bool {
    st.lost_since
        .as_deref()
        .and_then(parse)
        .is_some_and(|lost| now - lost >= Duration::days(LOST_DAYS))
}

/// 저장본의 조각이에요(`src`는 "jira", "confluence"). 끈 프로젝트·스페이스, 2년이 지난 글, 댓글만 든 파일은 빼고,
/// 연결이 끊긴 지 `LOST_DAYS`가 지났으면 비어요.
pub fn chunks_of(raw_dir: &Path, now: DateTime<Utc>) -> Result<Vec<RawChunk>, SourceError> {
    let st = load_state(raw_dir);
    if expired(&st, now) {
        return Ok(Vec::new());
    }
    let Some(cloud) = st.cloud.clone() else {
        return Ok(Vec::new());
    };
    let site_url = site(&st);
    let floor = now - Duration::days(WINDOW_DAYS);
    let fresh = |at: &str| when(at).is_some_and(|t| t > floor);
    let mut out = Vec::new();
    for id in item_ids(raw_dir, "jira") {
        let Some(v) = read_item(raw_dir, "jira", &id) else {
            continue;
        };
        if !fresh(s(&v, &["fields", "updated"])) {
            continue;
        }
        let (pid, pkey, _) = project_of(&v);
        if muted(&st.muted_projects, &pid, &pkey) {
            continue;
        }
        if let Some((d, starts)) = issue_doc(&v, &cloud, &site_url) {
            out.extend(windows_at(&d, "jira", &starts));
        }
    }
    for id in item_ids(raw_dir, "page") {
        let Some(v) = read_item(raw_dir, "page", &id) else {
            continue;
        };
        if !v["page"].is_object() || !fresh(s(&v, &["page", "version", "createdAt"])) {
            continue;
        }
        let space_id = crate::atlassian::confluence::sid(&v["page"]["spaceId"]).unwrap_or_default();
        let space = st.spaces.get(&space_id).cloned().unwrap_or_default();
        if muted(&st.muted_spaces, &space_id, &space.key) {
            continue;
        }
        if let Some((d, starts)) = page_doc(&v, &cloud, &site_url, &space.name, &st.names) {
            out.extend(windows_at(&d, "confluence", &starts));
        }
    }
    Ok(out)
}

/// 고르기 시트의 목록이에요: `{"projects":[{id,key,name,issues}], "spaces":[{id,key,name,pages}]}`.
pub fn scopes(raw_dir: &Path) -> Value {
    let st = load_state(raw_dir);
    // id → (key, 이름, 수)
    let mut projects: BTreeMap<String, (String, String, usize)> = BTreeMap::new();
    for id in item_ids(raw_dir, "jira") {
        let Some(v) = read_item(raw_dir, "jira", &id) else {
            continue;
        };
        let (pid, key, name) = project_of(&v);
        if pid.is_empty() && key.is_empty() {
            continue;
        }
        let e = projects
            .entry(if pid.is_empty() { key.clone() } else { pid })
            .or_insert((key, name, 0));
        e.2 += 1;
    }
    let mut spaces: BTreeMap<String, usize> = BTreeMap::new();
    for id in item_ids(raw_dir, "page") {
        let Some(v) = read_item(raw_dir, "page", &id) else {
            continue;
        };
        if !v["page"].is_object() {
            continue;
        }
        if let Some(sp) = crate::atlassian::confluence::sid(&v["page"]["spaceId"]) {
            *spaces.entry(sp).or_default() += 1;
        }
    }
    let mut projects: Vec<Value> = projects
        .into_iter()
        .map(|(id, (key, name, n))| json!({"id": id, "key": key, "name": name, "issues": n}))
        .collect();
    projects.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let mut spaces: Vec<Value> = spaces
        .into_iter()
        .map(|(id, n)| {
            let sp = st.spaces.get(&id).cloned().unwrap_or_default();
            json!({"id": id, "key": sp.key, "name": sp.name, "pages": n})
        })
        .collect();
    spaces.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    json!({"projects": projects, "spaces": spaces})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atlassian::state::{SpaceName, save_state, write_item};

    fn now() -> DateTime<Utc> {
        parse("2026-11-12T03:00:00.000Z").unwrap()
    }

    fn adf(text: &str) -> Value {
        json!({"type": "doc", "version": 1, "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": text}]}]})
    }

    fn seed(dir: &Path) {
        let mut st = State {
            cloud: Some("c1".into()),
            site_url: Some("https://acme.atlassian.net/".into()),
            ..Default::default()
        };
        st.names.insert("u-author".into(), "Ann".into());
        st.names.insert("u-c".into(), "Bo".into());
        st.spaces.insert(
            "9".into(),
            SpaceName {
                key: "ENG".into(),
                name: "Engineering".into(),
            },
        );
        st.spaces.insert(
            "8".into(),
            SpaceName {
                key: "HR".into(),
                name: "People".into(),
            },
        );
        st.muted_projects = vec!["OPS".into()];
        st.muted_spaces = vec!["HR".into()];
        save_state(dir, &st).unwrap();
        let issue = |id: &str, key: &str, project: (&str, &str, &str)| {
            json!({
                "id": id, "key": key,
                "project": {"id": project.0, "key": project.1, "name": project.2},
                "fields": {
                    "summary": "Login breaks", "updated": "2026-10-27T10:00:00.000+0900",
                    "status": {"name": "진행 중"}, "issuetype": {"name": "버그"},
                    "assignee": {"displayName": "Cy"}, "labels": ["auth"],
                    "description": adf("first line")
                },
                "comments": [{"id": "501", "author": {"displayName": "Eve"},
                              "created": "2026-10-27T09:00:00.000+0900", "body": adf("agreed")},
                             {"id": "500", "author": {"displayName": "Dee"},
                              "created": "2026-10-26T09:00:00.000+0900", "body": adf("we chose JWT")}]
            })
        };
        write_item(
            dir,
            "jira",
            "10",
            &issue("10", "ENG-123", ("1", "ENG", "Engine")),
        )
        .unwrap();
        write_item(
            dir,
            "jira",
            "11",
            &issue("11", "OPS-1", ("2", "OPS", "Ops")),
        )
        .unwrap();
        let page = |id: &str, space: &str| {
            json!({
                "id": id, "kind": "page",
                "page": {"id": id, "title": "Auth design", "spaceId": space, "authorId": "u-author",
                         "version": {"createdAt": "2026-10-27T01:00:00.000Z"},
                         "body": {"atlas_doc_format": {"value": adf("page body").to_string()}},
                         "_links": {"webui": format!("/spaces/ENG/pages/{id}")}},
                // 받은 순서와 쓴 시각이 달라요(78이 늦게 썼어요).
                "comments": [{"id": "78", "type": "footer",
                              "version": {"createdAt": "2026-10-29T01:00:00.000Z", "authorId": "u-unknown"},
                              "body": {"atlas_doc_format": {"value": adf("ok").to_string()}}},
                             {"id": "77", "type": "inline", "quote": "the quoted words",
                              "version": {"createdAt": "2026-10-28T01:00:00.000Z", "authorId": "u-c"},
                              "body": {"atlas_doc_format": {"value": adf("why?").to_string()}}}]
            })
        };
        write_item(dir, "page", "7", &page("7", "9")).unwrap();
        write_item(dir, "page", "8", &page("8", "8")).unwrap();
        // 글 없이 댓글만 든 파일이에요.
        write_item(
            dir,
            "page",
            "6",
            &json!({"id": "6", "kind": "page", "page": null, "comments": [
                {"id": "1", "version": {"createdAt": "2026-10-28T01:00:00.000Z"},
                 "body": {"atlas_doc_format": {"value": adf("orphan").to_string()}}}]}),
        )
        .unwrap();
    }

    fn text(c: &RawChunk) -> &str {
        c["text"].as_str().unwrap()
    }

    #[test]
    fn chunks_have_heads_links_and_skip_muted() {
        let d = tempfile::tempdir().unwrap();
        seed(d.path());
        let chunks = chunks_of(d.path(), now()).unwrap();
        let paths: Vec<&str> = chunks.iter().map(|c| c["path"].as_str().unwrap()).collect();
        assert!(paths.contains(&"jira/c1/10"), "{paths:?}");
        assert!(paths.contains(&"confluence/c1/7"), "{paths:?}");
        assert!(!paths.contains(&"jira/c1/11"), "muted project by key");
        assert!(!paths.contains(&"confluence/c1/8"), "muted space by key");
        assert!(!paths.contains(&"confluence/c1/6"), "comment-only file");

        let jira: Vec<&RawChunk> = chunks.iter().filter(|c| c["src"] == "jira").collect();
        assert!(text(jira[0]).starts_with(
            "jira: ENG-123 · Login breaks\nmeta: Jira · Engine · 버그 · 진행 중 · 담당 Cy · 10월 27일 수정 · 라벨 auth\nfirst line"
        ), "{}", text(jira[0]));
        assert_eq!(jira[0]["url"], "https://acme.atlassian.net/browse/ENG-123");
        assert_eq!(jira[0]["acl"], json!(["*"]));
        let comment = jira
            .iter()
            .find(|c| text(c).contains("Dee 10월 26일: we chose JWT"))
            .unwrap();
        assert_eq!(
            comment["url"],
            "https://acme.atlassian.net/browse/ENG-123?focusedCommentId=500"
        );

        let wiki: Vec<&RawChunk> = chunks.iter().filter(|c| c["src"] == "confluence").collect();
        assert!(text(wiki[0]).starts_with(
            "confluence: Auth design\nmeta: Confluence · Engineering · Ann · 10월 27일 수정\npage body"
        ), "{}", text(wiki[0]));
        assert_eq!(
            wiki[0]["url"],
            "https://acme.atlassian.net/wiki/spaces/ENG/pages/7"
        );
        let all: String = wiki.iter().map(|c| text(c)).collect::<Vec<_>>().join("\n");
        assert!(
            all.contains("> the quoted words\nBo 10월 28일: why?"),
            "{all}"
        );
        // 이름을 모르는 사람은 이름 칸을 빼요.
        assert!(all.contains("\n10월 29일: ok"), "{all}");
        // 댓글은 쓴 시각 순서예요.
        assert!(
            all.find("why?").unwrap() < all.find("10월 29일: ok").unwrap(),
            "{all}"
        );
        let issue: String = jira.iter().map(|c| text(c)).collect::<Vec<_>>().join("\n");
        assert!(
            issue.find("we chose JWT").unwrap() < issue.find("Eve 10월 27일: agreed").unwrap(),
            "{issue}"
        );
        let inline = wiki
            .iter()
            .find(|c| text(c).contains("> the quoted words"))
            .unwrap();
        assert_eq!(
            inline["url"],
            "https://acme.atlassian.net/wiki/spaces/ENG/pages/7?focusedCommentId=77"
        );
    }

    #[test]
    fn a_lost_connection_stops_chunks_after_seven_days() {
        let d = tempfile::tempdir().unwrap();
        seed(d.path());
        let mut st = load_state(d.path());
        st.lost_since = Some(crate::atlassian::state::iso(now() - Duration::days(6)));
        save_state(d.path(), &st).unwrap();
        assert!(!chunks_of(d.path(), now()).unwrap().is_empty());
        st.lost_since = Some(crate::atlassian::state::iso(now() - Duration::days(7)));
        save_state(d.path(), &st).unwrap();
        assert!(chunks_of(d.path(), now()).unwrap().is_empty());
    }

    #[test]
    fn old_items_are_left_out() {
        let d = tempfile::tempdir().unwrap();
        seed(d.path());
        let later = now() + Duration::days(WINDOW_DAYS);
        assert!(chunks_of(d.path(), later).unwrap().is_empty());
    }

    #[test]
    fn scopes_count_projects_and_spaces() {
        let d = tempfile::tempdir().unwrap();
        seed(d.path());
        let v = scopes(d.path());
        assert_eq!(
            v["projects"],
            json!([{"id": "1", "key": "ENG", "name": "Engine", "issues": 1},
                   {"id": "2", "key": "OPS", "name": "Ops", "issues": 1}])
        );
        assert_eq!(
            v["spaces"],
            json!([{"id": "9", "key": "ENG", "name": "Engineering", "pages": 1},
                   {"id": "8", "key": "HR", "name": "People", "pages": 1}])
        );
    }
}
