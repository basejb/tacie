//! Backchannel detection and meeting ledger parsing (`is_backchannel`, `parse_ledger`, `parse_overview`). What the
//! ledger model writes is kept only when it cites a sent utterance as its source (docs/04 4절).
//!
//! All character counts are in Unicode code points.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};

/// "네", "아 네네." 같은 말만 있는 발화는 판단을 부르지 않고 앞 대화 3줄에도 넣지 않아요(docs/04 4절).
pub static BACKCHANNELS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    "네 넵 넹 예 응 어 아 음 오 으음 아하 아아 네네 넵넵 예예 응응 그쵸 그죠 그렇죠 그렇네요 그렇구나 \
     맞아요 맞아 맞습니다 좋아요 좋습니다 오케이 알겠어요 알겠습니다 감사합니다 고마워요 \
     ok okay yes yeah yep"
        .split_whitespace()
        .collect()
});
pub const BACKCHANNEL_CHARS: usize = 12;
/// 장부의 종류예요: 정한 것, 누가 하기로 한 일, 정하지 못하고 남긴 것.
pub const LEDGER_KINDS: [&str; 3] = ["decision", "commitment", "open_question"];
pub const LEDGER_PER_KIND: usize = 12;
/// 결정과 남은 논의는 이유까지 한두 문장(프롬프트는 150자)이라 넉넉히 받아요.
pub const LEDGER_TEXT_MAX: usize = 200;
/// 장부 모델에 보내는 회의 글자 수예요. 아주 긴 회의는 뒷부분을 남겨요.
pub const LEDGER_CHARS: usize = 40000;
// 같은 호출로 메인 창의 회의 제목과 요약도 써요(spec 2026-10-23-main-window-settings 5-4).
pub const NOTES_TITLE_MAX: usize = 30;
pub const NOTES_SUMMARY_MAX: usize = 3;
/// 요약 한 문장이에요(프롬프트는 200자).
pub const NOTES_LINE_MAX: usize = 250;
/// 논의 흐름: 주제는 나온 순서대로 6개까지, 주제마다 2–5문장이에요(프롬프트는 문장마다 150자).
/// 세 단계 정리(`notes_v2`)는 안건을 12개까지, 안건마다 1–2문장으로 써요. 예전 한 호출 정리는 6개, 2–3문장이에요.
/// 둘 다 3문장에서 끊어요.
pub const TOPICS_MAX: usize = 12;
pub const TOPIC_POINTS_MAX: usize = 3;
pub const POINT_MAX: usize = 200;
/// 회의에 없는 사람이 맡은 일이면 모델이 쓴 이름을 이만큼까지 남겨요.
pub const OWNER_MAX: usize = 20;
pub const DUE_MAX: usize = 20;

/// 단어 글자도 공백도 아닌 글자예요. 단어 글자는 글자·숫자와 밑줄뿐이라서 Rust `\w`(결합 부호, 다른 이음표도
/// 들어가요)가 아니라 `\p{L}\p{N}_`로 써요.
static NOT_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}_\s]").expect("not-word regex"));

/// 목록에 있는 말만, 이어 붙여 12자까지, 물음표 없이예요("그래요?"는 판단이 정해요).
pub fn is_backchannel(text: &str) -> bool {
    if text.contains('?') || text.contains('？') {
        return false;
    }
    let lower = text.to_lowercase();
    let cleaned = NOT_WORD.replace_all(&lower, " ");
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    !words.is_empty()
        && words.iter().all(|w| BACKCHANNELS.contains(w))
        && words.iter().map(|w| w.chars().count()).sum::<usize>() <= BACKCHANNEL_CHARS
}

/// 받아쓰기가 소리를 적은 표시("[게임 소리]", "[박수 소리]")예요.
static SOUND_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[^\]]*\]").expect("sound-tag regex"));

