//! Where an AI request's prompt gets filled in.
//!
//! The shipped app's engine is built without `local-prompts` (apps/mac/scripts/build-engine.sh, Release). That
//! engine has no prompt text; it sends only the prompt name and the values to fill in to the Tacie AI relay server.
//! The relay server fills them in with the same text (`apps/server/src/prompts.json`) and passes the request to the
//! provider. This keeps the prompts from showing up when someone takes the app apart.
//!
//! With `local-prompts` (dev builds, eval, tests) the text is in the engine, so requests to anything other than the
//! relay server (a provider directly via key files, the fake servers in tests) are sent fully filled in, as before.
//! Requests to the relay server are sent by name in this build too.
//! The engine code (`openrouter.rs`, `terms.rs`, `jev.rs`) holds the original text and `prompts.json` is its copy.
//! `tests/prompts_json.rs` checks that the two match, and running it with `TACIE_WRITE_PROMPTS=1` rewrites the
//! copy.

use regex::{Captures, Regex};
use serde_json::{Map, Value, json};

use crate::settings;

/// 이 주소로 가는 요청을 이름과 값으로 보낼지예요: 중계 서버로 가거나, 엔진에 글이 없으면 그래요.
pub fn by_name(url: &str) -> bool {
    !cfg!(feature = "local-prompts") || settings::is_relay(url)
}

/// 프롬프트 하나예요: `prompts.json`의 `chat` 아래 이름과, `{이름}` 자리에 들어갈 값이에요.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub name: &'static str,
    pub fields: Vec<(&'static str, String)>,
}

impl Prompt {
    /// 중계 서버에 보내는 모양이에요: `{"name": …, "fields": {…}}`.
    pub fn named(&self) -> Value {
        let fields: Map<String, Value> = self
            .fields
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::from(v.as_str())))
            .collect();
        json!({"name": self.name, "fields": fields})
    }

    /// 다 채운 글이에요.
    #[cfg(feature = "local-prompts")]
    pub fn text(&self) -> String {
        fill(template(self.name), &self.fields)
    }
}

/// `{이름}` 자리를 한 번에 채워요(Python `str.format`처럼): 넣은 값 속의 자리 표시는 글자 그대로 남고, `fields`에 없는
/// 이름의 중괄호(JSON 예시)도 그대로예요. 중계 서버의 `fill`(apps/server/src/prompts.js)과 같아요.
pub fn fill(text: &str, fields: &[(&str, String)]) -> String {
    let names: Vec<String> = fields.iter().map(|(k, _)| regex::escape(k)).collect();
    let re = Regex::new(&format!(r"\{{({})\}}", names.join("|"))).expect("field regex");
    re.replace_all(text, |c: &Captures| {
        fields
            .iter()
            .find(|(k, _)| *k == &c[1])
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    })
    .into_owned()
}

