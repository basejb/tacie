//! Turns raw Dropbox data into documents and chunks (docs/33).
//!
//! - One file is one document: `dropbox/<fileId>`, header `dropbox: 이름` + `meta: Dropbox · 폴더 / 폴더 · 10월 27일 수정`.
//! - Paper, Word, PowerPoint, Excel and Markdown use the same Markdown splitting as Drive (headings `#`, table rows);
//!   PDF and text are 40-line windows of the lines as they are.
//! - Links are preview URLs without a shared link: `https://www.dropbox.com/home/<폴더>?preview=<이름>` (checked
//!   2026-11-13).
//! - Files that could not be fetched (`blocked`) have a single title line as their body; files not fetched yet produce
//!   no chunks.
//! - Files in muted folders and files older than 2 years are left out. Once `LOST_DAYS` have passed since the
//!   connection was lost, it is empty. The raw data stays as is.

use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, Duration, Utc};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use tacie_index::RawChunk;

use super::state::{WINDOW_DAYS, item_ids, load_state, read_item, under};
use crate::doc::{Doc, windows_at};
use crate::error::SourceError;
use crate::gdrive::docs::{LOST_DAYS, join, markdown_lines, md, table_cuts, ymd};
use crate::gdrive::fetch::parse;

const SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

/// Paper를 내보낸 Markdown의 그림 줄이에요. 첨부 주소만 있고 검색에는 쓸모가 없어요.
static IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[[^\]\n]*\]\([^)\n]*\)").unwrap());

/// 폴더 고르기 화면의 한 줄이에요.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FolderInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub files: usize,
    pub team_folder: bool,
}

/// 파일 미리보기 주소예요. 부모 폴더는 조각마다, 이름은 통째로 퍼센트 인코딩해요.
pub fn web_link(path_display: &str) -> String {
    let (parent, name) = path_display.rsplit_once('/').unwrap_or(("", path_display));
    let enc = |s: &str| utf8_percent_encode(s, SET).to_string();
    let parent: Vec<String> = parent
        .split('/')
        .filter(|p| !p.is_empty())
        .map(enc)
        .collect();
    if parent.is_empty() {
        format!("https://www.dropbox.com/home?preview={}", enc(name))
    } else {
        format!(
            "https://www.dropbox.com/home/{}?preview={}",
            parent.join("/"),
            enc(name)
        )
    }
}

/// 파일 하나의 문서와 창 시작 줄이에요.
pub(crate) fn document(item: &Value) -> Option<(Doc, Vec<usize>)> {
    let id = item["id"].as_str().filter(|i| !i.is_empty())?;
    let name = item["name"].as_str().unwrap_or("").to_string();
    let display = item["path_display"].as_str().unwrap_or("");
    let modified = item["server_modified"].as_str().unwrap_or("");
    let url = web_link(display);
    let folders: Vec<&str> = display
        .rsplit_once('/')
        .map_or("", |(p, _)| p)
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    let edited = if modified.is_empty() {
        String::new()
    } else {
        format!("{} 수정", md(modified))
    };
    let head = format!(
        "dropbox: {name}\nmeta: {}",
        join(&["Dropbox", &folders.join(" / "), &edited])
    );

    let raw = IMAGE.replace_all(item["text"].as_str().unwrap_or(""), "");
    let (lines, mut starts) = match item["type"].as_str() {
        Some("paper" | "docx" | "pptx" | "xlsx" | "md") => markdown_lines(&raw),
        _ => (
            raw.split('\n').map(|l| (l.to_string(), false)).collect(),
            Vec::new(),
        ),
    };
    starts.extend(table_cuts(&lines, &starts));
    let mut body: Vec<(String, String)> =
        lines.into_iter().map(|(l, _)| (l, url.clone())).collect();
    if body.iter().all(|(l, _)| l.trim().is_empty()) {
        body = vec![(name.clone(), url.clone())];
        starts.clear();
    }
    Some((
        Doc {
            path: format!("dropbox/{id}"),
            title: name,
            url,
            author: None,
            at: ymd(modified),
            acl: None,
            head,
            body,
        },
        starts,
    ))
}

/// 노트 출처가 받는 vault 아래 파일의 문서 경로(`dropbox/<id>`)예요(docs/34 6절). `local`은 노트 출처가 고른 이 Mac의
/// 폴더 경로예요. Dropbox의 vault 경로(소문자, `/일/노트`)로 끝나는 폴더를 고른 것이면 같은 vault로 봐요.
pub fn doc_paths_in_vaults(raw_dir: &Path, local: &[String]) -> std::collections::HashSet<String> {
    let st = load_state(raw_dir);
    let local: Vec<String> = local
        .iter()
        .map(|p| p.trim_end_matches('/').to_lowercase())
        .collect();
    let taken: Vec<&String> = st
        .vaults
        .iter()
        .filter(|v| local.iter().any(|l| l.ends_with(v.as_str())))
        .collect();
    if taken.is_empty() {
        return std::collections::HashSet::new();
    }
    item_ids(raw_dir)
        .into_iter()
        .filter(|id| {
            read_item(raw_dir, id).is_some_and(|v| {
                let p = v["path_lower"].as_str().unwrap_or("");
                taken.iter().any(|t| under(p, t))
            })
        })
        .map(|id| format!("dropbox/{id}"))
        .collect()
}

