//! Turns raw Asana data into documents and chunks (docs/32 6절 "조각").
//!
//! - task: `asana/task/<gid>`, header `asana: 이름` (a subtask is `asana: 부모 이름 › 이름`) + `meta: Asana · 프로젝트 · 섹션 ·
//!   완료|진행 중 · 담당 · 마감 10월 27일 · 10월 27일 수정` (empty fields are dropped). The body is `notes`, then comments
//!   in time order (`이름 10월 27일: 본문`). Every line links to the task's `permalink_url` (comments have no URL).
//! - project: `asana/project/<gid>`, header `asana: 이름` + `meta: Asana · 팀 · 10월 27일 수정`; the body is the
//!   description. A project with an empty description has no chunk.
//! - Mentions (`https://app.asana.com/0/<사용자 gid>/`) become `@이름`; when the name is unknown the URL is dropped.
//! - Items in muted or blocked workspaces are left out. A task in a muted project stays only if it is also in at least
//!   one unmuted (and fetched) project, or is a task outside projects (fetched with `assignee=me`). Subtasks follow
//!   their top parent. The raw data stays as is.
//! - Once `LOST_DAYS` have passed since the connection was lost (`lost_since`), no chunks are produced (the raw data
//!   is not deleted).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Utc};
use regex::Regex;
use serde_json::{Value, json};
use tacie_index::RawChunk;

use crate::asana::state::{State, WINDOW_DAYS, item_ids, load_state, parse, read_item};
use crate::doc::{Doc, windows, windows_at};
use crate::error::SourceError;

/// 연결이 끊긴 뒤 이만큼 지나면 Asana 자료를 찾지 않아요(원본은 남겨요).
pub const LOST_DAYS: i64 = 7;
const KST_SECS: i32 = 9 * 3600;
const SRC: &str = "asana";

static MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https://app\.asana\.com/0/(\d+)/").expect("mention"));