// Prompt guide (English, for contributors who do not read Korean). The prompt texts are Korean on purpose: the
// quality of Korean meetings comes from them. Each name takes the fields in `CHAT_FIELDS` and answers as below.
//
// During a meeting
//   answer            context (recent lines), question, passages (numbered search results)
//                     -> a short answer that cites passage numbers, or the fixed "no answer" sentence.
//   readings          terms (names found in the team's material) -> how each may be written when spoken in a
//                     Korean meeting, so speech-to-text can be corrected.
//
// Meeting notes (after a meeting). Every notes prompt answers JSON and cites utterance ids.
//   notes_classify    transcript (its start) -> the kind of recording: meeting, interview, or talk.
//   A meeting is written in three stages (`notes_v2.rs`):
//   notes_outline     attendees, transcript -> the agenda: up to 12 topics, each a title and a span of utterances.
//   notes_topic       title, attendees, transcript (one topic's part) -> 1-2 sentences on what was discussed.
//   notes_items_v2    attendees, agenda, transcript -> decisions, commitments (owner, due), open questions.
//   notes_summary     attendees, notes (the per-topic notes and decisions) -> title and summary.
//   The earlier two calls stay for when the stages fail, and for a rewrite with the person's request (`_ask`):
//   notes_overview_v2 attendees, transcript -> title, summary and topics.
//   notes_items       attendees, transcript -> decisions, commitments, open questions.
//   notes_interview_overview / notes_interview_items   the same for an interview: questions and answers, then
//                     key facts and quotes.
//   notes_talk_overview / notes_talk_items             the same for a talk: its outline, then key concepts and
//                     takeaways.
//   notes_questions   attendees, questions the meeting could not answer, transcript -> for each, whether it was a real
//                     question and a clearer way to ask it.
//   decision_changes  new_decisions, past_decisions (numbered) -> which past decisions this meeting changed.
//
// Tacie AI in the main window
//   panel_chat        today, scope, context, question -> the first message of a tool-using chat (search meetings and
//                     connected services, edit notes); `panel_chat_iphone` is the iPhone app's version.
//   panel_prep        items (left from past meetings), passages -> what to prepare for the next meeting.
//   panel_why         decision, window (the talk around it) -> why the decision was made.
//   panel_followup    prepared, ledger -> which prepared items the new meeting settled.
//
// Name endings
//   _en   the English text, for meetings that are not in Korean (it answers in the meeting's language).
//   _in   an English text that writes in `{language}` (Settings > Meeting > notes language).
//   _ask  the same text plus `{request}`: the person's own instruction when they rewrite the notes.
//
// Jev (`table()["jev"]`): the questions sent with each line to decide whether it asks to look something up and
// whether it ends the meeting (`gate`, `gate_en`), and the question per passage when reranking search results.
// Tools (`table()["tools"]`): the tool sets `panel_chat` may call.

/// Names old apps (0.1.21–0.1.23) still send to the relay: `notes_overview` and the one-call `ledger`. This engine
/// never asks for them, so their texts live only in the relay's copy (`apps/server/src/prompts.json`, not in the
/// open-source repository); the `prompts_json` test keeps them there when it rewrites the copy.
pub const RELAY_ONLY: &[&str] = &["notes_overview", "ledger"];

/// 이름의 글이에요.
#[cfg(feature = "local-prompts")]
pub fn template(name: &str) -> &'static str {
    match name {
        "answer" => crate::openrouter::ANSWER_PROMPT,
        "notes_overview_v2" => crate::openrouter::OVERVIEW_PROMPT,
        "notes_items" => crate::openrouter::ITEMS_PROMPT,
        "notes_outline" => crate::notes_v2::OUTLINE_PROMPT,
        "notes_topic" => crate::notes_v2::TOPIC_PROMPT,
        "notes_items_v2" => crate::notes_v2::ITEMS_V2_PROMPT,
        "notes_summary" => crate::notes_v2::SUMMARY_PROMPT,
        "notes_classify" => crate::openrouter::CLASSIFY_PROMPT,
        "notes_questions" => crate::openrouter::QUESTIONS_PROMPT,
        "notes_interview_overview" => crate::openrouter::INTERVIEW_OVERVIEW_PROMPT,
        "notes_interview_items" => crate::openrouter::INTERVIEW_ITEMS_PROMPT,
        "notes_talk_overview" => crate::openrouter::TALK_OVERVIEW_PROMPT,
        "notes_talk_items" => crate::openrouter::TALK_ITEMS_PROMPT,
        "readings" => crate::terms::READINGS_PROMPT,
        "decision_changes" => crate::openrouter::DECISION_CHANGES_PROMPT,
        "answer_en" => crate::prompts_en::ANSWER_PROMPT_EN,
        "notes_overview_v2_en" => crate::prompts_en::OVERVIEW_PROMPT_EN,
        "notes_items_en" => crate::prompts_en::ITEMS_PROMPT_EN,
        "notes_outline_en" => crate::prompts_en::OUTLINE_PROMPT_EN,
        "notes_topic_en" => crate::prompts_en::TOPIC_PROMPT_EN,
        "notes_items_v2_en" => crate::prompts_en::ITEMS_V2_PROMPT_EN,
        "notes_summary_en" => crate::prompts_en::SUMMARY_PROMPT_EN,
        "notes_questions_en" => crate::prompts_en::QUESTIONS_PROMPT_EN,
        "notes_interview_overview_en" => crate::prompts_en::INTERVIEW_OVERVIEW_PROMPT_EN,
        "notes_interview_items_en" => crate::prompts_en::INTERVIEW_ITEMS_PROMPT_EN,
        "notes_talk_overview_en" => crate::prompts_en::TALK_OVERVIEW_PROMPT_EN,
        "notes_talk_items_en" => crate::prompts_en::TALK_ITEMS_PROMPT_EN,
        "panel_prep" => crate::openrouter::PANEL_PREP_PROMPT,
        "panel_why" => crate::openrouter::PANEL_WHY_PROMPT,
        "panel_followup" => crate::openrouter::PANEL_FOLLOWUP_PROMPT,
        "panel_prep_en" => crate::prompts_en::PANEL_PREP_PROMPT_EN,
        "panel_why_en" => crate::prompts_en::PANEL_WHY_PROMPT_EN,
        "panel_followup_en" => crate::prompts_en::PANEL_FOLLOWUP_PROMPT_EN,
        "panel_chat" => crate::openrouter::PANEL_CHAT_PROMPT,
        "panel_chat_en" => crate::prompts_en::PANEL_CHAT_PROMPT_EN,
        "panel_chat_iphone" => crate::openrouter::PANEL_CHAT_IPHONE_PROMPT,
        "panel_chat_iphone_en" => crate::prompts_en::PANEL_CHAT_IPHONE_PROMPT_EN,
        _ => asked(name).unwrap_or_else(|| panic!("no prompt named {name}")),
    }
}

