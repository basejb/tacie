//! Groups voices by person within a meeting (docs/04 4절 "목소리로 화자 묶기"). Utterances arrive one at a time; if the
//! closest person is at or above the threshold it is that person, otherwise a new person. The values are measured
//! on public data (Zeroth-Korean, AMI).

use serde::Serialize;
use tacie_embed::voice::Voiceprint;

/// 같은 사람으로 보는 코사인 기준이에요(ERes2Net, AMI 실제 회의 92%·방 마이크 85%).
pub const THRESHOLD: f32 = 0.4;
/// 이보다 짧은 발화는 새 사람을 만들지 않아요. 없으면 "네", "음" 때문에 회의당 11–14명이 나왔어요. 2026-11-08 2.0 → 3.0:
/// 웃음·겹친 말 같은 2–3초 조각이 한 번 쓰고 마는 작은 화자를 만들었어요(docs/01 "작은 화자 줄이기").
pub const MIN_NEW_SECONDS: f32 = 3.0;
/// 기존 화자 누구와도 이보다 먼 목소리(코사인)는 `FAR_MIN_SECONDS`만 말해도 새 화자가 돼요. 3초 기준 때문에 손님의 짧은
/// 첫인사("반갑습니다. 안녕하세요.")가 진행자 줄에 붙었어요. 잠깐 섞인 소리는 대개 0.3–0.4 사이예요(docs/01 "작은 화자 줄이기").
pub const FAR_THRESHOLD: f32 = 0.3;
pub const FAR_MIN_SECONDS: f32 = 2.0;
/// 회의 전체(모든 갈래)에 둘 수 있는 사람 수예요. 넘으면 같은 갈래에서 가장 가까운 사람에게 붙여요.
pub const MAX_VOICES: usize = 8;

/// 목소리가 들어온 갈래예요: 대면 회의의 방 마이크, 원격 회의의 내 마이크와 상대(통화 소리)예요. 번호는 갈래를
/// 가로질러 하나로 세지만, 사람은 같은 갈래 안에서만 묶어요(같은 사람이 두 갈래로 들어오면 두 사람이에요).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Room,
    Mic,
    Call,
}

/// 기억한 사람으로 보는 코사인 기준이에요. 틀린 이름이 1% 이하인 가장 낮은 값은 0.60이지만, 낭독 영어로 잰 값이 낙관일 수 있고
/// 틀린 이름보다 "화자 N"이 낫고 다른 사람 최대가 0.604라 여유를 둔 0.65예요(docs/01, `eval/voice_person_threshold.py`).
pub const PERSON_THRESHOLD: f32 = 0.65;

/// 첫 발화로 못 알아본 목소리는 말이 이만큼(초) 쌓이면 그 중심으로 기억한 사람과 다시 견줘요. 발화 하나는 흔들려서
/// 같은 사람도 기준 밑으로 자주 내려가요. AMI 방 마이크 뒷회의 12개에서 첫 발화만 보면 기억한 사람 말의 55%에
/// 이름이 붙었고, 다시 견주면 86%였어요(docs/01 "기억한 사람 다시 견주기", `eval/voice_person_recall.py`).
pub const AGAIN_SECONDS: f32 = 8.0;
/// 다시 견줄 때(중심과 기억한 사람)의 코사인 기준이에요. 처음 듣는 사람에게 이름이 붙는 건 0.55에서 13%, 0.60부터 0%라
/// 첫 발화와 같은 0.65로 여유를 뒀어요.
pub const AGAIN_THRESHOLD: f32 = PERSON_THRESHOLD;

