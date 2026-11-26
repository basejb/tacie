//! The computation behind panel write tools (docs/38 12절). The engine does not change meeting files. It finds the
//! places to change and passes them to the app as a proposal (`proposal`); when the user applies it on the confirm
//! card, the app changes its own meeting file.
//!
//! A place to change is given as "the nth occurrence of `word` in that text" (`nth`, from 0). Character positions
//! are not used because Rust and Swift count them differently. The app finds the same occurrence of `word` in the
//! same text and replaces it, and skips it if the text has changed in the meantime.

use serde_json::{Value, json};
use tacie_sources::meeting::Meeting;

/// 앞뒤로 보여 주는 글자 수예요.
const CONTEXT: usize = 30;

/// 회의 안에서 낱말이 나온 한 곳이에요.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    /// 어느 글인지: `{"line": key}`, `{"title": true}`, `{"summary": i}`, `{"topic": i}`(주제 제목),
    /// `{"topic": i, "point": j}`, `{"ledger": i}`예요.
    pub at: Value,
    /// 그 글 안에서 `word`가 나온 차례(0부터)예요.
    pub nth: usize,
    /// 앞뒤 글이에요(모델과 확인 카드가 읽어요).
    pub before: String,
    pub word: String,
    pub after: String,
    /// 대화 기록이면 말한 사람이에요.
    pub speaker: Option<String>,
}

impl Place {
    pub fn in_transcript(&self) -> bool {
        self.at.get("line").is_some()
    }

    /// 사람이 읽는 자리 이름이에요(모델에게 보여 줘요).
    pub fn label(&self) -> String {
        let a = &self.at;
        if let Some(k) = a["line"].as_str() {
            return format!("대화 {k} {}", self.speaker.as_deref().unwrap_or(""));
        }
        if a.get("title").is_some() {
            return "제목".into();
        }
        if let Some(i) = a["summary"].as_u64() {
            return format!("요약 {}", i + 1);
        }
        if let Some(i) = a["topic"].as_u64() {
            return match a["point"].as_u64() {
                Some(j) => format!("주제 {} 내용 {}", i + 1, j + 1),
                None => format!("주제 {} 제목", i + 1),
            };
        }
        if let Some(i) = a["ledger"].as_u64() {
            return format!("장부 항목 {}", i + 1);
        }
        String::new()
    }

    pub fn json(&self) -> Value {
        json!({"at": self.at, "nth": self.nth, "before": self.before, "word": self.word, "after": self.after})
    }

    /// 이 자리에 넣을 글이에요. 기록이 쓴 가운뎃점을 그대로 살려요("심의․의견" → "심의․의결").
    pub fn replacement(&self, meant: &str) -> String {
        let Some(dot) = self.word.chars().find(|c| DOTS.contains(c)) else {
            return meant.to_string();
        };
        meant.chars().map(|c| if DOTS.contains(&c) { dot } else { c }).collect()
    }
}

/// 가운뎃점은 받아쓰기와 회의 정리가 서로 다른 글자로 써요("경․경매", "경·경매"). 찾을 때는 같은 글자로 봐요.
const DOTS: [char; 5] = ['·', '․', '‧', '・', '･'];

fn same(a: char, b: char) -> bool {
    a == b || (DOTS.contains(&a) && DOTS.contains(&b))
}

/// 가운뎃점만 다른 같은 말이에요.
pub fn same_word(a: &str, b: &str) -> bool {
    a.chars().count() == b.chars().count() && a.chars().zip(b.chars()).all(|(x, y)| same(x, y))
}