/// 회의 정리를 사용자 요청대로 다시 쓸 때의 이름이에요(패널의 `rewrite_notes`, docs/38 12절): 정리 프롬프트마다 뒤에
/// `_ask`가 붙고, 값 `request`가 하나 더 있어요. 글은 원래 글 뒤에 요청 단락을 붙인 거예요. 원래 이름은 그대로 두어서
/// 배포된 엔진이 보내는 요청은 바뀌지 않아요.
pub const ASK_NAMES: [(&str, &str); 12] = [
    ("notes_overview_v2", "notes_overview_v2_ask"),
    ("notes_items", "notes_items_ask"),
    ("notes_interview_overview", "notes_interview_overview_ask"),
    ("notes_interview_items", "notes_interview_items_ask"),
    ("notes_talk_overview", "notes_talk_overview_ask"),
    ("notes_talk_items", "notes_talk_items_ask"),
    ("notes_overview_v2_en", "notes_overview_v2_en_ask"),
    ("notes_items_en", "notes_items_en_ask"),
    ("notes_interview_overview_en", "notes_interview_overview_en_ask"),
    ("notes_interview_items_en", "notes_interview_items_en_ask"),
    ("notes_talk_overview_en", "notes_talk_overview_en_ask"),
    ("notes_talk_items_en", "notes_talk_items_en_ask"),
];

