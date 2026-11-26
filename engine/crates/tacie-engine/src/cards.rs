//! Turns ranked chunks into answer cards: one per document (repo file, PR, Slack thread), using its best chunk.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};
use tacie_index::Chunk;

const SNIPPET_LINES: usize = 12;

static HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(file|symbols|doc|section|github|slack|notion|linear|gdrive|dropbox|jira|confluence|asana|meeting|notes|aliases|tags|meta|files): ?(.*)$",
    )
    .expect("header regex")
});

/// 조각 본문 = 머리 줄들(file:/symbols:/doc:/section:,
/// github:/slack:/notion:/gdrive:/dropbox:/meeting:/notes:/aliases:/tags:/meta:) 다음에 본문이에요. aliases:·tags:는 노트의
/// 머리말이에요(docs/34).
pub fn split_chunk(text: &str) -> (HashMap<String, String>, Vec<String>) {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut head = HashMap::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(m) = HEADER.captures(lines[i]) else {
            break;
        };
        head.insert(m[1].to_string(), m[2].to_string());
        i += 1;
    }
    (head, lines[i..].iter().map(|s| s.to_string()).collect())
}

pub fn title_of(chunk: &Chunk, head: &HashMap<String, String>) -> String {
    if let Some(t) = chunk.title.as_deref().filter(|t| !t.is_empty()) {
        return t.to_string();
    }
    let name = Path::new(&chunk.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let label = if chunk.src == "docs" {
        let section = head.get("section").map(String::as_str).unwrap_or("");
        section
            .rsplit(" > ")
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    } else {
        let symbols = head.get("symbols").map(String::as_str).unwrap_or("");
        symbols.split(',').next().unwrap_or("").trim().to_string()
    };
    if label.is_empty() {
        name
    } else {
        format!("{name} · {label}")
    }
}

pub fn card(
    chunk: &Chunk,
    score: Option<f64>,
    meta: &Map<String, Value>,
    repo_dir: Option<&Path>,
    chunk_id: usize,
) -> Value {
    let (head, body) = split_chunk(&chunk.text);
    let start = chunk.line;
    let end = start + body.len().max(1) as u64 - 1;
    let in_repo = chunk.src == "code" || chunk.src == "docs";

    let digest = Sha1::digest(format!("{}:{}", chunk.path, start).as_bytes());
    let id = format!("c_{}", &hex::encode(digest)[..8]);

    let byline = if in_repo {
        Value::Null
    } else {
        let parts: Vec<&str> = ["slack", "meta"]
            .iter()
            .filter_map(|k| head.get(*k).map(String::as_str))
            .filter(|v| !v.is_empty())
            .collect();
        Value::String(parts.join(" · "))
    };
    let snippet = body
        .iter()
        .take(SNIPPET_LINES)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let snippet = snippet.trim_matches('\n');

    let nonempty = |k: &str| {
        meta.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let (url, github_url) = match (in_repo, nonempty("repo"), nonempty("commit")) {
        (true, Some(repo), Some(commit)) => {
            let u = format!(
                "https://github.com/{repo}/blob/{commit}/{}#L{start}-L{end}",
                chunk.path
            );
            (json!(u), json!(u))
        }
        _ => {
            let url = json!(chunk.url);
            let gh = if chunk.src == "github" {
                url.clone()
            } else {
                Value::Null
            };
            (url, gh)
        }
    };
    let local_path = match repo_dir {
        Some(dir) if in_repo => json!(dir.join(&chunk.path).to_string_lossy()),
        _ => Value::Null,
    };

    json!({
        "id": id,
        "path": chunk.path,
        "kind": chunk.src,
        "source": if in_repo { "repo" } else { chunk.src.as_str() },
        "line_start": start,
        "line_end": end,
        "title": title_of(chunk, &head),
        "byline": byline,
        "snippet": snippet,
        "score": score,
        "url": url,
        "github_url": github_url,
        "local_path": local_path,
        "chunk": chunk_id,
    })
}

/// `order` = 좋은 순 조각 번호, `scores[i]`는 `order[i]`의 점수(리랭커 점수가 없으면 None)예요.
pub fn build_cards(
    chunks: &[Chunk],
    order: &[usize],
    scores: &[Option<f64>],
    meta: &Map<String, Value>,
    n: usize,
    repo_dir: Option<&Path>,
) -> Vec<Value> {
    let mut out = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for (&cid, &score) in order.iter().zip(scores) {
        if out.len() >= n {
            break;
        }
        let chunk = &chunks[cid];
        if !seen.insert(chunk.path.as_str()) {
            continue;
        }
        out.push(card(chunk, score, meta, repo_dir, cid));
    }
    out
}

/// 회의 목록(`meetings.json`)의 회의 하나를 카드로 만들어요. 색인에 회의 조각이 없는 iPhone 엔진이 쓰고, 키는
/// `src: "meeting"` 조각 카드(`card`)와 같아서 앱이 같은 길로 열어요. 조각이 아니라서 `chunk`는 null이에요.
pub fn meeting_card(m: &tacie_sources::meeting::Meeting) -> Value {
    // 회의 조각의 acl `["*"]`를 그대로 믿어요: 회의는 이 기기 사용자의 파일이라 거를 사람이 없어요.
    let doc = tacie_sources::meeting::doc_of(m);
    let (head, _) = split_chunk(&doc.head);
    let start = 1u64;
    let end = doc.body.len().max(1) as u64;
    let snippet = doc.body.iter().take(SNIPPET_LINES).map(|(l, _)| l.as_str()).collect::<Vec<_>>().join("\n");
    let digest = Sha1::digest(format!("{}:{}", doc.path, start).as_bytes());
    json!({
        "id": format!("c_{}", &hex::encode(digest)[..8]),
        "path": doc.path,
        "kind": "meeting",
        "source": "meeting",
        "line_start": start,
        "line_end": end,
        "title": doc.title,
        "byline": head.get("meta").cloned().unwrap_or_default(),
        "snippet": snippet.trim_matches('\n'),
        "score": Value::Null,
        "url": doc.url,
        "github_url": Value::Null,
        "local_path": Value::Null,
        "chunk": Value::Null,
    })
}