/// 소리 표시만 있고 말이 없는 발화예요("[게임 소리]", "[효과음] [배경음악]"). 사람이 한 말이 아니라서 목소리로 묶지 않고
/// 판단하지도 않아요. 표시 옆에 말이 있으면("[박수 소리] 쉽지 않네.") 말이 있는 발화예요.
pub fn is_sound_only(text: &str) -> bool {
    SOUND_TAG.is_match(text)
        && !SOUND_TAG
            .replace_all(text, " ")
            .chars()
            .any(|c| c.is_alphanumeric())
}

/// 말한 구간의 길이(초)예요: 첫 단어 시작부터 마지막 단어 끝까지예요. 단어 시각이 없으면 None이에요.
/// 발화 WAV는 앞뒤 쉼과 배경 소리까지 들어 있어서, 새 화자를 만들 만큼 길게 말했는지는 이 길이로 봐요.
pub fn spoken_seconds(words: &[(f64, f64)]) -> Option<f64> {
    let start = words.iter().map(|w| w.0).reduce(f64::min)?;
    let end = words.iter().map(|w| w.1).reduce(f64::max)?;
    Some((end - start).max(0.0))
}

/// 로그 한 줄은 한 줄로 남아요: 앱이 그 앞말로 줄째 버릴 수 있어요.
pub fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

/// "items"가 있는 첫 JSON 객체예요. 어디서 시작하든 찾아요: 앞말이나 코드 울타리에 중괄호가 따로 있을 수 있어요.
/// `{`마다 그 자리에서 JSON 값 하나를 읽어 봐요(뒤에 남은 글은 상관없어요).
pub fn ledger_object(raw: &str) -> Result<Map<String, Value>, String> {
    ledger_object_with(raw, "items")
}

/// 글 속에서 `key`가 있는 첫 JSON 객체예요.
fn ledger_object_with(raw: &str, key: &str) -> Result<Map<String, Value>, String> {
    for (i, _) in raw.match_indices('{') {
        let mut values = serde_json::Deserializer::from_str(&raw[i..]).into_iter::<Value>();
        if let Some(Ok(Value::Object(obj))) = values.next()
            && obj.contains_key(key)
        {
            return Ok(obj);
        }
    }
    Err(format!("no JSON object with {key}"))
}

/// 근거가 비어 있지 않은 목록이고, 모두 보낸 발화의 id면 그 id들이에요. 다시 쓰기는 "e0:s1u3" 꼴로 보내는데 모델이
/// 앞부분을 떼고 "s1u3"로 돌려주곤 해요: 보낸 id 중 ":s1u3"로 끝나는 게 하나뿐이면 그 id예요.
fn grounded(refs: Option<&Value>, ids: &HashSet<String>) -> Option<Vec<String>> {
    let Some(Value::Array(refs)) = refs else {
        return None;
    };
    if refs.is_empty() {
        return None;
    }
    refs.iter()
        .map(|u| {
            let u = u.as_str()?;
            if ids.contains(u) {
                return Some(u.to_string());
            }
            let tail = format!(":{u}");
            let mut hits = ids.iter().filter(|id| id.ends_with(&tail));
            match (hits.next(), hits.next()) {
                (Some(id), None) => Some(id.clone()),
                _ => None,
            }
        })
        .collect()
}

/// 앞뒤 공백을 뺀 글이 1자 이상 max자 이하면 그 글이에요.
fn trimmed(v: Option<&Value>, max: usize) -> Option<&str> {
    let t = v?.as_str()?.trim();
    (1..=crate::lang::cap(max, t)).contains(&t.chars().count()).then_some(t)
}

/// 모델이 글 속에 단 근거 표시("[s1u3]", "[e0:s1u3]", 이어 붙인 "[u4][u5]")예요. 근거는 utterance_ids에만 있어요.
static INLINE_REFS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*(\[(?:e\d+:)?s?\d*u\d+\])+").expect("inline-refs regex"));

