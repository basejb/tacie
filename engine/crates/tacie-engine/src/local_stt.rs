//! On-device speech recognition. Transcribes on this Mac with whisper.cpp (`whisper-rs`). Audio never leaves the
//! Mac.
//! Input is 16kHz mono s16 WAV (the same utterance WAV `services::Stt` takes).

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::services::{SpokenWord, SttVia, Transcript};

/// 모델을 어디서 돌릴지예요. 보통은 Metal이고, CPU는 Metal 컨텍스트와 나란히 쓸 때만 써요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Metal,
    Cpu,
}

/// 힌트 프롬프트의 토큰 상한이에요(whisper 프롬프트 한계의 절반 안쪽).
pub const HINT_MAX_TOKENS: usize = 200;
/// 용어가 하나도 없을 때의 프롬프트예요. 빈 프롬프트는 무음·잡음에서 "감사합니다."·"아"를 지어내서(실측 30개 중 20개) 쓰지 않아요.
const NEUTRAL_PROMPT: &str = "회의 대화예요.";
const HINT_PREFIX: &str = "회의 용어: ";
/// 세그먼트의 무음 확률이 이 값을 넘으면 말이 없었던 거예요.
const NO_SPEECH_THRESHOLD: f32 = 0.6;
const THREADS: i32 = 8;

pub struct LocalWhisper {
    ctx: WhisperContext,
    /// The language whisper is told (`ko` until now); None lets whisper detect it.
    language: Option<String>,
}

impl LocalWhisper {
    pub fn load(path: &Path, device: Device) -> Result<Self> {
        let mut params = WhisperContextParameters::default();
        params.use_gpu(device == Device::Metal);
        let path = path.to_str().context("모델 경로가 UTF-8이 아니에요")?;
        let ctx = WhisperContext::new_with_params(path, params)
            .map_err(|e| anyhow::anyhow!("받아쓰기 모델을 못 열었어요: {e}"))?;
        Ok(Self { ctx, language: Some("ko".into()) })
    }

    /// The language whisper is told; None lets whisper detect it per utterance.
    pub fn set_language(&mut self, language: Option<String>) {
        self.language = language;
    }

    /// 발화 WAV 하나를 받아써요. `prompt`는 용어 힌트(`hint_prompt`)예요. 비어 있으면 중립 프롬프트를 써요.
    /// 말이 없다고 보면(환각 포함) 빈 글을 돌려줘요.
    pub fn transcribe(&self, wav: &[u8], prompt: &str) -> Result<Transcript> {
        let started = Instant::now();
        let samples = wav_to_f32(wav)?;
        // 사실상 무음이면 모델에 안 보내요. whisper는 무음에서 "감사합니다" 같은 말을 지어내고,
        // 그때도 `no_speech_prob`은 낮게 나와서(실측 0.00002) 환각 필터로는 못 거르는 경우가 있어요.
        if is_silent(&samples) {
            return Ok(Transcript {
                text: String::new(),
                ms: 0.0,
                cost: 0.0,
                confidence: None,
                words: Vec::new(),
                via: SttVia::Local,
            });
        }
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| anyhow::anyhow!("받아쓰기 상태를 못 만들었어요: {e}"))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(self.language.as_deref());
        params.set_initial_prompt(if prompt.is_empty() {
            NEUTRAL_PROMPT
        } else {
            prompt
        });
        params.set_token_timestamps(true);
        params.set_temperature(0.0);
        // whisper.cpp 기본값대로 온도를 올려 다시 풀게 둬요. 0으로 막으면 같은 말을 끝없이 되풀이한 글
        // ("더욱더 더욱더 …")이 그대로 나와요(2026-11-17, 한국어 발표 510개에서 같은 모델을 쓰는 다른 앱과 차이 난 이유).
        params.set_temperature_inc(0.2);
        params.set_n_threads(THREADS);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        params.set_no_context(true);
        state
            .full(params, &samples)
            .map_err(|e| anyhow::anyhow!("받아쓰기에 실패했어요: {e}"))?;