/// 정리 프롬프트 이름의 `_ask` 짝이에요. 없으면 그대로예요.
pub fn ask_name(name: &'static str) -> &'static str {
    ASK_NAMES.iter().find(|(b, _)| *b == name).map(|(_, a)| *a).unwrap_or(name)
}

/// 회의 정리를 정한 한 언어로 쓸 때의 이름이에요(2026-11-19). 영어판 정리 프롬프트 뒤에 `_in`이 붙고, 값 `language`
/// (영어 이름, 예: "English", "Japanese")가 하나 더 있어요. 글은 원래 글 뒤에 "모든 글을 그 언어로" 단락을 붙인 거예요.
/// 두 군데에서 써요: 설정 → 회의 → "정리 언어"가 "앱 언어로"이고 앱이 영어일 때("English"), 그리고 한국어가 아닌 회의를
/// 회의 언어대로 쓸 때 안건 나누기가 고른 회의 언어를 다음 단계에 넘길 때예요. 단계마다 그 구간만 보고 언어를 따로
/// 고르면 한국어·영어·일본어가 섞인 회의의 정리가 주제마다 다른 언어가 됐어요. 한국어로 쓸 때는 한국어 프롬프트예요.
pub const IN_NAMES: [(&str, &str); 10] = [
    ("notes_outline_en", "notes_outline_en_in"),
    ("notes_topic_en", "notes_topic_en_in"),
    ("notes_items_v2_en", "notes_items_v2_en_in"),
    ("notes_summary_en", "notes_summary_en_in"),
    ("notes_overview_v2_en", "notes_overview_v2_en_in"),
    ("notes_items_en", "notes_items_en_in"),
    ("notes_interview_overview_en", "notes_interview_overview_en_in"),
    ("notes_interview_items_en", "notes_interview_items_en_in"),
    ("notes_talk_overview_en", "notes_talk_overview_en_in"),
    ("notes_talk_items_en", "notes_talk_items_en_in"),
];

/// 영어판 정리 프롬프트의 `_in` 짝이에요. 없으면 그대로예요.
pub fn in_name(name: &'static str) -> &'static str {
    IN_NAMES.iter().find(|(b, _)| *b == name).map(|(_, a)| *a).unwrap_or(name)
}

#[cfg(feature = "local-prompts")]
const IN_LANG: &str = "\n\nWrite every text value in {language}, even where the meeting, or the part of it you see, was held in another language or mixes languages, and set \"language\" to \"{language}\" where the format has it. Keep people's names and quoted terms as they are. This comes before the language rules above.";

#[cfg(feature = "local-prompts")]
const ASK_KO: &str = "\n\n<사용자 요청>\n{request}\n</사용자 요청>\n이번 정리는 회의를 기록한 사람이 위 요청을 하고 다시 쓰는 거예요. 위 규칙과 JSON 형식은 그대로 지키고, 요청에 맞춰 무엇을 담을지와 길이를 바꿔요. 더 자세히 써 달라는 요청이면 주제 10개, 주제마다 문장 5개까지 늘려도 돼요. 요청이 회의에 없는 내용을 넣으라는 것이면 그 부분은 따르지 않아요. 요청이 정리를 쓸 언어를 정하면(예: \"영어로\", \"in English\") 모든 글 값을 그 언어로 써요. 위에서 정한 언어보다 요청이 먼저예요.";
#[cfg(feature = "local-prompts")]
const ASK_EN: &str = "\n\n<user_request>\n{request}\n</user_request>\nThis time the person who recorded the meeting asked for the notes again with the request above. Keep the rules and the JSON format above, and change what you include and how long it is to fit the request. If they ask for more detail, you may go up to 10 topics and 5 sentences per topic. If the request asks for anything that was not in the meeting, do not follow that part. If the request names a language for the notes (e.g. \"in Korean\", \"한국어로\", \"한글로\"), write every text value in that language and set \"language\" to it; this comes before the language rules above.";

#[cfg(feature = "local-prompts")]
fn asked(name: &str) -> Option<&'static str> {
    use std::collections::HashMap;
    use std::sync::LazyLock;
    static TEXTS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
        ASK_NAMES
            .iter()
            .map(|&(base, ask)| {
                let tail = if base.ends_with("_en") { ASK_EN } else { ASK_KO };
                (ask, &*Box::leak(format!("{}{tail}", template(base)).into_boxed_str()))
            })
            .chain(
                IN_NAMES
                    .iter()
                    .map(|&(base, named)| (named, &*Box::leak(format!("{}{IN_LANG}", template(base)).into_boxed_str()))),
            )
            .collect()
    });
    TEXTS.get(name).copied()
}