/// 저장본의 조각이에요(`src` = "dropbox"). 끈 폴더와 2년이 지난 파일, 받지 않은 파일은 빼고, 연결이 끊긴 지
/// `LOST_DAYS`가 지났으면 비어요.
pub fn chunks_of(raw_dir: &Path, now: DateTime<Utc>) -> Result<Vec<RawChunk>, SourceError> {
    let st = load_state(raw_dir);
    if let Some(lost) = st.lost_since.as_deref().and_then(parse)
        && now - lost >= Duration::days(LOST_DAYS)
    {
        return Ok(Vec::new());
    }
    let floor = now - Duration::days(WINDOW_DAYS);
    let mut out = Vec::new();
    for id in item_ids(raw_dir) {
        let Some(v) = read_item(raw_dir, &id) else {
            continue;
        };
        if v["server_modified"]
            .as_str()
            .and_then(parse)
            .is_none_or(|m| m <= floor)
        {
            continue;
        }
        let path = v["path_lower"].as_str().unwrap_or("");
        if st.muted_top(path) {
            continue;
        }
        if v["got"].is_null() && v["blocked"].is_null() {
            continue;
        }
        if let Some((d, starts)) = document(&v) {
            out.extend(windows_at(&d, "dropbox", &starts));
        }
    }
    Ok(out)
}