/// 근거 표시를 뺀 글을 `max`자에서 자른 거예요(넘으면 끝에 "…"). 비었으면 None이에요.
fn cut_prose(v: Option<&Value>, max: usize) -> Option<String> {
    let t = INLINE_REFS.replace_all(v?.as_str()?, "");
    let t = t.trim();
    if t.is_empty() {
        return None;
    }
    let max = crate::lang::cap(max, t);
    if t.chars().count() <= max {
        return Some(t.to_string());
    }
    let cut: String = t.chars().take(max - 1).collect();
    Some(format!("{}…", cut.trim_end()))
}

/// 근거 표시를 뺀 [`trimmed`]예요.
fn prose(v: Option<&Value>, max: usize) -> Option<String> {
    let t = INLINE_REFS.replace_all(v?.as_str()?, "");
    trimmed(Some(&Value::String(t.into_owned())), max).map(str::to_string)
}

/// (보여 줄 항목들, 버린 수)예요. 읽을 수 있는 JSON이 없으면 오류예요.
/// commitment에는 owner와 due(말한 그대로 또는 null)도 붙어요. `owners`는 참석자의 보인 이름 → 화자 키예요:
/// 모델이 참석자 이름(또는 키)을 쓰면 owner는 그 키예요. 회의에 없는 이름이면 그 이름을 `OWNER_MAX`자까지,
/// 비었거나 글이 아니면 null이에요.
pub fn parse_ledger(
    raw: &str,
    ids: &HashSet<String>,
    owners: &HashMap<String, String>,
) -> Result<(Vec<Value>, usize), String> {
    parse_ledger_for(raw, ids, owners, &LEDGER_KINDS)
}

/// `parse_ledger`와 같고, 받을 항목 종류를 유형에 맞게 줘요(`NotesKind::item_kinds`). 다른 종류는 버려요.
pub fn parse_ledger_for(
    raw: &str,
    ids: &HashSet<String>,
    owners: &HashMap<String, String>,
    kinds: &[&str],
) -> Result<(Vec<Value>, usize), String> {
    // the model sometimes writes `"kind": "open_question": "text": …` (a colon for the comma after the kind)
    let mended = regex::Regex::new(r#""kind"\s*:\s*"([a-z_]+)"\s*:\s*"text""#)
        .expect("kind-colon pattern")
        .replace_all(raw, r#""kind": "$1", "text""#);
    let obj = ledger_object(&mended)?;
    let Some(got) = obj.get("items").and_then(Value::as_array) else {
        return Err("items is not a list".into());
    };
    let mut per: HashMap<&str, usize> = kinds.iter().map(|&k| (k, 0)).collect();
    let mut items = Vec::new();
    for it in got {
        let Some(it) = it.as_object() else { continue };
        let Some((kind, count)) = it
            .get("kind")
            .and_then(Value::as_str)
            .and_then(|k| per.get_key_value(k).map(|(&k, &n)| (k, n)))
        else {
            continue;
        };
        let refs = it.get("utterance_ids");
        let Some(text) = prose(it.get("text"), LEDGER_TEXT_MAX) else {
            continue;
        };
        let Some(refs) = grounded(refs, ids).filter(|_| count < LEDGER_PER_KIND) else {
            continue;
        };
        per.insert(kind, count + 1);
        let mut item = json!({"kind": kind, "text": text, "utterance_ids": refs});
        if kind == "commitment" {
            item["owner"] = json!(owner_of(it.get("owner"), owners));
            item["due"] = json!(trimmed(it.get("due"), DUE_MAX));
        }
        items.push(item);
    }
    let dropped = got.len() - items.len();
    Ok((items, dropped))
}

/// 참석자 이름이면 그 화자 키, 키를 그대로 썼으면 그 키, 다른 이름이면 앞 `OWNER_MAX`자예요.
fn owner_of(v: Option<&Value>, owners: &HashMap<String, String>) -> Option<String> {
    let o = v?.as_str()?.trim();
    if o.is_empty() {
        return None;
    }
    if let Some(key) = owners.get(o) {
        return Some(key.clone());
    }
    if owners.values().any(|k| k == o) {
        return Some(o.to_string());
    }
    Some(
        o.chars()
            .take(OWNER_MAX)
            .collect::<String>()
            .trim_end()
            .to_string(),
    )
}

/// 근거가 있는 글 줄(`[{"text", "utterance_ids"}]`)을 `max`개까지예요. 근거가 없거나 길면 그 줄만 빠져요.
fn grounded_lines(
    v: Option<&Value>,
    ids: &HashSet<String>,
    chars: usize,
    max: usize,
) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .filter_map(|ln| {
                    let ln = ln.as_object()?;
                    let text = prose(ln.get("text"), chars)?;
                    grounded(ln.get("utterance_ids"), ids).map(|_| text)
                })
                .take(max)
                .collect()
        })
        .unwrap_or_default()
}