        let mut text = String::new();
        let mut tokens: Vec<(Vec<u8>, f64, f64)> = Vec::new();
        let mut probs: Vec<f64> = Vec::new();
        let mut no_speech = 0f32;
        for seg in state.as_iter() {
            no_speech = no_speech.max(seg.no_speech_probability());
            text.push_str(&seg.to_str_lossy().unwrap_or_default());
            for j in 0..seg.n_tokens() {
                let Some(tok) = seg.get_token(j) else {
                    continue;
                };
                // 바이트 단위 BPE라 한글 한 글자가 토큰 둘에 걸칠 수 있어요. 글로 바꾸지 말고 바이트로 모아요.
                let Ok(piece) = tok.to_bytes() else { continue };
                let piece = piece.to_vec();
                // `[_BEG_]`, `<|ko|>` 같은 특수 토큰은 글이 아니에요.
                if piece.starts_with(b"[_") || piece.starts_with(b"<|") {
                    continue;
                }
                let d = tok.token_data();
                tokens.push((piece, d.t0 as f64 / 100.0, d.t1 as f64 / 100.0));
                probs.push(tok.token_probability() as f64);
            }
        }
        let text = text.trim().to_string();
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        if text.is_empty() || is_hallucination(&text, prompt, no_speech) {
            return Ok(Transcript {
                text: String::new(),
                ms,
                cost: 0.0,
                confidence: None,
                words: Vec::new(),
                via: SttVia::Local,
            });
        }
        let confidence =
            (!probs.is_empty()).then(|| probs.iter().sum::<f64>() / probs.len() as f64);
        Ok(Transcript {
            text,
            ms,
            cost: 0.0,
            confidence,
            words: group_tokens(&tokens),
            via: SttVia::Local,
        })
    }
}

/// 토큰을 띄어쓰기로 묶어 단어로 만들어요. 앞에 공백(0x20)이 붙은 토큰이 새 단어의 시작이에요. 시각은 초예요.
/// 바이트로 묶은 뒤에 글로 바꿔요: 한글 한 글자가 두 토큰에 걸쳐 있어도 깨지지 않아요.
fn group_tokens(tokens: &[(Vec<u8>, f64, f64)]) -> Vec<SpokenWord> {
    let mut groups: Vec<(Vec<u8>, f64, f64)> = Vec::new();
    for (piece, start, end) in tokens {
        match groups.last_mut() {
            Some(g) if piece.first() != Some(&b' ') => {
                g.0.extend_from_slice(piece);
                g.2 = *end;
            }
            _ => groups.push((piece.clone(), *start, *end)),
        }
    }
    groups
        .into_iter()
        .filter_map(|(bytes, start, end)| {
            let text = String::from_utf8_lossy(&bytes).trim().to_string();
            (!text.is_empty()).then_some(SpokenWord { text, start, end })
        })
        .collect()
}

/// 16kHz mono s16 WAV를 f32 표본으로 바꿔요.
fn wav_to_f32(wav: &[u8]) -> Result<Vec<f32>> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        bail!("WAV가 아니에요");
    }
    let mut pos = 12;
    let mut fmt_ok = false;
    while pos + 8 <= wav.len() {
        let id = &wav[pos..pos + 4];
        let size = u32::from_le_bytes(wav[pos + 4..pos + 8].try_into()?) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= wav.len() {
            let tag = u16::from_le_bytes(wav[body..body + 2].try_into()?);
            let channels = u16::from_le_bytes(wav[body + 2..body + 4].try_into()?);
            let rate = u32::from_le_bytes(wav[body + 4..body + 8].try_into()?);
            let bits = u16::from_le_bytes(wav[body + 14..body + 16].try_into()?);
            if tag != 1 || channels != 1 || rate != 16_000 || bits != 16 {
                bail!("16kHz mono s16 WAV만 받아요");
            }
            fmt_ok = true;
        } else if id == b"data" {
            if !fmt_ok {
                bail!("fmt 조각이 data보다 앞에 있어야 해요");
            }
            // 스트리밍으로 쓴 WAV는 크기가 비정상일 수 있어서 남은 길이로 줄여요.
            let end = (body + size).min(wav.len());
            return Ok(wav[body..end]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
                .collect());
        }
        pos = body + size + (size & 1);
    }
    bail!("WAV에 data 조각이 없어요")
}

/// 가장 큰 표본이 이 값보다 작으면 무음이에요.
const SILENCE_PEAK: f32 = 0.002;

fn is_silent(samples: &[f32]) -> bool {
    samples.iter().all(|s| s.abs() < SILENCE_PEAK)
}

