//! Finds where the voice changes inside one utterance chunk and splits it there (docs/01 "조각 안 화자 나누기 스파이크",
//! 2026-11-02). In meetings where people talk without pauses, one chunk the app cut at the 15-second limit holds
//! two people. The chunk is scanned with a 2-second window every 0.5 seconds, a speaker embedding is taken per
//! window, and it splits at the moment where the mean of a few windows before and a few after drift apart. The
//! moment is aligned to a pause between words.
//! Measured on 16 AMI meetings: speech time attributed to the right person 66% → 88%, speakers per meeting 2.2 →
//! 6.3 (actual 3.9).

use tacie_embed::voice::{VoiceEmbed, Voiceprint};

use crate::voices::normalize;

/// 창 길이(초)예요.
pub const WINDOW_SECONDS: f32 = 2.0;
/// 창 사이 간격이자 바뀐 곳을 찾는 시각의 간격(초)이에요.
pub const HOP_SECONDS: f32 = 0.5;
/// 시각 앞뒤로 평균을 내는 창 수예요.
pub const BLOCK: usize = 3;
/// 앞뒤 평균의 코사인이 이보다 낮으면 바뀐 곳이에요. 0.3(한국어 한 화자 조각을 잘못 나눔 1%, 두 화자 조각 바뀜 찾기
/// 76%)은 유튜브 대담에서 진행자와 손님이 0.3–0.4로 가까워 바뀐 곳을 놓쳤어요. 경계 다시 맞추기(`best_cut`)와 같이 0.4로
/// 올리니 AMI 16회의 맞는 사람 몫 82.6% → 85.7%(16회의 모두 오름), 화자 5.25 → 5.5명이었어요(docs/01 "화자 경계 다시 맞추기").
pub const CHANGE_SIM: f32 = 0.4;
/// 조각 하나에서 나누는 곳은 이만큼까지예요.
pub const MAX_CHANGES: usize = 3;
/// 이보다 짧은 WAV는 훑지 않아요: 앞뒤로 창 `BLOCK`개씩 들어갈 자리가 없어요.
pub const SPLIT_MIN_SECONDS: f32 = WINDOW_SECONDS * 2.0 + HOP_SECONDS * (BLOCK as f32 - 1.0) * 2.0;
/// 바뀐 시각에서 이 안의 단어 사이 쉼 중 가장 긴 곳에서 나눠요(초).
pub const SNAP_SECONDS: f64 = 0.5;

/// 경계 다시 맞추기: 경계 앞뒤 이만큼(초) 안의 단어를 하나씩 두 목소리 중심과 견줘요.
pub const RESEG_SECONDS: f64 = 2.5;
/// 단어 하나의 목소리를 보는 소리 길이(초)예요. 단어가 더 길면 단어 길이예요.
pub const WORD_WINDOW_SECONDS: f64 = 1.0;
/// 앞 조각과 이만큼(ms)보다 짧게 떨어진 조각은 앞 조각에서 이어진 말이에요(앱이 15초에서 자른 조각은 0이에요).
/// 이런 조각의 맨 앞은 앞 사람의 말끝일 수 있어요.
pub const HEAD_GAP_MS: u64 = 300;

const SAMPLE_RATE: usize = 16000;

/// 창이 시작하는 시각들이에요. 마지막 창은 끝에 맞춰요.
pub fn window_starts(seconds: f32, window: f32, hop: f32) -> Vec<f32> {
    let mut out = Vec::new();
    let mut s = 0.0f32;
    while s + window <= seconds + 1e-4 {
        out.push(s);
        s += hop;
    }
    if out.last().is_none_or(|l| l + window < seconds - 1e-4) {
        out.push((seconds - window).max(0.0));
    }
    out
}

/// 창마다 (시작 초, 정규화한 임베딩)이에요. 임베딩이 없는 창(프레임 없음)은 0 벡터예요.
pub fn embed_windows(
    voice: &dyn VoiceEmbed,
    samples: &[i16],
) -> anyhow::Result<Vec<(f32, Vec<f32>)>> {
    let seconds = samples.len() as f32 / SAMPLE_RATE as f32;
    let mut out = Vec::new();
    for s in window_starts(seconds, WINDOW_SECONDS, HOP_SECONDS) {
        let a = (s * SAMPLE_RATE as f32) as usize;
        let b = (((s + WINDOW_SECONDS) * SAMPLE_RATE as f32) as usize).min(samples.len());
        let v = voice
            .embed(&wav_bytes(&samples[a..b]))?
            .map(|p| p.vector)
            .unwrap_or_default();
        out.push((s, v));
    }
    Ok(out)
}