/// [`grounded_lines`]처럼 고르고, 줄마다 역할(`notes_v2::ROLES`)도 같이예요.
fn grounded_points(
    v: Option<&Value>,
    ids: &HashSet<String>,
    chars: usize,
    max: usize,
) -> Vec<(String, Option<String>)> {
    v.and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .filter_map(|ln| {
                    let ln = ln.as_object()?;
                    let text = prose(ln.get("text"), chars)?;
                    grounded(ln.get("utterance_ids"), ids)?;
                    let role = ln
                        .get("role")
                        .and_then(Value::as_str)
                        .filter(|r| crate::notes_v2::ROLES.contains(r))
                        .map(str::to_string);
                    Some((text, role))
                })
                .take(max)
                .collect()
        })
        .unwrap_or_default()
}

/// 회의 제목, 요약 세 문장까지, 논의 흐름(주제)이에요. 문장마다 보낸 발화를 근거로 달아야 해요.
/// 주제는 제목이 있고 근거 있는 문장이 하나라도 남아야 해요.
/// 읽을 수 없는 JSON이면 모두 없어요(그건 장부 쪽이 알려요).
pub fn parse_overview(raw: &str, ids: &HashSet<String>) -> Value {
    let Ok(data) = ledger_object(raw) else {
        return json!({"title": null, "summary": [], "topics": []});
    };
    let title = trimmed(data.get("title"), NOTES_TITLE_MAX);
    let summary = grounded_lines(data.get("summary"), ids, NOTES_LINE_MAX, NOTES_SUMMARY_MAX);
    let topics: Vec<Value> = data
        .get("topics")
        .and_then(Value::as_array)
        .map(|topics| {
            topics
                .iter()
                .filter_map(|t| {
                    let t = t.as_object()?;
                    let title = prose(t.get("title"), NOTES_TITLE_MAX)?;
                    let kept = grounded_points(t.get("points"), ids, POINT_MAX, TOPIC_POINTS_MAX);
                    if kept.is_empty() {
                        return None;
                    }
                    let (points, roles): (Vec<String>, Vec<Option<String>>) = kept.into_iter().unzip();
                    let mut topic = json!({"title": title, "points": points});
                    // 줄마다 역할(배경, 논의, 결론, 미정)이에요. 세 단계 정리만 써요. 모르는 줄은 빈 글이에요.
                    if roles.iter().any(Option::is_some) {
                        topic["roles"] = json!(roles.into_iter().map(Option::unwrap_or_default).collect::<Vec<_>>());
                    }
                    Some(topic)
                })
                .take(TOPICS_MAX)
                .collect()
        })
        .unwrap_or_default();
    json!({"title": title, "summary": summary, "topics": topics})
}

/// 글 속 사람 표시 `[[이름]]`이에요(`notes_overview_v2`).
static PERSON_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\[\]]{1,40})\]\]").expect("person-ref regex"));

/// 괄호 없는 화자 번호예요. 앞 글자는 `[[`가 아니어야 해요(regex 크레이트에는 뒤돌아보기가 없어요).
static BARE_SPEAKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^\[])(화자 \d+)").expect("bare-speaker regex"));

