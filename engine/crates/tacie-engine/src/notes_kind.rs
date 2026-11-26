//! Recording kind (docs/23_녹음_유형별_정리.md): meetings, interviews/talks, and lectures/presentations each get a
//! different notes template.
//! The engine picks the kind from the start of the transcript (`notes_classify`), and when a person changes it, the
//! notes are rewritten for that kind.

use serde_json::Value;

/// 분류에 보내는 대화 기록 앞부분의 글자 수예요.
pub const CLASSIFY_CHARS: usize = 3000;
/// 이 확신보다 낮으면 회의로 써요. 회의가 다른 유형으로 잘못 가는 쪽이 더 아파요.
pub const CLASSIFY_MIN: f64 = 0.7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotesKind {
    Meeting,
    Interview,
    Talk,
}

impl NotesKind {
    pub fn parse(s: &str) -> Option<NotesKind> {
        match s {
            "meeting" => Some(NotesKind::Meeting),
            "interview" => Some(NotesKind::Interview),
            "talk" => Some(NotesKind::Talk),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            NotesKind::Meeting => "meeting",
            NotesKind::Interview => "interview",
            NotesKind::Talk => "talk",
        }
    }

    /// 이 유형의 정리에 받을 항목 종류예요. 다른 유형의 종류는 버려요.
    pub fn item_kinds(self) -> &'static [&'static str] {
        match self {
            NotesKind::Meeting => &["decision", "commitment", "open_question"],
            NotesKind::Interview => &["fact", "quote"],
            NotesKind::Talk => &["concept", "takeaway"],
        }
    }

    /// (제목·요약·흐름, 아래 항목) 프롬프트 이름이에요.
    pub fn prompt_names(self) -> (&'static str, &'static str) {
        match self {
            NotesKind::Meeting => ("notes_overview_v2", "notes_items"),
            NotesKind::Interview => ("notes_interview_overview", "notes_interview_items"),
            NotesKind::Talk => ("notes_talk_overview", "notes_talk_items"),
        }
    }
}

/// 분류 답 `{"kind", "confidence"}`의 유형이에요. 못 읽거나, 모르는 값이거나, 확신이 낮으면 회의예요.
pub fn parse_kind(raw: &str) -> NotesKind {
    let Some(obj) = crate::notes::first_object(raw) else {
        return NotesKind::Meeting;
    };
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .and_then(NotesKind::parse);
    let sure = obj.get("confidence").and_then(Value::as_f64).unwrap_or(0.0);
    match kind {
        Some(k) if sure >= CLASSIFY_MIN => k,
        _ => NotesKind::Meeting,
    }
}

/// 분류에 보낼 앞부분이에요: 줄 단위로 `CLASSIFY_CHARS`자(줄바꿈 포함)를 넘지 않게 잘라요.
/// 첫 줄부터 넘으면 그 줄을 글자로 잘라요.
pub fn classify_sample(transcript: &str) -> String {
    let mut out = String::new();
    let mut n = 0;
    for line in transcript.lines() {
        let len = line.chars().count() + usize::from(!out.is_empty());
        if n + len > CLASSIFY_CHARS {
            if out.is_empty() {
                return line.chars().take(CLASSIFY_CHARS).collect();
            }
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        n += len;
    }
    out
}
