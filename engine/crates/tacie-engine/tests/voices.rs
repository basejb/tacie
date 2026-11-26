//! The rule for grouping voices by person within a meeting (the same rule as measured in docs/04 4절).

use tacie_embed::voice::Voiceprint;
use tacie_engine::voices::{
    AGAIN_SECONDS, AGAIN_THRESHOLD, Assigned, Channel, MAX_VOICES, MIN_NEW_SECONDS,
    PERSON_THRESHOLD, Person, Restored, THRESHOLD, Tuning, Voices,
};

/// 정규화한 벡터 + 길이예요.
fn p(v: &[f32], seconds: f32) -> Voiceprint {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    Voiceprint {
        vector: v.iter().map(|x| x / n).collect(),
        seconds,
        ident: None,
    }
}

/// 수동 나누기에 넘기는 (화자 나누기 벡터, ERes2Net 벡터)예요. 모델 하나면 둘이 같아요.
fn pair(v: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let x = p(v, 3.0).vector;
    (x.clone(), x)
}

const A: [f32; 3] = [1.0, 0.0, 0.0];
const B: [f32; 3] = [0.0, 1.0, 0.0];

#[test]
fn first_utterance_is_voice_one() {
    let mut v = Voices::default();
    assert_eq!(
        v.assign(Channel::Room, &p(&A, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 1")
    );
}

#[test]
fn close_voice_joins_far_voice_starts_new() {
    let mut v = Voices::default();
    assert_eq!(
        v.assign(Channel::Room, &p(&A, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 1")
    );
    // cos 0.9
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.9, 0.436, 0.0], 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 1")
    );
    // cos 0.1 to 화자 1
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.1, -0.2, 0.975], 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 2")
    );
}