fn kst(s: &str) -> Option<DateTime<FixedOffset>> {
    parse(s).map(|t| t.with_timezone(&FixedOffset::east_opt(KST_SECS).expect("KST")))
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

/// 마감(`due_on`, `2026-10-27`)이에요. 날짜뿐이라 시간대를 옮기지 않아요.
fn due(s: &str) -> String {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_or_else(
        |_| String::new(),
        |d| format!("마감 {}월 {}일", d.month(), d.day()),
    )
}

fn edited(at: &str) -> String {
    let d = md(at);
    if d.is_empty() {
        d
    } else {
        format!("{d} 수정")
    }
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
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 글의 멘션이에요: `https://app.asana.com/0/<사용자 gid>/` 바로 뒤가 끝이거나 ASCII 글자·숫자가 아닌 것만이에요
/// (`/0/<프로젝트>/<task>` 같은 task 주소는 빼요). (시작, 끝, 사용자 gid)예요. 받기(`fetch`)와 조각이 같이 써요.
pub(crate) fn mention_spans(text: &str) -> Vec<(usize, usize, &str)> {
    MENTION
        .captures_iter(text)
        .filter_map(|c| {
            let m = c.get(0)?;
            let next = text.as_bytes().get(m.end());
            if next.is_some_and(u8::is_ascii_alphanumeric) {
                return None;
            }
            Some((m.start(), m.end(), c.get(1)?.as_str()))
        })
        .collect()
}

/// 멘션 주소를 `@이름`으로 바꿔요. 이름을 모르면 주소를 빼요.
fn mentions(text: &str, users: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (start, end, gid) in mention_spans(text) {
        out.push_str(&text[last..start]);
        if let Some(name) = users.get(gid).filter(|n| !n.is_empty()) {
            out.push('@');
            out.push_str(name);
        }
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

/// 연결이 끊긴 지 `LOST_DAYS`가 지났는지예요.
pub(crate) fn expired(st: &State, now: DateTime<Utc>) -> bool {
    st.lost_since
        .as_deref()
        .and_then(parse)
        .is_some_and(|lost| now - lost >= Duration::days(LOST_DAYS))
}

/// 글의 프로젝트 gid예요(`memberships`).
fn projects_of(v: &Value) -> Vec<&str> {
    v["memberships"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["project"]["gid"].as_str())
        .collect()
}

/// 이 워크스페이스의 것을 조각으로 내도 되는지예요: 끄지 않았고 막히지 않았고, 지난 `users/me` 목록에 있어요
/// (빠진 워크스페이스는 재조정이 지우기 전에도 바로 찾지 않아요. 원본은 남아요). task 파일은 목록을 받은 뒤에만
/// 생겨서, 목록이 비었으면 모든 워크스페이스에서 나간 거예요: 아무것도 내지 않아요.
fn open_ws(st: &State, ws: &str) -> bool {
    !st.muted_workspaces.iter().any(|m| m == ws)
        && st.workspaces.iter().any(|w| w.gid == ws && !w.blocked)
}

/// 맨 위 task(하위 task면 맨 위 부모)를 남길지예요. 받아 둔 프로젝트 중 끄지 않은 것에 들어 있거나, 받아 둔 프로젝트에
/// 하나도 들어 있지 않으면(프로젝트 밖 내 task) 남아요.
fn top_kept(st: &State, v: &Value) -> bool {
    let stored: Vec<&str> = projects_of(v)
        .into_iter()
        .filter(|p| st.projects.contains_key(*p))
        .collect();
    stored.is_empty()
        || stored
            .iter()
            .any(|p| !st.muted_projects.iter().any(|m| m == p))
}

/// 하위 task의 맨 위 부모 원본이에요. 맨 위 task거나 부모 원본이 없으면 None이에요.
fn top_of(raw_dir: &Path, v: &Value) -> Option<Value> {
    let g = v["parents"].as_array()?.first()?["gid"].as_str()?;
    read_item(raw_dir, "task", g)
}

fn is_subtask(v: &Value) -> bool {
    v["parents"].as_array().is_some_and(|c| !c.is_empty()) || v["parent"]["gid"].is_string()
}

/// task를 조각으로 낼지예요. 하위 task는 맨 위 부모(`top`)를 따라요. 부모 원본이 없는 하위 task는 끄지 않은 받아 둔
/// 프로젝트에 직접 들어 있을 때만 남아요(끈 프로젝트의 부모 밑 글이 새지 않게요).
fn task_kept(st: &State, v: &Value, top: Option<&Value>) -> bool {
    if !open_ws(st, s(v, &["workspace"])) {
        return false;
    }
    match top {
        Some(t) => top_kept(st, t),
        None if is_subtask(v) => projects_of(v)
            .into_iter()
            .any(|p| st.projects.contains_key(p) && !st.muted_projects.iter().any(|m| m == p)),
        None => top_kept(st, v),
    }
}

/// Asana가 프로젝트를 만들 때 붙이는 기본 섹션 이름이에요(계정 언어를 따라요). 메타 줄에 넣지 않아요.
const UNTITLED_SECTIONS: &[&str] = &["Untitled section", "제목 없는 섹션"];

/// 메타 줄의 프로젝트와 섹션이에요: 끄지 않은 첫 프로젝트예요. 없으면(하위 task) 맨 위 부모의 것이에요.
fn place(st: &State, v: &Value, top: Option<&Value>) -> (String, String) {
    let pick = |v: &Value| {
        v["memberships"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| {
                let g = s(m, &["project", "gid"]);
                !g.is_empty() && !st.muted_projects.iter().any(|x| x == g)
            })
            .map(|m| {
                let g = s(m, &["project", "gid"]);
                let name = match s(m, &["project", "name"]) {
                    "" => st.projects.get(g).map_or("", |p| p.name.as_str()),
                    n => n,
                };
                let section = s(m, &["section", "name"]);
                // Asana's default section says nothing about the task (its name follows the account's language)
                let section = if UNTITLED_SECTIONS.contains(&section.trim()) { "" } else { section };
                (name.to_string(), section.to_string())
            })
    };
    pick(v).or_else(|| pick(top?)).unwrap_or_default()
}

/// The task's chunk, plus the body line where each comment starts (each comment opens a new window, so a card on a
/// comment links to that comment).
fn task_doc(st: &State, v: &Value, top: Option<&Value>) -> Option<(Doc, Vec<usize>)> {
    let gid = s(v, &["gid"]);
    if gid.is_empty() {
        return None;
    }
    let name = s(v, &["name"]).trim().to_string();
    let url = match s(v, &["permalink_url"]) {
        "" => format!("https://app.asana.com/0/0/{gid}"),
        u => u.to_string(),
    };
    let parent = v["parents"]
        .as_array()
        .and_then(|c| c.last())
        .map_or_else(|| s(v, &["parent", "name"]), |p| s(p, &["name"]))
        .trim()
        .to_string();
    let title = if parent.is_empty() {
        name.clone()
    } else {
        format!("{parent} › {name}")
    };
    let (project, section) = place(st, v, top);
    let state = if v["completed"].as_bool().unwrap_or(false) {
        "완료"
    } else {
        "진행 중"
    };
    let assignee = match s(v, &["assignee", "name"]) {
        "" => st
            .users
            .get(s(v, &["assignee", "gid"]))
            .map_or("", String::as_str),
        n => n,
    };
    let at = s(v, &["modified_at"]);
    let head = format!(
        "asana: {title}\nmeta: {}",
        join(&[
            "Asana",
            &project,
            &section,
            state,
            assignee,
            &due(s(v, &["due_on"])),
            &edited(at),
        ])
    );
    let mut body: Vec<(String, String)> = mentions(s(v, &["notes"]), &st.users)
        .lines()
        .map(|l| (l.to_string(), url.clone()))
        .collect();
    let mut stories: Vec<&Value> = v["stories"].as_array().into_iter().flatten().collect();
    stories.sort_by_key(|c| parse(s(c, &["created_at"])));
    let ws = s(v, &["workspace"]);
    let mut starts = Vec::new();
    for c in stories {
        // the link Asana's "댓글 링크 복사" gives (checked 2026-11-13); without the ids, the task's own link
        let at_comment = match s(c, &["gid"]) {
            story if !story.is_empty() && !ws.is_empty() => {
                format!("https://app.asana.com/1/{ws}/task/{gid}/comment/{story}?focus=true")
            }
            _ => url.clone(),
        };
        starts.push(body.len());
        let who = match s(c, &["created_by", "name"]) {
            "" => st
                .users
                .get(s(c, &["created_by", "gid"]))
                .map_or("", String::as_str),
            n => n,
        };
        let date = md(s(c, &["created_at"]));
        let lead = [who, date.as_str()]
            .iter()
            .filter(|p| !p.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        let lead = if lead.is_empty() {
            lead
        } else {
            format!("{lead}: ")
        };
        let text = mentions(s(c, &["text"]), &st.users);
        let mut lines = text.lines();
        body.push((format!("{lead}{}", lines.next().unwrap_or("")), at_comment.clone()));
        body.extend(lines.map(|l| (l.to_string(), at_comment.clone())));
    }
    if body.iter().all(|(l, _)| l.trim().is_empty()) {
        body = vec![(title.clone(), url.clone())];
    }
    Some((
        Doc {
            path: format!("asana/task/{gid}"),
            title,
            url,
            author: None,
            at: ymd(at),
            acl: None,
            head,
            body,
        },
        starts,
    ))
}

fn project_doc(st: &State, v: &Value) -> Option<Doc> {
    let gid = s(v, &["gid"]);
    let notes = mentions(s(v, &["notes"]), &st.users);
    if gid.is_empty() || notes.trim().is_empty() {
        return None;
    }
    let name = s(v, &["name"]).trim().to_string();
    let url = match s(v, &["permalink_url"]) {
        "" => format!("https://app.asana.com/0/{gid}"),
        u => u.to_string(),
    };
    let at = s(v, &["modified_at"]);
    let head = format!(
        "asana: {name}\nmeta: {}",
        join(&["Asana", s(v, &["team", "name"]), &edited(at)])
    );
    let body = notes
        .lines()
        .map(|l| (l.to_string(), url.clone()))
        .collect();
    Some(Doc {
        path: format!("asana/project/{gid}"),
        title: name,
        url,
        author: None,
        at: ymd(at),
        acl: None,
        head,
        body,
    })
}

/// 저장본의 조각이에요(`src`는 "asana"). 끈·막힌 것과 2년이 지난 task는 빼고, 연결이 끊긴 지 `LOST_DAYS`가 지났으면
/// 비어요.
pub fn chunks_of(raw_dir: &Path, now: DateTime<Utc>) -> Result<Vec<RawChunk>, SourceError> {
    let st = load_state(raw_dir);
    if expired(&st, now) {
        return Ok(Vec::new());
    }
    let floor = now - Duration::days(WINDOW_DAYS);
    let mut out = Vec::new();
    for gid in item_ids(raw_dir, "task") {
        let Some(v) = read_item(raw_dir, "task", &gid) else {
            continue;
        };
        if parse(s(&v, &["modified_at"])).is_some_and(|t| t <= floor) {
            continue;
        }
        let top = top_of(raw_dir, &v);
        if !task_kept(&st, &v, top.as_ref()) {
            continue;
        }
        if let Some((d, starts)) = task_doc(&st, &v, top.as_ref()) {
            out.extend(windows_at(&d, SRC, &starts));
        }
    }
    for gid in item_ids(raw_dir, "project") {
        let Some(v) = read_item(raw_dir, "project", &gid) else {
            continue;
        };
        if !open_ws(&st, s(&v, &["workspace"])) || st.muted_projects.contains(&gid) {
            continue;
        }
        if let Some(d) = project_doc(&st, &v) {
            out.extend(windows(&d, SRC));
        }
    }
    Ok(out)
}

/// 고르기 시트의 목록이에요: `{"workspaces":[{gid,name,tasks,blocked}], "projects":[{gid,workspace,name,tasks}]}`.
/// 끈 것도 들어가요(다시 켤 수 있게요). 관리자가 막은 워크스페이스는 `blocked`가 참이에요(시트가 켜고 끌 수 없게 보여요). task 수는 받아 둔 task 파일을 세요(프로젝트는 `memberships`로).
pub fn scopes(raw_dir: &Path) -> Value {
    let st = load_state(raw_dir);
    let mut per_ws: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_project: BTreeMap<String, usize> = BTreeMap::new();
    for gid in item_ids(raw_dir, "task") {
        let Some(v) = read_item(raw_dir, "task", &gid) else {
            continue;
        };
        *per_ws.entry(s(&v, &["workspace"]).to_string()).or_default() += 1;
        let mut ps = projects_of(&v);
        ps.sort_unstable();
        ps.dedup();
        for p in ps {
            *per_project.entry(p.to_string()).or_default() += 1;
        }
    }
    let mut workspaces: Vec<Value> = st
        .workspaces
        .iter()
        .map(|w| {
            json!({"gid": w.gid, "name": w.name, "tasks": per_ws.get(&w.gid).copied().unwrap_or(0),
                   "blocked": w.blocked})
        })
        .collect();
    workspaces.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let mut projects: Vec<Value> = st
        .projects
        .iter()
        .map(|(g, p)| {
            json!({"gid": g, "workspace": p.ws, "name": p.name,
                   "tasks": per_project.get(g).copied().unwrap_or(0)})
        })
        .collect();
    projects.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    json!({"workspaces": workspaces, "projects": projects})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asana::state::{Proj, Ws, iso, save_state, write_item};

    fn now() -> DateTime<Utc> {
        parse("2026-11-12T03:00:00.000Z").unwrap()
    }

    /// 워크스페이스 w1(acme)에 프로젝트 p1(로드맵), p2(결제)가 있고, 사용자 u1은 지윤이에요.
    fn seed(dir: &Path) -> State {
        let mut st = State {
            user_gid: Some("me-1".into()),
            workspaces: vec![Ws {
                gid: "w1".into(),
                name: "acme".into(),
                ..Ws::default()
            }],
            ..State::default()
        };
        for (g, n) in [("p1", "로드맵"), ("p2", "결제")] {
            st.projects.insert(
                g.into(),
                Proj {
                    ws: "w1".into(),
                    name: n.into(),
                    ..Proj::default()
                },
            );
        }
        st.users.insert("u1".into(), "지윤".into());
        save_state(dir, &st).unwrap();
        st
    }

    fn task(gid: &str, name: &str, projects: &[&str]) -> Value {
        let memberships: Vec<Value> = projects
            .iter()
            .map(|p| {
                json!({"project": {"gid": p, "name": if *p == "p1" { "로드맵" } else { "결제" }},
                            "section": {"name": "진행"}})
            })
            .collect();
        json!({
            "gid": gid, "name": name, "notes": "", "completed": false,
            "modified_at": "2026-10-27T01:00:00.000Z",
            "permalink_url": format!("https://app.asana.com/1/w1/task/{gid}"),
            "memberships": memberships, "workspace": "w1", "parents": [], "stories": [],
        })
    }

    fn put(dir: &Path, v: &Value) {
        write_item(dir, "task", v["gid"].as_str().unwrap(), v).unwrap();
    }

    fn by_path<'a>(chunks: &'a [RawChunk], path: &str) -> Vec<&'a RawChunk> {
        chunks.iter().filter(|c| c["path"] == path).collect()
    }

    fn text(c: &RawChunk) -> &str {
        c["text"].as_str().unwrap()
    }

    #[test]
    fn the_default_untitled_section_is_left_out_of_the_meta_line() {
        let d = tempfile::tempdir().unwrap();
        seed(d.path());
        for (gid, section) in [("10", "제목 없는 섹션"), ("11", "Untitled section"), ("12", "진행")] {
            let mut t = task(gid, "할 일", &["p2"]);
            t["memberships"][0]["section"]["name"] = json!(section);
            put(d.path(), &t);
        }
        let chunks = chunks_of(d.path(), now()).unwrap();
        let meta = |p: &str| text(by_path(&chunks, p)[0]).lines().nth(1).unwrap().to_string();
        assert_eq!(meta("asana/task/10"), "meta: Asana · 결제 · 진행 중 · 10월 27일 수정");
        assert_eq!(meta("asana/task/11"), "meta: Asana · 결제 · 진행 중 · 10월 27일 수정");
        assert_eq!(meta("asana/task/12"), "meta: Asana · 결제 · 진행 · 진행 중 · 10월 27일 수정");
    }

    #[test]
    fn a_comment_without_ids_keeps_the_task_link() {
        let d = tempfile::tempdir().unwrap();
        seed(d.path());
        let mut t = task("10", "할 일", &["p2"]);
        t["stories"] = json!([{"text": "네", "created_at": "2026-10-27T02:00:00.000Z",
                               "created_by": {"gid": "u1", "name": "지윤"}}]);
        put(d.path(), &t);
        let chunks = chunks_of(d.path(), now()).unwrap();
        assert_eq!(chunks.len(), 1); // empty notes: the comment is the only window
        assert_eq!(chunks[0]["url"], "https://app.asana.com/1/w1/task/10");
    }

    #[test]
    fn task_chunk_header_meta_and_comment_lines() {
        let d = tempfile::tempdir().unwrap();
        seed(d.path());
        let mut t = task("10", "결제 웹훅 중복", &["p2"]);
        t["notes"] = json!("웹훅이 두 번 와요.\n재시도 때문이에요.");
        t["assignee"] = json!({"gid": "u1", "name": "지윤"});
        t["due_on"] = json!("2026-11-03");
        t["completed"] = json!(true);
        t["stories"] = json!([
            {"gid": "s2", "text": "제가 볼게요", "created_at": "2026-10-28T02:00:00.000Z",
             "created_by": {"gid": "u2", "name": "민호"}},
            {"gid": "s1", "text": "언제부터예요?", "created_at": "2026-10-27T02:00:00.000Z",
             "created_by": {"gid": "u1", "name": "지윤"}},
        ]);
        put(d.path(), &t);
        let chunks = chunks_of(d.path(), now()).unwrap();
        // the notes, then one window per comment so a card on a comment opens that comment
        assert_eq!(chunks.len(), 3);
        let urls: Vec<&str> = chunks.iter().map(|c| c["url"].as_str().unwrap()).collect();
        assert_eq!(
            urls,
            [
                "https://app.asana.com/1/w1/task/10",
                "https://app.asana.com/1/w1/task/10/comment/s1?focus=true",
                "https://app.asana.com/1/w1/task/10/comment/s2?focus=true",
            ]
        );
        let all: Vec<&str> = chunks.iter().map(text).collect();
        assert!(all[1].ends_with("지윤 10월 27일: 언제부터예요?"), "{}", all[1]);
        assert!(all[2].ends_with("민호 10월 28일: 제가 볼게요"), "{}", all[2]);
        let c = &chunks[0];
        assert_eq!(c["src"], "asana");
        assert_eq!(c["path"], "asana/task/10");
        assert_eq!(c["url"], "https://app.asana.com/1/w1/task/10");
        assert_eq!(c["acl"], json!(["*"]));
        assert_eq!(c["at"], "2026-10-27");
        assert_eq!(
            text(c),
            "asana: 결제 웹훅 중복\n\
             meta: Asana · 결제 · 진행 · 완료 · 지윤 · 마감 11월 3일 · 10월 27일 수정\n\
             웹훅이 두 번 와요.\n재시도 때문이에요."
        );
        // 빈 칸은 빼요.
        put(d.path(), &task("11", "빈 task", &[]));
        let chunks = chunks_of(d.path(), now()).unwrap();
        let c = by_path(&chunks, "asana/task/11")[0];
        assert_eq!(
            text(c),
            "asana: 빈 task\nmeta: Asana · 진행 중 · 10월 27일 수정\n빈 task"
        );
    }

    #[test]
    fn subtask_header_names_the_parent() {
        let d = tempfile::tempdir().unwrap();
        seed(d.path());
        put(d.path(), &task("1", "출시 준비", &["p1"]));
        let mut sub = task("2", "QA 체크", &[]);
        sub["parents"] = json!([{"gid": "1", "name": "출시 준비"}]);
        sub["notes"] = json!("회귀 테스트");
        put(d.path(), &sub);
        let mut deeper = task("3", "로그인", &[]);
        deeper["parents"] =
            json!([{"gid": "1", "name": "출시 준비"}, {"gid": "2", "name": "QA 체크"}]);
        put(d.path(), &deeper);
        let chunks = chunks_of(d.path(), now()).unwrap();
        let c = by_path(&chunks, "asana/task/2")[0];
        // 프로젝트 칸은 맨 위 부모의 프로젝트예요.
        assert!(
            text(c)
                .starts_with("asana: 출시 준비 › QA 체크\nmeta: Asana · 로드맵 · 진행 · 진행 중"),
            "{}",
            text(c)
        );
        assert_eq!(c["title"], "출시 준비 › QA 체크");
        let c = by_path(&chunks, "asana/task/3")[0];
        assert!(
            text(c).starts_with("asana: QA 체크 › 로그인\n"),
            "{}",
            text(c)
        );
    }

    #[test]
    fn mentions_become_names() {
        let d = tempfile::tempdir().unwrap();
        seed(d.path());
        let mut t = task("1", "결정", &["p1"]);
        t["notes"] = json!(
            "https://app.asana.com/0/1201/ 님 확인 부탁해요, https://app.asana.com/0/999/ 도요. \
             참고 https://app.asana.com/0/1201/55\n\
             https://app.asana.com/0/1201/님 이 정했어요"
        );
        t["stories"] = json!([{"gid": "s1", "text": "https://app.asana.com/0/1201/ 맞아요",
                               "created_at": "2026-10-27T02:00:00.000Z",
                               "created_by": {"gid": "u1", "name": "지윤"}}]);
        put(d.path(), &t);
        let mut st = load_state(d.path());
        st.users.insert("1201".into(), "민호".into());
        st.users.insert("999".into(), String::new());
        save_state(d.path(), &st).unwrap();
        let chunks = chunks_of(d.path(), now()).unwrap();
        let body = chunks.iter().map(text).collect::<Vec<_>>().join("\n");
        assert!(
            body.contains("@민호 님 확인 부탁해요,  도요. 참고 https://app.asana.com/0/1201/55"),
            "{body}"
        );
        assert!(body.contains("\n@민호님 이 정했어요"), "{body}");
        assert!(body.contains("지윤 10월 27일: @민호 맞아요"), "{body}");
    }

    #[test]
    fn muted_project_task_left_out_unless_in_an_unmuted_project() {
        let d = tempfile::tempdir().unwrap();
        let mut st = seed(d.path());
        put(d.path(), &task("1", "결제만", &["p2"]));
        put(d.path(), &task("2", "둘 다", &["p1", "p2"]));
        put(d.path(), &task("3", "프로젝트 밖 내 task", &[]));
        // 받아 두지 않은 프로젝트에만 든 task는 프로젝트 밖으로 봐요.
        put(d.path(), &task("4", "모르는 프로젝트", &["p9"]));
        st.muted_projects = vec!["p2".into()];
        save_state(d.path(), &st).unwrap();
        let chunks = chunks_of(d.path(), now()).unwrap();
        assert!(by_path(&chunks, "asana/task/1").is_empty());
        let both = by_path(&chunks, "asana/task/2");
        assert_eq!(both.len(), 1);
        // 메타의 프로젝트는 끄지 않은 프로젝트예요.
        assert!(
            text(both[0]).contains("meta: Asana · 로드맵 ·"),
            "{}",
            text(both[0])
        );
        assert_eq!(by_path(&chunks, "asana/task/3").len(), 1);
        assert_eq!(by_path(&chunks, "asana/task/4").len(), 1);
        // 끈 워크스페이스와 막힌 워크스페이스는 모두 빠져요.
        st.muted_projects.clear();
        st.muted_workspaces = vec!["w1".into()];
        save_state(d.path(), &st).unwrap();
        assert!(chunks_of(d.path(), now()).unwrap().is_empty());
        st.muted_workspaces.clear();
        st.workspaces[0].blocked = true;
        save_state(d.path(), &st).unwrap();
        assert!(chunks_of(d.path(), now()).unwrap().is_empty());
    }

    #[test]
    fn subtask_follows_its_parent_mute() {
        let d = tempfile::tempdir().unwrap();
        let mut st = seed(d.path());
        put(d.path(), &task("1", "결제 부모", &["p2"]));
        // 하위 task가 켜진 프로젝트에도 들어 있어도 부모를 따라요.
        let mut sub = task("2", "결제 하위", &["p1"]);
        sub["parents"] = json!([{"gid": "1", "name": "결제 부모"}]);
        put(d.path(), &sub);
        put(d.path(), &task("3", "로드맵 부모", &["p1"]));
        let mut sub2 = task("4", "로드맵 하위", &[]);
        sub2["parents"] = json!([{"gid": "3", "name": "로드맵 부모"}]);
        put(d.path(), &sub2);
        st.muted_projects = vec!["p2".into()];
        save_state(d.path(), &st).unwrap();
        let chunks = chunks_of(d.path(), now()).unwrap();
        let paths: Vec<&str> = chunks.iter().map(|c| c["path"].as_str().unwrap()).collect();
        assert_eq!(paths, ["asana/task/3", "asana/task/4"]);
    }

    #[test]
    fn subtask_without_its_parent_file_needs_its_own_unmuted_project() {
        let d = tempfile::tempdir().unwrap();
        seed(d.path());
        let orphan = |gid: &str, projects: &[&str]| {
            let mut t = task(gid, "고아", projects);
            t["parents"] = json!([{"gid": "404", "name": "없는 부모"}]);
            t
        };
        put(d.path(), &orphan("1", &[]));
        put(d.path(), &orphan("2", &["p9"]));
        put(d.path(), &orphan("3", &["p1"]));
        let chunks = chunks_of(d.path(), now()).unwrap();
        let paths: Vec<&str> = chunks.iter().map(|c| c["path"].as_str().unwrap()).collect();
        assert_eq!(paths, ["asana/task/3"]);
    }

    #[test]
    fn empty_project_notes_make_no_chunk() {
        let d = tempfile::tempdir().unwrap();
        let mut st = seed(d.path());
        let p = |gid: &str, notes: &str| {
            json!({"gid": gid, "name": "로드맵", "notes": notes, "modified_at": "2026-10-27T01:00:00.000Z",
                   "team": {"name": "플랫폼"}, "permalink_url": format!("https://app.asana.com/0/{gid}"),
                   "workspace": "w1"})
        };
        write_item(d.path(), "project", "p1", &p("p1", "  \n")).unwrap();
        write_item(d.path(), "project", "p2", &p("p2", "분기 목표예요")).unwrap();
        let chunks = chunks_of(d.path(), now()).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["path"], "asana/project/p2");
        assert_eq!(chunks[0]["url"], "https://app.asana.com/0/p2");
        assert_eq!(
            text(&chunks[0]),
            "asana: 로드맵\nmeta: Asana · 플랫폼 · 10월 27일 수정\n분기 목표예요"
        );
        st.muted_projects = vec!["p2".into()];
        save_state(d.path(), &st).unwrap();
        assert!(chunks_of(d.path(), now()).unwrap().is_empty());
    }

    #[test]
    fn scopes_counts_tasks_per_workspace_and_project() {
        let d = tempfile::tempdir().unwrap();
        let mut st = seed(d.path());
        st.workspaces.push(Ws {
            gid: "w2".into(),
            name: "beta".into(),
            blocked: true,
            ..Ws::default()
        });
        st.muted_projects = vec!["p2".into()];
        save_state(d.path(), &st).unwrap();
        put(d.path(), &task("1", "가", &["p1", "p2"]));
        put(d.path(), &task("2", "나", &["p2"]));
        put(d.path(), &task("3", "다", &[]));
        let v = scopes(d.path());
        assert_eq!(
            v["workspaces"],
            json!([{"gid": "w1", "name": "acme", "tasks": 3, "blocked": false},
                   {"gid": "w2", "name": "beta", "tasks": 0, "blocked": true}])
        );
        assert_eq!(
            v["projects"],
            json!([
                {"gid": "p2", "workspace": "w1", "name": "결제", "tasks": 2},
                {"gid": "p1", "workspace": "w1", "name": "로드맵", "tasks": 1},
            ])
        );
    }

    #[test]
    fn lost_for_seven_days_hides_chunks_and_old_tasks_are_left_out() {
        let d = tempfile::tempdir().unwrap();
        let mut st = seed(d.path());
        put(d.path(), &task("1", "가", &["p1"]));
        let mut old = task("2", "오래됨", &["p1"]);
        old["modified_at"] = json!(iso(now() - Duration::days(WINDOW_DAYS + 1)));
        put(d.path(), &old);
        assert_eq!(chunks_of(d.path(), now()).unwrap().len(), 1);
        st.lost_since = Some(iso(now() - Duration::days(LOST_DAYS - 1)));
        save_state(d.path(), &st).unwrap();
        assert_eq!(chunks_of(d.path(), now()).unwrap().len(), 1);
        st.lost_since = Some(iso(now() - Duration::days(LOST_DAYS)));
        save_state(d.path(), &st).unwrap();
        assert!(chunks_of(d.path(), now()).unwrap().is_empty());
    }

    /// `users/me`에서 빠진 워크스페이스의 task는 재조정이 지우기 전에도 조각이 되지 않아요. 원본은 남아요.
    #[test]
    fn a_workspace_gone_from_the_list_yields_no_chunks() {
        let d = tempfile::tempdir().unwrap();
        let mut st = seed(d.path());
        put(d.path(), &task("1", "가", &["p1"]));
        assert_eq!(chunks_of(d.path(), now()).unwrap().len(), 1);
        st.workspaces = vec![Ws {
            gid: "w9".into(),
            name: "other".into(),
            ..Ws::default()
        }];
        save_state(d.path(), &st).unwrap();
        assert!(chunks_of(d.path(), now()).unwrap().is_empty());
        assert!(read_item(d.path(), "task", "1").is_some());
        // 목록이 비었으면(모든 워크스페이스에서 나갔어요) 아무것도 내지 않아요.
        st.workspaces.clear();
        save_state(d.path(), &st).unwrap();
        assert!(chunks_of(d.path(), now()).unwrap().is_empty());
    }

    /// 모든 워크스페이스에서 나가면 `users/me`가 빈 목록을 주고, 받아 둔 task는 재조정을 기다리지 않고 바로 빠져요.
    #[test]
    fn leaving_every_workspace_yields_no_chunks() {
        let d = tempfile::tempdir().unwrap();
        let mut st = seed(d.path());
        put(d.path(), &task("1", "가", &["p1"]));
        put(d.path(), &task("2", "나", &[]));
        st.fetched_at = Some(iso(now()));
        assert!(!chunks_of(d.path(), now()).unwrap().is_empty());
        st.workspaces = Vec::new();
        save_state(d.path(), &st).unwrap();
        assert!(chunks_of(d.path(), now()).unwrap().is_empty());
        assert!(read_item(d.path(), "task", "1").is_some());
        assert!(scopes(d.path())["workspaces"].as_array().unwrap().is_empty());
    }
}