/// `text` 안에서 `word`가 나온 곳마다 (실제 글자, 그 글자가 나온 차례, 앞 글, 뒤 글)이에요. 겹치지 않게 앞에서부터
/// 세요. 차례는 실제 글자 그대로가 나온 차례예요: 앱은 같은 글자를 같은 차례에서 찾아 바꿔요.
fn hits(text: &str, word: &str) -> Vec<(String, usize, String, String)> {
    let t: Vec<char> = text.chars().collect();
    let w: Vec<char> = word.chars().collect();
    if w.is_empty() || t.len() < w.len() {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut i = 0;
    while i + w.len() <= t.len() {
        if t[i..i + w.len()].iter().zip(&w).all(|(&a, &b)| same(a, b)) {
            spans.push(i);
            i += w.len();
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for &st in &spans {
        let found: String = t[st..st + w.len()].iter().collect();
        // 이 글 안에서 같은 글자 그대로가 앞에 몇 번 나왔는지(겹치지 않게)예요.
        let nth = spans.iter().take_while(|&&o| o < st).filter(|&&o| t[o..o + w.len()].iter().collect::<String>() == found).count();
        let before: String = t[st.saturating_sub(CONTEXT)..st].iter().collect();
        let after: String = t[st + w.len()..(st + w.len() + CONTEXT).min(t.len())].iter().collect();
        out.push((found, nth, before, after));
    }
    out
}

/// 회의의 대화 기록(앱이 붙인 발화)과 정리 글에서 `word`가 나온 곳을 모두 찾아요. 대화 기록이 먼저예요.
pub fn find(meeting: Option<&Meeting>, transcript: Option<&Vec<Value>>, word: &str) -> Vec<Place> {
    let word = word.trim();
    let mut out = Vec::new();
    let mut add = |at: Value, text: &str, speaker: Option<String>| {
        for (found, nth, before, after) in hits(text, word) {
            out.push(Place { at: at.clone(), nth, before, word: found, after, speaker: speaker.clone() });
        }
    };
    for l in transcript.into_iter().flatten() {
        let (Some(k), Some(t)) = (l["key"].as_str(), l["text"].as_str()) else {
            continue;
        };
        add(json!({"line": k}), t, l["speaker"].as_str().map(String::from));
    }
    if let Some(m) = meeting {
        add(json!({"title": true}), &m.title, None);
        for (i, s) in m.summary.iter().enumerate() {
            add(json!({"summary": i}), s, None);
        }
        for (i, t) in m.topics.iter().enumerate() {
            add(json!({"topic": i}), &t.title, None);
            for (j, p) in t.points.iter().enumerate() {
                add(json!({"topic": i, "point": j}), p, None);
            }
        }
        for (i, e) in m.ledger.iter().enumerate() {
            add(json!({"ledger": i}), &e.text, None);
        }
    }
    out
}

/// 잘못 들린 말 고치기 제안이에요. 앱의 확인 카드가 이대로 그려요.
pub fn fix_words(id: &str, meeting_id: &str, heard: &str, meant: &str, places: &[Place]) -> Value {
    let lines = places.iter().filter(|p| p.in_transcript()).count();
    json!({
        "id": id, "kind": "fix_words", "meeting_id": meeting_id, "heard": heard, "meant": meant,
        "places": places.iter().map(|p| {
            let mut v = p.json();
            v["to"] = json!(p.replacement(meant));
            v
        }).collect::<Vec<_>>(),
        "lines": lines, "notes": places.len() - lines,
    })
}

/// 장부 고치기에서 항목 하나에 바꿀 것이에요. 앱은 `was`(지금 글)가 맞을 때만 바꿔요.
pub fn ledger_change(index: usize, was: &str, args: &Value) -> Option<Value> {
    let mut c = json!({"index": index, "was": was});
    let mut any = false;
    if args["remove"].as_bool() == Some(true) {
        c["remove"] = json!(true);
        return Some(c);
    }
    for k in ["text", "owner", "due"] {
        if let Some(v) = args[k].as_str().map(str::trim).filter(|v| !v.is_empty()) {
            c[k] = json!(v);
            any = true;
        }
    }
    if let Some(d) = args["done"].as_bool() {
        c["done"] = json!(d);
        any = true;
    }
    any.then_some(c)
}

/// 새 할 일이에요. 글이 없으면 None이에요.
pub fn ledger_add(args: &Value) -> Option<Value> {
    let text = args["text"].as_str().map(str::trim).filter(|t| !t.is_empty())?;
    let mut a = json!({"kind": "commitment", "text": text});
    for k in ["owner", "due"] {
        if let Some(v) = args[k].as_str().map(str::trim).filter(|v| !v.is_empty()) {
            a[k] = json!(v);
        }
    }
    Some(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tacie_sources::meeting::{Entry, Topic};

    fn meeting() -> Meeting {
        Meeting {
            id: "m1".into(),
            title: "환불 유예 논의".into(),
            start: "2023-05-16T10:00:00+09:00".into(),
            end: "2023-05-16T11:00:00+09:00".into(),
            summary: vec!["환불와 환불를 같이 유예해요".into()],
            topics: vec![Topic { title: "유예".into(), points: vec!["환불 기간".into()] }],
            ledger: vec![Entry {
                kind: "decision".into(),
                text: "환불 유예".into(),
                owner: None,
                due: None,
                done: false,
                replaces: None,
            }],
        }
    }

    #[test]
    fn finds_every_place_with_its_turn_in_the_text() {
        let lines = vec![
            json!({"key": "e0:s1u1", "speaker": "화자 1", "text": "환불․환불 둘 다 유예해요"}),
            json!({"key": "e0:s1u2", "speaker": "화자 2", "text": "네"}),
        ];
        let m = meeting();
        let found = find(Some(&m), Some(&lines), "환불");
        let at: Vec<(Value, usize)> = found.iter().map(|p| (p.at.clone(), p.nth)).collect();
        assert_eq!(
            at,
            vec![
                (json!({"line": "e0:s1u1"}), 0),
                (json!({"line": "e0:s1u1"}), 1),
                (json!({"title": true}), 0),
                (json!({"summary": 0}), 0),
                (json!({"summary": 0}), 1),
                (json!({"topic": 0, "point": 0}), 0),
                (json!({"ledger": 0}), 0),
            ]
        );
        assert_eq!(found[1].before, "환불․");
        assert_eq!(found[1].after, " 둘 다 유예해요");
        assert_eq!(found[0].label(), "대화 e0:s1u1 화자 1");
        let p = fix_words("p1", "m1", "경매", "공매", &found[..2]);
        assert_eq!((p["lines"].as_u64(), p["notes"].as_u64()), (Some(2), Some(0)));
    }

    #[test]
    fn middle_dots_match_each_other_and_keep_the_record_s_own() {
        let lines = vec![json!({"key": "k", "speaker": "", "text": "경․경매 그리고 경·경매"})];
        let found = find(None, Some(&lines), "경·경매");
        let got: Vec<(&str, usize)> = found.iter().map(|p| (p.word.as_str(), p.nth)).collect();
        assert_eq!(got, vec![("경․경매", 0), ("경·경매", 0)]);
        assert_eq!(found[0].replacement("경·공매"), "경․공매");
        assert_eq!(found[1].replacement("경·공매"), "경·공매");
    }

    #[test]
    fn nothing_for_an_empty_word() {
        assert!(find(Some(&meeting()), None, "  ").is_empty());
    }

    #[test]
    fn ledger_changes_need_something_to_change() {
        assert!(ledger_change(2, "글", &json!({"owner": " "})).is_none());
        let c = ledger_change(2, "글", &json!({"owner": "지윤", "done": true})).unwrap();
        assert_eq!(c, json!({"index": 2, "was": "글", "owner": "지윤", "done": true}));
        let r = ledger_change(1, "글", &json!({"remove": true, "text": "x"})).unwrap();
        assert_eq!(r, json!({"index": 1, "was": "글", "remove": true}));
        assert_eq!(ledger_add(&json!({"text": "보고하기", "due": "금요일"})).unwrap()["kind"], "commitment");
        assert!(ledger_add(&json!({"text": ""})).is_none());
    }
}
