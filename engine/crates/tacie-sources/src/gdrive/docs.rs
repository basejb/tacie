//! Turns raw Drive data into documents and chunks (docs/19 10절).
//!
//! - One file is one document: `gdrive/<fileId>`, header `gdrive: 제목` + `meta: Google Docs · 내 드라이브 · 수정한 사람 ·
//!   10월 27일 수정` (the card's byline). The link is `webViewLink`.
//! - Docs start a new window at every Markdown heading (`#`). Slides use 40-line windows because slide boundaries
//!   cannot be trusted (7-1절). Sheets attach the column-name row (the first of the first 10 rows with two or more
//!   filled cells) to every chunk as a `section:` header.
//! - Tables (Markdown tables in Docs, CSV rows in Sheets) become one sentence per row with column names attached
//!   (`영역 정산: 담당자는 박지호`). `| 정산 | 박지호 |` lost what the line was about in the embedding and did not match
//!   "정산 누가 봐요?" (2026-11-01).
//! - Files that could not be fetched (`blocked`) have a single title line as their body (it still answers "where was
//!   that document").
//! - Files in muted shared drives and files older than 2 years are left out. The raw data stays as is.
//! - Once `LOST_DAYS` have passed since the connection was lost (`lost_since`), no chunks are produced (the raw data
//!   is kept).

use std::path::Path;

use chrono::{DateTime, Datelike, Duration, FixedOffset, Utc};
use serde::Serialize;
use serde_json::Value;
use tacie_index::RawChunk;

use crate::doc::{Doc, windows_at};
use crate::error::SourceError;
use crate::gdrive::fetch::{
    DOC, DOCX, PDF, PPTX, SHEET, SLIDES, WINDOW_DAYS, XLSX, item_ids, load_state, parse, read_item, unescape_md,
};

/// 연결이 끊긴 뒤 이만큼 지나면 Drive 자료를 찾지 않아요(Linear와 같아요).
pub const LOST_DAYS: i64 = 7;
const KST_SECS: i32 = 9 * 3600;
/// `section:` 머리에 넣는 열 이름의 최대 글자 수예요.
const HEADER_CHARS: usize = 300;
/// 시트의 열 이름 줄을 이만큼의 앞 줄에서 찾아요.
const HEADER_SCAN: usize = 10;

/// 공유 드라이브 고르기 화면의 한 줄이에요.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DriveInfo {
    pub id: String,
    pub name: String,
    pub files: usize,
}

fn kst(iso: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|t| t.with_timezone(&FixedOffset::east_opt(KST_SECS).expect("KST")))
}

/// `10월 27일`이에요.
pub(crate) fn md(iso: &str) -> String {
    kst(iso).map_or_else(String::new, |t| format!("{}월 {}일", t.month(), t.day()))
}

