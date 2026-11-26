//! Meeting mode: remembers the last three lines as prior conversation, and which files were already shown.
//!
//! The record is a JSON object with a fixed key order. Keys starting with `_` (`_context`, `_line`) are used
//! internally only and are not exported. A meeting's utterances are processed in order: the gate and search
//! (through rerank) wait while holding the session lock. Transcription and the ledger model wait outside the lock.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use tacie_embed::voice::Voiceprint;
use tokio::sync::{Mutex, Notify};

use crate::errors::Kind;
use crate::notes::{is_backchannel, is_sound_only, one_line, spoken_seconds};
use crate::notes_kind::NotesKind;
use crate::pipeline::{Engine, head};
use crate::sample::MODEL_MISSING;
use crate::services::{SttVia, Transcript};
use crate::split;
use crate::stt_clean;
use crate::voices::{Assigned, Channel, Voices, dot, normalize};

/// 같은 파일을 이 시간(초) 안에 다시 보이면 카드에 `repeat`을 달아요.
pub const REPEAT_SECONDS: f64 = 300.0;
/// 한 사람이 말하다 이만큼(ms) 짧게 쉬면 질문이 둘로 잘린 거예요: 두 조각을 같이 읽어요.
pub const MERGE_GAP_MS: u64 = 1000;
/// 회의가 끝날 때 마지막 말이 아직 받아쓰기에 있을 수 있어요: 장부는 받아쓰기를 이만큼까지 기다려요.
pub const LEDGER_WAIT: Duration = Duration::from_secs(5);
/// 앞 대화로 기억하는 줄 수예요.
const RECENT_LINES: usize = 3;

pub type Record = Map<String, Value>;

/// `split`이 옮길 말이 없을 때예요(목소리가 붙지 않은 말, 모르는 id).
pub const SPLIT_NONE: &str = "no voiced utterance to move";
/// `split`의 말들이 여러 목소리에 걸칠 때예요.
pub const SPLIT_MIXED: &str = "utterances belong to more than one voice";

pub struct Session {
    pub id: String,
    /// 온보딩 회의 연습이에요: 찾기는 예시 자료에서만 해요.
    pub sample: bool,
    /// 찾을 자료가 아직 없어도 질문인지는 판단해요(유료). 앱이 그 질문을 적어 뒀다가 연결이나 첫 동기화가 끝나면
    /// 다시 찾아요(docs/11 11절). 기본은 꺼져 있어서 빈 색인에는 판단을 부르지 않아요.
    pub gate_empty: bool,
    state: Mutex<State>,
    /// 아직 받아쓰는 중인 발화 수예요. 받아쓰기(`hear_audio`)와 장부가 써요.
    busy: AtomicUsize,
    /// 받아쓰기가 끝날 때마다 깨워요(장부가 남은 받아쓰기를 기다려요).
    idle: Notify,
    /// 질문인지 판단해 답을 찾아요. 꺼지면 기록만 해요: 판단(유료)을 부르지 않고 줄은 회의 정리용으로만 남겨요.
    /// 회의 중에도 앱이 바꿔요(`POST /sessions/{id}/lookup`).
    lookup: AtomicBool,
    /// 나·상대(또는 앱이 이름을 준 화자) 표시를 한 번이라도 받았어요: 원격 회의예요. 원격의 나와 상대는 각자
    /// 갈래(`Mic`, `Call`)에서 묶지만, 그 뒤 표시 없이 온 발화(앱이 어느 쪽인지 못 가른 것)는 묶지 않아요.
    remote: AtomicBool,
}

struct State {
    /// 앞 대화로 쓰는 마지막 세 줄("말한 사람: 글")이에요.
    recent: VecDeque<String>,
    /// 경로마다 마지막으로 화면에 보인 시각이에요.
    shown_at: HashMap<String, f64>,
    log: Vec<Record>,
    /// (그때까지 들은 발화 수, 결과): 새 발화 없이 다시 물으면 돈이 들지 않아요.
    ledger: Option<(usize, Option<crate::lang::Lang>, Value)>,
    /// 이 회의에서 들은 목소리들이에요(docs/04 4절).
    voices: Voices,
    /// 목소리가 붙은 발화마다 남긴 임베딩이에요. `/voices`가 줄을 빼고 대표 벡터를 다시 셀 때 써요.
    /// 메모리에만 있고 회의를 닫으면 없어져요.
    prints: Vec<Kept>,
    /// 앱이 화자 키("화자 2")에 붙인 이름이에요(`/names`). 앞 대화 줄과 장부 줄에 키 대신 써요.
    names: HashMap<String, String>,
}

/// 목소리가 붙은 발화 하나의 (발화 id, 목소리, 화자 나누기 임베딩, ERes2Net 임베딩, 초)예요. 화자 나누기 모델이 없으면
/// 두 임베딩이 같아요.
#[derive(Debug, Clone)]
struct Kept {
    uid: String,
    voice: String,
    vector: Vec<f32>,
    ident: Vec<f32>,
    seconds: f32,
}

impl State {
    /// 기록을 남기고, 목소리가 붙은 발화면 그 임베딩도 같이 남겨요. 판단을 기다리다 버려진 호출은 둘 다 남기지 않아요.
    fn keep(&mut self, rec: Record, print: Option<Kept>) {
        self.prints.extend(print);
        self.log.push(rec);
    }
}

impl Session {
    /// 질문을 판단해 답을 찾는지예요. 꺼져 있으면 기록만 해요.
    pub fn lookup(&self) -> bool {
        self.lookup.load(Ordering::SeqCst)
    }

    /// 기록만 하기를 켜고 꺼요. 바꾼 뒤에 들은 발화부터 따라요.
    pub fn set_lookup(&self, on: bool) {
        self.lookup.store(on, Ordering::SeqCst);
    }

    pub(crate) fn new(id: String, sample: bool, gate_empty: bool, voices: Voices) -> Self {
        Self {
            id,
            sample,
            gate_empty,
            lookup: AtomicBool::new(true),
            state: Mutex::new(State {
                recent: VecDeque::with_capacity(RECENT_LINES),
                shown_at: HashMap::new(),
                log: Vec::new(),
                ledger: None,
                voices,
                prints: Vec::new(),
                names: HashMap::new(),
            }),
            busy: AtomicUsize::new(0),
            idle: Notify::new(),
            remote: AtomicBool::new(false),
        }
    }

    /// hear + find를 한 번에 해요(이미 받아쓴 글이에요).
    pub async fn utterance(
        &self,
        engine: &Engine,
        text: &str,
        speaker: Option<&str>,
        gate_thr: f64,
        show_thr: f64,
        now: Option<f64>,
    ) -> Value {
        let rec = self
            .hear(
                engine,
                text,
                speaker,
                gate_thr,
                now,
                None,
                None,
                engine.settings.end,
            )
            .await;
        if rec["fired"] != true {
            return rec;
        }
        let Some(uid) = rec["utterance_id"].as_str() else {
            return rec;
        };
        // 방금 넣은 기록이라 찾지 못할 수 없어요.
        match self.find(engine, uid, show_thr).await {
            Some(found) => found,
            None => rec,
        }
    }