/// 요약과 논의 흐름 글의 `[[이름]]` 중 참석자가 아닌 것은 괄호를 풀어 이름만 남겨요. 참석자 표시는 그대로 두고, 앱이
/// 화자 키로 바꿔 지금 이름으로 보여 줘요.
pub fn keep_attendee_refs(notes: &mut Value, attendees: &HashSet<String>) {
    let fix = |v: &mut Value| {
        if let Some(t) = v.as_str() {
            let out = PERSON_REF.replace_all(t, |c: &regex::Captures| {
                let name = c[1].trim();
                if attendees.contains(name) {
                    format!("[[{name}]]")
                } else {
                    name.to_string()
                }
            });
            // 모델이 괄호를 빠뜨린 "화자 3"도 참석자면 표시로 감싸요(번호는 다른 말일 수 없어요).
            let out = BARE_SPEAKER.replace_all(&out, |c: &regex::Captures| {
                let (before, name) = (&c[1], &c[2]);
                if attendees.contains(name) {
                    format!("{before}[[{name}]]")
                } else {
                    c[0].to_string()
                }
            });
            *v = Value::String(out.into_owned());
        }
    };
    if let Some(lines) = notes.get_mut("summary").and_then(Value::as_array_mut) {
        lines.iter_mut().for_each(fix);
    }
    if let Some(topics) = notes.get_mut("topics").and_then(Value::as_array_mut) {
        for t in topics {
            if let Some(points) = t.get_mut("points").and_then(Value::as_array_mut) {
                points.iter_mut().for_each(fix);
            }
        }
    }
}

/// 한도에서 잘린 JSON 글을, 목록 안에서 마지막으로 끝난 객체까지 남기고 열린 괄호를 닫은 글이에요. 긴 회의는 모델이
/// 개수 한도를 넘겨 쓰다 잘리곤 해요(2026-11-03): 끝난 항목은 살려요. 끝난 객체가 없으면 None이에요.
pub fn close_cut(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let mut stack: Vec<char> = Vec::new();
    let (mut in_str, mut escaped) = (false, false);
    let mut last: Option<(usize, Vec<char>)> = None;
    for (i, c) in raw[start..].char_indices() {
        if in_str {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => stack.push(c),
            '}' | ']' => {
                stack.pop();
                if c == '}' && stack.last() == Some(&'[') {
                    last = Some((start + i + 1, stack.clone()));
                }
            }
            _ => {}
        }
    }
    let (end, open) = last?;
    let closers: String = open
        .iter()
        .rev()
        .map(|&c| if c == '{' { '}' } else { ']' })
        .collect();
    Some(format!("{}{closers}", &raw[start..end]))
}

/// 회의 정리 두 호출의 글을 한 JSON 글로 합쳐요: 앞 호출의 title, summary, topics와 뒤 호출의 items예요.
/// 뒤 호출을 읽을 수 없으면 그 글 그대로예요(부르는 쪽이 "unreadable"로 알려요). 앞 호출을 읽을 수 없으면 제목·요약
/// 없이 items만이에요.
pub fn merge_notes(overview: &str, items: &str) -> String {
    let head = first_object(overview);
    let mut out = match ledger_object(items) {
        Ok(out) => out,
        // 할 일 쪽만 깨졌으면(작은 로컬 모델이 같은 줄을 되풀이하다 잘린 것) 요약·주제는 살리고 items는 비워요.
        Err(_) if matches!(head, Some(Value::Object(_))) => Map::from_iter([("items".to_string(), Value::Array(vec![]))]),
        Err(_) => return items.to_string(),
    };
    if let Some(Value::Object(head)) = head {
        for key in ["title", "summary", "topics"] {
            if let Some(v) = head.get(key) {
                out.insert(key.to_string(), v.clone());
            }
        }
    }
    Value::Object(out).to_string()
}