#[test]
fn short_utterance_never_starts_a_voice() {
    let mut v = Voices::default();
    v.assign(Channel::Room, &p(&A, 3.0)).unwrap();
    assert_eq!(
        v.assign(Channel::Room, &p(&B, 1.5))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 1")
    );
    // 먼 목소리는 2초, 그 밖에는 3초부터 새 사람이 될 수 있어요
    assert_eq!(
        v.assign(Channel::Room, &p(&B, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 2")
    );
}

#[test]
fn short_far_utterance_does_not_move_the_centroid() {
    let mut v = Voices::default();
    v.assign(Channel::Room, &p(&A, 3.0)).unwrap();
    assert_eq!(
        v.assign(Channel::Room, &p(&B, 1.5))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 1")
    );
    // A와 cos 0.45: 중심이 그대로면 붙어요. B 쪽으로 옮겨졌다면((A+B)/√2와 cos < 0) 새 사람이 됐을 거예요.
    let c = [0.45, -0.6, (1.0f32 - 0.45 * 0.45 - 0.36).sqrt()];
    assert_eq!(
        v.assign(Channel::Room, &p(&c, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 1")
    );
}

#[test]
fn caps_at_max_voices() {
    let mut v = Voices::default();
    let mut names = Vec::new();
    for i in 0..10 {
        let mut e = [0.0f32; 10];
        e[i] = 1.0;
        names.push(v.assign(Channel::Room, &p(&e, 3.0)).unwrap().voice);
    }
    let first: Vec<String> = (1..=MAX_VOICES).map(|n| format!("화자 {n}")).collect();
    assert_eq!(names[..MAX_VOICES], first[..]);
    assert!(first.contains(&names[8]) && first.contains(&names[9]));
}

#[test]
fn labels_never_renumber() {
    let mut v = Voices::default();
    assert_eq!(
        v.assign(Channel::Room, &p(&A, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 1")
    );
    assert_eq!(
        v.assign(Channel::Room, &p(&B, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 2")
    );
    for _ in 0..5 {
        assert_eq!(
            v.assign(Channel::Room, &p(&[0.8, 0.0, 0.6], 3.0))
                .map(|a| a.voice)
                .as_deref(),
            Some("화자 1")
        );
    }
    assert_eq!(
        v.assign(Channel::Room, &p(&B, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 2")
    );
}

#[test]
fn short_first_utterance_does_not_start_a_voice() {
    let mut v = Voices::default();
    // "음"(1초)으로 회의가 시작돼도 유령 화자가 생기지 않아요.
    assert_eq!(v.assign(Channel::Room, &p(&A, 1.0)), None);
    assert_eq!(
        v.assign(Channel::Room, &p(&B, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 1")
    );
}

#[test]
fn channels_share_one_numbering() {
    let mut v = Voices::default();
    assert_eq!(
        v.assign(Channel::Mic, &p(&A, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 1")
    );
    assert_eq!(
        v.assign(Channel::Call, &p(&B, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 2")
    );
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.0, 0.0, 1.0], 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 3")
    );
}

#[test]
fn same_vector_on_two_channels_is_two_speakers() {
    let mut v = Voices::default();
    assert_eq!(
        v.assign(Channel::Mic, &p(&A, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 1")
    );
    assert_eq!(
        v.assign(Channel::Call, &p(&A, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 2")
    );
    // 각자 자기 갈래의 사람에게 붙어요.
    assert_eq!(
        v.assign(Channel::Mic, &p(&A, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 1")
    );
    assert_eq!(
        v.assign(Channel::Call, &p(&A, 3.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 2")
    );
}

#[test]
fn cap_counts_every_channel() {
    let mut v = Voices::default();
    for i in 0..MAX_VOICES {
        let mut e = [0.0f32; 10];
        e[i] = 1.0;
        let ch = if i % 2 == 0 {
            Channel::Mic
        } else {
            Channel::Call
        };
        assert_eq!(
            v.assign(ch, &p(&e, 3.0)).map(|a| a.voice).as_deref(),
            Some(format!("화자 {}", i + 1).as_str())
        );
    }
    // 아홉째 먼 목소리는 같은 갈래(Mic)에서 가장 가까운 사람에게 붙어요.
    let mut far = [0.0f32; 10];
    far[8] = 1.0;
    let got = v.assign(Channel::Mic, &p(&far, 3.0)).unwrap().voice;
    let mic: Vec<String> = (0..MAX_VOICES)
        .step_by(2)
        .map(|i| format!("화자 {}", i + 1))
        .collect();
    assert!(mic.contains(&got));
    // Room에는 사람이 없어서 붙일 곳이 없어요.
    assert_eq!(v.assign(Channel::Room, &p(&far, 3.0)), None);
}

// 기억한 사람과 다시 연 회의: 새 목소리를 기억한 사람과 견주고, 엔진이 다시 떠도 번호를 이어 써요.

fn person(id: &str, v: &[f32]) -> Person {
    Person {
        id: id.into(),
        vector: p(v, 0.0).vector,
    }
}

fn assigned(voice: &str, person: Option<&str>) -> Option<Assigned> {
    Some(Assigned {
        voice: voice.into(),
        person: person.map(str::to_string),
    })
}

#[test]
fn new_voice_close_to_a_person_is_linked() {
    const { assert!(0.9 >= PERSON_THRESHOLD) };
    let mut v = Voices::with(vec![person("p1", &A)], vec![]);
    // dot 0.9
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.9, 0.436, 0.0], 3.0)),
        assigned("화자 1", Some("p1"))
    );
    // 이어지는 말도 그 사람이에요.
    assert_eq!(
        v.assign(Channel::Room, &p(&A, 3.0)),
        assigned("화자 1", Some("p1"))
    );
    assert_eq!(v.near("화자 1"), None);
}

#[test]
fn new_voice_between_thresholds_is_only_near() {
    const { assert!(0.5 >= THRESHOLD && 0.5 < PERSON_THRESHOLD) };
    let mut v = Voices::with(vec![person("p0", &B), person("p1", &A)], vec![]);
    // dot 0.5 to p1, 0 to p0
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.5, 0.0, 0.866], 3.0)),
        assigned("화자 1", None)
    );
    assert_eq!(v.near("화자 1").as_deref(), Some("p1"));
    // 멀면 near도 없어요.
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.0, 0.0, 1.0], 3.0)),
        assigned("화자 1", None)
    );
    let mut far = Voices::with(vec![person("p1", &A)], vec![]);
    far.assign(Channel::Room, &p(&[0.0, 0.0, 1.0], 3.0))
        .unwrap();
    assert_eq!(far.near("화자 1"), None);
}

/// 첫 발화가 기준 밑이어도 말이 쌓여 중심이 그 사람과 가까워지면 이어요. 그 전 줄은 이름 없이 남아요.
#[test]
fn voice_is_linked_again_once_its_centre_is_close() {
    const { assert!(0.55 < PERSON_THRESHOLD && 0.66 >= AGAIN_THRESHOLD) };
    let mut v = Voices::with(vec![person("p1", &[1.0, 0.0, 0.0, 0.0])], vec![]);
    // 발화마다 p1과 cos 0.55쯤이고 흔들리는 쪽이 서로 달라서, 쌓이면 중심이 p1 쪽으로 가요.
    let a = [0.55, 0.835, 0.0, 0.0];
    let b = [0.55, 0.40, 0.73, 0.0];
    let c = [0.55, 0.6, -0.58, 0.0];
    let step = (AGAIN_SECONDS / 3.0 + 0.01).max(MIN_NEW_SECONDS);
    assert_eq!(
        v.assign(Channel::Room, &p(&a, step)),
        assigned("화자 1", None)
    );
    assert_eq!(v.near("화자 1").as_deref(), Some("p1"));
    assert_eq!(
        v.assign(Channel::Room, &p(&b, step)),
        assigned("화자 1", None)
    );
    assert_eq!(
        v.assign(Channel::Room, &p(&c, step)),
        assigned("화자 1", Some("p1"))
    );
    assert_eq!(v.near("화자 1"), None);
    // 한 번 이으면 다음 말이 멀어도 풀지 않아요.
    assert_eq!(
        v.assign(Channel::Room, &p(&a, 1.0)),
        assigned("화자 1", Some("p1"))
    );
}

/// 말이 덜 쌓였으면 중심이 가까워도 다시 견주지 않아요.
#[test]
fn too_little_speech_is_not_linked_again() {
    let mut v = Voices::with(vec![person("p1", &[1.0, 0.0, 0.0, 0.0])], vec![]);
    // 둘의 중심은 p1과 cos 0.67이지만 합쳐 AGAIN_SECONDS가 안 돼요.
    let half = AGAIN_SECONDS / 2.0 - 0.5;
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.55, 0.835, 0.0, 0.0], half)),
        assigned("화자 1", None)
    );
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.55, 0.6, -0.58, 0.0], half)),
        assigned("화자 1", None)
    );
}

/// 다시 견줘도 그 사람이 이미 다른 목소리면 잇지 않아요.
#[test]
fn again_does_not_take_a_person_twice() {
    let mut v = Voices::with(vec![person("p1", &[1.0, 0.0, 0.0, 0.0])], vec![]);
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.9, -0.436, 0.0, 0.0], 3.0)),
        assigned("화자 1", Some("p1"))
    );
    // 둘 다 화자 1과 멀어서 화자 2가 되고, 중심은 p1과 cos 0.67이에요.
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.55, 0.835, 0.0, 0.0], AGAIN_SECONDS)),
        assigned("화자 2", None)
    );
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.55, 0.6, -0.58, 0.0], AGAIN_SECONDS)),
        assigned("화자 2", None)
    );
    assert_eq!(v.near("화자 2").as_deref(), Some("p1"));
}

