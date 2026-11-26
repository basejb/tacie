//! Documents outside the repo (GitHub issues and PRs, Slack conversations) and their chunks.
//!
//! A document is a header and body lines, and each body line carries the URL it came from. Chunks are cut as 40-line
//! windows moved 30 lines at a time, and link to the URL of the window's first non-empty line (the card opens at that
//! comment or reply). All chunks of a document share the same `path`, so there is one card per document.

use serde_json::{Map, Value};
use tacie_index::RawChunk;

use crate::py;
use crate::repo::MAX_CHARS;

/// 40줄 창, 10줄 겹침이에요. 레포 코드·문서는 내용 기준 경계로 바꿨고(`repo::pieces`, docs/17 1절), 레포 밖
/// 문서는 아직 이 창이에요.
pub const WIN: usize = 40;
pub const STEP: usize = 30;

/// 문서 하나예요.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    /// 문서 id예요(`github/pull/655`, `slack/C0123/1729571234.000100`).
    pub path: String,
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    /// `YYYY-MM-DD`예요.
    pub at: String,
    /// 볼 수 있는 사람이에요. 없으면 모두(`["*"]`)예요.
    pub acl: Option<Vec<String>>,
    /// 머리 줄(`github:`, `meta:` …)이에요. 조각마다 앞에 붙어요.
    pub head: String,
    /// (줄, 그 줄이 온 URL)이에요.
    pub body: Vec<(String, String)>,
}

impl Doc {
    /// 문서를 JSON으로 쓴 모양이에요: path, title, url, author, at, [acl], head, body([[줄, url]…]).
    /// acl은 있을 때만 들어가요(GitHub 문서에는 없어요).
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("path".into(), Value::from(self.path.as_str()));
        m.insert("title".into(), Value::from(self.title.as_str()));
        m.insert("url".into(), Value::from(self.url.as_str()));
        m.insert(
            "author".into(),
            self.author.as_deref().map_or(Value::Null, Value::from),
        );
        m.insert("at".into(), Value::from(self.at.as_str()));
        if let Some(acl) = &self.acl {
            m.insert("acl".into(), strings(acl));
        }
        m.insert("head".into(), Value::from(self.head.as_str()));
        let body = self
            .body
            .iter()
            .map(|(line, url)| {
                Value::Array(vec![Value::from(line.as_str()), Value::from(url.as_str())])
            })
            .collect();
        m.insert("body".into(), Value::Array(body));
        Value::Object(m)
    }
}

fn strings(xs: &[String]) -> Value {
    Value::Array(xs.iter().map(|x| Value::from(x.as_str())).collect())
}

/// 문서의 조각이에요. 키 순서는 바꾸지 않아요: src, path, title, author, at, acl, line, url, text.
/// 글은 창의 줄을 `\n`으로 이어 앞뒤 공백을 뺀 것이고(비면 조각이 없어요), 머리와 합쳐 2,000자(코드 포인트)로 잘라요.
pub fn windows(doc: &Doc, src: &str) -> Vec<RawChunk> {
    windows_at(doc, src, &[])
}

/// `windows`와 같지만 `starts`(본문 줄 번호)마다 새 창을 시작해요. 창은 다음 시작을 넘지 않아요. Linear는 댓글마다
/// 창을 시작해서, 댓글에 붙은 카드가 그 댓글로 열려요(조각의 링크는 창의 첫 줄 주소예요).
pub fn windows_at(doc: &Doc, src: &str, starts: &[usize]) -> Vec<RawChunk> {
    let mut base = Map::new();
    base.insert("src".into(), Value::from(src));
    base.insert("path".into(), Value::from(doc.path.as_str()));
    base.insert("title".into(), Value::from(doc.title.as_str()));
    base.insert(
        "author".into(),
        doc.author.as_deref().map_or(Value::Null, Value::from),
    );
    base.insert("at".into(), Value::from(doc.at.as_str()));
    let acl = match &doc.acl {
        Some(acl) => strings(acl),
        None => strings(&["*".to_string()]),
    };
    base.insert("acl".into(), acl);

    let all = &doc.body;
    let mut cuts: Vec<usize> = starts
        .iter()
        .copied()
        .filter(|&s| s > 0 && s < all.len())
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    cuts.insert(0, 0);
    let mut out = Vec::new();
    for (k, &seg_lo) in cuts.iter().enumerate() {
        let seg_hi = cuts.get(k + 1).copied().unwrap_or(all.len());
        let lines = &all[seg_lo..seg_hi.max(seg_lo)];
        window_into(doc, &base, lines, seg_lo, &mut out);
    }
    out
}

fn window_into(
    doc: &Doc,
    base: &Map<String, Value>,
    lines: &[(String, String)],
    offset: usize,
    out: &mut Vec<RawChunk>,
) {
    // STEP줄씩 옮기고(줄이 없어도 한 번은 돌아요), 창 끝이 마지막 줄에 닿으면 멈춰요
    let mut lo = 0;
    while lo < lines.len().max(1) {
        let part = &lines[lo.min(lines.len())..(lo + WIN).min(lines.len())];
        let joined = part
            .iter()
            .map(|(line, _)| line.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let text = py::strip(&joined);
        if !text.is_empty() {
            let url = part
                .iter()
                .find(|(line, _)| !py::strip(line).is_empty())
                .map_or(doc.url.as_str(), |(_, url)| url.as_str());
            let full = format!("{}\n{text}", doc.head);
            let mut chunk = base.clone();
            chunk.insert("line".into(), Value::from(offset + lo + 1));
            chunk.insert("url".into(), Value::from(url));
            chunk.insert("text".into(), Value::from(py::take_chars(&full, MAX_CHARS)));
            out.push(chunk);
        }
        if lo + WIN >= lines.len() {
            break;
        }
        lo += STEP;
    }
}
