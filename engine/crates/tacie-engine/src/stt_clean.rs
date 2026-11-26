//! Removes characters outside the meeting languages from transcribed text. Both server and on-device transcription
//! pass through it (`session`).
//!
//! Even when the language is sent as `ko`, a Korean meeting sometimes comes back with Arabic, Cyrillic or Han
//! characters mixed in ("你？", 2026-11-02).
//! Korean meetings accept only Hangul and Latin letters. Terms and English sentences are Latin, so they stay.
//! Characters of the meeting languages the app passes (`lang::speech_langs`, languages added on the Mac) are
//! accepted too: on a Mac with Japanese added, kana and Han characters stay (2026-11-14).
//! If more than half of a line's characters are outside, the line is dropped; otherwise only the words containing
//! outside characters are removed.
//! The word list (`words`) is trimmed along with the text. Speaker splitting and spoken-span calculation use the
//! word times.

use crate::services::Transcript;

/// 글을 어떻게 고쳤는지예요. 발화 기록의 `stt_cleaned` 값이에요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleaned {
    /// 밖의 글자가 절반을 넘어서 줄을 통째로 버렸어요.
    Dropped,
    /// 밖의 글자가 든 단어만 뺐어요.
    Trimmed,
}

impl Cleaned {
    pub fn as_str(self) -> &'static str {
        match self {
            Cleaned::Dropped => "foreign_dropped",
            Cleaned::Trimmed => "foreign_trimmed",
        }
    }
}

/// 받아쓴 글을 고쳐요. 밖의 글자가 없으면 `None`이고 글은 그대로예요. 줄을 버리면 글과 단어가 비어요.
pub fn clean(t: &mut Transcript) -> Option<Cleaned> {
    clean_with(t, crate::lang::speech_langs())
}

/// `clean`과 같고, 회의 언어(`ko`, `en`, `ja` …)를 받아요.
pub fn clean_with(t: &mut Transcript, langs: &[String]) -> Option<Cleaned> {
    let is_foreign = |c: char| is_foreign(c, langs);
    // 소리 표시(`[배경 소음]`) 안의 글자는 세지 않아요: 말이 아니고, 소리 표시는 따로 다뤄요(`notes::is_sound_only`).
    let spoken = outside_brackets(&t.text);
    let (letters, foreign) = spoken
        .chars()
        .filter(|c| c.is_alphabetic())
        .fold((0usize, 0usize), |(l, f), c| {
            (l + 1, f + usize::from(is_foreign(c)))
        });
    if foreign == 0 {
        return None;
    }
    let kept: Vec<&str> = t
        .text
        .split_whitespace()
        .filter(|w| !w.chars().any(is_foreign))
        .collect();
    let kept = kept.join(" ");
    if foreign * 2 > letters || !kept.chars().any(char::is_alphanumeric) {
        t.text.clear();
        t.words.clear();
        return Some(Cleaned::Dropped);
    }
    t.text = kept;
    t.words.retain(|w| !w.text.chars().any(is_foreign));
    Some(Cleaned::Trimmed)
}

fn outside_brackets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0u32;
    for c in text.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// 회의 밖의 글자면 true예요. 한글(음절·자모)과 로마자(악센트 포함)는 언제나 회의 안이고, 그 밖의 글자는 회의 언어의
/// 글자일 때만 안이에요(`script_of`). 숫자와 문장부호는 글자가 아니라서 언제나 회의 안이에요.
fn is_foreign(c: char, langs: &[String]) -> bool {
    if !c.is_alphabetic() {
        return false;
    }
    let hangul = matches!(c,
        '\u{AC00}'..='\u{D7A3}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}'
        | '\u{A960}'..='\u{A97F}' | '\u{D7B0}'..='\u{D7FF}');
    let latin =
        c.is_ascii_alphabetic() || matches!(c, '\u{00C0}'..='\u{024F}' | '\u{1E00}'..='\u{1EFF}');
    if hangul || latin {
        return false;
    }
    !langs.iter().any(|l| script_of(l).iter().any(|s| s.contains(c)))
}