#[test]
fn one_person_links_one_voice() {
    let mut v = Voices::with(vec![person("p1", &[1.0, 0.0, 0.0, 0.0])], vec![]);
    // 둘 다 p1과 cos 0.7이지만 서로는 cos 0.02예요: 두 목소리예요.
    let a = [0.7, 0.714, 0.0, 0.0];
    let b = [0.7, -0.7, 0.14, 0.0];
    assert_eq!(
        v.assign(Channel::Room, &p(&a, 3.0)),
        assigned("화자 1", Some("p1"))
    );
    assert_eq!(
        v.assign(Channel::Room, &p(&b, 3.0)),
        assigned("화자 2", None)
    );
    assert_eq!(v.near("화자 2").as_deref(), Some("p1"));
}

#[test]
fn restored_voices_keep_numbers_and_next_is_max_plus_one() {
    let restored = |n: usize, e: &[f32]| Restored {
        number: n,
        channel: Channel::Room,
        vector: p(e, 0.0).vector,
        diar: None,
        seconds: 12.0,
        person: None,
    };
    let mut v = Voices::with(vec![], vec![restored(1, &A), restored(3, &B)]);
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.0, 0.0, 1.0], 3.0)),
        assigned("화자 4", None)
    );
    // cos 0.9 to 화자 3
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.0, 0.9, 0.436], 3.0)),
        assigned("화자 3", None)
    );
}

