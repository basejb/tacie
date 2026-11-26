//! Turns raw Linear data into documents and chunks (docs/18 9절).
//!
//! - Issue: `linear/issue/<id>`, header `linear: COR-7 · 제목 · 팀 · 담당 … · 라벨 …` (context for the judging model) +
//!   `meta: Linear · COR-7 · 완료 · 10월 27일` (the card's byline). The body is the description → attachments →
//!   comments (replies indented two spaces), and each top-level comment starts a new window that opens at that
//!   comment's URL.
//! - Project update: `linear/update/<id>`, `meta: Linear · 프로젝트 업데이트 · 위험 · 10월 27일`.
//! - Document: `linear/doc/<id>`, `meta: Linear 문서 · 10월 27일 수정`.
//! - Items of muted teams (`muted` in `state.json`) are left out. The raw data stays, so turning a team back on needs
//!   no refetch.
//! - Once `LOST_DAYS` have passed since the connection was lost (`lost_since`), no chunks are produced (the raw data
//!   is kept).

use std::path::Path;

use chrono::{DateTime, Datelike, Duration, FixedOffset, Utc};
use serde::Serialize;
use serde_json::{Map, Value};
use tacie_index::RawChunk;

use crate::doc::{Doc, windows_at};
use crate::error::SourceError;
use crate::linear::fetch::{item_names, load_state, read_item, teams_of};

/// 연결이 끊긴 뒤 이만큼 지나면 Linear 자료를 찾지 않아요.
pub const LOST_DAYS: i64 = 7;
const KST_SECS: i32 = 9 * 3600;

/// 팀 고르기 화면의 한 줄이에요.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TeamInfo {
    pub id: String,
    pub name: String,
    pub issues: usize,
}

fn kst(iso: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|t| t.with_timezone(&FixedOffset::east_opt(KST_SECS).expect("KST")))
}

/// `10월 27일`이에요.
fn md(iso: &str) -> String {
    kst(iso).map_or_else(String::new, |t| format!("{}월 {}일", t.month(), t.day()))
}

/// `2026-10-27`이에요.
fn ymd(iso: &str) -> String {
    kst(iso).map_or_else(
        || iso.chars().take(10).collect(),
        |t| t.format("%Y-%m-%d").to_string(),
    )
}

fn s<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    path.iter()
        .try_fold(v, |v, k| v.get(*k))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// 상태 종류의 한국어예요. Linear의 상태 이름은 팀마다 달라서 종류로 말해요.
fn state_word(kind: &str) -> &'static str {
    match kind {
        "completed" => "완료",
        "started" => "진행 중",
        "unstarted" => "할 일",
        "backlog" => "백로그",
        "triage" => "분류 전",
        _ => "",
    }
}

fn health_word(h: &str) -> &'static str {
    match h {
        "onTrack" => "순조로움",
        "atRisk" => "위험",
        "offTrack" => "늦음",
        _ => "",
    }
}