/// 글자 묶음이에요(유니코드 범위).
struct Script(&'static [(char, char)]);

impl Script {
    fn contains(&self, c: char) -> bool {
        self.0.iter().any(|&(a, b)| (a..=b).contains(&c))
    }
}

const HAN: Script = Script(&[
    ('\u{3005}', '\u{3007}'),
    ('\u{3021}', '\u{3029}'),
    ('\u{3400}', '\u{4DBF}'),
    ('\u{4E00}', '\u{9FFF}'),
    ('\u{F900}', '\u{FAFF}'),
    ('\u{20000}', '\u{3134F}'),
]);
const KANA: Script = Script(&[
    ('\u{3040}', '\u{30FF}'),
    ('\u{31F0}', '\u{31FF}'),
    ('\u{FF66}', '\u{FF9F}'),
]);
const BOPOMOFO: Script = Script(&[('\u{3100}', '\u{312F}'), ('\u{31A0}', '\u{31BF}')]);
const CYRILLIC: Script = Script(&[('\u{0400}', '\u{052F}')]);
const GREEK: Script = Script(&[('\u{0370}', '\u{03FF}'), ('\u{1F00}', '\u{1FFF}')]);
const ARABIC: Script = Script(&[
    ('\u{0600}', '\u{06FF}'),
    ('\u{0750}', '\u{077F}'),
    ('\u{FB50}', '\u{FDFF}'),
    ('\u{FE70}', '\u{FEFF}'),
]);
const HEBREW: Script = Script(&[('\u{0590}', '\u{05FF}')]);
const DEVANAGARI: Script = Script(&[('\u{0900}', '\u{097F}')]);
const BENGALI: Script = Script(&[('\u{0980}', '\u{09FF}')]);
const GURMUKHI: Script = Script(&[('\u{0A00}', '\u{0A7F}')]);
const GUJARATI: Script = Script(&[('\u{0A80}', '\u{0AFF}')]);
const TAMIL: Script = Script(&[('\u{0B80}', '\u{0BFF}')]);
const TELUGU: Script = Script(&[('\u{0C00}', '\u{0C7F}')]);
const KANNADA: Script = Script(&[('\u{0C80}', '\u{0CFF}')]);
const MALAYALAM: Script = Script(&[('\u{0D00}', '\u{0D7F}')]);
const THAI: Script = Script(&[('\u{0E00}', '\u{0E7F}')]);

/// 언어 코드가 쓰는 글자 묶음이에요. 로마자를 쓰는 언어(스페인어, 독일어, 베트남어 …)는 로마자가 언제나 회의 안이라
/// 더할 게 없어요.
fn script_of(lang: &str) -> &'static [&'static Script] {
    match lang {
        "ja" => &[&KANA, &HAN],
        "zh" => &[&HAN, &BOPOMOFO],
        "ru" | "uk" | "be" | "bg" | "mk" | "sr" | "kk" => &[&CYRILLIC],
        "el" => &[&GREEK],
        "ar" | "fa" | "ur" => &[&ARABIC],
        "he" => &[&HEBREW],
        "hi" | "mr" => &[&DEVANAGARI],
        "bn" => &[&BENGALI],
        "pa" => &[&GURMUKHI],
        "gu" => &[&GUJARATI],
        "ta" => &[&TAMIL],
        "te" => &[&TELUGU],
        "kn" => &[&KANNADA],
        "ml" => &[&MALAYALAM],
        "th" => &[&THAI],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{SpokenWord, SttVia};

    fn t(text: &str, words: &[&str]) -> Transcript {
        Transcript {
            text: text.into(),
            ms: 0.0,
            cost: 0.0,
            confidence: None,
            words: words
                .iter()
                .enumerate()
                .map(|(i, w)| SpokenWord {
                    text: (*w).into(),
                    start: i as f64,
                    end: i as f64 + 0.5,
                })
                .collect(),
            via: SttVia::Server,
        }
    }

    fn clean(t: &mut Transcript) -> Option<Cleaned> {
        clean_with(t, &crate::lang::parse_speech_langs(None))
    }

    fn texts(t: &Transcript) -> Vec<&str> {
        t.words.iter().map(|w| w.text.as_str()).collect()
    }

    #[test]
    fn korean_english_numbers_and_sound_tags_are_untouched() {
        for s in [
            "OpenRouter(오픈라우터)에서 보내요",
            "PR 올렸어요, review 부탁해요",
            "café 이름이 뭐였죠?",
            "3시에 봐요! 50%요",
            "[배경 소음]",
            "[헛기침] 안녕하세요. 음",
            "ㅋㅋ 네",
        ] {
            let mut x = t(s, &["a"]);
            assert_eq!(clean(&mut x), None, "{s}");
            assert_eq!(x.text, s);
            assert_eq!(x.words.len(), 1);
        }
    }

    #[test]
    fn mostly_foreign_line_is_dropped() {
        for s in [
            "你？",
            "مرحبا بكم",
            "Привет всем",
            "ありがとう",
            "مرحبا 네",
            "ok 謝謝你們大家",
        ] {
            let mut x = t(s, &["x"]);
            assert_eq!(clean(&mut x), Some(Cleaned::Dropped), "{s}");
            assert!(x.text.is_empty() && x.words.is_empty());
        }
    }

    #[test]
    fn foreign_words_inside_korean_are_removed() {
        let mut x = t(
            "그럼 배포는 مرحبا 내일 하죠",
            &["그럼", "배포는", "مرحبا", "내일", "하죠"],
        );
        assert_eq!(clean(&mut x), Some(Cleaned::Trimmed));
        assert_eq!(x.text, "그럼 배포는 내일 하죠");
        assert_eq!(texts(&x), ["그럼", "배포는", "내일", "하죠"]);
    }

    #[test]
    fn a_foreign_word_glued_to_korean_takes_the_whole_word() {
        // 띄어쓰기 단위로 빼요: 붙어 나온 단어는 통째로 빠져요. 한 글자 섞인 긴 줄이 사라지지 않는 게 더 중요해요.
        let mut x = t("이건 你가 맡아 주세요", &[]);
        assert_eq!(clean(&mut x), Some(Cleaned::Trimmed));
        assert_eq!(x.text, "이건 맡아 주세요");
    }

    #[test]
    fn letters_of_the_macs_languages_stay() {
        let ja = crate::lang::parse_speech_langs(Some("ja"));
        for s in ["ありがとうございます", "デプロイは来週にしましょう", "PRを見てください", "その件は 다음 주에"] {
            let mut x = t(s, &["x"]);
            assert_eq!(clean_with(&mut x, &ja), None, "{s}");
            assert_eq!(x.text, s);
        }
        // 일본어를 넣은 Mac에도 아랍어는 회의 밖이에요
        let mut x = t("مرحبا بكم", &["x"]);
        assert_eq!(clean_with(&mut x, &ja), Some(Cleaned::Dropped));

        let ru = crate::lang::parse_speech_langs(Some("ru"));
        let mut x = t("Привет всем", &["x"]);
        assert_eq!(clean_with(&mut x, &ru), None);
        let mut x = t("你？", &["x"]);
        assert_eq!(clean_with(&mut x, &ru), Some(Cleaned::Dropped));
    }
}