#[test]
fn restored_person_stays_linked() {
    let mut v = Voices::with(
        vec![person("p1", &A)],
        vec![Restored {
            number: 2,
            channel: Channel::Room,
            vector: p(&B, 0.0).vector,
            diar: None,
            seconds: 5.0,
            person: Some("p1".into()),
        }],
    );
    assert_eq!(
        v.assign(Channel::Room, &p(&B, 3.0)),
        assigned("화자 2", Some("p1"))
    );
    // p1은 이미 화자 2예요: p1과 같은 목소리여도 새 사람에게 다시 붙이지 않아요.
    assert_eq!(
        v.assign(Channel::Room, &p(&A, 3.0)),
        assigned("화자 3", None)
    );
}

/// 앱이 벡터를 받기 전에 엔진이 다시 떴으면 그 "화자 N"은 넘겨받지 못해요: `after`가 그 번호까지 비워 둬요.
#[test]
fn after_reserves_numbers() {
    let mut v = Voices::default().after(3);
    assert_eq!(
        v.assign(Channel::Room, &p(&A, 3.0)),
        assigned("화자 4", None)
    );
    let restored = Restored {
        number: 5,
        channel: Channel::Room,
        vector: p(&A, 0.0).vector,
        diar: None,
        seconds: 12.0,
        person: None,
    };
    // 넘겨받은 번호가 더 크면 그 다음이에요.
    let mut v = Voices::with(vec![], vec![restored]).after(2);
    assert_eq!(
        v.assign(Channel::Room, &p(&B, 3.0)),
        assigned("화자 6", None)
    );
}

/// 회의 중에 사람이 한 목소리의 말 몇 개에 다른 이름을 주면, 그 말들은 새 목소리가 되고 다음 발화부터 그쪽과도 견줘요.
#[test]
fn split_moves_named_lines_to_a_new_voice() {
    let mut v = Voices::default();
    // 두 사람(A, 가까운 C)이 한 목소리 "화자 1"로 묶였어요: cos(A, C) 0.6
    let c = [0.6, 0.8, 0.0];
    assert_eq!(
        v.assign(Channel::Room, &p(&A, 3.0)),
        assigned("화자 1", None)
    );
    assert_eq!(
        v.assign(Channel::Room, &p(&c, 3.0)),
        assigned("화자 1", None)
    );
    let to = v.split("화자 1", &[pair(&c)], &[pair(&A)], 3.0, None);
    assert_eq!(to.as_deref(), Some("화자 2"));
    // C의 다음 말은 화자 2, A의 다음 말은 화자 1이에요.
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.55, 0.83, 0.0], 3.0)),
        assigned("화자 2", None)
    );
    assert_eq!(
        v.assign(Channel::Room, &p(&[0.98, 0.2, 0.0], 3.0)),
        assigned("화자 1", None)
    );
}