/// 한 화자에 두 사람이 묶였는지 볼 때 쓰는 줄의 최소 길이(초)예요. 짧은 줄은 임베딩이 흔들려요.
pub const SPLIT_LINE_SECONDS: f32 = 2.0;
/// 이만큼 긴 줄이 쌓여야 나눠 봐요.
pub const SPLIT_MIN_LINES: usize = 6;
/// 한 화자에 말이 이만큼(초) 쌓여야 나눠 봐요.
pub const SPLIT_MIN_SECONDS: f32 = 30.0;
/// 두 무리 중심의 코사인이 이보다 낮아야 두 사람으로 봐요. 유튜브 대담의 두 진행자(0.45–0.66)가 갈리는 값이에요.
/// 2026-11-08 0.65 → 0.70: 진행자 둘의 두 무리 코사인이 받아쓰기에 따라 0.657–0.673을 오가서 들을 때마다 갈리거나 묶였어요(docs/01
/// "자동 나누기 흔들림"). 실루엣은 0.75 안팎으로 안정적이라 그쪽을 0.72로 조였어요.
pub const SPLIT_MAX_COS: f32 = 0.70;
/// 줄들이 두 무리로 또렷하게 모여야 해요(가중 실루엣, 코사인 거리). 한 사람의 목소리가 조금씩 달라진 경우(0.47–0.57)를
/// 두 사람(0.70 이상)과 가려요(docs/01 "비슷한 두 목소리 자동으로 나누기").
pub const SPLIT_MIN_SILHOUETTE: f32 = 0.72;
/// 작은 무리도 이만큼(초)과 이 몫 이상 말해야 떼어요.
pub const SPLIT_MIN_SIDE_SECONDS: f32 = 10.0;
pub const SPLIT_MIN_SIDE_SHARE: f32 = 0.2;

/// 앱이 넘기는 화자 번호(`restore`, `after`)의 한도예요. 다음 번호(+1)가 넘치지 않게 막아요.
pub const MAX_NUMBER: usize = 10000;

/// 화자를 묶는 값이에요. 회의 안에서 화자를 묶는 임베딩(`Voiceprint::vector`)의 모델마다 달라요. 기억한 참석자와 견주는
/// 값(`PERSON_THRESHOLD`, `AGAIN_THRESHOLD`)은 늘 ERes2Net 임베딩(`Voiceprint::ident`)이라 여기 없어요.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    pub threshold: f32,
    pub min_new: f32,
    pub far: f32,
    pub far_min: f32,
    pub split_cos: f32,
    pub split_sil: f32,
    /// 발화 안에서 화자가 바뀐 곳을 찾는 코사인 기준이에요(`split::change_points`).
    pub change_sim: f32,
    /// 화자 나누기 모델의 임베딩 길이예요. None이면 ERes2Net 하나로 묶어요.
    pub diar_dim: Option<usize>,
}

impl Tuning {
    /// ERes2Net 하나로 묶을 때예요(위 상수들).
    pub const ERES: Tuning = Tuning {
        threshold: THRESHOLD,
        min_new: MIN_NEW_SECONDS,
        far: FAR_THRESHOLD,
        far_min: FAR_MIN_SECONDS,
        split_cos: SPLIT_MAX_COS,
        split_sil: SPLIT_MIN_SILHOUETTE,
        change_sim: crate::split::CHANGE_SIM,
        diar_dim: None,
    };

    /// CAM++ 파인튜닝(AMI·Zeroth-Korean으로 더 학습, 192차원)으로 묶을 때예요. 값은 맞추는 녹음(VoxConverse dev 15개 +
    /// AMI 시험 EN·ES 8개)에서만 골랐어요. 처음 보는 녹음 83개 89.3% → 93.0%, 실제 회의실 회의(AliMeeting·AISHELL-4)
    /// 48개 91.7% → 93.3%예요(docs/01 "화자 모델 파인튜닝").
    pub const fn diar(dim: usize) -> Tuning {
        Tuning {
            threshold: 0.55,
            min_new: 3.5,
            far: 0.4,
            far_min: 2.5,
            split_cos: 0.70,
            split_sil: 0.72,
            change_sim: 0.6,
            diar_dim: Some(dim),
        }
    }

    /// 화자 임베딩 모델이 알려 준 화자 나누기 임베딩 길이로 골라요.
    pub fn for_dim(diar_dim: Option<usize>) -> Tuning {
        diar_dim.map_or(Tuning::ERES, Tuning::diar)
    }
}