/// 이름마다 채울 값의 이름이에요(글에 나오는 순서).
pub const CHAT_FIELDS: [(&str, &[&str]); 37] = [
    ("answer", &["context", "question", "passages"]),
    ("notes_overview_v2", &["attendees", "transcript"]),
    ("notes_items", &["attendees", "transcript"]),
    ("notes_outline", &["attendees", "transcript"]),
    ("notes_topic", &["title", "attendees", "transcript"]),
    ("notes_items_v2", &["attendees", "agenda", "transcript"]),
    ("notes_summary", &["attendees", "notes"]),
    ("notes_classify", &["transcript"]),
    ("notes_questions", &["attendees", "questions", "transcript"]),
    ("notes_interview_overview", &["attendees", "transcript"]),
    ("notes_interview_items", &["attendees", "transcript"]),
    ("notes_talk_overview", &["attendees", "transcript"]),
    ("notes_talk_items", &["attendees", "transcript"]),
    ("readings", &["terms"]),
    ("decision_changes", &["new_decisions", "past_decisions"]),
    ("answer_en", &["context", "question", "passages"]),
    ("notes_overview_v2_en", &["attendees", "transcript"]),
    ("notes_items_en", &["attendees", "transcript"]),
    ("notes_outline_en", &["attendees", "transcript"]),
    ("notes_topic_en", &["title", "attendees", "transcript"]),
    ("notes_items_v2_en", &["attendees", "agenda", "transcript"]),
    ("notes_summary_en", &["attendees", "notes"]),
    (
        "notes_questions_en",
        &["attendees", "questions", "transcript"],
    ),
    ("notes_interview_overview_en", &["attendees", "transcript"]),
    ("notes_interview_items_en", &["attendees", "transcript"]),
    ("notes_talk_overview_en", &["attendees", "transcript"]),
    ("notes_talk_items_en", &["attendees", "transcript"]),
    ("panel_prep", &["items", "passages"]),
    ("panel_why", &["decision", "window"]),
    ("panel_followup", &["prepared", "ledger"]),
    ("panel_prep_en", &["items", "passages"]),
    ("panel_why_en", &["decision", "window"]),
    ("panel_followup_en", &["prepared", "ledger"]),
    ("panel_chat", &["today", "scope", "context", "question"]),
    ("panel_chat_en", &["today", "scope", "context", "question"]),
    ("panel_chat_iphone", &["today", "scope", "context", "question"]),
    ("panel_chat_iphone_en", &["today", "scope", "context", "question"]),
];

/// `prompts.json`에 쓰는 내용 전부예요: 답 모델의 글과 Jev 질문이에요. 리랭크 질문은 조각 하나의 틀이고, 중계 서버가
/// `{k}`를 0부터 조각 수만큼 바꿔 늘어놓아요.
#[cfg(feature = "local-prompts")]
pub fn table() -> Value {
    let mut chat: Map<String, Value> = CHAT_FIELDS
        .iter()
        .map(|&(name, fields)| {
            (
                name.to_string(),
                json!({"fields": fields, "text": template(name)}),
            )
        })
        .collect();
    for (base, ask) in ASK_NAMES {
        let fields = CHAT_FIELDS.iter().find(|(n, _)| *n == base).map(|(_, f)| f.to_vec()).unwrap_or_default();
        let mut fields: Vec<&str> = fields;
        fields.push("request");
        chat.insert(ask.to_string(), json!({"fields": fields, "text": template(ask)}));
    }
    for (base, named) in IN_NAMES {
        let mut fields: Vec<&str> = CHAT_FIELDS.iter().find(|(n, _)| *n == base).map(|(_, f)| f.to_vec()).unwrap_or_default();
        fields.push("language");
        chat.insert(named.to_string(), json!({"fields": fields, "text": template(named)}));
    }
    json!({
        "chat": chat,
        "jev": {
            "gate": crate::jev::gate_questions(),
            "gate_en": crate::jev::gate_questions_en(),
            "rerank": crate::jev::rerank_question("{k}"),
        },
        "tools": crate::panel_tools::sets(),
    })
}

#[cfg(all(test, feature = "local-prompts"))]
mod ask_tests {
    use super::*;

    #[test]
    fn asked_prompts_are_the_notes_prompts_with_a_request() {
        for (base, ask) in ASK_NAMES {
            let t = template(ask);
            assert!(t.starts_with(template(base)), "{ask}");
            assert!(t.contains("{request}"), "{ask}");
            assert_eq!(ask_name(base), ask);
        }
        assert_eq!(ask_name("notes_classify"), "notes_classify");
        assert!(table()["chat"]["notes_items_ask"]["fields"].as_array().unwrap().iter().any(|f| f == "request"));
    }

    #[test]
    fn in_language_prompts_are_the_english_notes_prompts_with_the_language_rule() {
        for (base, named) in IN_NAMES {
            let t = template(named);
            assert!(t.starts_with(template(base)), "{named}");
            assert!(t.ends_with(IN_LANG) && t.contains("{language}"), "{named}");
            assert_eq!(in_name(base), named);
            assert_eq!(table()["chat"][named]["fields"].as_array().unwrap().last().unwrap(), "language", "{named}");
        }
        assert_eq!(in_name("notes_outline"), "notes_outline");
    }
}