fn join(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 댓글을 본문 줄로 붙이고, 맨 위 댓글이 시작하는 줄 번호를 `starts`에 넣어요.
fn comment_lines(v: &Value, body: &mut Vec<(String, String)>, starts: &mut Vec<usize>) {
    let comments = v["comments"].as_array().cloned().unwrap_or_default();
    let top: Vec<&Value> = comments
        .iter()
        .filter(|c| s(c, &["parent", "id"]).is_empty())
        .collect();
    let emit = |c: &Value, indent: &str, body: &mut Vec<(String, String)>| {
        let url = s(c, &["url"]).to_string();
        let head = format!(
            "{indent}{} {}: ",
            s(c, &["user", "name"]),
            md(s(c, &["createdAt"]))
        );
        for (k, line) in s(c, &["body"]).lines().enumerate() {
            let text = if k == 0 {
                format!("{head}{line}")
            } else {
                format!("{indent}{line}")
            };
            body.push((text, url.clone()));
        }
    };
    let mut seen = std::collections::HashSet::new();
    for c in top {
        starts.push(body.len());
        emit(c, "", body);
        seen.insert(s(c, &["id"]).to_string());
        for r in comments
            .iter()
            .filter(|r| s(r, &["parent", "id"]) == s(c, &["id"]))
        {
            emit(r, "  ", body);
            seen.insert(s(r, &["id"]).to_string());
        }
    }
    // 부모를 모르는 답글(부모가 지워졌어요)도 빠뜨리지 않아요.
    for r in comments.iter().filter(|r| !seen.contains(s(r, &["id"]))) {
        starts.push(body.len());
        emit(r, "", body);
    }
}

/// 본문이 비면 제목 한 줄을 본문으로 둬요(제목만 있는 이슈도 찾을 수 있게요).
fn doc(
    path: String,
    title: String,
    url: &str,
    at: &str,
    head: String,
    mut body: Vec<(String, String)>,
) -> Doc {
    if body.iter().all(|(l, _)| l.trim().is_empty()) {
        body = vec![(title.clone(), url.to_string())];
    }
    Doc {
        path,
        title,
        url: url.to_string(),
        author: None,
        at: ymd(at),
        acl: None,
        head,
        body,
    }
}

/// 원본 항목 하나의 문서와 창 시작 줄이에요.
fn document(name: &str, v: &Value) -> Option<(Doc, Vec<usize>)> {
    let (kind, id) = name.split_once('-')?;
    let url = s(v, &["url"]);
    let at = s(v, &["updatedAt"]);
    let mut body: Vec<(String, String)> = Vec::new();
    let mut starts = Vec::new();
    match kind {
        "issue" => {
            let key = s(v, &["identifier"]);
            let title = s(v, &["title"]);
            let labels: Vec<&str> = v["labels"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| l["name"].as_str())
                .collect();
            let assignee = s(v, &["assignee", "name"]);
            let who = if assignee.is_empty() {
                "담당 없음".to_string()
            } else {
                format!("담당 {assignee}")
            };
            let label = if labels.is_empty() {
                String::new()
            } else {
                format!("라벨 {}", labels.join(", "))
            };
            let head = format!(
                "linear: {}\nmeta: {}",
                join(&[key, title, s(v, &["team", "name"]), &who, &label]),
                join(&["Linear", key, state_word(s(v, &["state", "type"])), &md(at)])
            );
            body.extend(
                s(v, &["description"])
                    .lines()
                    .map(|l| (l.to_string(), url.to_string())),
            );
            for a in v["attachments"]["nodes"].as_array().into_iter().flatten() {
                body.push((
                    format!("첨부: {} {}", s(a, &["title"]), s(a, &["url"])),
                    url.to_string(),
                ));
            }
            comment_lines(v, &mut body, &mut starts);
            Some((
                doc(
                    format!("linear/issue/{id}"),
                    join(&[key, title]),
                    url,
                    at,
                    head,
                    body,
                ),
                starts,
            ))
        }
        "update" => {
            let project = s(v, &["project", "name"]);
            let head = format!(
                "linear: {}\nmeta: {}",
                join(&[project, "업데이트", s(v, &["user", "name"])]),
                join(&[
                    "Linear",
                    "프로젝트 업데이트",
                    health_word(s(v, &["health"])),
                    &md(at)
                ])
            );
            body.extend(
                s(v, &["body"])
                    .lines()
                    .map(|l| (l.to_string(), url.to_string())),
            );
            comment_lines(v, &mut body, &mut starts);
            Some((
                doc(
                    format!("linear/update/{id}"),
                    format!("{project} · 업데이트"),
                    url,
                    at,
                    head,
                    body,
                ),
                starts,
            ))
        }
        "doc" => {
            let title = s(v, &["title"]);
            let place = [s(v, &["project", "name"]), s(v, &["team", "name"])]
                .into_iter()
                .find(|p| !p.is_empty())
                .unwrap_or("");
            let head = format!(
                "linear: {}\nmeta: Linear 문서 · {} 수정",
                join(&[title, place]),
                md(at)
            );
            body.extend(
                s(v, &["content"])
                    .lines()
                    .map(|l| (l.to_string(), url.to_string())),
            );
            Some((
                doc(
                    format!("linear/doc/{id}"),
                    title.to_string(),
                    url,
                    at,
                    head,
                    body,
                ),
                starts,
            ))
        }
        _ => None,
    }
}

/// 저장본의 조각이에요(`src` = "linear"). 끈 팀은 빼고, 연결이 끊긴 지 `LOST_DAYS`가 지났으면 비어요.
pub fn chunks_of(raw_dir: &Path, now: DateTime<Utc>) -> Result<Vec<RawChunk>, SourceError> {
    let st = load_state(raw_dir);
    if let Some(lost) = st
        .lost_since
        .as_deref()
        .and_then(|l| DateTime::parse_from_rfc3339(l).ok())
        && now - lost.with_timezone(&Utc) >= Duration::days(LOST_DAYS)
    {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for name in item_names(raw_dir) {
        let Some(v) = read_item(raw_dir, &name) else {
            continue;
        };
        let ts = teams_of(raw_dir, &name, &v);
        if !ts.is_empty() && ts.iter().all(|t| st.muted.contains(t)) {
            continue;
        }
        if let Some((d, starts)) = document(&name, &v) {
            out.extend(windows_at(&d, "linear", &starts));
        }
    }
    Ok(out)
}

/// 보이는 팀과 그 팀의 이슈 수예요(저장된 순서).
pub fn teams(raw_dir: &Path) -> Vec<TeamInfo> {
    let st = load_state(raw_dir);
    let mut counts: Map<String, Value> = Map::new();
    for name in item_names(raw_dir)
        .into_iter()
        .filter(|n| n.starts_with("issue-"))
    {
        if let Some(v) = read_item(raw_dir, &name) {
            let t = s(&v, &["team", "id"]).to_string();
            let n = counts.get(&t).and_then(Value::as_u64).unwrap_or(0) + 1;
            counts.insert(t, Value::from(n));
        }
    }
    st.teams
        .iter()
        .filter_map(|t| {
            let id = t["id"].as_str()?.to_string();
            Some(TeamInfo {
                issues: counts.get(&id).and_then(Value::as_u64).unwrap_or(0) as usize,
                name: s(t, &["name"]).to_string(),
                id,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linear::fetch::{State, save_state};
    use chrono::TimeZone;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap()
    }
    fn put(dir: &Path, name: &str, v: Value) {
        std::fs::create_dir_all(dir.join("items")).unwrap();
        std::fs::write(
            dir.join("items").join(format!("{name}.json")),
            v.to_string(),
        )
        .unwrap();
    }
    fn setup(dir: &Path, muted: &[&str]) {
        let s = State {
            teams: vec![
                json!({"id": "t1", "name": "Core", "key": "COR"}),
                json!({"id": "t2", "name": "Web", "key": "WEB"}),
            ],
            muted: muted.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        save_state(dir, &s).unwrap();
        put(
            dir,
            "issue-1",
            json!({
            "id": "1", "identifier": "COR-7", "title": "Login breaks", "description": "line one\nline two",
            "url": "https://linear.app/a/issue/COR-7", "state": {"name": "Done", "type": "completed"},
            "assignee": {"name": "Ann"}, "labels": {"nodes": [{"name": "bug"}]}, "team": {"id": "t1", "name": "Core"},
            "updatedAt": "2026-10-27T03:00:00.000Z", "attachments": {"nodes": [{"title": "PR #5", "url": "https://github.com/o/r/pull/5"}]},
            "comments": [
                {"id": "c1", "body": "we chose JWT\nbecause of mobile", "user": {"name": "Bo"}, "url": "https://linear.app/a/issue/COR-7#comment-c1", "createdAt": "2026-10-26T01:00:00.000Z"},
                {"id": "c2", "body": "agreed", "user": {"name": "Cy"}, "parent": {"id": "c1"}, "url": "https://linear.app/a/issue/COR-7#comment-c2", "createdAt": "2026-10-26T02:00:00.000Z"}
            ], "thread": true}),
        );
        put(
            dir,
            "issue-2",
            json!({"id": "2", "identifier": "WEB-1", "title": "Navbar", "description": "x",
            "url": "https://linear.app/a/issue/WEB-1", "state": {"type": "started"}, "team": {"id": "t2", "name": "Web"},
            "updatedAt": "2026-10-20T00:00:00.000Z", "comments": []}),
        );
        put(
            dir,
            "update-u1",
            json!({"id": "u1", "body": "slipping a week", "health": "atRisk", "url": "https://linear.app/a/u1",
            "updatedAt": "2026-10-27T00:00:00.000Z", "user": {"name": "Ann"}, "project": {"name": "Launch", "teams": {"nodes": [{"id": "t1"}]}}, "comments": []}),
        );
    }
    fn chunks(dir: &Path) -> Vec<RawChunk> {
        chunks_of(dir, now()).unwrap()
    }
    fn of<'a>(cs: &'a [RawChunk], path: &str) -> Vec<&'a RawChunk> {
        cs.iter().filter(|c| c["path"] == path).collect()
    }

    #[test]
    fn issue_becomes_one_document_with_comment_links() {
        let d = tempfile::tempdir().unwrap();
        setup(d.path(), &[]);
        let cs = chunks(d.path());
        let i = of(&cs, "linear/issue/1");
        assert!(!i.is_empty());
        assert_eq!(i[0]["src"], "linear");
        assert_eq!(i[0]["title"], "COR-7 · Login breaks");
        assert_eq!(i[0]["acl"], json!(["*"]));
        let text = i[0]["text"].as_str().unwrap();
        assert!(text.starts_with("linear: COR-7 · Login breaks · Core · 담당 Ann · 라벨 bug\nmeta: Linear · COR-7 · 완료 · 10월 27일\n"), "{text}");
        assert!(text.contains("첨부: PR #5 https://github.com/o/r/pull/5"));
        assert_eq!(i[0]["url"], "https://linear.app/a/issue/COR-7");
    }

    #[test]
    fn each_comment_starts_a_window_that_links_to_it() {
        let d = tempfile::tempdir().unwrap();
        setup(d.path(), &[]);
        let cs = chunks(d.path());
        let i = of(&cs, "linear/issue/1");
        let c1 = i
            .iter()
            .find(|c| c["url"] == "https://linear.app/a/issue/COR-7#comment-c1")
            .expect("a window starts at c1");
        let text = c1["text"].as_str().unwrap();
        assert!(
            text.contains("Bo 10월 26일: we chose JWT\nbecause of mobile"),
            "{text}"
        );
    }

    #[test]
    fn replies_are_indented_under_their_comment() {
        let d = tempfile::tempdir().unwrap();
        setup(d.path(), &[]);
        let cs = chunks(d.path());
        let all: String = of(&cs, "linear/issue/1")
            .iter()
            .map(|c| c["text"].as_str().unwrap().to_string())
            .collect();
        assert!(all.contains("\n  Cy 10월 26일: agreed"), "{all}");
    }

    #[test]
    fn project_update_head_names_its_health() {
        let d = tempfile::tempdir().unwrap();
        setup(d.path(), &[]);
        let cs = chunks(d.path());
        let u = of(&cs, "linear/update/u1");
        assert_eq!(u[0]["title"], "Launch · 업데이트");
        assert!(
            u[0]["text"]
                .as_str()
                .unwrap()
                .contains("meta: Linear · 프로젝트 업데이트 · 위험 · 10월 27일")
        );
    }

    #[test]
    fn muted_teams_are_left_out_without_refetching() {
        let d = tempfile::tempdir().unwrap();
        setup(d.path(), &["t2"]);
        let cs = chunks(d.path());
        assert!(of(&cs, "linear/issue/2").is_empty());
        assert!(!of(&cs, "linear/issue/1").is_empty());
        crate::linear::set_muted(d.path(), &[]).unwrap();
        assert!(!of(&chunks(d.path()), "linear/issue/2").is_empty());
    }

    #[test]
    fn all_teams_muted_gives_no_chunks_not_an_error() {
        let d = tempfile::tempdir().unwrap();
        setup(d.path(), &["t1", "t2"]);
        assert!(chunks(d.path()).is_empty());
    }

    #[test]
    fn seven_days_after_a_refusal_linear_leaves_search() {
        let d = tempfile::tempdir().unwrap();
        setup(d.path(), &[]);
        let mut s = crate::linear::fetch::load_state(d.path());
        s.lost_since = Some("2026-10-26T12:00:00.000Z".into());
        save_state(d.path(), &s).unwrap();
        assert!(!chunks(d.path()).is_empty()); // 6 days
        s.lost_since = Some("2026-10-25T12:00:00.000Z".into());
        save_state(d.path(), &s).unwrap();
        assert!(chunks(d.path()).is_empty()); // 7 days
    }

    #[test]
    fn teams_list_counts_issues() {
        let d = tempfile::tempdir().unwrap();
        setup(d.path(), &[]);
        let ts = teams(d.path());
        assert_eq!(ts.len(), 2);
        assert_eq!(
            (ts[0].id.as_str(), ts[0].name.as_str(), ts[0].issues),
            ("t1", "Core", 1)
        );
    }

    #[test]
    fn an_issue_with_only_a_title_is_still_searchable() {
        let d = tempfile::tempdir().unwrap();
        setup(d.path(), &[]);
        put(
            d.path(),
            "issue-3",
            json!({"id": "3", "identifier": "COR-9", "title": "Rotate keys", "url": "https://linear.app/a/issue/COR-9",
            "state": {"type": "unstarted"}, "team": {"id": "t1", "name": "Core"}, "updatedAt": "2026-10-28T00:00:00.000Z", "comments": []}),
        );
        let cs = chunks(d.path());
        let i = of(&cs, "linear/issue/3");
        assert_eq!(i.len(), 1);
        assert!(
            i[0]["text"]
                .as_str()
                .unwrap()
                .ends_with("\nCOR-9 · Rotate keys")
        );
    }
}