/// 지난 회의에서 이름을 붙인 사람이에요(앱이 기억해 회의를 열 때 넘겨요). `vector`는 대표 벡터예요.
#[derive(Debug, Clone)]
pub struct Person {
    pub id: String,
    pub vector: Vec<f32>,
}

/// 회의 중에 엔진이 다시 떴을 때 앱이 다시 넘기는 목소리예요. 번호를 그대로 이어 써요. `vector`는 ERes2Net(기억한
/// 참석자와 같은 길이), `diar`는 화자 나누기 모델의 대표 벡터예요(그 모델로 묶던 회의만).
#[derive(Debug, Clone)]
pub struct Restored {
    pub number: usize,
    pub channel: Channel,
    pub vector: Vec<f32>,
    pub diar: Option<Vec<f32>>,
    pub seconds: f32,
    /// 다시 뜨기 전에 이 목소리에 이어 둔 기억한 사람이에요.
    pub person: Option<String>,
}

/// 발화 하나를 말한 목소리("화자 2")와, 그 목소리가 기억한 사람이면 그 사람 id예요.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assigned {
    pub voice: String,
    pub person: Option<String>,
}

/// 목소리 하나를 밖에 보이는 값이에요(`/voices`).
#[derive(Debug, Clone)]
pub struct Known {
    pub voice: String,
    pub channel: Channel,
    pub person: Option<String>,
    pub near: Option<String>,
    /// 다시 넘겨받은 목소리면 그때의 벡터(화자 나누기, ERes2Net)와 초예요.
    pub restored: Option<Back>,
}

/// 다시 넘겨받은 목소리의 그때 값이에요: 화자 나누기 벡터(모르면 빈 벡터), ERes2Net 벡터, 초예요.
#[derive(Debug, Clone)]
pub struct Back {
    pub diar: Vec<f32>,
    pub ident: Vec<f32>,
    pub seconds: f32,
}

struct Voice {
    channel: Channel,
    /// 첫 발화 순서대로 1부터 붙인 번호예요. 바뀌지 않아요.
    number: usize,
    /// 붙은 임베딩(화자 나누기)의 합과 그 정규화한 중심이에요. 다시 넘겨받은 목소리가 그 벡터를 몰랐으면 비어 있고,
    /// 그 목소리에는 붙지 않아요.
    sum: Vec<f32>,
    center: Vec<f32>,
    /// 붙은 ERes2Net 임베딩의 합과 중심이에요. 기억한 참석자와 견줄 때 써요. 화자 나누기 모델이 없으면 위와 같아요.
    ident_sum: Vec<f32>,
    ident_center: Vec<f32>,
    /// 이 목소리가 기억한 사람이에요(`PERSON_THRESHOLD` 이상). 한 사람은 회의에서 한 목소리에만 이어요.
    person: Option<String>,
    /// 기억한 사람과 꽤 가깝지만 확실하지 않을 때 그 사람이에요(`THRESHOLD` 이상). 앱이 고를 때 먼저 보여 줘요.
    near: Option<String>,
    /// 다시 넘겨받은 벡터와 초예요(`Restored`).
    restored: Option<Back>,
    /// 중심에 들어간 말의 초예요(다시 넘겨받은 초 포함). `AGAIN_SECONDS`를 넘으면 기억한 사람과 다시 견줘요.
    seconds: f32,
    /// 이 회의에서 붙은 긴 줄(`SPLIT_LINE_SECONDS` 이상)의 임베딩과 초예요. 두 사람이 묶였는지 볼 때 써요(`auto_split`).
    lines: Vec<(Vec<f32>, f32)>,
    /// `lines`와 같은 줄의 ERes2Net 임베딩이에요. 나눌 때 두 쪽의 참석자 확인 중심을 다시 세요.
    line_idents: Vec<Vec<f32>>,
}

pub struct Voices {
    voices: Vec<Voice>,
    people: Vec<Person>,
    /// 새 목소리 번호는 이 번호보다 커요(`after`).
    after: usize,
    tuning: Tuning,
}

impl Default for Voices {
    fn default() -> Self {
        Self::with(Vec::new(), Vec::new())
    }
}