/// 글 속 첫 JSON 객체예요(앞말이나 코드 울타리를 건너뛰어요).
pub(crate) fn first_object(raw: &str) -> Option<Value> {
    raw.match_indices('{').find_map(|(i, _)| {
        serde_json::Deserializer::from_str(&raw[i..])
            .into_iter::<Value>()
            .next()
            .and_then(Result::ok)
            .filter(Value::is_object)
    })
}

/// 질문 판정의 종류예요(docs/24 4절 1번): 회의 안에서 답이 나옴, 문서가 필요함, 질문이 아님.
pub const QUESTION_VERDICTS: [&str; 3] = ["answered", "doc", "not_question"];
/// 답이 아니라는 말이에요: answered인데 답 글에 이게 있으면 doc이에요.
const NO_ANSWER: [&str; 4] = ["답은 없", "답이 없", "답하지 않", "답은 나오지 않"];
/// 영어 프롬프트가 쓰는 같은 말이에요(소문자로 봐요).
const NO_ANSWER_EN: [&str; 4] = ["no answer", "not answered", "wasn't answered", "didn't answer"];
/// 다시 쓴 질문의 글자 수 한도예요(프롬프트는 60자 안쪽이라고 해요).
pub const REVIEW_QUESTION_MAX: usize = 100;
/// 회의에서 나온 답의 글자 수 한도예요(프롬프트는 150자 안쪽이라고 해요).
pub const REVIEW_ANSWER_MAX: usize = 200;

/// `notes_questions`의 답이에요: 받은 질문 id마다 하나, 처음 나온 것만이에요.
/// `{id, verdict, question, answer, utterance_ids}`이고, 다시 쓴 질문이 없거나 길면 null(앱은 원문을 써요)이에요.
/// `questions`는 질문 id → 그 질문이 나온 발화 id예요. answered는 답 글과, 보낸 발화 중 질문 줄이 아닌 근거가 하나라도
/// 있어야 해요(질문 줄만 근거면 답이 아니에요). 아니면 doc이에요. 모르는 근거 id는 빼고, 긴 답은 한도에서 잘라요.
/// 읽을 수 있는 JSON이 없으면 오류예요.
pub fn parse_question_review(
    raw: &str,
    questions: &HashMap<String, Option<String>>,
    ids: &HashSet<String>,
) -> Result<Vec<Value>, String> {
    let obj = ledger_object_with(raw, "questions")?;
    let list = obj
        .get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut seen: HashSet<String> = HashSet::new();
    Ok(list
        .iter()
        .filter_map(|q| {
            let q = q.as_object()?;
            let id = q.get("id")?.as_str()?.trim().to_string();
            let own = questions.get(&id)?;
            if !seen.insert(id.clone()) {
                return None;
            }
            let verdict = q.get("verdict")?.as_str()?;
            if !QUESTION_VERDICTS.contains(&verdict) {
                return None;
            }
            let question = prose(q.get("question"), REVIEW_QUESTION_MAX);
            let (verdict, answer, refs) = match verdict {
                "answered" => {
                    let refs: Vec<String> = q
                        .get("utterance_ids")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|u| grounded(Some(&json!([u])), ids)?.pop())
                        .filter(|u| Some(u) != own.as_ref())
                        .take(3)
                        .collect();
                    // "답은 없음"이라고 쓴 답은 답이 아니에요(모델이 answered와 함께 그렇게 쓰곤 해요)
                    let answer = cut_prose(q.get("answer"), REVIEW_ANSWER_MAX)
                        .filter(|a| !NO_ANSWER.iter().any(|n| a.contains(n)))
                        .filter(|a| {
                            let a = a.to_lowercase();
                            !NO_ANSWER_EN.iter().any(|n| a.contains(n))
                        });
                    match (answer, refs.is_empty()) {
                        (Some(a), false) => ("answered", Some(a), refs),
                        _ => ("doc", None, Vec::new()),
                    }
                }
                v => (v, None, Vec::new()),
            };
            Some(
                json!({"id": id, "verdict": verdict, "question": question, "answer": answer,
                        "utterance_ids": refs}),
            )
        })
        .collect())
}