/// 글자 수로 토큰 수를 넉넉하게 어림해요: 한글 글자마다 1, 그 밖의 글자 둘마다 1(올림)이에요.
fn estimate_tokens(s: &str) -> usize {
    let (ko, other) = s.chars().fold((0usize, 0usize), |(k, o), c| {
        if ('\u{AC00}'..='\u{D7A3}').contains(&c) {
            (k + 1, o)
        } else {
            (k, o + 1)
        }
    });
    ko + other.div_ceil(2)
}

/// 용어 힌트 프롬프트예요: "회의 용어: " + 용어를 ", "로 이어요. 앞(많이 나온 쪽)부터 채우고 `max_tokens`
/// 추정을 넘기 전에 잘라요. 용어가 하나도 안 들어가면 빈 글이에요.
pub fn hint_prompt(terms: &[String], max_tokens: usize) -> String {
    let mut out = String::from(HINT_PREFIX);
    let mut used = estimate_tokens(&out);
    let mut any = false;
    for term in terms {
        let term = term.replace('\0', "");
        if term.is_empty() {
            continue;
        }
        let piece = if any { format!(", {term}") } else { term };
        let cost = estimate_tokens(&piece);
        if used + cost > max_tokens {
            break;
        }
        out.push_str(&piece);
        used += cost;
        any = true;
    }
    if any { out } else { NEUTRAL_PROMPT.to_string() }
}

/// whisper가 조용한 소리에서 지어내는 말이에요(방송 끝 인사, 구독 유도 등).
const KNOWN_PHRASES: &[&str] = &[
    "시청해 주셔서 감사합니다",
    "시청해주셔서 감사합니다",
    "MBC 뉴스",
    "구독과 좋아요",
    "구독 좋아요",
    "자막 제공",
    "자막 by",
    "다음 영상에서 만나요",
];

/// 받아쓴 글이 환각이면 true예요: 무음 확률이 높거나, 알려진 문구이거나, 힌트 프롬프트를 따라 읊은 글이에요.
pub fn is_hallucination(text: &str, prompt: &str, no_speech: f32) -> bool {
    if no_speech > NO_SPEECH_THRESHOLD {
        return true;
    }
    if KNOWN_PHRASES.iter().any(|p| text.contains(p)) {
        return true;
    }
    only_sound_tags(text) || echoes_prompt(text, prompt)
}