/// 목소리가 바뀐 시각(초, 오름차순)이에요. 시각 t의 앞은 t까지 끝난 마지막 `BLOCK`개 창, 뒤는 t부터 시작한 첫 `BLOCK`개
/// 창이에요. 코사인이 `change_sim`(ERes2Net은 `CHANGE_SIM`, `Tuning::change_sim`)보다 낮은 시각 중 낮은 것부터, 창 길이
/// 이상 떨어진 것만 `MAX_CHANGES`개까지 골라요.
pub fn change_points(windows: &[(f32, Vec<f32>)], change_sim: f32) -> Vec<f32> {
    let end = windows.last().map_or(0.0, |w| w.0 + WINDOW_SECONDS);
    let mut cands: Vec<(f32, f32)> = Vec::new();
    let mut t = WINDOW_SECONDS;
    while t <= end - WINDOW_SECONDS + 1e-4 {
        let left: Vec<&Vec<f32>> = windows
            .iter()
            .filter(|w| w.0 + WINDOW_SECONDS <= t + 1e-4)
            .map(|w| &w.1)
            .collect();
        let right: Vec<&Vec<f32>> = windows
            .iter()
            .filter(|w| w.0 >= t - 1e-4)
            .map(|w| &w.1)
            .collect();
        if left.len() >= BLOCK && right.len() >= BLOCK {
            let l = mean(&left[left.len() - BLOCK..]);
            let r = mean(&right[..BLOCK]);
            let s: f32 = l.iter().zip(&r).map(|(a, b)| a * b).sum();
            if s < change_sim {
                cands.push((t, s));
            }
        }
        t += HOP_SECONDS;
    }
    cands.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut picked: Vec<f32> = Vec::new();
    for (t, _) in cands {
        if picked.iter().all(|p| (t - p).abs() >= WINDOW_SECONDS) {
            picked.push(t);
        }
        if picked.len() == MAX_CHANGES {
            break;
        }
    }
    picked.sort_by(f32::total_cmp);
    picked
}

/// 시각마다 나눌 단어 자리 k(앞 조각은 단어 0..k)예요. 시각 ±`SNAP_SECONDS` 안의 단어 사이 중 쉼이 가장 긴 곳,
/// 없으면 가장 가까운 단어 사이예요. 같은 자리는 한 번만이고, 단어가 둘 미만이면 없어요.
pub fn snap(points: &[f32], words: &[(f64, f64)]) -> Vec<usize> {
    if words.len() < 2 {
        return Vec::new();
    }
    let mut out: Vec<usize> = Vec::new();
    for &p in points {
        let p = f64::from(p);
        let mid = |k: usize| (words[k - 1].1 + words[k].0) / 2.0;
        let gap = |k: usize| words[k].0 - words[k - 1].1;
        let near: Vec<usize> = (1..words.len())
            .filter(|&k| (mid(k) - p).abs() <= SNAP_SECONDS)
            .collect();
        let k = if near.is_empty() {
            (1..words.len()).min_by(|&a, &b| (mid(a) - p).abs().total_cmp(&(mid(b) - p).abs()))
        } else {
            near.into_iter().max_by(|&a, &b| gap(a).total_cmp(&gap(b)))
        };
        if let Some(k) = k
            && !out.contains(&k)
        {
            out.push(k);
        }
    }
    out.sort_unstable();
    out
}

/// 나뉜 조각 하나예요: 단어 범위와, 원래 WAV에서의 소리 범위(샘플)예요.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub words: std::ops::Range<usize>,
    pub samples: std::ops::Range<usize>,
}

/// 나눌 단어 자리들로 조각을 만들어요. 소리 경계는 두 단어 사이 쉼의 가운데예요.
pub fn pieces(cuts: &[usize], words: &[(f64, f64)], total_samples: usize) -> Vec<Piece> {
    let mut edges = vec![(0usize, 0usize)];
    for &k in cuts {
        let at = ((words[k - 1].1 + words[k].0) / 2.0 * SAMPLE_RATE as f64) as usize;
        edges.push((k, at.min(total_samples)));
    }
    edges.push((words.len(), total_samples));
    edges
        .windows(2)
        .map(|e| Piece {
            words: e[0].0..e[1].0,
            samples: e[0].1..e[1].1,
        })
        .collect()
}

/// 단어 하나의 목소리를 볼 소리 범위(샘플)예요: 단어 가운데를 중심으로 `WORD_WINDOW_SECONDS`(단어가 더 길면 단어
/// 길이)이고, WAV 밖으로 나가면 길이를 지킨 채 안쪽으로 밀어요.
pub fn word_window(word: (f64, f64), total_samples: usize) -> std::ops::Range<usize> {
    let total = total_samples as f64 / SAMPLE_RATE as f64;
    let len = WORD_WINDOW_SECONDS.max(word.1 - word.0).min(total);
    let mid = (word.0 + word.1) / 2.0;
    let a = (mid - len / 2.0).clamp(0.0, (total - len).max(0.0));
    let at = |t: f64| ((t * SAMPLE_RATE as f64) as usize).min(total_samples);
    at(a)..at(a + len)
}

