//! Meeting language. Non-Korean meetings (English, Japanese, Spanish …) write answers and meeting notes with the
//! English prompts (`prompts_en`), and those prompts say to write in the meeting language. Korean meetings use the
//! current text as is.
//! The languages a meeting may use (`speech_langs`) come from the app as `TACIE_SPEECH_LANGS`: Korean, English and
//! the languages added on the Mac.

use std::sync::OnceLock;

/// 답과 회의 정리를 쓸 프롬프트예요. `En`은 영어 회의만이 아니라 한국어가 아닌 회의 모두예요(프롬프트 글은 영어이고
/// 답은 회의 언어로 써요).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Ko,
    En,
}

/// 회의 정리를 쓸 언어로 앱이 보낸 값이에요(설정 → 회의 → "회의 정리 언어"가 "앱 언어로"일 때, 2026-11-19): `ko`나 `en`.
/// 다른 값과 빈 값은 없는 것(회의 언어대로)이에요.
pub fn notes_target(code: Option<&str>) -> Option<Lang> {
    match code.map(|c| c.trim().to_ascii_lowercase()).as_deref() {
        Some("ko") => Some(Lang::Ko),
        Some("en") => Some(Lang::En),
        _ => None,
    }
}

/// 회의에서 쓸 수 있는 말의 언어 코드예요(`ko`, `en`, `ja` …). 앱이 엔진을 띄울 때 `TACIE_SPEECH_LANGS=ko,en,ja`로
/// 넘겨요. 값이 없으면(예전 앱, 개발 도구) 한국어·영어예요.
pub fn speech_langs() -> &'static [String] {
    static LANGS: OnceLock<Vec<String>> = OnceLock::new();
    LANGS.get_or_init(|| parse_speech_langs(std::env::var("TACIE_SPEECH_LANGS").ok().as_deref()))
}

/// The Mac's first language (`TACIE_MAC_LANG`, an ISO 639-1 code the app sends), if the app sent one.
pub fn mac_lang() -> Option<&'static str> {
    static LANG: OnceLock<Option<String>> = OnceLock::new();
    LANG.get_or_init(|| std::env::var("TACIE_MAC_LANG").ok().map(|v| v.trim().to_ascii_lowercase()).filter(|v| !v.is_empty()))
        .as_deref()
}

/// The language this Mac's whisper is told. A Korean Mac (or an app that did not say) keeps `ko`, as before; any other
/// Mac lets whisper detect the language, so an English meeting is not written as Korean. Detecting costs about 0.7 s
/// per utterance and changed no Korean text in the 2026-11-24 measurement.
pub fn whisper_language(mac: Option<&str>) -> Option<String> {
    match mac.map(str::trim) {
        None | Some("") | Some("ko") => Some("ko".to_string()),
        Some(_) => None,
    }
}

/// `ko,en,ja` 꼴을 읽어요. 한국어·영어는 늘 들어 있어요.
pub fn parse_speech_langs(value: Option<&str>) -> Vec<String> {
    let mut out = vec!["ko".to_string(), "en".to_string()];
    for code in value.unwrap_or("").split(',') {
        let code = code.trim().to_ascii_lowercase();
        if !code.is_empty() && code.chars().all(|c| c.is_ascii_lowercase()) && !out.contains(&code) {
            out.push(code);
        }
    }
    out
}

/// 한국어·영어 밖의 말도 오는 회의인지예요.
pub fn beyond_korean_english(langs: &[String]) -> bool {
    langs.iter().any(|l| l != "ko" && l != "en")
}

fn is_hangul(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c) || ('\u{3131}'..='\u{318E}').contains(&c)
}

/// 글의 언어예요. 한국어 회의에도 영어 용어가 많이 섞여서, 한글이 글자의 10%도 안 되고 한글이 아닌 글자(로마자,
/// 가나, 한자, 키릴 …)가 20자 이상일 때만 한국어가 아닌 회의예요. 가나와 한자는 한 글자에 뜻이 많이 들어서 세 글자로
/// 세요("检索时先取多少个候选？"는 10자예요). 글자가 거의 없으면 한국어(지금까지와 같아요)예요.
pub fn of(text: &str) -> Lang {
    let (ko, other) = count(text);
    if other >= 20 && ko * 10 < ko + other {
        Lang::En
    } else {
        Lang::Ko
    }
}