impl Voices {
    /// 기억한 사람들과 다시 넘겨받은 목소리로 시작해요(ERes2Net 하나로 묶어요). 벡터는 정규화해요.
    pub fn with(people: Vec<Person>, restore: Vec<Restored>) -> Self {
        Self::tuned(people, restore, Tuning::ERES)
    }

    /// `with`와 같은데 화자를 `tuning` 값(과 그 모델의 임베딩)으로 묶어요. 화자 나누기 모델로 묶는 회의에서 다시 넘겨받은
    /// 목소리가 그 벡터(`diar`, 같은 길이)를 모르면 번호와 이은 사람만 남고 발화는 붙지 않아요.
    pub fn tuned(people: Vec<Person>, restore: Vec<Restored>, tuning: Tuning) -> Self {
        let people = people
            .into_iter()
            .map(|p| Person {
                vector: normalize(&p.vector),
                ..p
            })
            .collect();
        let mut voices: Vec<Voice> = restore
            .into_iter()
            .map(|r| {
                let ident = normalize(&r.vector);
                let center = match tuning.diar_dim {
                    None => ident.clone(),
                    Some(d) => r
                        .diar
                        .filter(|v| v.len() == d)
                        .map(|v| normalize(&v))
                        .unwrap_or_default(),
                };
                Voice {
                    channel: r.channel,
                    number: r.number,
                    sum: center.clone(),
                    restored: Some(Back {
                        diar: center.clone(),
                        ident: ident.clone(),
                        seconds: r.seconds,
                    }),
                    seconds: r.seconds,
                    center,
                    ident_sum: ident.clone(),
                    ident_center: ident,
                    person: r.person,
                    near: None,
                    lines: Vec::new(),
                    line_idents: Vec::new(),
                }
            })
            .collect();
        voices.sort_by_key(|v| v.number);
        Self {
            voices,
            people,
            after: 0,
            tuning,
        }
    }

    /// 화자를 묶는 값이에요.
    pub fn tuning(&self) -> Tuning {
        self.tuning
    }

    /// 새 목소리를 `after` 다음 번호부터 붙여요. 앱이 벡터를 받기 전에 엔진이 다시 떠서 넘기지 못한
    /// "화자 N"이 있어도 그 번호를 다시 쓰지 않아요.
    pub fn after(self, after: usize) -> Self {
        Self { after, ..self }
    }

