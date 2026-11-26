//! The past meetings source (docs/30). When the app sends all finished meetings (summary, topics, meeting ledger)
//! with `POST /meetings`, they are kept in `raw/meeting/meetings.json`, and each meeting becomes one document
//! (`meeting/<id>`). Transcribed conversation and found answers are not accepted. The app's meeting files are not
//! read.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tacie_index::RawChunk;

use crate::doc::{Doc, windows};
use crate::store::{Stored, read_json, write_json};

/// 받은 회의를 두는 파일이에요(`raw/meeting` 아래).
pub const FILE: &str = "meetings.json";

/// 앱이 보낸 회의 하나예요. 글 안의 화자 표시는 앱이 이름이나 "화자 N"으로 바꿔서 보내요.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    /// ISO 8601(시간대 포함)이에요.
    pub start: String,
    pub end: String,
    #[serde(default)]
    pub summary: Vec<String>,
    #[serde(default)]
    pub topics: Vec<Topic>,
    #[serde(default)]
    pub ledger: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Topic {
    pub title: String,
    #[serde(default)]
    pub points: Vec<String>,
}

/// 회의 장부 항목이에요. `kind`는 `decision`, `commitment`, `open_question`이에요.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub done: bool,
    /// 이 결정이 바꾼 지난 결정이에요(docs/30 8절). 없으면 키를 쓰지 않아서 예전 파일과 같은 글이에요.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<Replaced>,
}

/// 바뀐 지난 결정: 그 글, 그 회의의 날짜(`YYYY-MM-DD`)와 제목이에요.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Replaced {
    pub text: String,
    pub at: String,
    #[serde(default)]
    pub title: String,
}

fn is_hangul(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c) || ('\u{3131}'..='\u{318E}').contains(&c)
}

/// 영어 회의인지예요. `tacie-engine/src/lang.rs`의 `of`와 같은 규칙이에요(크레이트가 달라서 옮겨 적었어요):
/// 한글이 글자의 10%도 안 되고 로마자가 20자 이상이면 영어예요.
fn english(text: &str) -> bool {
    let (mut ko, mut latin) = (0usize, 0usize);
    for c in text.chars() {
        if is_hangul(c) {
            ko += 1;
        } else if c.is_ascii_alphabetic() {
            latin += 1;
        }
    }
    latin >= 20 && ko * 10 < ko + latin
}

struct Labels {
    meta: &'static str,
    decision: &'static str,
    todo: &'static str,
    done: &'static str,
    question: &'static str,
    topic: &'static str,
    before: &'static str,
}

const KO: Labels = Labels {
    meta: "회의",
    decision: "결정",
    todo: "할 일",
    done: "끝냄",
    question: "열린 질문",
    topic: "주제",
    before: "이전 결정",
};

const EN: Labels = Labels {
    meta: "Meeting",
    decision: "Decision",
    todo: "To-do",
    done: "done",
    question: "Open question",
    topic: "Topic",
    before: "earlier decision",
};

/// 회의가 영어로 쓰였는지예요(요약과 장부 글로 봐요). 줄 앞 꼬리표를 고를 때 써요.
pub fn is_english(m: &Meeting) -> bool {
    let mut all = m.summary.join("\n");
    for e in &m.ledger {
        all.push('\n');
        all.push_str(&e.text);
    }
    english(&all)
}

/// 장부 항목 한 줄이에요: 회의 언어의 꼬리표, 글, 담당·기한·끝냄, 바꾼 이전 결정(날짜와 회의 제목까지)이에요.
/// 회의 문서(`doc_of`)와 iPhone 하네스의 검색 결과가 같이 써요.
pub fn ledger_line(e: &Entry, english: bool) -> String {
    let l = if english { &EN } else { &KO };
    match e.kind.as_str() {
        "decision" => match &e.replaces {
            Some(r) => format!("{}: {} ({} {} {}: {})", l.decision, e.text, l.before, r.at, r.title, r.text),
            None => format!("{}: {}", l.decision, e.text),
        },
        "open_question" => format!("{}: {}", l.question, e.text),
        _ => {
            let who: Vec<&str> = [e.owner.as_deref(), e.due.as_deref()]
                .into_iter()
                .flatten()
                .filter(|s| !s.trim().is_empty())
                .collect();
            let mut line = format!("{}: {}", l.todo, e.text);
            if !who.is_empty() {
                line.push_str(&format!(" ({})", who.join(", ")));
            }
            if e.done {
                line.push_str(&format!(" · {}", l.done));
            }
            line
        }
    }
}

/// 주제 줄 앞 꼬리표예요("주제", "Topic").
pub fn topic_label(english: bool) -> &'static str {
    if english { EN.topic } else { KO.topic }
}