/// `2026-10-27`이에요.
pub(crate) fn ymd(iso: &str) -> String {
    kst(iso).map_or_else(
        || iso.chars().take(10).collect(),
        |t| t.format("%Y-%m-%d").to_string(),
    )
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

pub(crate) fn join(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn kind_word(mime: &str) -> &'static str {
    match mime {
        DOC => "Google Docs",
        SHEET => "Google Sheets",
        SLIDES => "Google Slides",
        PDF => "PDF",
        DOCX => "Word",
        PPTX => "PowerPoint",
        XLSX => "Excel",
        _ => "Google Drive",
    }
}

/// 비지 않은 칸을 쉼표로 이어요(열 이름 줄, 열 이름이 없는 줄이에요).
fn plain(cells: &[String]) -> String {
    cells
        .iter()
        .filter(|c| !c.is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// 받침이 있는 한글로 끝나는지예요(은/는을 고를 때요).
fn ends_with_batchim(w: &str) -> bool {
    w.chars()
        .last()
        .is_some_and(|c| ('가'..='힣').contains(&c) && !(c as u32 - '가' as u32).is_multiple_of(28))
}

/// 표의 한 행을 열 이름이 붙은 문장으로 바꿔요: `영역 정산: 담당자는 박지호, 백업은 김민준`. 첫 칸은 그 행의 이름으로
/// 앞에 두고, 나머지 칸은 `열 이름은 값`이에요(한글로 끝나지 않는 열 이름은 `열 이름: 값`). 빈 칸은 빼고, 열 이름이
/// 없는 칸(이름 칸이 비었거나 머리보다 칸이 많을 때)은 값만 써요. 칸이 모두 비면 빈 줄이에요.
///
/// bge-m3로 재 보니(docs/19 10-1절) `| 정산 | 박지호 |`나 `영역 정산 · 담당자 박지호`보다 이 모양이 "정산 누가 봐요?"에
/// 가까웠어요. 원래 행은 남기지 않아요: 같은 값이 한 번 더 들어가 조각만 길어지고 순위는 나아지지 않았어요.
fn row_sentence(header: &[String], cells: &[String]) -> String {
    let name = |i: usize| header.get(i).map(String::as_str).filter(|h| !h.is_empty());
    let first = cells
        .first()
        .filter(|c| !c.is_empty())
        .map(|c| match name(0) {
            Some(h) => format!("{h} {c}"),
            None => c.clone(),
        });
    let rest = cells
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, c)| !c.is_empty())
        .map(|(i, c)| match name(i) {
            Some(h) if ends_with_batchim(h) => format!("{h}은 {c}"),
            Some(h) if h.chars().last().is_some_and(|x| ('가'..='힣').contains(&x)) => {
                format!("{h}는 {c}")
            }
            Some(h) => format!("{h}: {c}"),
            None => c.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    match (first, rest.is_empty()) {
        (Some(f), true) => f,
        (Some(f), false) => format!("{f}: {rest}"),
        (None, _) => rest,
    }
}

/// Markdown 표의 한 행이면 칸들이에요(`|`로 시작하는 줄). 앞에 `\`가 붙은 `\|`는 칸 경계가 아니에요. 칸 안의 줄바꿈
/// (`<br>`)은 띄어쓰기로 바꿔요.
fn md_cells(line: &str) -> Option<Vec<String>> {
    let t = line.trim();
    let inner = t.strip_prefix('|')?;
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for c in inner.chars() {
        if esc {
            cur.push(c);
            esc = false;
        } else if c == '\\' {
            cur.push(c);
            esc = true;
        } else if c == '|' {
            cells.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    // 끝의 `|` 뒤에 남은 것은 칸이 아니에요(닫는 `|`가 없는 줄은 마지막 칸이에요)
    if !cur.trim().is_empty() {
        cells.push(cur);
    }
    Some(
        cells
            .into_iter()
            .map(|c| {
                c.replace("<br>", " ")
                    .replace("<br/>", " ")
                    .trim()
                    .to_string()
            })
            .collect(),
    )
}

/// 표 머리 밑의 구분 줄(`| --- | :---: |`)이에요.
fn is_separator(line: &str) -> bool {
    let Some(cells) = md_cells(&line.replace('\\', "")) else {
        return false;
    };
    !cells.is_empty()
        && cells.iter().all(|c| {
            let d = c.trim_matches(':');
            d.len() >= 3 && d.chars().all(|x| x == '-')
        })
}

/// Docs Markdown의 표(머리 행 + 구분 줄 + 행들)를 행마다 열 이름이 붙은 문장 한 줄로 바꿔요. 머리 행과 구분 줄은
/// 빠져요(열 이름은 모든 행에 들어가요). 행만 있고 구분 줄이 없는 줄은 표가 아니라 그대로 둬요. (줄, 표의 행인가)예요.
fn md_tables(md: &str) -> Vec<(String, bool)> {
    // 표가 아니라 글로 쓴 표(`\| 영역 \| 담당자 \|`)는 모든 `|`가 이스케이프돼 있어서, 그런 줄은 먼저 풀어요
    let orig: Vec<&str> = md.split('\n').collect();
    let raw: Vec<std::borrow::Cow<str>> = orig
        .iter()
        .map(|&l| {
            if l.trim_start().starts_with("\\|") {
                unescape_md(l).into()
            } else {
                l.into()
            }
        })
        .collect();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let header = md_cells(&raw[i]).filter(|h| !h.is_empty());
        match header {
            Some(header) if raw.get(i + 1).is_some_and(|l| is_separator(l)) => {
                let mut j = i + 2;
                while let Some(cells) = raw.get(j).and_then(|l| md_cells(l)) {
                    out.push((row_sentence(&header, &cells), true));
                    j += 1;
                }
                if j == i + 2 {
                    // 행이 없는 표는 열 이름만 남겨요
                    out.push((plain(&header), true));
                }
                i = j;
            }
            _ => {
                out.push((orig[i].to_string(), false));
                i += 1;
            }
        }
    }
    out
}

/// CSV를 레코드(칸들)로 나눠요(RFC 4180: 따옴표 안의 쉼표·줄바꿈, `""`는 따옴표 하나). 칸 안의 줄바꿈은 띄어쓰기로
/// 바꾸고 칸의 앞뒤 공백은 지워요. 빈 줄도 (빈) 레코드예요.
fn csv_records(text: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let push_cell = |row: &mut Vec<String>, cell: &mut String| {
        let c = std::mem::take(cell);
        row.push(c.split_whitespace().collect::<Vec<_>>().join(" "));
    };
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    cell.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                _ => cell.push(c),
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => push_cell(&mut row, &mut cell),
            '\r' => {}
            '\n' => {
                push_cell(&mut row, &mut cell);
                out.push(std::mem::take(&mut row));
            }
            _ => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        push_cell(&mut row, &mut cell);
        out.push(row);
    }
    out
}

/// 표의 행이 이어지면 이만큼(글자)마다 새 창을 시작해요. 행마다 열 이름이 붙어 길어져서, 40줄 창이면 뒤쪽 행이 임베딩
/// 512토큰과 조각 2,000자 밖으로 밀려나요. 행은 혼자 읽혀서 창끼리 겹치지 않아도 돼요.
const TABLE_CHARS: usize = 900;

/// 표의 행에서 새 창을 시작할 줄들이에요. 이미 시작하는 줄(`starts`, Docs의 제목)에서 글자 수를 다시 세요.
pub(crate) fn table_cuts(lines: &[(String, bool)], starts: &[usize]) -> Vec<usize> {
    let mut cuts = Vec::new();
    let mut size = 0;
    for (i, (l, row)) in lines.iter().enumerate() {
        if starts.contains(&i) {
            size = 0;
        }
        let n = l.chars().count() + 1;
        if *row && size > 0 && size + n > TABLE_CHARS {
            cuts.push(i);
            size = 0;
        }
        size += n;
    }
    cuts
}

/// Markdown 글을 (줄, 표의 행인가)와 제목(`#`) 줄 번호로 나눠요. Drive와 Dropbox가 같이 써요.
pub(crate) fn markdown_lines(raw: &str) -> (Vec<(String, bool)>, Vec<usize>) {
    let mut lines: Vec<(String, bool)> = Vec::new();
    // 표는 이스케이프를 풀기 전에 칸을 나눠요(칸 안의 `\|`는 칸 경계가 아니에요)
    for (l, row) in md_tables(raw) {
        let l = unescape_md(&l);
        lines.push((l, row));
    }
    let starts = lines
        .iter()
        .enumerate()
        .filter(|(_, (l, _))| {
            let hashes = l.chars().take_while(|c| *c == '#').count();
            (1..=6).contains(&hashes) && l[hashes..].starts_with(' ')
        })
        .map(|(i, _)| i)
        .collect();
    (lines, starts)
}

/// 파일 하나의 문서와 창 시작 줄이에요. `place`는 "내 드라이브"나 공유 드라이브 이름이에요.
pub(crate) fn document(v: &Value, place: &str) -> Option<(Doc, Vec<usize>)> {
    let id = s(v, "id");
    if id.is_empty() {
        return None;
    }
    let title = s(v, "name").to_string();
    let url = s(v, "webViewLink").to_string();
    let at = s(v, "modifiedTime");
    let mime = s(v, "mimeType");
    let edited = if at.is_empty() {
        String::new()
    } else {
        format!("{} 수정", md(at))
    };
    let mut head = format!(
        "gdrive: {title}\nmeta: {}",
        join(&[kind_word(mime), place, s(v, "who"), &edited])
    );
    // 저장본은 받은 그대로라, Docs의 이스케이프와 표 풀기는 여기서 해요(다시 받지 않아도 고쳐져요)
    let raw = s(v, "text");
    // (줄, 표의 행인가)예요
    let lines: Vec<(String, bool)>;
    let mut starts = Vec::new();
    match mime {
        // Word·PowerPoint·Excel도 Markdown(제목 `#`, 표 `|`)으로 받아 와서 Docs처럼 나눠요
        DOC | DOCX | PPTX | XLSX => {
            (lines, starts) = markdown_lines(raw);
        }
        SHEET => {
            // 첫 줄이 비었거나 시트 제목인 시트가 많아서, 앞 `HEADER_SCAN`줄에서 칸이 둘 이상 찬 첫 줄을 열 이름으로 써요.
            // 그 앞의 줄(제목 같은 것)은 값만 남겨요
            let records = csv_records(raw);
            let at = records
                .iter()
                .take(HEADER_SCAN)
                .position(|r| r.iter().filter(|c| !c.is_empty()).count() >= 2);
            if let Some(k) = at {
                let cols: String = plain(&records[k]).chars().take(HEADER_CHARS).collect();
                head.push_str(&format!("\nsection: {cols}"));
            }
            let header = at.map(|k| records[k].clone()).unwrap_or_default();
            lines = records
                .iter()
                .enumerate()
                .filter(|(k, _)| Some(*k) != at)
                .map(|(k, r)| match at {
                    Some(a) if k > a => (row_sentence(&header, r), true),
                    _ => (plain(r), false),
                })
                .collect();
        }
        _ => lines = raw.split('\n').map(|l| (l.to_string(), false)).collect(),
    }
    starts.extend(table_cuts(&lines, &starts));
    let mut body: Vec<(String, String)> =
        lines.into_iter().map(|(l, _)| (l, url.clone())).collect();
    if body.iter().all(|(l, _)| l.trim().is_empty()) {
        body = vec![(title.clone(), url.clone())];
        starts.clear();
    }
    Some((
        Doc {
            path: format!("gdrive/{id}"),
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

/// 저장본의 조각이에요(`src` = "gdrive"). 끈 공유 드라이브와 2년이 지난 파일은 빼고, 연결이 끊긴 지 `LOST_DAYS`가
/// 지났으면 비어요.
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
        if v["modifiedTime"]
            .as_str()
            .and_then(parse)
            .is_none_or(|m| m <= floor)
        {
            continue;
        }
        let drive = v["driveId"].as_str();
        if drive.is_some_and(|d| st.muted.iter().any(|m| m == d)) {
            continue;
        }
        let place = match drive {
            None => "내 드라이브".to_string(),
            Some(d) => st
                .drives
                .iter()
                .find(|x| x["id"].as_str() == Some(d))
                .and_then(|x| x["name"].as_str())
                .unwrap_or("공유 드라이브")
                .to_string(),
        };
        if let Some((d, starts)) = document(&v, &place) {
            out.extend(windows_at(&d, "gdrive", &starts));
        }
    }
    Ok(out)
}

/// 공유 드라이브 고르기 화면의 목록이에요(받은 것이 없으면 비어요).
pub fn drives(raw_dir: &Path) -> Vec<DriveInfo> {
    let st = load_state(raw_dir);
    let mut counts: std::collections::HashMap<String, usize> = Default::default();
    for id in item_ids(raw_dir) {
        if let Some(d) =
            read_item(raw_dir, &id).and_then(|v| v["driveId"].as_str().map(str::to_string))
        {
            *counts.entry(d).or_default() += 1;
        }
    }
    st.drives
        .iter()
        .filter_map(|d| {
            let id = d["id"].as_str()?.to_string();
            Some(DriveInfo {
                files: counts.get(&id).copied().unwrap_or(0),
                name: s(d, "name").to_string(),
                id,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdrive::fetch::{ITEMS, State, save_state};
    use serde_json::json;

    fn put(dir: &Path, v: Value) {
        let id = v["id"].as_str().unwrap().to_string();
        std::fs::create_dir_all(dir.join(ITEMS)).unwrap();
        crate::store::write_json(&dir.join(ITEMS).join(format!("{id}.json")), &v, None).unwrap();
    }
    fn now() -> DateTime<Utc> {
        parse("2026-11-01T12:00:00.000Z").unwrap()
    }
    fn file(id: &str, mime: &str, text: Option<&str>, drive: Option<&str>) -> Value {
        json!({"id": id, "name": format!("Plan {id}"), "mimeType": mime, "modifiedTime": "2026-10-27T01:00:00.000Z",
               "webViewLink": format!("https://docs.google.com/d/{id}"), "who": "Kim", "driveId": drive,
               "scope": drive.map_or("my".to_string(), |d| format!("drive:{d}")),
               "text": text, "blocked": if text.is_none() { json!("canDownload") } else { Value::Null }})
    }

    #[test]
    fn a_doc_is_cut_at_headings_with_a_meta_line() {
        let d = tempfile::tempdir().unwrap();
        put(
            d.path(),
            file("a", DOC, Some("# One\ntext\n## Two\nmore"), None),
        );
        let c = chunks_of(d.path(), now()).unwrap();
        assert_eq!(c.len(), 2);
        let text = c[0]["text"].as_str().unwrap();
        assert!(
            text.starts_with(
                "gdrive: Plan a\nmeta: Google Docs · 내 드라이브 · Kim · 10월 27일 수정\n# One"
            ),
            "{text}"
        );
        assert_eq!(c[1]["line"], 3);
        assert_eq!(c[0]["src"], "gdrive");
        assert_eq!(c[0]["path"], "gdrive/a");
        assert_eq!(c[0]["acl"], json!(["*"]));
        assert_eq!(c[0]["url"], "https://docs.google.com/d/a");
    }

    #[test]
    fn an_escaped_doc_is_unescaped_and_cut_at_its_headings() {
        let d = tempfile::tempdir().unwrap();
        put(
            d.path(),
            file(
                "e",
                DOC,
                Some(
                    "\\# 운영 메모\n\n\\#\\# 배포  \n\\- 화요일에 해요.  \n\n\\#\\# 환불\n\\- 7일 안에는 전액",
                ),
                None,
            ),
        );
        let c = chunks_of(d.path(), now()).unwrap();
        let texts: Vec<&str> = c.iter().map(|x| x["text"].as_str().unwrap()).collect();
        assert_eq!(texts.len(), 3, "{texts:?}");
        assert!(
            texts[1].ends_with("## 배포\n- 화요일에 해요."),
            "{}",
            texts[1]
        );
        assert!(!texts.iter().any(|t| t.contains('\\')));
    }

    #[test]
    fn a_sheet_carries_its_header_in_every_chunk() {
        let d = tempfile::tempdir().unwrap();
        let rows: Vec<String> = (0..80).map(|i| format!("{i},x")).collect();
        put(
            d.path(),
            file(
                "s",
                SHEET,
                Some(&format!("name,value\n{}", rows.join("\n"))),
                None,
            ),
        );
        let c = chunks_of(d.path(), now()).unwrap();
        assert!(c.len() > 1);
        for ch in &c {
            assert!(
                ch["text"]
                    .as_str()
                    .unwrap()
                    .contains("\nsection: name, value\n")
            );
        }
    }

    fn body_of(mime: &str, text: &str) -> Vec<String> {
        let (d, _) = document(&file("t", mime, Some(text), None), "내 드라이브").unwrap();
        d.body.into_iter().map(|(l, _)| l).collect()
    }

    #[test]
    fn a_doc_table_becomes_rows_with_column_names() {
        let md = "## 담당\n\n| 영역 | 담당자 | 백업 |\n| :---- | :---- | ----: |\n| 정산 | 박지호 | 김민준 |\n| 배포 \\| 운영 | 이서연 | 최유나 |\n\n끝";
        assert_eq!(
            body_of(DOC, md),
            [
                "## 담당",
                "",
                "영역 정산: 담당자는 박지호, 백업은 김민준",
                "영역 배포 | 운영: 담당자는 이서연, 백업은 최유나",
                "",
                "끝"
            ]
        );
    }

    #[test]
    fn empty_cells_are_skipped_and_ragged_rows_keep_their_values() {
        let md = "| 영역 |  | 백업 |\n|---|---|---|\n| 정산 |  |  |\n| 환불 | 메모<br>둘 |\n| 배포 | 이서연 | 최유나 | 덤 |\n|  |  |  |";
        assert_eq!(
            body_of(DOC, md),
            [
                "영역 정산",
                "영역 환불: 메모 둘",
                "영역 배포: 이서연, 백업은 최유나, 덤",
                ""
            ]
        );
    }

    #[test]
    fn lines_with_pipes_that_are_not_tables_are_left_alone() {
        let md = "a | b | c\n| 머리만 | 있고 |\n구분 줄이 없어요\n| --- |";
        assert_eq!(
            body_of(DOC, md),
            [
                "a | b | c",
                "| 머리만 | 있고 |",
                "구분 줄이 없어요",
                "| --- |"
            ]
        );
    }

    #[test]
    fn a_table_typed_as_text_is_read_like_a_table() {
        // 글로 친 표는 Docs가 `|`까지 모두 이스케이프해서 보내요
        let md = "\\| 영역 \\| 담당자 \\| 백업 \\|  \n\\| \\--- \\| \\--- \\| \\--- \\|  \n\\| 정산 \\| 박지호 \\| 김민준 \\|  ";
        assert_eq!(
            body_of(DOC, md),
            ["영역 정산: 담당자는 박지호, 백업은 김민준"]
        );
    }

    #[test]
    fn sheet_rows_carry_column_names_and_csv_quotes_are_read() {
        let csv = "영역,담당자,메모\r\n정산,박지호,\"월말, 분기말\"\r\n\"환불 \"\"긴급\"\"\",,\"두\n줄\"\r\n,,\r\n배포,이서연";
        assert_eq!(
            body_of(SHEET, csv),
            [
                "영역 정산: 담당자는 박지호, 메모는 월말, 분기말",
                "영역 환불 \"긴급\": 메모는 두 줄",
                "",
                "영역 배포: 담당자는 이서연"
            ]
        );
    }

    #[test]
    fn a_sheet_finds_its_header_below_a_blank_line_and_a_title() {
        let csv = ",,\n2026 담당표,,\n영역,담당자,ID\n정산,박지호,7";
        let (d, _) = document(&file("t", SHEET, Some(csv), None), "내 드라이브").unwrap();
        assert!(
            d.head.ends_with("\nsection: 영역, 담당자, ID"),
            "{}",
            d.head
        );
        let body: Vec<&str> = d.body.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(
            body,
            ["", "2026 담당표", "영역 정산: 담당자는 박지호, ID: 7"]
        );
    }

    #[test]
    fn a_long_table_starts_new_windows_before_the_chunk_overflows() {
        let rows: Vec<String> = (0..60)
            .map(|i| format!("| 항목 {i} | 아주 긴 설명이 들어가는 칸이에요 {i} | 담당자 {i} |"))
            .collect();
        let md = format!(
            "# 표\n| 이름 | 설명 | 담당 |\n| --- | --- | --- |\n{}",
            rows.join("\n")
        );
        let d = tempfile::tempdir().unwrap();
        put(d.path(), file("l", DOC, Some(&md), None));
        let c = chunks_of(d.path(), now()).unwrap();
        assert!(c.len() > 2, "{}", c.len());
        let mut seen = 0;
        for ch in &c {
            let t = ch["text"].as_str().unwrap();
            assert!(t.chars().count() < 1100, "{}", t.chars().count());
            seen += t.matches("이름 항목").count();
        }
        assert_eq!(seen, 60, "every row is in exactly one chunk");
    }

    #[test]
    fn a_blocked_file_is_searched_by_its_title() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), file("b", SLIDES, None, None));
        let c = chunks_of(d.path(), now()).unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0]["text"].as_str().unwrap().ends_with("\nPlan b"));
    }

    #[test]
    fn muted_drives_old_files_and_long_lost_connections_are_left_out() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), file("x", DOC, Some("x"), Some("D1")));
        put(d.path(), file("y", DOC, Some("y"), Some("D2")));
        let mut old = file("o", DOC, Some("o"), None);
        old["modifiedTime"] = json!("2024-01-01T00:00:00.000Z");
        put(d.path(), old);
        let mut st = State {
            drives: vec![
                json!({"id": "D1", "name": "Team"}),
                json!({"id": "D2", "name": "Ops"}),
            ],
            muted: vec!["D2".into()],
            ..Default::default()
        };
        save_state(d.path(), &st).unwrap();
        let c = chunks_of(d.path(), now()).unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0]["text"].as_str().unwrap().contains("· Team ·"));
        let ds = drives(d.path());
        assert_eq!(ds.len(), 2);
        assert_eq!(ds[0].files, 1);

        st.lost_since = Some("2026-10-20T00:00:00.000Z".into());
        save_state(d.path(), &st).unwrap();
        assert!(chunks_of(d.path(), now()).unwrap().is_empty());
        st.lost_since = Some("2026-10-30T00:00:00.000Z".into());
        save_state(d.path(), &st).unwrap();
        assert_eq!(chunks_of(d.path(), now()).unwrap().len(), 1);
    }
}