    /// 이 발화를 말한 사람이에요("화자 2"). 새 사람을 못 만들 때(짧은 발화, 자리 없음) 같은 갈래에 붙일
    /// 사람이 없으면 None이에요. 새 목소리는 먼저 기억한 사람들과 견줘요: 번호는 그대로예요.
    pub fn assign(&mut self, channel: Channel, print: &Voiceprint) -> Option<Assigned> {
        let e = &print.vector;
        let id = print.ident();
        let t = self.tuning;
        let best = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.channel == channel)
            .map(|(k, v)| (k, dot(&v.center, e)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let k = match best {
            Some((k, sim)) if sim >= t.threshold => {
                let v = &mut self.voices[k];
                v.sum.iter_mut().zip(e).for_each(|(s, x)| *s += x);
                v.center = normalize(&v.sum);
                v.ident_sum.iter_mut().zip(id).for_each(|(s, x)| *s += x);
                v.ident_center = normalize(&v.ident_sum);
                v.seconds += print.seconds;
                if print.seconds >= SPLIT_LINE_SECONDS {
                    v.lines.push((e.clone(), print.seconds));
                    v.line_idents.push(id.to_vec());
                }
                self.again(k);
                self.auto_split(k, e)
            }
            // 짧거나 자리가 없어서 붙은 먼 발화는 중심을 옮기지 않아요.
            Some((k, sim))
                if print.seconds < if sim < t.far { t.far_min } else { t.min_new }
                    || self.voices.len() >= MAX_VOICES =>
            {
                k
            }
            None if print.seconds < t.min_new || self.voices.len() >= MAX_VOICES => {
                return None;
            }
            _ => {
                let (person, near) = self.match_person(id, PERSON_THRESHOLD);
                let number = self
                    .voices
                    .iter()
                    .map(|v| v.number)
                    .max()
                    .unwrap_or(0)
                    .max(self.after)
                    + 1;
                self.voices.push(Voice {
                    channel,
                    number,
                    sum: e.clone(),
                    center: e.clone(),
                    ident_sum: id.to_vec(),
                    ident_center: id.to_vec(),
                    person,
                    near,
                    restored: None,
                    seconds: print.seconds,
                    lines: if print.seconds >= SPLIT_LINE_SECONDS {
                        vec![(e.clone(), print.seconds)]
                    } else {
                        Vec::new()
                    },
                    line_idents: if print.seconds >= SPLIT_LINE_SECONDS {
                        vec![id.to_vec()]
                    } else {
                        Vec::new()
                    },
                });
                self.voices.len() - 1
            }
        };
        let v = &self.voices[k];
        Some(Assigned {
            voice: label(v.number),
            person: v.person.clone(),
        })
    }

    /// 목소리 `k`의 긴 줄들이 서로 먼 두 무리로 또렷하게 갈리면 두 사람이 묶인 거예요. 말을 더 한 무리는 `k`에 두고,
    /// 다른 무리로 새 목소리를 만들어요. 이미 붙은 줄의 이름은 바꾸지 않고 다음 발화부터 두 쪽과 견줘요. 유튜브 대담의
    /// 두 진행자, VoxConverse 15개 중 4개에서 섞인 두 사람이 갈렸어요(docs/01 "비슷한 두 목소리 자동으로 나누기").
    /// 방금 붙은 발화 `e`가 이제 속한 목소리예요.
    fn auto_split(&mut self, k: usize, e: &[f32]) -> usize {
        let v = &self.voices[k];
        if self.voices.len() >= MAX_VOICES
            || v.seconds < SPLIT_MIN_SECONDS
            || v.lines.len() < SPLIT_MIN_LINES
        {
            return k;
        }
        let Some(groups) = two_groups(&v.lines) else {
            return k;
        };
        let (big, small) = groups;
        let total = big.seconds + small.seconds;
        if dot(&big.center, &small.center) >= self.tuning.split_cos
            || groups_silhouette(&v.lines, &big, &small) < self.tuning.split_sil
            || small.seconds < SPLIT_MIN_SIDE_SECONDS
            || small.seconds / total < SPLIT_MIN_SIDE_SHARE
        {
            return k;
        }
        let channel = v.channel;
        let take = |g: &Group, lines: &[(Vec<f32>, f32)]| -> Vec<(Vec<f32>, f32)> {
            g.members.iter().map(|&i| lines[i].clone()).collect()
        };
        let keep = take(&big, &v.lines);
        let gone = take(&small, &v.lines);
        let ids_of = |g: &Group| -> Vec<Vec<f32>> {
            g.members
                .iter()
                .map(|&i| v.line_idents[i].clone())
                .collect()
        };
        let (keep_ids, gone_ids) = (ids_of(&big), ids_of(&small));
        let sum_of = |ls: &[(Vec<f32>, f32)]| {
            let mut s = vec![0.0f32; e.len()];
            for (x, _) in ls {
                s.iter_mut().zip(x).for_each(|(a, b)| *a += b);
            }
            s
        };
        let id_sum_of = |ls: &[Vec<f32>]| {
            let mut s = vec![0.0f32; ls.first().map_or(0, Vec::len)];
            for x in ls {
                s.iter_mut().zip(x).for_each(|(a, b)| *a += b);
            }
            s
        };
        let (keep_id, gone_id) = (id_sum_of(&keep_ids), id_sum_of(&gone_ids));
        let v = &mut self.voices[k];
        v.sum = sum_of(&keep);
        v.center = big.center.clone();
        v.ident_center = normalize(&keep_id);
        v.ident_sum = keep_id;
        v.seconds = big.seconds;
        v.lines = keep;
        v.line_idents = keep_ids;
        let number = self.next_number();
        self.voices.push(Voice {
            channel,
            number,
            sum: sum_of(&gone),
            center: small.center.clone(),
            ident_center: normalize(&gone_id),
            ident_sum: gone_id,
            person: None,
            near: None,
            restored: None,
            seconds: small.seconds,
            lines: gone,
            line_idents: gone_ids,
        });
        let new = self.voices.len() - 1;
        self.again(new);
        if dot(e, &big.center) >= dot(e, &small.center) {
            k
        } else {
            new
        }
    }

    fn next_number(&self) -> usize {
        self.voices
            .iter()
            .map(|v| v.number)
            .max()
            .unwrap_or(0)
            .max(self.after)
            + 1
    }

    /// 아직 기억한 사람이 아닌 목소리 `k`에 말이 `AGAIN_SECONDS` 넘게 쌓였으면 그 중심으로 다시 견줘요. 이으면 그 뒤로
    /// 풀지 않아요(번호처럼 회의 중에 이름이 오가지 않게). 가까운 사람도 중심 기준으로 고쳐요.
    fn again(&mut self, k: usize) {
        let v = &self.voices[k];
        if v.person.is_some() || v.seconds < AGAIN_SECONDS {
            return;
        }
        let (person, near) = self.match_person(&v.ident_center, AGAIN_THRESHOLD);
        let v = &mut self.voices[k];
        v.person = person;
        v.near = near;
    }

    /// ERes2Net 임베딩 `e`에 이을 기억한 사람(`linked` 이상)과 가까운 사람(`THRESHOLD` 이상)이에요. 가장 가까운 사람만 봐요:
    /// 그 사람이 이미 이 회의의 다른 목소리면 잇지 않고 가까운 사람으로만 남겨요.
    fn match_person(&self, e: &[f32], linked: f32) -> (Option<String>, Option<String>) {
        let Some((p, sim)) = self
            .people
            .iter()
            // a CAM++-only engine (iPhone) has no ERes2Net vector to compare with remembered people
            .filter(|p| p.vector.len() == e.len())
            .map(|p| (p, dot(&p.vector, e)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
        else {
            return (None, None);
        };
        let taken = self.voices.iter().any(|v| v.person.as_ref() == Some(&p.id));
        if sim >= linked && !taken {
            (Some(p.id.clone()), None)
        } else if sim >= THRESHOLD {
            (None, Some(p.id.clone()))
        } else {
            (None, None)
        }
    }

    /// 이 회의의 목소리들이에요(번호 순서).
    /// 목소리("화자 2")의 지금 중심(정규화한 벡터)이에요. 경계 다시 맞추기가 단어를 견줘요(`split::best_cut`).
    pub fn center(&self, voice: &str) -> Option<&[f32]> {
        self.voices
            .iter()
            .find(|v| label(v.number) == voice && !v.center.is_empty())
            .map(|v| v.center.as_slice())
    }

    pub fn known(&self) -> Vec<Known> {
        let mut out: Vec<Known> = self
            .voices
            .iter()
            .map(|v| Known {
                voice: label(v.number),
                channel: v.channel,
                person: v.person.clone(),
                near: v.near.clone(),
                restored: v.restored.clone(),
            })
            .collect();
        out.sort_by_key(|k| number_of(&k.voice));
        out
    }

    /// 사람이 회의 중에 목소리 `from`의 말 몇 개(`moved`)에 다른 이름을 줬어요(이 말만): 두 사람이 한 목소리로 묶인
    /// 거예요. 그 말들을 `into`(같은 갈래의 다른 목소리) 또는 새 목소리로 떼어 내고, `from`의 중심은 남은 말(`rest`)로
    /// 다시 세요. 다음 발화부터 두 쪽과 견줘요. AMI 비슷한 목소리 쌍에서 한 줄만 고쳐도 그 사람의 뒤 발화 64%가 맞는
    /// 쪽으로 갔어요(전에는 0%, docs/01 "회의 중 화자 바로잡기"). 남은 말 중 비슷한 말까지 옮겨도 나아지지 않았어요.
    /// 떼어 낸 목소리 이름("화자 7")이고, `from`을 모르거나 `moved`가 비었으면 None이에요. 말마다 (화자 나누기 벡터,
    /// ERes2Net 벡터)예요.
    pub fn split(
        &mut self,
        from: &str,
        moved: &[(Vec<f32>, Vec<f32>)],
        rest: &[(Vec<f32>, Vec<f32>)],
        seconds: f32,
        into: Option<&str>,
    ) -> Option<String> {
        let k = self.voices.iter().position(|v| label(v.number) == from)?;
        let (dim, id_dim) = moved.first().map(|(v, i)| (v.len(), i.len()))?;
        let channel = self.voices[k].channel;
        let sum = |vs: &[(Vec<f32>, Vec<f32>)], ident: bool| {
            let mut s = vec![0.0f32; if ident { id_dim } else { dim }];
            for (v, i) in vs {
                let x = normalize(if ident { i } else { v });
                s.iter_mut().zip(&x).for_each(|(a, b)| *a += b);
            }
            s
        };
        let rest: Vec<(Vec<f32>, Vec<f32>)> = rest
            .iter()
            .filter(|(v, i)| v.len() == dim && i.len() == id_dim)
            .cloned()
            .collect();
        let (kept, kept_id) = (sum(&rest, false), sum(&rest, true));
        let (gone, gone_id) = (sum(moved, false), sum(moved, true));
        // 사람이 고친 뒤로는 그 전 줄로 다시 나누지 않아요(어느 줄이 어느 쪽인지 여기서는 몰라요).
        self.voices[k].lines.clear();
        self.voices[k].line_idents.clear();
        // 남은 말이 하나도 없으면(모두 떼어 냈으면) 원래 중심을 그대로 둬요.
        if kept.iter().any(|x| *x != 0.0) {
            let v = &mut self.voices[k];
            v.sum = kept;
            v.center = normalize(&v.sum);
            v.ident_sum = kept_id;
            v.ident_center = normalize(&v.ident_sum);
        }
        let target = into.and_then(|t| {
            self.voices.iter().position(|v| {
                label(v.number) == t && v.channel == channel && v.number != self.voices[k].number
            })
        });
        let t = match target {
            Some(t) => {
                let v = &mut self.voices[t];
                if v.sum.len() == gone.len() {
                    v.sum.iter_mut().zip(&gone).for_each(|(a, b)| *a += b);
                } else {
                    // 벡터를 몰랐던 다시 넘겨받은 목소리예요: 옮긴 말로 시작해요.
                    v.sum = gone.clone();
                }
                v.center = normalize(&v.sum);
                v.ident_sum
                    .iter_mut()
                    .zip(&gone_id)
                    .for_each(|(a, b)| *a += b);
                v.ident_center = normalize(&v.ident_sum);
                v.seconds += seconds;
                t
            }
            None => {
                let number = self.next_number();
                self.voices.push(Voice {
                    channel,
                    number,
                    center: normalize(&gone),
                    sum: gone,
                    ident_center: normalize(&gone_id),
                    ident_sum: gone_id,
                    person: None,
                    near: None,
                    restored: None,
                    seconds,
                    lines: Vec::new(),
                    line_idents: Vec::new(),
                });
                self.voices.len() - 1
            }
        };
        Some(label(self.voices[t].number))
    }

    /// 이 목소리와 가깝지만 잇지 않은 기억한 사람이에요.
    pub fn near(&self, voice: &str) -> Option<String> {
        self.voices
            .iter()
            .find(|v| label(v.number) == voice)
            .and_then(|v| v.near.clone())
    }
}

/// 줄 무리 하나예요: 줄 번호, 가중 중심(정규화), 초 합이에요.
struct Group {
    members: Vec<usize>,
    center: Vec<f32>,
    seconds: f32,
}

/// 줄들을 초로 가중한 2-means로 두 무리로 나눠요. 시작점 하나가 홀로 떨어진 줄에 걸려 갇히지 않게 여러 시작점에서
/// 돌려 가장 잘 모이는 쪽을 골라요. 말을 더 한 무리가 먼저예요. 한쪽이 비면 None이에요.
fn two_groups(lines: &[(Vec<f32>, f32)]) -> Option<(Group, Group)> {
    let n = lines.len();
    let center_of = |lab: &[bool], side: bool| {
        let mut s = vec![0.0f32; lines[0].0.len()];
        for ((x, w), l) in lines.iter().zip(lab) {
            if *l == side {
                s.iter_mut().zip(x).for_each(|(a, b)| *a += b * w);
            }
        }
        normalize(&s)
    };
    let mut best: Option<(f32, Vec<bool>, Vec<f32>, Vec<f32>)> = None;
    for i0 in (0..n).step_by((n / 12).max(1)) {
        let mut c1 = lines[i0].0.clone();
        let far = (0..n)
            .min_by(|&a, &b| dot(&lines[a].0, &c1).total_cmp(&dot(&lines[b].0, &c1)))
            .unwrap_or(0);
        let mut c2 = lines[far].0.clone();
        let mut lab = vec![true; n];
        let mut ok = false;
        for _ in 0..15 {
            lab = lines
                .iter()
                .map(|(x, _)| dot(x, &c1) > dot(x, &c2))
                .collect();
            ok = lab.iter().any(|l| *l) && lab.iter().any(|l| !*l);
            if !ok {
                break;
            }
            c1 = center_of(&lab, true);
            c2 = center_of(&lab, false);
        }
        if !ok {
            continue;
        }
        let inertia: f32 = lines
            .iter()
            .zip(&lab)
            .map(|((x, w), l)| w * (1.0 - dot(x, if *l { &c1 } else { &c2 })))
            .sum();
        if best.as_ref().is_none_or(|b| inertia < b.0) {
            best = Some((inertia, lab, c1, c2));
        }
    }
    let (_, lab, c1, c2) = best?;
    let group = |side: bool, center: Vec<f32>| {
        let members: Vec<usize> = (0..n).filter(|&i| lab[i] == side).collect();
        let seconds = members.iter().map(|&i| lines[i].1).sum();
        Group {
            members,
            center,
            seconds,
        }
    };
    let (a, b) = (group(true, c1), group(false, c2));
    Some(if a.seconds >= b.seconds {
        (a, b)
    } else {
        (b, a)
    })
}

/// 두 무리가 얼마나 또렷하게 갈렸는지예요: 줄마다 (다른 중심까지 거리 − 제 중심까지 거리) / 둘 중 큰 거리를 초로
/// 가중 평균해요(거리는 1 − 코사인). 1에 가까울수록 또렷해요.
fn groups_silhouette(lines: &[(Vec<f32>, f32)], big: &Group, small: &Group) -> f32 {
    let mut own_big = vec![false; lines.len()];
    big.members.iter().for_each(|&i| own_big[i] = true);
    let (mut s, mut w) = (0.0f32, 0.0f32);
    for ((x, sec), b) in lines.iter().zip(own_big) {
        let (own, other) = if b {
            (&big.center, &small.center)
        } else {
            (&small.center, &big.center)
        };
        let a = 1.0 - dot(x, own);
        let o = 1.0 - dot(x, other);
        s += sec * (o - a) / a.max(o).max(1e-6);
        w += sec;
    }
    if w > 0.0 { s / w } else { 0.0 }
}

/// "화자 N"이에요. 화면 이름이 아니라 회의 안의 화자 키예요.
pub fn label(number: usize) -> String {
    format!("화자 {number}")
}

/// "화자 N"의 N이에요(1 이상). 다른 글이면 None이에요.
pub fn number_of(voice: &str) -> Option<usize> {
    let n: usize = voice.strip_prefix("화자 ")?.parse().ok()?;
    (n >= 1 && voice == label(n)).then_some(n)
}

/// 길이 1로 맞춰요. 0 벡터는 그대로예요.
pub fn normalize(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter().map(|x| x / n).collect()
    } else {
        v.to_vec()
    }
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