/// 경계를 다시 맞춰요. `d[i]`는 후보 단어 i가 앞 목소리에 얼마나 더 가까운지(앞 중심 코사인 − 뒤 중심 코사인)예요.
/// 앞 목소리로 줄 단어 수 k(0..=len)는 `d[..k]`의 합 − `d[k..]`의 합이 가장 큰 곳이고, 같으면 지금 자리 `orig`에 가까운
/// 곳이에요. 목소리를 모르는 단어는 0이에요.
pub fn best_cut(d: &[f32], orig: usize) -> usize {
    let total: f32 = d.iter().sum();
    let mut before = 0.0f32;
    let mut best = (f32::NEG_INFINITY, orig);
    for k in 0..=d.len() {
        if k > 0 {
            before += d[k - 1];
        }
        let score = before - (total - before);
        let closer = k.abs_diff(orig) < best.1.abs_diff(orig);
        if score > best.0 + 1e-6 || ((score - best.0).abs() <= 1e-6 && closer) {
            best = (score, k);
        }
    }
    best.1
}

/// 16 kHz mono 16-bit PCM WAV예요(44바이트 머리).
pub fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    w.extend_from_slice(&(SAMPLE_RATE as u32 * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

/// 나뉜 조각의 화자 임베딩이에요.
pub fn embed_piece(voice: &dyn VoiceEmbed, samples: &[i16]) -> anyhow::Result<Option<Voiceprint>> {
    voice.embed(&wav_bytes(samples))
}

fn mean(vs: &[&Vec<f32>]) -> Vec<f32> {
    let dim = vs.iter().map(|v| v.len()).max().unwrap_or(0);
    let mut sum = vec![0.0f32; dim];
    for v in vs {
        for (s, x) in sum.iter_mut().zip(v.iter()) {
            *s += x;
        }
    }
    normalize(&sum)
}

/// 문장 가운데서 화자가 바뀐 것처럼 나뉜 경계를 문장부호로 맞출 때, 두 단어 사이 쉼이 이보다 짧아야 해요(초).
pub const PUNCT_GAP_SECONDS: f64 = 0.3;
/// 앞 줄 문장을 끝내는 다음 줄 첫 단어는 이만큼(단어 수, 초)까지만 옮겨요.
pub const PUNCT_HEAD_WORDS: usize = 3;
pub const PUNCT_HEAD_SECONDS: f64 = 1.5;
/// 다음 줄 문장을 시작하는 앞 줄 끝 단어는 이만큼까지만 옮겨요.
pub const PUNCT_TAIL_WORDS: usize = 2;
pub const PUNCT_TAIL_SECONDS: f64 = 1.2;
/// 혼자 하는 대답이라 앞 문장의 끝으로 보지 않는 말이에요.
const SOLO: &[&str] = &[
    "네",
    "예",
    "아",
    "어",
    "음",
    "맞아요",
    "그쵸",
    "그렇죠",
    "네네",
    "와",
    "오",
    "응",
];

fn ends_sentence(t: &str) -> bool {
    matches!(t.trim().chars().last(), Some('.' | '?' | '!'))
}

fn ends_clause(t: &str) -> bool {
    ends_sentence(t) || t.trim().ends_with(',')
}

/// 앞 줄이 문장 가운데서 끝났고(`before`에 부호가 없어요) 다음 줄 첫 단어 몇 개가 그 문장을 끝내면("좋을 것 / 같아요."),
/// 앞 줄로 돌려보낼 단어 수예요. `gap`은 두 단어 사이 쉼(초), `after`는 다음 줄 단어(시작, 끝, 글)예요. 목소리만으로는
/// 쉼이 0.1초 남짓한 빠른 말 주고받기에서 단어 하나를 가르지 못해요(docs/01 "문장부호로 경계 맞추기").
pub fn punct_head(before: &str, gap: f64, after: &[(f64, f64, &str)]) -> usize {
    if gap >= PUNCT_GAP_SECONDS || ends_clause(before) || after.is_empty() {
        return 0;
    }
    for (j, w) in after.iter().enumerate().take(PUNCT_HEAD_WORDS) {
        if w.1 - after[0].0 > PUNCT_HEAD_SECONDS {
            return 0;
        }
        if ends_sentence(w.2) {
            let solo = after[..=j].iter().all(|w| {
                let t: String = w.2.chars().filter(|c| !".?!,".contains(*c)).collect();
                SOLO.contains(&t.trim())
            });
            return if solo { 0 } else { j + 1 };
        }
    }
    0
}

/// 앞 줄이 문장을 끝낸 뒤 한두 단어를 더 달고("…됩니다. 대표님") 다음 줄이 곧바로 시작하면, 그 단어들은 다음 줄
/// 문장의 시작이에요. 다음 줄로 보낼 단어 수예요. `before`는 앞 줄 단어, `gap`은 앞 줄 끝과 다음 줄 시작 사이 쉼(초)이에요.
pub fn punct_tail(before: &[(f64, f64, &str)], gap: f64) -> usize {
    if gap >= PUNCT_GAP_SECONDS || before.is_empty() {
        return 0;
    }
    let n = before
        .iter()
        .rev()
        .take(PUNCT_TAIL_WORDS)
        .take_while(|w| !ends_sentence(w.2))
        .count();
    if n == 0 || n >= before.len() {
        return 0;
    }
    let first = before.len() - n;
    if ends_sentence(before[first - 1].2)
        && before[before.len() - 1].1 - before[first].0 <= PUNCT_TAIL_SECONDS
    {
        n
    } else {
        0
    }
}