/// 회의 하나의 문서예요. 줄 앞 꼬리표는 회의 언어로 써요(영어 회의 조각에 한국어 꼬리표가 섞이지 않게요).
pub fn doc_of(m: &Meeting) -> Doc {
    let en = is_english(m);
    let l = if en { &EN } else { &KO };
    let at: String = m.start.chars().take(10).collect();
    let mut body: Vec<String> = m.summary.clone();
    for kind in ["decision", "commitment", "open_question"] {
        for e in m.ledger.iter().filter(|e| e.kind == kind) {
            body.push(ledger_line(e, en));
        }
    }
    for t in &m.topics {
        body.push(format!("{}: {}", l.topic, t.title));
        body.extend(t.points.iter().map(|p| format!("- {p}")));
    }
    Doc {
        path: format!("meeting/{}", m.id),
        title: m.title.clone(),
        url: String::new(),
        author: None,
        at: at.clone(),
        acl: Some(vec!["*".to_string()]),
        head: format!("meeting: {}\nmeta: {} · {at}", m.title, l.meta),
        body: body.into_iter().map(|line| (line, String::new())).collect(),
    }
}

fn read(raw_dir: &Path) -> Option<Map<String, Value>> {
    match read_json(&raw_dir.join(FILE)) {
        Ok(Stored::Value(Value::Object(m))) => Some(m),
        _ => None,
    }
}

fn meetings_of(raw: &Map<String, Value>) -> Vec<Meeting> {
    raw.get("meetings")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// 받은 회의 목록을 저장해요. 저장된 목록과 같으면 쓰지 않고 false예요(갱신을 걸지 않아요).
/// 두 `POST /meetings`가 겹쳐도 한 번에 하나씩 읽고 써요(같은 임시 파일을 써서, 겹치면 파일이 깨지거나 옛 목록이 남아요).
static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn save(raw_dir: &Path, meetings: &[Meeting]) -> std::io::Result<bool> {
    let _one = SAVING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(raw) = read(raw_dir)
        && meetings_of(&raw) == meetings
    {
        return Ok(false);
    }
    let saved_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let v = json!({"meetings": meetings, "saved_at": saved_at});
    write_json(&raw_dir.join(FILE), &v, None)?;
    Ok(true)
}

/// 저장된 회의 전부예요(받은 순서). 파일이 없으면 비어요.
pub fn all(raw_dir: &Path) -> Vec<Meeting> {
    read(raw_dir).map(|raw| meetings_of(&raw)).unwrap_or_default()
}

/// 저장된 회의 중 `ids` 순서대로 돌려줘요. 없는 id는 건너뛰어요.
pub fn load(raw_dir: &Path, ids: &[String]) -> Vec<Meeting> {
    let Some(raw) = read(raw_dir) else {
        return Vec::new();
    };
    let all = meetings_of(&raw);
    ids.iter()
        .filter_map(|id| all.iter().find(|m| &m.id == id).cloned())
        .collect()
}

/// 아직 끝나지 않은 장부 항목이에요(열린 질문, 못 끝낸 할 일).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenItem {
    pub meeting_id: String,
    /// 회의 장부 안의 자리예요.
    pub index: usize,
    pub kind: String,
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
}

/// 열린 질문과 끝내지 않은 할 일만 골라요. 회의 순서, 회의 안에서는 장부 순서예요.
pub fn open_items(meetings: &[Meeting]) -> Vec<OpenItem> {
    meetings
        .iter()
        .flat_map(|m| {
            m.ledger.iter().enumerate().filter_map(|(index, e)| {
                let open = e.kind == "open_question" || (e.kind == "commitment" && !e.done);
                open.then(|| OpenItem {
                    meeting_id: m.id.clone(),
                    index,
                    kind: e.kind.clone(),
                    text: e.text.clone(),
                    owner: e.owner.clone(),
                    due: e.due.clone(),
                })
            })
        })
        .collect()
}

/// 갱신의 회의 몫이에요: 저장된 회의의 조각과 info(`{meetings, chunks, saved_at}`)예요. 파일이 없으면 둘 다 비어요.
pub fn gather(raw_dir: &Path) -> (Vec<RawChunk>, Option<Value>) {
    let Some(raw) = read(raw_dir) else {
        return (Vec::new(), None);
    };
    let meetings = meetings_of(&raw);
    let chunks: Vec<RawChunk> = meetings
        .iter()
        .flat_map(|m| windows(&doc_of(m), "meeting"))
        .collect();
    let info = json!({
        "meetings": meetings.len(),
        "chunks": chunks.len(),
        "saved_at": raw.get("saved_at").cloned().unwrap_or(Value::Null),
    });
    (chunks, Some(info))
}

/// 받은 회의를 지워요. 다음 갱신이 회의 조각을 빼요.
pub fn forget(raw_dir: &Path) -> std::io::Result<()> {
    match fs::remove_file(raw_dir.join(FILE)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}