/// 이미 있는 목소리로 옮기면 새 목소리를 만들지 않아요. 모르는 목소리나 빈 말은 아무것도 하지 않아요.
#[test]
fn split_into_an_existing_voice() {
    let mut v = Voices::default();
    v.assign(Channel::Room, &p(&A, 3.0)).unwrap();
    v.assign(Channel::Room, &p(&B, 3.0)).unwrap();
    let to = v.split("화자 1", &[pair(&A)], &[], 3.0, Some("화자 2"));
    assert_eq!(to.as_deref(), Some("화자 2"));
    assert_eq!(v.known().len(), 2);
    assert_eq!(v.split("화자 9", &[pair(&A)], &[], 3.0, None), None);
    assert_eq!(v.split("화자 1", &[], &[], 0.0, None), None);
}

/// 코사인 0.5인 두 사람이에요: 묶기 기준(0.4)은 넘어 처음에는 한 화자로 묶여요.
const X: [f32; 3] = [1.0, 0.0, 0.0];
const Y: [f32; 3] = [0.5, 0.866, 0.0];

#[test]
fn two_close_people_in_one_voice_get_split() {
    let mut v = Voices::default();
    let mut last = (String::new(), String::new());
    for _ in 0..5 {
        let x = v.assign(Channel::Call, &p(&X, 5.0)).unwrap().voice;
        let y = v.assign(Channel::Call, &p(&Y, 5.0)).unwrap().voice;
        last = (x, y);
    }
    assert_eq!(v.known().len(), 2);
    assert_ne!(
        last.0, last.1,
        "after the split each person gets their own voice"
    );
}

#[test]
fn one_voice_that_drifts_is_not_split() {
    let mut v = Voices::default();
    for i in 0..12 {
        let a = (i % 7) as f32 * 10.0_f32.to_radians();
        v.assign(Channel::Call, &p(&[a.cos(), a.sin(), 0.0], 5.0))
            .unwrap();
    }
    assert_eq!(v.known().len(), 1);
}

#[test]
fn short_lines_never_split_a_voice() {
    let mut v = Voices::default();
    v.assign(Channel::Call, &p(&X, 3.0)).unwrap();
    for _ in 0..30 {
        v.assign(Channel::Call, &p(&X, 1.5)).unwrap();
        v.assign(Channel::Call, &p(&Y, 1.5)).unwrap();
    }
    assert_eq!(v.known().len(), 1);
}

#[test]
fn a_clearly_different_voice_starts_after_two_seconds() {
    let mut v = Voices::default();
    v.assign(Channel::Call, &p(&A, 5.0)).unwrap();
    // B is far from A (cosine 0): two seconds are enough
    assert_eq!(
        v.assign(Channel::Call, &p(&B, 2.0))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 2")
    );
    // a voice between 0.3 and 0.4 still needs three seconds
    let mut w = Voices::default();
    w.assign(Channel::Call, &p(&A, 5.0)).unwrap();
    assert_eq!(
        w.assign(Channel::Call, &p(&[0.35, 0.937, 0.0], 2.5))
            .map(|a| a.voice)
            .as_deref(),
        Some("화자 1")
    );
}

/// 화자 나누기 모델(길이 2)과 ERes2Net(길이 3)으로 만든 발화예요.
fn dual(diar: &[f32], ident: &[f32], seconds: f32) -> Voiceprint {
    let n = |v: &[f32]| {
        let l = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / l).collect::<Vec<f32>>()
    };
    Voiceprint {
        vector: n(diar),
        seconds,
        ident: Some(n(ident)),
    }
}

const D1: [f32; 2] = [1.0, 0.0];
const D2: [f32; 2] = [0.0, 1.0];