/// 글자나 숫자가 하나도 없거나("...", "!"), 소리 설명(`*Clap*`, `[음악]`, `(박수)`, `♪…♪`)만 있으면 true예요.
/// 문장 안에 괄호가 섞인 글("OpenRouter(오픈라우터)에서 보내요")은 설명 밖에 글이 남아서 false예요.
fn only_sound_tags(text: &str) -> bool {
    let mut rest = text;
    let mut kept = String::new();
    while let Some(c) = rest.chars().next() {
        let close = match c {
            '*' => Some('*'),
            '[' => Some(']'),
            '(' => Some(')'),
            '♪' => Some('♪'),
            _ => None,
        };
        let skip = close.and_then(|cl| {
            rest[c.len_utf8()..]
                .find(cl)
                .map(|i| c.len_utf8() + i + cl.len_utf8())
        });
        match skip {
            Some(n) => rest = &rest[n..],
            None => {
                kept.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    !kept.chars().any(|c| c.is_alphanumeric())
}

/// 글의 절반 넘는 글자가 프롬프트 용어로 채워져 있으면 프롬프트를 읊은 거예요. 용어 하나가 문장에 섞여
/// 들어간 건("OpenRouter에서 보내요") 진짜 말이라서, 용어가 둘 이상이거나 글이 용어 하나뿐일 때만 봐요.
fn echoes_prompt(text: &str, prompt: &str) -> bool {
    let list = prompt.strip_prefix(HINT_PREFIX).unwrap_or(prompt);
    let mut terms: Vec<&str> = list
        .split(", ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    terms.sort_by_key(|t| std::cmp::Reverse(t.chars().count()));
    let total = text.chars().filter(|c| c.is_alphanumeric()).count();
    if total == 0 || terms.is_empty() {
        return false;
    }
    let mut rest = text.to_string();
    let mut hits = 0;
    let mut covered = 0;
    for t in terms {
        if rest.contains(t) {
            hits += 1;
            covered += t.chars().filter(|c| c.is_alphanumeric()).count() * rest.matches(t).count();
            rest = rest.replace(t, " ");
        }
    }
    let leftover = rest.chars().filter(|c| c.is_alphanumeric()).count();
    covered * 2 > total && (hits >= 2 || leftover == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(t: &str, a: f64, b: f64) -> (Vec<u8>, f64, f64) {
        (t.as_bytes().to_vec(), a, b)
    }

    #[test]
    fn words_survive_a_syllable_split_across_tokens() {
        // "한"은 ED 95 9C 세 바이트인데 토큰 둘에 나뉘어 들어와요.
        let tokens = [
            (b" \xED\x95".to_vec(), 0.0, 0.1),
            (b"\x9C\xEA\xB8\x80".to_vec(), 0.1, 0.3),
            (" 네".as_bytes().to_vec(), 0.4, 0.5),
        ];
        let words = group_tokens(&tokens);
        let got: Vec<(&str, f64, f64)> = words
            .iter()
            .map(|w| (w.text.as_str(), w.start, w.end))
            .collect();
        assert_eq!(got, vec![("한글", 0.0, 0.3), ("네", 0.4, 0.5)]);
    }

    #[test]
    fn words_group_tokens_by_leading_space() {
        let tokens = [
            tok(" 오픈", 0.0, 0.2),
            tok("라우", 0.2, 0.4),
            tok("터", 0.4, 0.5),
            tok(" 보내", 0.6, 0.8),
            tok("요", 0.8, 0.9),
        ];
        let words = group_tokens(&tokens);
        let got: Vec<(&str, f64, f64)> = words
            .iter()
            .map(|w| (w.text.as_str(), w.start, w.end))
            .collect();
        assert_eq!(got, vec![("오픈라우터", 0.0, 0.5), ("보내요", 0.6, 0.9)]);
    }

    #[test]
    fn sound_tags_and_letterless_text_are_hallucinations() {
        for t in [
            "*Clap*",
            "*sad music*",
            "...",
            "!",
            "[음악]",
            "(박수)",
            "♪ ♪",
            "*Click* [음악].",
            "*tap-tap*",
        ] {
            assert!(is_hallucination(t, "", 0.0), "{t}");
        }
        for t in [
            "OpenRouter(오픈라우터)에서 보내요",
            "안녕하세요 (박수)",
            "별표 *가 있어요",
            "네",
        ] {
            assert!(!is_hallucination(t, "", 0.0), "{t}");
        }
    }

    #[test]
    fn hint_prompt_caps_tokens() {
        let terms: Vec<String> = (0..300).map(|i| format!("용어{i}")).collect();
        let p = hint_prompt(&terms, 200);
        assert!(p.starts_with("회의 용어: 용어0, 용어1"), "{p}");
        assert!(estimate_tokens(&p) <= 200, "{}", estimate_tokens(&p));
        assert!(!p.contains("용어299"));
        let nul = hint_prompt(&["a\0b".to_string()], 200);
        assert_eq!(nul, "회의 용어: ab");
        assert_eq!(hint_prompt(&[], 200), "회의 대화예요.");
        assert_eq!(hint_prompt(&["\0".to_string()], 200), "회의 대화예요.");
    }

    #[test]
    fn filters_known_phrases() {
        for t in [
            "시청해 주셔서 감사합니다",
            "MBC 뉴스 김성현입니다",
            "구독과 좋아요",
        ] {
            assert!(is_hallucination(t, "", 0.0), "{t}");
        }
    }

    #[test]
    fn filters_prompt_echo() {
        let prompt = "회의 용어: OpenRouter, scribe-v2, ZDR";
        assert!(is_hallucination("OpenRouter, scribe-v2, ZDR", prompt, 0.0));
        assert!(is_hallucination("OpenRouter", prompt, 0.0));
        assert!(!is_hallucination("OpenRouter에서 보내요", prompt, 0.0));
    }

    #[test]
    fn filters_no_speech() {
        assert!(is_hallucination("네", "", 0.8));
        assert!(!is_hallucination("네", "", 0.2));
    }

    #[test]
    fn silence_is_detected() {
        assert!(is_silent(&[0.0; 100]));
        assert!(is_silent(&[]));
        assert!(!is_silent(&[0.0, 0.05, 0.0]));
    }

    #[test]
    fn wav_decodes_s16_mono() {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&16_000u32.to_le_bytes());
        wav.extend_from_slice(&32_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&16384i16.to_le_bytes());
        wav.extend_from_slice(&(-32768i16).to_le_bytes());
        assert_eq!(wav_to_f32(&wav).unwrap(), vec![0.5, -1.0]);
        assert!(wav_to_f32(b"nope").is_err());
    }
}