/// 폴더 고르기 화면의 목록이에요(이름 순, 받은 것이 없으면 비어요).
pub fn folders(raw_dir: &Path) -> Vec<FolderInfo> {
    let st = load_state(raw_dir);
    let paths: Vec<String> = item_ids(raw_dir)
        .iter()
        .filter_map(|id| read_item(raw_dir, id))
        .filter_map(|v| v["path_lower"].as_str().map(str::to_string))
        .collect();
    let mut out: Vec<FolderInfo> = st
        .tops
        .iter()
        .map(|(id, t)| FolderInfo {
            id: id.clone(),
            name: t.name.clone(),
            path: t.path_lower.clone(),
            files: paths.iter().filter(|p| under(p, &t.path_lower)).count(),
            team_folder: st.root_tag.as_deref() == Some("team")
                && st.home_path.as_deref() != Some(t.path_lower.as_str()),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dropbox::state::{State, Top, save_state, write_item};
    use crate::gdrive::fetch::{iso, parse};
    use chrono::Duration;
    use serde_json::{Value, json};
    use std::path::Path;

    fn now() -> DateTime<Utc> {
        parse("2026-11-01T12:00:00.000Z").unwrap()
    }

    fn item(id: &str, path: &str, ty: &str, text: Option<&str>) -> Value {
        json!({"id": id, "name": path.rsplit('/').next().unwrap(), "path_lower": path.to_lowercase(),
               "path_display": path, "server_modified": "2026-10-27T01:00:00Z", "rev": "r", "type": ty,
               "text": text, "blocked": null, "got": "h"})
    }

    fn put(dir: &Path, v: &Value) {
        write_item(dir, v["id"].as_str().unwrap(), v).unwrap();
    }

    #[test]
    fn the_link_opens_the_preview_without_a_shared_link() {
        assert_eq!(
            web_link("/영업/제안서 v2.docx"),
            "https://www.dropbox.com/home/%EC%98%81%EC%97%85?preview=%EC%A0%9C%EC%95%88%EC%84%9C%20v2.docx"
        );
        assert_eq!(
            web_link("/Sarah/기획 (최종)/a&b+c#.pdf"),
            "https://www.dropbox.com/home/Sarah/%EA%B8%B0%ED%9A%8D%20%28%EC%B5%9C%EC%A2%85%29?preview=a%26b%2Bc%23.pdf"
        );
    }

    #[test]
    fn a_root_file_links_to_home_and_has_no_folder_part() {
        assert_eq!(web_link("/a.md"), "https://www.dropbox.com/home?preview=a.md");
        let (d, _) = document(&item("x", "/a.md", "md", Some("# 제목\n본문"))).unwrap();
        assert_eq!(d.head, "dropbox: a.md\nmeta: Dropbox · 10월 27일 수정");
    }

    #[test]
    fn a_paper_doc_is_cut_at_headings_with_a_meta_line() {
        let (d, starts) = document(&item(
            "p1",
            "/영업/제안서/회의록.paper",
            "paper",
            Some("# 회의\n결정\n## 다음\n할 일"),
        ))
        .unwrap();
        assert_eq!(d.path, "dropbox/p1");
        assert_eq!(
            d.head,
            "dropbox: 회의록.paper\nmeta: Dropbox · 영업 / 제안서 · 10월 27일 수정"
        );
        assert_eq!(d.at, "2026-10-27");
        assert_eq!(d.url, web_link("/영업/제안서/회의록.paper"));
        assert_eq!(starts, [0, 2]);
    }

    #[test]
    fn paper_image_lines_are_dropped() {
        let raw = "# 회의\n![img](https://paper-attachments.dropboxusercontent.com/s/x.png)\n결정";
        let (d, _) = document(&item("p2", "/a/b.paper", "paper", Some(raw))).unwrap();
        assert!(d.body.iter().all(|(l, _)| !l.contains("paper-attachments")));
        assert!(d.body.iter().any(|(l, _)| l == "결정"));
    }

    #[test]
    fn a_sheet_table_becomes_rows_with_column_names_and_pdf_text_uses_windows() {
        let t = "| 영역 | 담당자 |\n| --- | --- |\n| 정산 | 박지호 |";
        let (d, _) = document(&item("s1", "/a/s.xlsx", "xlsx", Some(t))).unwrap();
        assert!(d.body.iter().any(|(l, _)| l.contains("영역 정산: 담당자는 박지호")));

        let plain: String = (0..100).map(|i| format!("줄 {i}\n")).collect();
        let (d, starts) = document(&item("t1", "/a/t.pdf", "pdf", Some(&plain))).unwrap();
        assert!(starts.is_empty());
        let chunks = windows_at(&d, "dropbox", &starts);
        assert!(chunks.len() >= 3, "{}", chunks.len());
    }

    #[test]
    fn a_blocked_file_is_searched_by_its_title_and_an_unfetched_one_is_not() {
        let d = tempfile::tempdir().unwrap();
        let mut blocked = item("b1", "/a/큰 파일.pdf", "pdf", None);
        blocked["blocked"] = json!("tooLarge");
        put(d.path(), &blocked);
        let mut unfetched = item("u1", "/a/아직.pdf", "pdf", None);
        unfetched["got"] = Value::Null;
        put(d.path(), &unfetched);
        let chunks = chunks_of(d.path(), now()).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["path"], "dropbox/b1");
        assert!(chunks[0]["text"].as_str().unwrap().contains("큰 파일.pdf"));
    }

    #[test]
    fn muted_tops_old_files_and_long_lost_connections_are_left_out() {
        let d = tempfile::tempdir().unwrap();
        let mut st = State::default();
        st.tops.insert("f1".into(), Top { path_lower: "/a".into(), name: "A".into() });
        st.muted = vec!["f1".into()];
        save_state(d.path(), &st).unwrap();
        put(d.path(), &item("m1", "/a/x.md", "md", Some("글")));
        put(d.path(), &item("k1", "/b/x.md", "md", Some("글")));
        let mut old = item("o1", "/b/old.md", "md", Some("글"));
        old["server_modified"] = json!(iso(now() - Duration::days(731)));
        put(d.path(), &old);
        let ids: Vec<_> = chunks_of(d.path(), now())
            .unwrap()
            .iter()
            .map(|c| c["path"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, ["dropbox/k1"]);

        st.lost_since = Some(iso(now() - Duration::days(8)));
        save_state(d.path(), &st).unwrap();
        assert!(chunks_of(d.path(), now()).unwrap().is_empty());
    }

    #[test]
    fn a_vault_the_notes_source_takes_is_left_out_of_dropbox() {
        let d = tempfile::tempdir().unwrap();
        let mut st = State::default();
        st.vaults.insert("/일/노트".into());
        save_state(d.path(), &st).unwrap();
        put(d.path(), &item("v1", "/일/노트/결정.md", "md", Some("글")));
        put(
            d.path(),
            &item("v2", "/일/노트들/다른.md", "md", Some("글")),
        );
        put(d.path(), &item("k1", "/일/x.md", "md", Some("글")));
        let local = ["/Users/a/Library/CloudStorage/Dropbox/일/노트".to_string()];
        let skip = doc_paths_in_vaults(d.path(), &local);
        assert_eq!(skip, ["dropbox/v1".to_string()].into_iter().collect());
        assert!(doc_paths_in_vaults(d.path(), &["/Users/a/다른".to_string()]).is_empty());
    }

    #[test]
    fn folders_lists_tops_with_counts_and_team_flags() {
        let d = tempfile::tempdir().unwrap();
        let mut st = State {
            root_tag: Some("team".into()),
            home_path: Some("/sarah".into()),
            ..State::default()
        };
        st.tops.insert("f1".into(), Top { path_lower: "/sarah".into(), name: "Sarah".into() });
        st.tops.insert("f2".into(), Top { path_lower: "/marketing".into(), name: "Marketing".into() });
        save_state(d.path(), &st).unwrap();
        put(d.path(), &item("1", "/Sarah/a.md", "md", Some("a")));
        put(d.path(), &item("2", "/Sarah/x/b.md", "md", Some("b")));
        put(d.path(), &item("3", "/Marketing/c.md", "md", Some("c")));
        put(d.path(), &item("4", "/root.md", "md", Some("d")));
        // 이름 순이라 Marketing이 먼저예요
        assert_eq!(
            folders(d.path()),
            [
                FolderInfo { id: "f2".into(), name: "Marketing".into(), path: "/marketing".into(), files: 1, team_folder: true },
                FolderInfo { id: "f1".into(), name: "Sarah".into(), path: "/sarah".into(), files: 2, team_folder: false },
            ]
        );
    }
}