#[test]
fn with_a_diar_model_voices_group_by_its_vector_and_people_by_eres2net() {
    // two utterances close in the diarization space are one voice even if their ERes2Net vectors differ
    let mut v = Voices::tuned(Vec::new(), Vec::new(), Tuning::diar(2));
    assert_eq!(
        v.assign(Channel::Room, &dual(&D1, &A, 4.0)).unwrap().voice,
        "화자 1"
    );
    assert_eq!(
        v.assign(Channel::Room, &dual(&D1, &B, 4.0)).unwrap().voice,
        "화자 1"
    );
    assert_eq!(
        v.assign(Channel::Room, &dual(&D2, &A, 4.0)).unwrap().voice,
        "화자 2"
    );
    // a remembered person (ERes2Net vector) is matched by ERes2Net, not by the diarization vector
    let person = Person {
        id: "p1".into(),
        vector: B.to_vec(),
    };
    let mut w = Voices::tuned(vec![person], Vec::new(), Tuning::diar(2));
    let a = w.assign(Channel::Room, &dual(&D1, &B, 4.0)).unwrap();
    assert_eq!(a.person.as_deref(), Some("p1"));
    let b = w.assign(Channel::Room, &dual(&D2, &A, 4.0)).unwrap();
    assert_eq!((b.voice.as_str(), b.person), ("화자 2", None));
}

#[test]
fn with_a_diar_model_the_diar_threshold_applies() {
    // cosine 0.5 joins with ERes2Net values (0.4) but not with the diarization values (0.55)
    let half = [0.5, 0.866];
    let mut e = Voices::default();
    e.assign(Channel::Room, &p(&[1.0, 0.0], 4.0)).unwrap();
    assert_eq!(
        e.assign(Channel::Room, &p(&half, 4.0)).unwrap().voice,
        "화자 1"
    );
    let mut d = Voices::tuned(Vec::new(), Vec::new(), Tuning::diar(2));
    d.assign(Channel::Room, &dual(&D1, &A, 4.0)).unwrap();
    assert_eq!(
        d.assign(Channel::Room, &dual(&half, &A, 4.0))
            .unwrap()
            .voice,
        "화자 2"
    );
    assert_eq!(Tuning::for_dim(None), Tuning::ERES);
    assert_eq!(Tuning::for_dim(Some(192)).diar_dim, Some(192));
}

#[test]
fn a_restored_voice_without_its_diar_vector_keeps_its_number_but_gets_no_utterances() {
    let restored = |n: usize, diar: Option<Vec<f32>>| Restored {
        number: n,
        channel: Channel::Room,
        vector: A.to_vec(),
        diar,
        seconds: 20.0,
        person: None,
    };
    // with the diarization vector the voice continues
    let mut v = Voices::tuned(
        Vec::new(),
        vec![restored(3, Some(D1.to_vec()))],
        Tuning::diar(2),
    );
    assert_eq!(
        v.assign(Channel::Room, &dual(&D1, &A, 4.0)).unwrap().voice,
        "화자 3"
    );
    // without it (or with another length) a new voice starts after it
    for diar in [None, Some(vec![1.0, 0.0, 0.0])] {
        let mut w = Voices::tuned(Vec::new(), vec![restored(3, diar)], Tuning::diar(2));
        assert_eq!(
            w.assign(Channel::Room, &dual(&D1, &A, 4.0)).unwrap().voice,
            "화자 4"
        );
        assert!(w.center("화자 3").is_none());
    }
    // without a diar model the ERes2Net vector is the voice's vector, as before
    let mut e = Voices::with(Vec::new(), vec![restored(3, Some(D1.to_vec()))]);
    assert_eq!(
        e.assign(Channel::Room, &p(&A, 4.0)).unwrap().voice,
        "화자 3"
    );
}

/// A CAM++-only engine (iPhone) has no ERes2Net vector: a remembered person of another length never links or shows as
/// near, even where the first components line up.
#[test]
fn person_of_another_length_is_never_matched() {
    let mut v = Voices::with(vec![person("p1", &[1.0, 0.0, 0.0, 0.0])], vec![]);
    assert_eq!(v.assign(Channel::Room, &p(&A, 3.0)), assigned("화자 1", None));
    assert_eq!(v.near("화자 1"), None);
}