/// 답의 언어예요. 지금 질문으로 정하고, 앞 답(Tacie가 쓴 글)은 보지 않아요: 한국어 회의를 영어로 묻는 대화에서 앞 답이
/// 한국어라 답이 한국어로 바뀌던 일이 있었어요. 질문이 짧아 글자로 정하기 어려우면(8자 미만, 가나·한자는 세 글자) 앞
/// 질문들까지 같이 봐요. 앞 대화 한 줄은 `나: 질문 / Tacie: 답`(메인 창 패널)이나 회의 발화예요.
pub fn of_question(question: &str, context: &[String]) -> Lang {
    let (ko, other) = count(question);
    if ko + other >= 8 {
        return if other > 0 && ko * 10 < ko + other { Lang::En } else { Lang::Ko };
    }
    let asked: Vec<&str> = context.iter().map(|l| asked_part(l)).collect();
    of(&format!("{question}\n{}", asked.join("\n")))
}

/// 앞 대화 한 줄에서 사람이 한 말만 남겨요(`나: ` 머리와 ` / Tacie: ` 뒤의 답을 떼요).
fn asked_part(line: &str) -> &str {
    let line = line.strip_prefix("나: ").unwrap_or(line);
    line.split(" / Tacie: ").next().unwrap_or(line)
}

/// (한글 글자 수, 그 밖의 글자 수)예요. 가나와 한자는 세 글자로 세요.
fn count(text: &str) -> (usize, usize) {
    let (mut ko, mut other) = (0usize, 0usize);
    for c in text.chars() {
        if is_hangul(c) {
            ko += 1;
        } else if is_cjk(c) {
            other += 3;
        } else if c.is_alphabetic() {
            other += 1;
        }
    }
    (ko, other)
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30FF}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
}