    /// 발화 하나(WAV)를 받아쓴 뒤 들어요. 빈 글은 기록하지 않아요. 기준은 설정값(gate, end)이고 시각은 지금이에요.
    /// 받아쓰기 확신은 기록만 해요: 실제 녹음이 쌓일 때까지 기준이 없어요(docs/04 §5).
    pub async fn hear_audio(
        &self,
        engine: &Engine,
        wav: &[u8],
        speaker: Option<&str>,
        gap_ms: Option<u64>,
    ) -> Value {
        self.hear_audio_with(engine, wav, speaker, gap_ms, None)
            .await
    }

    /// `given`: 앱이 Soniox 실시간 연결에서 받은 글이에요(docs/25). 있으면 받아쓰지 않고 이 글로 나머지(언어 밖 글자,
    /// 용어 고치기, 화자, 판단)를 해요.
    pub async fn hear_audio_with(
        &self,
        engine: &Engine,
        wav: &[u8],
        speaker: Option<&str>,
        gap_ms: Option<u64>,
        given: Option<Transcript>,
    ) -> Value {
        // 받아쓰는 수를 올리고, 어느 갈래로 끝나든(버려져도) 내려요.
        self.busy.fetch_add(1, Ordering::SeqCst);
        let _busy = Busy(self);
        // 화자 임베딩(10–80 ms)은 받아쓰기(약 1초)와 나란히 뽑아요. 받아쓴 글이 비면 버려요. 묶지 않을 발화(원격
        // 회의의 표시 없는 발화, 앱이 이름을 준 화자)는 뽑지 않아요.
        if speaker.is_some() {
            self.remote.store(true, Ordering::SeqCst);
        }
        let print = engine
            .voice
            .clone()
            .filter(|_| self.channel(speaker).is_some())
            .map(|v| {
                let wav = wav.to_vec();
                tokio::task::spawn_blocking(move || v.embed(&wav))
            });
        // 쉼 없이 말한 조각에는 두 사람이 들어 있을 수 있어요: 창마다 임베딩을 받아쓰기와 나란히 뽑아 둬요(`split`).
        let samples: Option<Arc<Vec<i16>>> = print.as_ref().and_then(|_| {
            let s = tacie_embed::voice::pcm16_mono(wav).ok()?;
            (s.len() as f32 / 16000.0 >= split::SPLIT_MIN_SECONDS).then(|| {
                Arc::new(
                    s.iter()
                        .map(|x| (x * 32768.0).clamp(-32768.0, 32767.0) as i16)
                        .collect(),
                )
            })
        });
        let windows = engine.voice.clone().zip(samples.clone()).map(|(v, s)| {
            tokio::task::spawn_blocking(move || split::embed_windows(v.as_ref(), &s))
        });
        let heard = match given {
            Some(t) => Ok(t),
            None => engine.services.transcriber.transcribe(wav).await,
        };
        let mut stt = match heard {
            Ok(stt) => stt,
            Err(e) => {
                // 로컬 모델은 `model`로만 실패해요(서버 받아쓰기는 이 종류로 실패하지 않아요).
                let mut rec = json!({"utterance_id": null, "text": "", "fired": false,
                                     "error": head(&e.message), "error_kind": e.kind});
                let via = if e.kind == Kind::Model {
                    SttVia::Local
                } else {
                    SttVia::Server
                };
                tag_stt(&mut rec, via, engine);
                return rec;
            }
        };
        // 한국어 회의에 섞여 나온 다른 언어 글자(아랍어, 한자 등)를 덜어내요(`stt_clean`).
        let cleaned = stt_clean::clean(&mut stt);
        if let Some(c) = cleaned {
            eprintln!("[stt] {}", c.as_str());
        }
        if stt.text.is_empty() {
            let mut rec = json!({"utterance_id": null, "text": "", "fired": false,
                                 "timing_ms": {"stt": stt.ms}});
            if let Some(c) = cleaned {
                rec["stt_cleaned"] = json!(c.as_str());
            }
            tag_stt(&mut rec, stt.via, engine);
            return rec;
        }
        // 받아쓰기가 한글로 적은 팀 용어를 원래 용어로 고쳐요(`terms`). 예시 자료로 하는 연습은 고치지 않아요.
        if !self.sample {
            let (text, words) = engine.term_fixer().fix(&stt.text, &stt.words);
            if engine.log_text && text != stt.text {
                eprintln!("[terms] {} -> {}", stt.text, text);
            }
            stt.text = text;
            stt.words = words;
        }
        // 임베딩이 실패해도 발화는 그대로예요: 이름만 없어요. 임베딩 값은 쓰지 않아요.
        let voice = match print {
            None => None,
            Some(task) => joined_print(task.await),
        };
        // 새 화자를 만들 만큼 길게 말했는지는 WAV 길이가 아니라 말한 구간으로 봐요(`MIN_NEW_SECONDS`).
        // 쉼과 게임 소리가 붙은 3초짜리 WAV에 0.2초 "你？"만 있어도 새 화자가 생겼어요(2026-11-02 원격 회의).
        let words: Vec<(f64, f64)> = stt.words.iter().map(|w| (w.start, w.end)).collect();
        let voice = voice.map(|mut p| {
            if let Some(s) = spoken_seconds(&words) {
                p.seconds = p.seconds.min(s as f32);
            }
            p
        });
        let cuts = match windows {
            Some(task) => match task.await {
                Ok(Ok(w)) => {
                    let sim = engine.voice_tuning().change_sim;
                    split::snap(&split::change_points(&w, sim), &words)
                }
                Ok(Err(e)) => {
                    eprintln!(
                        "voice split: {}",
                        head(&format!("{e:#}")).replace('\n', " ")
                    );
                    Vec::new()
                }
                Err(e) => {
                    eprintln!("voice split: {e}");
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        let (Some(samples), Some(model), Some(channel)) =
            (samples, engine.voice.clone(), self.channel(speaker))
        else {
            let rec = self
                .hear_with_voice(
                    engine,
                    &stt.text,
                    speaker,
                    engine.settings.gate,
                    None,
                    gap_ms,
                    stt.confidence,
                    engine.settings.end,
                    voice,
                    None,
                )
                .await;
            return with_stt(rec, &stt, &stt.words, None, engine);
        };
        // 조각마다 임베딩을 뽑아요. 나누지 않은 조각은 WAV 전체의 임베딩 그대로예요.
        let piece_text = |r: std::ops::Range<usize>| {
            stt.words[r]
                .iter()
                .map(|w| w.text.trim())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut edges: Vec<usize> = std::iter::once(0)
            .chain(cuts.iter().copied())
            .chain(std::iter::once(words.len()))
            .collect();
        let mut prints: Vec<Option<Voiceprint>> = Vec::new();
        if cuts.is_empty() {
            prints.push(voice);
        } else {
            for p in split::pieces(&cuts, &words, samples.len()) {
                let part: Vec<i16> = samples[p.samples.clone()].to_vec();
                let m = model.clone();
                let spans = &words[p.words.clone()];
                prints.push(
                    joined_print(
                        tokio::task::spawn_blocking(move || split::embed_piece(m.as_ref(), &part))
                            .await,
                    )
                    .map(|mut v| {
                        if let Some(s) = spoken_seconds(spans) {
                            v.seconds = v.seconds.min(s as f32);
                        }
                        v
                    }),
                );
            }
        }
        // 조각마다 목소리를 정해요. 맞장구와 소리 표시는 묶지 않아요(`hear_with_voice`가 기록만 해요).
        let quiet = |t: &str| is_backchannel(t) || is_sound_only(t);
        let (mut assigned, prev, prev_last, centers) = {
            let mut st = self.state.lock().await;
            let assigned: Vec<Option<Assigned>> = edges
                .windows(2)
                .zip(&prints)
                .map(|(e, p)| {
                    let text = if cuts.is_empty() {
                        stt.text.clone()
                    } else {
                        piece_text(e[0]..e[1])
                    };
                    p.as_ref()
                        .filter(|_| !quiet(&text))
                        .and_then(|p| st.voices.assign(channel, p))
                })
                .collect();
            // 앞 조각에서 이어진 말이면 앞 조각의 마지막 진짜 발화(같은 자리)의 목소리예요.
            let prev_rec = gap_ms
                .filter(|g| *g < split::HEAD_GAP_MS)
                .and_then(|_| st.log.iter().rev().find(|r| !truthy(r.get("skipped"))))
                .filter(|r| r.get("speaker").and_then(Value::as_str) == speaker);
            let prev = prev_rec.and_then(|r| {
                let voice = r.get("voice").and_then(Value::as_str)?.to_string();
                let person = r.get("person").and_then(Value::as_str).map(str::to_string);
                Some(Assigned { voice, person })
            });
            // 앞 줄의 마지막 단어예요(문장부호로 경계 맞추기).
            let prev_last = prev_rec
                .and_then(|r| r.get("text").and_then(Value::as_str))
                .and_then(|t| t.split_whitespace().last())
                .map(str::to_string);
            let centers: HashMap<String, Vec<f32>> = assigned
                .iter()
                .flatten()
                .chain(prev.as_ref())
                .filter_map(|a| Some((a.voice.clone(), st.voices.center(&a.voice)?.to_vec())))
                .collect();
            (assigned, prev, prev_last, centers)
        };
        // 경계 다시 맞추기(docs/01 "화자 경계 다시 맞추기"): 2초 창으로 찾은 경계는 0.5–1.5초 어긋나고, 앱이 15초에서 자른
        // 조각의 맨 앞 3초는 바뀐 곳을 아예 찾지 못해요. 경계 앞뒤 단어를 하나씩 두 목소리 중심과 견줘 자리를 다시 정해요.
        // 맨 앞 경계는 앞 조각의 목소리로 줄 단어만 골라요(앞 조각은 이미 앱에 갔어요).
        struct Boundary {
            at: usize, // edges 자리(1.. 조각 사이, 0은 맨 앞)
            lo: usize,
            hi: usize,
            before: String,
            after: String,
        }
        let near = |i: usize, lo_limit: usize, hi_limit: usize| {
            let t = words[i].0;
            let lo = (lo_limit..i)
                .find(|&k| words[k].0 > t - split::RESEG_SECONDS)
                .unwrap_or(i);
            let hi = (i..hi_limit)
                .take_while(|&k| words[k].0 < t + split::RESEG_SECONDS)
                .last()
                .map_or(i, |k| k + 1);
            (lo, hi)
        };
        let mut bounds: Vec<Boundary> = Vec::new();
        if let (Some(p), Some(Some(first))) = (&prev, assigned.first())
            && p.voice != first.voice
            && edges[1] > 0
        {
            let (_, hi) = near(0, 0, edges[1]);
            bounds.push(Boundary {
                at: 0,
                lo: 0,
                hi,
                before: p.voice.clone(),
                after: first.voice.clone(),
            });
        }
        for j in 1..edges.len() - 1 {
            if let (Some(a), Some(b)) = (&assigned[j - 1], &assigned[j])
                && a.voice != b.voice
            {
                let (lo, hi) = near(edges[j], edges[j - 1], edges[j + 1]);
                bounds.push(Boundary {
                    at: j,
                    lo,
                    hi,
                    before: a.voice.clone(),
                    after: b.voice.clone(),
                });
            }
        }
        let mut head = 0usize;
        if !bounds.is_empty()
            && bounds
                .iter()
                .all(|b| centers.contains_key(&b.before) && centers.contains_key(&b.after))
        {
            let ranges: Vec<std::ops::Range<usize>> = bounds
                .iter()
                .flat_map(|b| b.lo..b.hi)
                .map(|k| split::word_window(words[k], samples.len()))
                .collect();
            let (m, s) = (model.clone(), samples.clone());
            let vecs: Vec<Option<Vec<f32>>> = tokio::task::spawn_blocking(move || {
                ranges
                    .into_iter()
                    .map(|r| {
                        split::embed_piece(m.as_ref(), &s[r])
                            .ok()
                            .flatten()
                            .map(|p| normalize(&p.vector))
                    })
                    .collect()
            })
            .await
            .unwrap_or_default();
            let mut next = 0usize;
            for b in &bounds {
                let (ca, cb) = (&centers[&b.before], &centers[&b.after]);
                let d: Vec<f32> = (b.lo..b.hi)
                    .map(|_| {
                        let v = vecs.get(next).cloned().flatten();
                        next += 1;
                        v.map_or(0.0, |v| dot(ca, &v) - dot(cb, &v))
                    })
                    .collect();
                if vecs.len() < next {
                    break;
                }
                if b.at == 0 {
                    head = split::best_cut(&d, 0);
                } else {
                    // 앞 경계가 옮겨졌으면 그 뒤로만 옮겨요. 첫 조각 경계는 맨 앞에서 앞 줄로 돌려보낸 단어도 넘지 않아요.
                    let before = if b.at == 1 { head } else { edges[b.at - 1] };
                    let hi = edges[b.at + 1];
                    let lo = b.lo.max(before).min(hi);
                    let k = b.lo + split::best_cut(&d, edges[b.at] - b.lo);
                    edges[b.at] = k.clamp(lo, hi);
                }
            }
        }
        // 맨 앞의 앞 사람 말끝은 앞 조각 목소리의 조각이 돼요: 잇기 규칙으로 앞 줄에 이어 붙어요. 그 목소리 중심은 옮기지 않아요.
        let mut head = head.min(edges[1]);
        // 문장부호로 경계 맞추기(docs/01): 목소리로 다시 맞춘 뒤에도 문장 가운데서 바뀐 경계는 한두 단어를 옮겨요.
        let tok = |i: usize| (words[i].0, words[i].1, stt.words[i].text.trim());
        let side = |r: std::ops::Range<usize>| r.map(tok).collect::<Vec<_>>();
        if let (Some(p), Some(Some(first))) = (&prev, assigned.first())
            && p.voice != first.voice
            && head < edges[1]
        {
            let (before, gap) = if head > 0 {
                (
                    stt.words[head - 1].text.trim().to_string(),
                    words[head].0 - words[head - 1].1,
                )
            } else {
                (
                    prev_last.clone().unwrap_or_else(|| ".".into()),
                    gap_ms.unwrap_or(u64::MAX) as f64 / 1000.0,
                )
            };
            head += split::punct_head(&before, gap, &side(head..edges[1]));
        }
        for j in 1..edges.len() - 1 {
            let (Some(a), Some(b)) = (&assigned[j - 1], &assigned[j]) else {
                continue;
            };
            let e = edges[j];
            let lo = if j == 1 {
                head.max(edges[0])
            } else {
                edges[j - 1]
            };
            if a.voice == b.voice || e <= lo || e >= edges[j + 1] {
                continue;
            }
            let gap = words[e].0 - words[e - 1].1;
            let n = split::punct_head(stt.words[e - 1].text.trim(), gap, &side(e..edges[j + 1]));
            edges[j] = if n > 0 {
                e + n
            } else {
                e - split::punct_tail(&side(lo..e), gap)
            };
        }
        if head > 0 {
            edges.insert(1, head);
            assigned.insert(0, prev.clone());
            prints.insert(0, None);
        }
        // 빈 조각은 빼요.
        let mut keep: Vec<(std::ops::Range<usize>, Option<Assigned>, Option<Voiceprint>)> = edges
            .windows(2)
            .zip(assigned)
            .zip(prints)
            .filter(|((e, _), _)| e[0] < e[1] || words.is_empty())
            .map(|((e, a), p)| (e[0]..e[1], a, p))
            .collect();
        if keep.len() <= 1 {
            let (_, a, p) = keep.pop().unwrap_or((0..0, None, None));
            let rec = self
                .hear_with_voice(
                    engine,
                    &stt.text,
                    speaker,
                    engine.settings.gate,
                    None,
                    gap_ms,
                    stt.confidence,
                    engine.settings.end,
                    p,
                    Some(a),
                )
                .await;
            return with_stt(rec, &stt, &stt.words, None, engine);
        }
        // 목소리가 바뀐 곳에서 나눠 조각마다 따로 들어요. 같은 목소리로 다시 묶인 뒤 조각은 앞 조각에 이어 붙어요(잇기 규칙).
        let final_cuts: Vec<usize> = keep.iter().skip(1).map(|(r, _, _)| r.start).collect();
        let mut recs = Vec::new();
        for (j, (p, (r, a, print))) in split::pieces(&final_cuts, &words, samples.len())
            .into_iter()
            .zip(keep)
            .enumerate()
        {
            debug_assert_eq!(p.words, r);
            let ws = &stt.words[p.words.clone()];
            let text = piece_text(p.words.clone());
            let gap = if j == 0 {
                gap_ms
            } else {
                let before = stt.words[p.words.start - 1].end;
                Some(((ws[0].start - before).max(0.0) * 1000.0) as u64)
            };
            let rec = self
                .hear_with_voice(
                    engine,
                    &text,
                    speaker,
                    engine.settings.gate,
                    None,
                    gap,
                    stt.confidence,
                    engine.settings.end,
                    print,
                    Some(a),
                )
                .await;
            let offset = p.samples.start as f64 / 16000.0;
            recs.push(with_stt(rec, &stt, ws, (j > 0).then_some(offset), engine));
        }
        let mut first = recs.remove(0);
        if let Value::Object(f) = &mut first {
            for r in &mut recs {
                if let Value::Object(r) = r {
                    r.remove("timing_ms");
                }
            }
            f.insert("then".into(), Value::Array(recs));
        }
        first
    }

    /// 판단만 해요: 찾아볼 질문인지, 말한 사람이 회의를 끝내는 중인지(`ending`)예요. 이 줄을 뒤에 올 말의 앞 대화로 기억해요.
    /// 같은 사람의 판단을 넘지 않은 조각 뒤에 짧게 쉬고 온 조각은 앞 조각과 같이 읽어요.
    /// 맞장구는 기록하지만 판단하지도 기억하지도 않아요. 쉼이 모두 짧으면 잇기는 맞장구를 건너뛰어요.
    #[allow(clippy::too_many_arguments)] // 발화에 딸린 값을 하나씩 받아요
    pub async fn hear(
        &self,
        engine: &Engine,
        text: &str,
        speaker: Option<&str>,
        gate_thr: f64,
        now: Option<f64>,
        gap_ms: Option<u64>,
        stt_confidence: Option<f64>,
        end_thr: f64,
    ) -> Value {
        self.hear_with_voice(
            engine,
            text,
            speaker,
            gate_thr,
            now,
            gap_ms,
            stt_confidence,
            end_thr,
            None,
            None,
        )
        .await
    }

    /// 목소리가 들어온 갈래예요: 대면 회의의 발화는 방(`Room`), 원격의 나는 마이크(`Mic`), 상대는 통화(`Call`)예요.
    /// 앱이 이름을 준 화자와 원격 회의에서 표시 없이 온 발화는 묶지 않아요.
    fn channel(&self, speaker: Option<&str>) -> Option<Channel> {
        match speaker {
            None if self.remote.load(Ordering::SeqCst) => None,
            None => Some(Channel::Room),
            Some("나") => Some(Channel::Mic),
            Some("상대") => Some(Channel::Call),
            Some(_) => None,
        }
    }

    /// `hear`에 화자 임베딩을 더해요. 대면 회의(`speaker` 없음)와 원격의 나·상대는 목소리로 묶어 이름(`voice`)을 붙이고,
    /// 그 이름을 앞 대화 줄에 써요. 앱이 이름을 준 화자는 묶지 않아요. 묶기는 세션 자물쇠 안에서
    /// 기록과 같은 순서로 해요.
    #[allow(clippy::too_many_arguments)]
    async fn hear_with_voice(
        &self,
        engine: &Engine,
        text: &str,
        speaker: Option<&str>,
        gate_thr: f64,
        now: Option<f64>,
        gap_ms: Option<u64>,
        stt_confidence: Option<f64>,
        end_thr: f64,
        print: Option<Voiceprint>,
        // 이미 정한 목소리예요(`hear_audio`가 조각마다 먼저 정해요). None이면 여기서 `print`로 정해요.
        pre: Option<Option<Assigned>>,
    ) -> Value {
        let mut guard = self.state.lock().await;
        let st = &mut *guard; // 필드마다 따로 빌려요(log를 보면서 recent를 고쳐요)
        let now = now.unwrap_or_else(unix_now);
        let uid = format!("{}u{}", self.id, st.log.len() + 1);
        // 맞장구와 소리 표시만 있는 발화("[게임 소리]")는 기록만 해요: 판단하지 않고 목소리로 묶지도 않아요.
        let skip = if is_backchannel(text) {
            Some("backchannel")
        } else if is_sound_only(text) {
            Some("sound")
        } else {
            None
        };
        if let Some(reason) = skip {
            let rec = record([
                ("utterance_id", json!(uid)),
                ("text", json!(text)),
                ("speaker", json!(speaker)),
                // 맞장구는 짧고 소리가 섞여서 묶지 않아요(실측은 0.5초 미만 발화를 뺐어요). 소리 표시는 사람이 아니에요.
                ("voice", Value::Null),
                ("person", Value::Null),
                ("at", json!(now)),
                ("gap_ms", json!(gap_ms)),
                ("cards", json!([])),
                ("shown", json!(false)),
                ("merged_from", Value::Null),
                ("skipped", json!(reason)),
                ("p_lookup", Value::Null),
                ("fired", json!(false)),
                ("p_end", Value::Null),
                ("ending", json!(false)),
                ("stt_confidence", json!(stt_confidence)),
            ]);
            if engine.log_text {
                let line = heard_line(&uid, None, None, None, true, text);
                eprintln!(
                    "{}",
                    line.replacen("skipped=backchannel", &format!("skipped={reason}"), 1)
                );
            }
            let out = public(&rec);
            st.log.push(rec);
            return out;
        }

        let assigned = match pre {
            Some(a) => a,
            None => self
                .channel(speaker)
                .zip(print.as_ref())
                .and_then(|(channel, p)| st.voices.assign(channel, p)),
        };
        // `/voices`가 다시 셀 임베딩이에요. 기록과 같이 남겨요.
        let kept = assigned.as_ref().zip(print).map(|(a, p)| Kept {
            uid: uid.clone(),
            voice: a.voice.clone(),
            ident: p.ident().to_vec(),
            vector: p.vector,
            seconds: p.seconds,
        });
        let (mut voice, mut person) = match assigned {
            Some(a) => (Some(a.voice), a.person),
            None => (None, None),
        };

        // 앞의 맞장구를 건너뛰어 마지막 진짜 발화를 찾아요. 건너뛴 맞장구의 쉼도 모두 짧아야 이어요.
        let mut prev: Option<&Record> = None;
        let mut gaps = vec![gap_ms];
        for r in st.log.iter().rev() {
            if !truthy(r.get("skipped")) {
                prev = Some(r);
                break;
            }
            gaps.push(r.get("gap_ms").and_then(Value::as_u64));
        }
        let mut text = text.to_string();
        let mut merged_from: Option<String> = None;
        let mut joins = false;
        if let Some(prev) = prev {
            let short = gaps.iter().all(|g| g.is_some_and(|g| g < MERGE_GAP_MS));
            // 같은 자리(나·상대)여도 목소리가 다르면 다른 사람이에요. 한쪽 목소리를 모르면(임베딩 실패, 너무 짧음)
            // 자리만 봐요.
            let prev_voice = prev.get("voice").and_then(Value::as_str);
            let same_voice = match (prev_voice, voice.as_deref()) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            };
            let same_speaker = prev.get("speaker").and_then(Value::as_str) == speaker && same_voice;
            if short && !truthy(prev.get("fired")) && !st.recent.is_empty() && same_speaker {
                merged_from = prev["utterance_id"].as_str().map(str::to_string);
                text = format!("{} {text}", prev["text"].as_str().unwrap_or_default());
                joins = true;
                if voice.is_none() {
                    voice = prev_voice.map(str::to_string);
                    person = prev
                        .get("person")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
        }
        let who = voice
            .as_deref()
            .map(|v| st.names.get(v).map_or(v, String::as_str))
            .or(speaker.filter(|s| !s.is_empty()))
            .unwrap_or("나");
        let line = format!("{who}: {text}");
        // 앞 대화는 이은 경우 앞 조각의 줄을 뺀 `recent`예요. `recent` 자체는 판단을 기다린 뒤 기록과 함께 바꿔요:
        // 판단을 기다리다 버려진 호출(앱이 그만둠)은 세션에 아무것도 남기지 않아요.
        let context: Vec<String> = st
            .recent
            .iter()
            .take(st.recent.len() - usize::from(joins))
            .cloned()
            .collect();
        let mut rec = record([
            ("utterance_id", json!(uid)),
            ("text", json!(text)),
            ("speaker", json!(speaker)),
            ("voice", json!(voice)),
            ("person", json!(person)),
            ("at", json!(now)),
            ("gap_ms", json!(gap_ms)),
            ("stt_confidence", json!(stt_confidence)),
            ("cards", json!([])),
            ("shown", json!(false)),
            ("merged_from", json!(merged_from)),
            ("_context", json!(context)),
            ("_line", json!(line)),
        ]);
        // 판단만 하는 엔진(iPhone)은 이 엔진에서 찾지 않으니 모델과 색인이 없어도 판단해요.
        let gate_only = engine.gate_only && !self.sample;
        if !self.sample && !engine.model_ready && !gate_only {
            // 검색 모델을 받는 중이에요: 찾을 수 없으니 판단(유료)도 부르지 않아요. 줄은 회의 정리용으로 남아요.
            update(
                &mut rec,
                [
                    ("p_lookup", Value::Null),
                    ("fired", json!(false)),
                    ("p_end", Value::Null),
                    ("ending", json!(false)),
                    ("error", json!(MODEL_MISSING)),
                    ("error_kind", json!(Kind::Model)),
                ],
            );
            remember(&mut st.recent, joins, line);
            let out = public(&rec);
            st.keep(rec, kept);
            return out;
        }
        if !self.lookup()
            || (engine.index_is_empty_in(self.sample) && !self.gate_empty && !gate_only)
        {
            // 기록만 하거나 찾을 게 없어요: 판단을 부르지 않아요. 줄은 회의 정리용으로 남아요.
            // 회의 끝 알아듣기(`p_end`)도 같은 판단 요청이라 기록만 할 때는 없어요.
            update(
                &mut rec,
                [
                    ("p_lookup", Value::Null),
                    ("fired", json!(false)),
                    ("p_end", Value::Null),
                    ("ending", json!(false)),
                ],
            );
            remember(&mut st.recent, joins, line);
            let out = public(&rec);
            st.keep(rec, kept);
            return out;
        }
        match engine.services.gater.gate(&context, &line).await {
            Err(e) => {
                update(
                    &mut rec,
                    [
                        ("p_lookup", Value::Null),
                        ("fired", json!(false)),
                        ("p_end", Value::Null),
                        ("ending", json!(false)),
                        ("error", json!(head(&e.message))),
                        ("error_kind", json!(e.kind)),
                    ],
                );
            }
            Ok(g) => {
                // 같은 요청이 말한 사람이 회의를 끝내는 중인지도 물어요.
                let ending = g.p_end.is_some_and(|p| p >= end_thr);
                update(
                    &mut rec,
                    [
                        ("p_lookup", json!(g.p_lookup)),
                        ("fired", json!(g.p_lookup >= gate_thr)),
                        ("p_end", json!(g.p_end)),
                        ("ending", json!(ending)),
                        ("timing_ms", json!({"gate": g.ms})),
                    ],
                );
                if engine.log_text {
                    let heard = heard_line(
                        &uid,
                        Some(g.p_lookup),
                        g.p_end,
                        merged_from.as_deref(),
                        false,
                        &text,
                    );
                    eprintln!("{heard}");
                }
            }
        }
        remember(&mut st.recent, joins, line);
        let out = public(&rec);
        st.keep(rec, kept);
        out
    }

    /// 판단을 넘은 발화를 찾고 리랭크해요. 그런 발화가 없으면 None이에요(서버는 404).
    /// 맞장구 기록에는 앞 대화와 줄이 없어서 없는 발화로 봐요.
    pub async fn find(&self, engine: &Engine, utterance_id: &str, show_thr: f64) -> Option<Value> {
        let mut guard = self.state.lock().await;
        let st = &mut *guard;
        let i = st
            .log
            .iter()
            .position(|r| r.get("utterance_id").and_then(Value::as_str) == Some(utterance_id))?;
        let rec = &st.log[i];
        let text = rec.get("text").and_then(Value::as_str)?.to_string();
        let context: Vec<String> = rec
            .get("_context")?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        let line = rec.get("_line").and_then(Value::as_str)?.to_string();
        let at = rec.get("at").and_then(Value::as_f64).unwrap_or(0.0);

        if engine.gate_only && !self.sample {
            // iPhone: 카드는 앱이 서버에 물어서 채워요. 질문(`text`)과 앞 대화만 돌려줘요.
            let rec = &mut st.log[i];
            update(rec, [("cards", json!([])), ("context", json!(context))]);
            return Some(public(rec));
        }

        let mut out = match engine
            .answer_in(self.sample, &text, &context, &line, show_thr, 3)
            .await
        {
            Value::Object(out) => out,
            _ => Map::new(),
        };
        let rec = &mut st.log[i];
        // 판단 시간 옆에 검색 시간을 더해요.
        if let Some(Value::Object(timing)) = out.shift_remove("timing_ms") {
            let slot = rec
                .entry("timing_ms")
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(slot) = slot {
                update(slot, timing);
            }
        }
        update(rec, out);
        if let Some(Value::Array(cards)) = rec.get_mut("cards") {
            for c in cards.iter_mut() {
                let last = path_of(c).and_then(|p| st.shown_at.get(p).copied());
                let repeat = at - last.unwrap_or(-1e9) < REPEAT_SECONDS;
                if let Value::Object(c) = c {
                    c.insert("repeat".into(), json!(repeat));
                }
            }
        }
        // 화면에 보인 경우만 "보였다"로 기억해요.
        if truthy(rec.get("shown"))
            && let Some(Value::Array(cards)) = rec.get("cards")
        {
            for c in cards {
                if let Some(p) = path_of(c) {
                    st.shown_at.insert(p.to_string(), at);
                }
            }
        }
        Some(public(rec))
    }

    /// 지금까지의 결정, 할 일, 남은 질문이에요. 맞장구와 뒤 줄에 이어 붙은 조각은 빼요.
    /// 아직 받아쓰는 발화가 있으면 다 기록될 때까지 `LEDGER_WAIT`까지 기다려요. 새 발화가 없으면 지난 결과예요.
    /// `kind`는 사람이 고른 녹음 유형이에요(없으면 엔진이 정해요). 저장된 결과와 유형이 다르면 다시 써요.
    pub async fn ledger(&self, engine: &Engine, kind: Option<NotesKind>) -> Value {
        self.ledger_in(engine, kind, None).await
    }

    /// [`Session::ledger`]를 정한 언어로 써요(설정 → 회의 → "회의 정리 언어", 2026-11-19). 언어가 다르면 지난 정리를 다시 쓰지 않아요.
    pub async fn ledger_in(&self, engine: &Engine, kind: Option<NotesKind>, lang: Option<crate::lang::Lang>) -> Value {
        // 받아쓰는 발화가 끝나기를 5초까지 기다려요: 시간이 다 되면 그때까지의 기록으로 써요.
        let _ = tokio::time::timeout(LEDGER_WAIT, self.until_idle()).await;
        let (heard, lines, attendees, asked) = {
            let st = self.state.lock().await;
            let heard = st.log.len();
            if let Some((n, done_in, out)) = &st.ledger
                && *n == heard
                && *done_in == lang
                && kind.is_none_or(|k| out["kind"] == k.as_str())
            {
                let mut out = out.clone();
                // the person picked the kind the engine had picked: it is their pick now (다시 쓰기 keeps it)
                if kind.is_some() {
                    out["kind_source"] = "user".into();
                }
                return out;
            }
            let joined: HashSet<&str> = st
                .log
                .iter()
                .filter(|r| truthy(r.get("merged_from")))
                .filter_map(|r| r["merged_from"].as_str())
                .collect();
            // 줄마다 보인 이름은 목소리면 앱이 붙인 이름(없으면 키), 아니면 화자예요. 참석자는 (보인 이름, 키)예요:
            // 목소리 없는 줄은 화자 이름이 키예요. 화자가 없는 줄의 "화자"는 사람이 아니라 참석자가 아니에요.
            let mut attendees: Vec<(String, String)> = Vec::new();
            let lines: Vec<(String, String)> = st
                .log
                .iter()
                .filter(|r| !truthy(r.get("skipped")))
                .filter_map(|r| {
                    let uid = r.get("utterance_id")?.as_str()?;
                    if joined.contains(uid) {
                        return None;
                    }
                    let key = r
                        .get("voice")
                        .and_then(Value::as_str)
                        .or_else(|| r.get("speaker").and_then(Value::as_str))
                        .filter(|s| !s.is_empty());
                    let who = match key {
                        Some(key) => {
                            let who = st.names.get(key).map_or(key, String::as_str);
                            if !attendees.iter().any(|(_, k)| k == key) {
                                attendees.push((who.to_string(), key.to_string()));
                            }
                            who
                        }
                        None => "화자",
                    };
                    let text = r.get("text").and_then(Value::as_str).unwrap_or_default();
                    Some((uid.to_string(), format!("[{uid}] {who}: {text}")))
                })
                .collect();
            let asked: HashSet<String> = st
                .log
                .iter()
                .filter(|r| truthy(r.get("fired")))
                .filter_map(|r| r["utterance_id"].as_str().map(str::to_string))
                .collect();
            (heard, lines, attendees, asked)
        };
        // 장부 모델은 자물쇠 밖에서 기다려요: 그동안 들어온 발화는 다음 장부에 들어가요.
        let out = engine.notes_in(&lines, &attendees, &asked, kind, None, lang).await;
        if out.get("error").is_none() {
            self.state.lock().await.ledger = Some((heard, lang, out.clone()));
        }
        out
    }

    /// 앱이 붙인 화자 이름 표(화자 키 → 이름)로 통째로 바꿔요. 다음 발화의 앞 대화 줄과 다음 장부부터 써요:
    /// 이미 쓴 앞 대화 줄은 그대로예요. 이름이 바뀌면 장부도 다시 써요(장부 모델에 보내는 줄이 달라요).
    pub async fn set_names(&self, names: HashMap<String, String>) {
        let mut st = self.state.lock().await;
        // 이름은 한 줄로, 앞뒤 공백 없이 써요. 빈 이름은 이름이 없는 거예요.
        let names: HashMap<String, String> = names
            .into_iter()
            .map(|(k, v)| (k, one_line(&v).trim().to_string()))
            .filter(|(_, v)| !v.is_empty())
            .collect();
        if st.names != names {
            st.ledger = None;
        }
        st.names = names;
    }

    /// 받아쓰는 발화가 없을 때까지 기다려요. 깨울 알림을 먼저 걸고 수를 봐요: 그 사이에 끝난 받아쓰기도 놓치지 않아요.
    async fn until_idle(&self) {
        loop {
            let notified = self.idle.notified();
            if self.busy.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }

    /// 들은 발화 수, 판단을 넘은 수, 답한 것(보인 카드), 답하지 못한 것이에요.
    pub async fn summary(&self) -> Value {
        let st = self.state.lock().await;
        let fired: Vec<&Record> = st.log.iter().filter(|r| truthy(r.get("fired"))).collect();
        let answered: Vec<Value> = fired
            .iter()
            .filter(|r| truthy(r.get("shown")))
            .map(|r| public(r))
            .collect();
        let unanswered: Vec<Value> = fired
            .iter()
            .filter(|r| !truthy(r.get("shown")))
            .map(|r| {
                json!({"utterance_id": r["utterance_id"], "text": r["text"],
                       "best_p": r.get("best_p").cloned().unwrap_or(Value::Null)})
            })
            .collect();
        json!({"session_id": self.id, "utterances": st.log.len(), "fired": fired.len(),
               "answered": answered, "unanswered": unanswered})
    }

    /// 사람이 회의 중에 말 몇 개(`utterances`, 엔진 id)에 그 화자와 다른 이름을 줬어요(이 말만). 그 말들을 원래 목소리에서
    /// 떼어 `into` 목소리(같은 갈래) 또는 새 목소리로 옮기고, 다음 발화부터 그쪽과도 견줘요(`Voices::split`). 이어 붙인
    /// 줄(`merged_from`)은 앞 조각까지 함께 옮겨요. 기록과 `/voices`의 목소리도 새 목소리로 바뀌어요. 옮긴 목소리 이름이고,
    /// 목소리가 붙은 말이 없거나 여러 목소리에 걸치면 Err예요.
    pub async fn split(
        &self,
        utterances: &[String],
        into: Option<&str>,
    ) -> Result<String, &'static str> {
        let mut guard = self.state.lock().await;
        let st = &mut *guard;
        // 이어 붙인 줄은 앞 조각들까지
        let mut ids: Vec<String> = Vec::new();
        for u in utterances {
            let mut cur = Some(u.clone());
            while let Some(id) = cur {
                if ids.contains(&id) {
                    break;
                }
                cur = st
                    .log
                    .iter()
                    .find(|r| r["utterance_id"].as_str() == Some(id.as_str()))
                    .and_then(|r| r["merged_from"].as_str().map(str::to_string));
                ids.push(id);
            }
        }
        let moved: Vec<&Kept> = st.prints.iter().filter(|p| ids.contains(&p.uid)).collect();
        let Some(from) = moved.first().map(|p| p.voice.clone()) else {
            return Err(SPLIT_NONE);
        };
        if moved.iter().any(|p| p.voice != from) {
            return Err(SPLIT_MIXED);
        }
        let vectors: Vec<(Vec<f32>, Vec<f32>)> = moved
            .iter()
            .map(|p| (p.vector.clone(), p.ident.clone()))
            .collect();
        let seconds: f32 = moved.iter().map(|p| p.seconds).sum();
        let mut rest: Vec<(Vec<f32>, Vec<f32>)> = st
            .prints
            .iter()
            .filter(|p| p.voice == from && !ids.contains(&p.uid))
            .map(|p| (p.vector.clone(), p.ident.clone()))
            .collect();
        // 다시 넘겨받은 목소리는 그때의 벡터가 남은 말 하나처럼 들어가요.
        if let Some(back) = st
            .voices
            .known()
            .into_iter()
            .find(|k| k.voice == from)
            .and_then(|k| k.restored)
        {
            rest.push((back.diar, back.ident));
        }
        let to = st
            .voices
            .split(&from, &vectors, &rest, seconds, into)
            .ok_or(SPLIT_NONE)?;
        for p in st.prints.iter_mut().filter(|p| ids.contains(&p.uid)) {
            p.voice = to.clone();
        }
        for r in st.log.iter_mut().filter(|r| {
            r["utterance_id"]
                .as_str()
                .is_some_and(|u| ids.iter().any(|i| i == u))
        }) {
            r.insert("voice".into(), json!(to));
        }
        st.ledger = None;
        Ok(to)
    }

    /// 이 회의의 목소리들이에요: `{"voices": [{"voice", "channel", "vector", "diar_vector"?, "seconds", "person",
    /// "near"}]}`. `vector`는 ERes2Net 대표 벡터(기억한 참석자와 같은 길이)이고, 화자 나누기 모델로 묶는 회의면
    /// `diar_vector`도 있어요(엔진이 다시 뜨면 `restore`로 다시 받아요). 대표 벡터는 그 목소리가 붙은 발화의 임베딩을
    /// `exclude` 줄만 빼고 더해 정규화하고, 초도 같은 발화로 더해요. 말이 모두 빠진 목소리는 없어요. 다시 넘겨받은
    /// 목소리의 벡터와 초는 빼지 못하는 한 줄처럼 들어가요.
    pub async fn voices(&self, exclude: &[String]) -> Value {
        let st = self.state.lock().await;
        let dual = st.voices.tuning().diar_dim.is_some();
        let add = |sum: &mut Option<Vec<f32>>, v: &[f32]| match sum {
            Some(s) if s.len() == v.len() => s.iter_mut().zip(v).for_each(|(s, x)| *s += x),
            Some(_) => {}
            None => *sum = Some(v.to_vec()),
        };
        let out: Vec<Value> = st
            .voices
            .known()
            .into_iter()
            .filter_map(|k| {
                let (mut ident, mut diar, mut seconds) = match k.restored {
                    Some(b) => (
                        Some(b.ident),
                        Some(b.diar).filter(|d| !d.is_empty()),
                        b.seconds,
                    ),
                    None => (None, None, 0.0),
                };
                for p in &st.prints {
                    if p.voice != k.voice || exclude.contains(&p.uid) {
                        continue;
                    }
                    add(&mut ident, &p.ident);
                    add(&mut diar, &p.vector);
                    seconds += p.seconds;
                }
                let mut v = json!({"voice": k.voice, "channel": k.channel,
                                   "vector": normalize(&ident?), "seconds": seconds,
                                   "person": k.person, "near": k.near});
                if dual && let Some(d) = diar {
                    v["diar_vector"] = json!(normalize(&d));
                }
                Some(v)
            })
            .collect();
        json!({"voices": out})
    }

    /// 지금 앞 대화로 쓰는 줄들이에요(오래된 것부터, 많아야 세 줄).
    pub async fn recent(&self) -> Vec<String> {
        self.state.lock().await.recent.iter().cloned().collect()
    }
}

/// 받아쓰는 수를 내리고 장부를 깨워요. `hear_audio`가 어느 갈래로 끝나든, 앱이 그만둬 future가 버려져도 내려가요.
struct Busy<'a>(&'a Session);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.busy.fetch_sub(1, Ordering::SeqCst);
        self.0.idle.notify_waiters();
    }
}

/// 개발용 `[heard]` 로그 한 줄이에요(`TACIE_LOG_TEXT=1`). 글은 한 줄로 바꿔요: 앱이 그 앞말로 줄째 버려요.
/// 맞장구는 `[heard] {id} skipped=backchannel {글}`, 판단한 발화는 `[heard] {id} p=0.90 end=0.12 (joined {id}) {글}`이에요.
pub fn heard_line(
    uid: &str,
    p_lookup: Option<f64>,
    p_end: Option<f64>,
    merged_from: Option<&str>,
    skipped: bool,
    text: &str,
) -> String {
    let text = one_line(text);
    if skipped {
        return format!("[heard] {uid} skipped=backchannel {text}");
    }
    let p = p_lookup.map_or_else(|| "None".to_string(), |p| format!("{p:.2}"));
    let end = p_end.map_or_else(String::new, |e| format!(" end={e:.2}"));
    let joined = merged_from.map_or_else(String::new, |m| format!(" (joined {m})"));
    format!("[heard] {uid} p={p}{end}{joined} {text}")
}

/// 들은 줄을 앞 대화에 넣어요. 이은 줄이면 앞 조각의 줄을 먼저 빼요. 많아야 세 줄이에요.
fn remember(recent: &mut VecDeque<String>, joins: bool, line: String) {
    if joins {
        recent.pop_back();
    }
    if recent.len() == RECENT_LINES {
        recent.pop_front();
    }
    recent.push_back(line);
}

/// 키 순서대로 기록을 만들어요.
fn record<const N: usize>(pairs: [(&str, Value); N]) -> Record {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// 기록에 값을 넣어요: 있던 키는 그 자리에서 값만 바꾸고, 새 키는 끝에 붙여요.
fn update<K: Into<String>>(rec: &mut Record, pairs: impl IntoIterator<Item = (K, Value)>) {
    for (k, v) in pairs {
        rec.insert(k.into(), v);
    }
}

/// 밖으로 내보내는 기록이에요: `_`로 시작하는 키를 빼요.
fn public(rec: &Record) -> Value {
    Value::Object(
        rec.iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

/// 참/거짓으로 읽어요(null, false, 0, 빈 글은 거짓이에요).
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn path_of(card: &Value) -> Option<&str> {
    card.get("path").and_then(Value::as_str)
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// 화자 임베딩 작업의 결과예요. 실패해도 발화는 그대로라 이름만 없어요.
fn joined_print(
    r: Result<anyhow::Result<Option<Voiceprint>>, tokio::task::JoinError>,
) -> Option<Voiceprint> {
    match r {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            eprintln!("voice: {}", head(&format!("{e:#}")).replace('\n', " "));
            None
        }
        Err(e) => {
            eprintln!("voice: {e}");
            None
        }
    }
}

/// 이 발화를 어디서 받아썼는지(`stt`)와, 로컬 모델이 고장 났는지(`stt_broken`)를 붙여요. 앱이 설정 화면에 보여 줘요.
fn tag_stt_map(rec: &mut Map<String, Value>, via: SttVia, engine: &Engine) {
    rec.insert(
        "stt".into(),
        json!(match via {
            SttVia::Local => "local",
            SttVia::Server => "server",
            SttVia::Stream => "stream",
        }),
    );
    if engine.stt.as_ref().is_some_and(|s| s.status().broken) {
        rec.insert("stt_broken".into(), json!(true));
    }
}

fn tag_stt(rec: &mut Value, via: SttVia, engine: &Engine) {
    if let Value::Object(rec) = rec {
        tag_stt_map(rec, via, engine);
    }
}

/// 돌려주는 기록에 받아쓰기 시간과 단어 시각을 붙여요. 나뉜 뒤 조각(`offset` 초)은 `offset_ms`가 붙고 단어 시각은 그
/// 조각 시작부터예요.
fn with_stt(
    mut rec: Value,
    stt: &crate::services::Transcript,
    words: &[crate::services::SpokenWord],
    offset: Option<f64>,
    engine: &Engine,
) -> Value {
    // `timing_ms`의 맨 앞에 stt를 넣어요: `timing_ms`가 있던 키면 제자리, 없으면 끝에 붙어요.
    // 돌려주는 사본만 고쳐요: 세션에 남은 기록에는 받아쓰기 시간이 없어요.
    if let Value::Object(rec) = &mut rec {
        let mut timing = Map::new();
        timing.insert("stt".into(), json!(stt.ms));
        if let Some(Value::Object(before)) = rec.get("timing_ms") {
            update(&mut timing, before.clone());
        }
        rec.insert("timing_ms".into(), Value::Object(timing));
        if let Some(o) = offset {
            rec.insert("offset_ms".into(), json!((o * 1000.0).round() as u64));
        }
        tag_stt_map(rec, stt.via, engine);
        // 앱이 녹음 위치로 옮기는 단어 시각이에요. 돌려주는 사본에만, 시각이 있을 때만 마지막 키로 붙어요.
        if !words.is_empty() {
            let base = offset.unwrap_or(0.0);
            let words = words
                .iter()
                .map(|w| json!({"text": w.text, "start": w.start - base, "end": w.end - base}));
            rec.insert("words".into(), Value::Array(words.collect()));
        }
    }
    rec
}