/// 언어에 맞는 프롬프트 이름이에요. 영어판이 없는 이름(분류, 용어 읽기)은 그대로예요.
pub fn prompt_name(name: &'static str, lang: Lang) -> &'static str {
    if lang == Lang::Ko {
        return name;
    }
    match name {
        "answer" => "answer_en",
        "panel_prep" => "panel_prep_en",
        "panel_why" => "panel_why_en",
        "panel_followup" => "panel_followup_en",
        "panel_chat" => "panel_chat_en",
        "panel_chat_iphone" => "panel_chat_iphone_en",
        "notes_overview_v2" => "notes_overview_v2_en",
        "notes_items" => "notes_items_en",
        "notes_outline" => "notes_outline_en",
        "notes_topic" => "notes_topic_en",
        "notes_items_v2" => "notes_items_v2_en",
        "notes_summary" => "notes_summary_en",
        "notes_questions" => "notes_questions_en",
        "notes_interview_overview" => "notes_interview_overview_en",
        "notes_interview_items" => "notes_interview_items_en",
        "notes_talk_overview" => "notes_talk_overview_en",
        "notes_talk_items" => "notes_talk_items_en",
        other => other,
    }
}

/// 글자 수 한도예요. 영어는 같은 내용에 글자가 두 배쯤 들어서, 한글이 하나도 없는 글은 두 배까지 받아요.
pub fn cap(max: usize, text: &str) -> usize {
    if text.chars().any(is_hangul) {
        max
    } else {
        max * 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_keeps_korean_on_a_korean_mac_and_detects_elsewhere() {
        // measured 2026-11-24 (FLEURS en_us 40, ko_kr 40, Korean meeting talk 40 + 2.5 s cuts): telling whisper `ko`
        // made English 8.0% WER (one clip translated to Korean), detecting it 6.6%; Korean text was identical either
        // way, but detecting costs about 0.7 s per utterance, so a Korean Mac keeps `ko`
        assert_eq!(whisper_language(Some("ko")), Some("ko".to_string()));
        assert_eq!(whisper_language(None), Some("ko".to_string()));
        assert_eq!(whisper_language(Some("")), Some("ko".to_string()));
        assert_eq!(whisper_language(Some("en")), None);
        assert_eq!(whisper_language(Some("ja")), None);
    }

    #[test]
    fn the_question_sets_the_answer_language_not_earlier_answers() {
        let history = vec![
            "나: What's still undecided from this meeting? / Tacie: 찾은 자료에는 답이 없어요.".to_string(),
            "나: Who raised the deposit cap? / Tacie: 보증금 상한은 국토교통부가 5억 원으로 올리자고 했어요.".to_string(),
        ];
        assert_eq!(of_question("What's still undecided from this meeting?", &history), Lang::En);
        // a short question goes by the earlier questions, not the Korean answers
        assert_eq!(of_question("Who objected?", &history), Lang::En);
        assert_eq!(of_question("누가 반대했어?", &["나: 이 회의에서 뭐 정했어? / Tacie: Retry is three.".to_string()]), Lang::Ko);
        assert_eq!(of_question("그럼 retry policy는 config.ts에서 몇 번이야?", &history), Lang::Ko);
        assert_eq!(of_question("Who objected?", &[]), Lang::En);
        assert_eq!(of_question("왜?", &[]), Lang::Ko);
    }

    #[test]
    fn korean_meetings_with_english_terms_stay_korean() {
        assert_eq!(of("그럼 retry policy는 config.ts에서 maxAttempts로 3번까지 하기로 해요"), Lang::Ko);
        assert_eq!(of("PR 올렸어요"), Lang::Ko);
        assert_eq!(of(""), Lang::Ko);
        assert_eq!(of("OK"), Lang::Ko);
    }

    #[test]
    fn english_meetings_are_english() {
        assert_eq!(of("What's the default retry count? I think it's three, let me check."), Lang::En);
    }

    #[test]
    fn other_languages_are_not_korean() {
        assert_eq!(of("デフォルトのリトライ回数は何回でしたっけ？三回だったと思います。"), Lang::En);
        assert_eq!(of("默认的重试次数是多少？我记得是三次，我查一下。"), Lang::En);
        assert_eq!(of("检索时先取多少个候选？"), Lang::En);
        assert_eq!(of("リトライは何回？"), Lang::En);
        assert_eq!(of("¿Cuántos reintentos hay por defecto? Creo que son tres."), Lang::En);
        assert_eq!(of("Сколько повторов по умолчанию? Кажется, три."), Lang::En);
        // 한국어 회의에 일본어 한 마디가 섞여도 한국어예요
        assert_eq!(of("그럼 다음 주에 배포하기로 하고, 일본 팀에는 ありがとう라고 전해 주세요. 리뷰는 제가 볼게요."), Lang::Ko);
    }

    #[test]
    fn speech_langs_always_hold_korean_and_english() {
        assert_eq!(parse_speech_langs(None), ["ko", "en"]);
        assert_eq!(parse_speech_langs(Some("ja, EN,ko,zh,,x1")), ["ko", "en", "ja", "zh"]);
        assert!(!beyond_korean_english(&parse_speech_langs(Some("en,ko"))));
        assert!(beyond_korean_english(&parse_speech_langs(Some("ja"))));
    }

    #[test]
    fn names_switch_only_for_english() {
        assert_eq!(prompt_name("answer", Lang::Ko), "answer");
        assert_eq!(prompt_name("answer", Lang::En), "answer_en");
        assert_eq!(prompt_name("notes_classify", Lang::En), "notes_classify");
        assert_eq!(prompt_name("notes_outline", Lang::En), "notes_outline_en");
        assert_eq!(prompt_name("notes_summary", Lang::Ko), "notes_summary");
    }

    #[test]
    fn caps_double_only_without_hangul() {
        assert_eq!(cap(30, "결제 실패 재시도 정책"), 30);
        assert_eq!(cap(30, "Retry policy for failed payments"), 60);
    }
}
