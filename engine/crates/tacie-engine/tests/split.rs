//! Speaker splitting inside a chunk (`split`): finding the change moment, aligning to a gap between words, and
//! building the chunks.

use tacie_engine::split::{
    BLOCK, CHANGE_SIM, HOP_SECONDS, MAX_CHANGES, WINDOW_SECONDS, best_cut, change_points, pieces,
    snap, window_starts, word_window,
};

const A: [f32; 2] = [1.0, 0.0];
const B: [f32; 2] = [0.0, 1.0];

/// 0.5초마다 창 하나, 글자 'a'/'b'가 그 창의 목소리예요.
fn windows(pattern: &str) -> Vec<(f32, Vec<f32>)> {
    pattern
        .chars()
        .enumerate()
        .map(|(i, c)| {
            (
                i as f32 * HOP_SECONDS,
                if c == 'a' { A.to_vec() } else { B.to_vec() },
            )
        })
        .collect()
}

#[test]
fn window_starts_cover_the_whole_wav() {
    let s = window_starts(15.0, WINDOW_SECONDS, HOP_SECONDS);
    assert_eq!(s.first(), Some(&0.0));
    assert!((s.last().unwrap() + WINDOW_SECONDS - 15.0).abs() < 1e-4);
    assert_eq!(window_starts(1.0, WINDOW_SECONDS, HOP_SECONDS), vec![0.0]);
}

#[test]
fn one_voice_has_no_change() {
    assert!(change_points(&windows(&"a".repeat(27)), CHANGE_SIM).is_empty());
}

#[test]
fn two_voices_change_once_between_them() {
    // 창 i는 i*0.5초에 시작해요: a 14개, b 13개면 a 창은 8.5초까지 끝나요
    let p = change_points(
        &windows(&format!("{}{}", "a".repeat(14), "b".repeat(13))),
        CHANGE_SIM,
    );
    assert_eq!(p.len(), 1, "{p:?}");
    assert!((p[0] - 7.5).abs() <= 1.0, "{p:?}");
}

#[test]
fn one_odd_window_is_not_a_change() {
    let mut w = "a".repeat(27);
    w.replace_range(13..14, "b");
    assert!(
        change_points(&windows(&w), CHANGE_SIM).is_empty(),
        "BLOCK {BLOCK}"
    );
}

#[test]
fn changes_are_capped() {
    let w = "aaaaaaaabbbbbbbbaaaaaaaabbbbbbbbaaaaaaaabbbbbbbb";
    assert!(change_points(&windows(w), CHANGE_SIM).len() <= MAX_CHANGES);
}

#[test]
fn snap_takes_the_widest_word_gap_nearby() {
    let words = [
        (0.0, 0.8),
        (0.9, 1.6),
        (1.7, 2.4),
        (3.0, 3.6),
        (3.7, 4.4),
        (4.5, 5.2),
    ];
    assert_eq!(snap(&[2.5], &words), vec![3]);
    // 0.5초 안에 단어 사이가 없으면 가장 가까운 단어 사이예요
    assert_eq!(snap(&[0.0], &words), vec![1]);
    assert!(snap(&[2.5], &[(0.0, 1.0)]).is_empty());
    assert_eq!(snap(&[2.5, 2.6], &words), vec![3]);
}

#[test]
fn pieces_split_words_and_sound_at_the_gap() {
    let words = [(0.0, 0.8), (0.9, 1.6), (3.0, 3.6), (3.7, 4.4)];
    let p = pieces(&[2], &words, 16000 * 5);
    assert_eq!(p.len(), 2);
    assert_eq!((p[0].words.clone(), p[1].words.clone()), (0..2, 2..4));
    assert_eq!(p[0].samples, 0..36800); // (1.6 + 3.0) / 2 = 2.3초
    assert_eq!(p[1].samples, 36800..80000);
}

#[test]
fn best_cut_gives_each_side_its_closer_words() {
    // 앞 두 단어는 앞 목소리, 뒤 세 단어는 뒤 목소리에 가까워요
    assert_eq!(best_cut(&[0.4, 0.2, -0.3, -0.1, -0.5], 4), 2);
    assert_eq!(best_cut(&[0.4, 0.2, -0.3, -0.1, -0.5], 0), 2);
    // 모두 뒤 목소리면 앞에 줄 단어가 없어요
    assert_eq!(best_cut(&[-0.2, -0.3], 1), 0);
    // 모르면(0) 지금 자리를 지켜요
    assert_eq!(best_cut(&[0.0, 0.0, 0.0], 1), 1);
    assert_eq!(best_cut(&[], 0), 0);
}

#[test]
fn word_window_is_one_second_inside_the_wav() {
    let total = 16000 * 10;
    assert_eq!(
        word_window((4.9, 5.1), total),
        16000 * 9 / 2..16000 * 11 / 2
    );
    // 맨 앞과 맨 뒤 단어는 길이를 지킨 채 안쪽으로 밀어요
    assert_eq!(word_window((0.0, 0.2), total), 0..16000);
    assert_eq!(word_window((9.9, 10.0), total), 16000 * 9..total);
    // 1초보다 긴 단어는 단어 길이예요
    assert_eq!(word_window((2.0, 3.5), total), 32000..56000);
    // WAV가 1초보다 짧으면 WAV 전체예요
    assert_eq!(word_window((0.1, 0.3), 8000), 0..8000);
}

#[test]
fn a_word_that_ends_the_previous_sentence_goes_back() {
    let after = [
        (0.1, 0.26, "같아요."),
        (0.46, 0.66, "저희는"),
        (0.74, 1.2, "일단"),
    ];
    assert_eq!(tacie_engine::split::punct_head("것", 0.1, &after), 1);
    // the previous line already ended its sentence, or there was a real pause
    assert_eq!(tacie_engine::split::punct_head("것.", 0.1, &after), 0);
    assert_eq!(tacie_engine::split::punct_head("것", 0.5, &after), 0);
    // a short answer of its own stays with the new speaker
    assert_eq!(
        tacie_engine::split::punct_head("아", 0.1, &[(0.1, 0.4, "맞아요."), (0.5, 0.8, "사실")]),
        0
    );
    // no sentence end within the first words
    assert_eq!(
        tacie_engine::split::punct_head(
            "그래서",
            0.1,
            &[
                (0.0, 0.5, "보통"),
                (0.6, 1.0, "한국은"),
                (1.1, 1.4, "운동"),
                (1.5, 1.8, "문화가.")
            ]
        ),
        0
    );
}

#[test]
fn words_after_a_finished_sentence_go_to_the_next_line() {
    let before = [
        (0.0, 0.4, "주시면"),
        (0.5, 0.8, "됩니다."),
        (1.3, 1.5, "대표님"),
    ];
    assert_eq!(tacie_engine::split::punct_tail(&before, 0.16), 1);
    assert_eq!(tacie_engine::split::punct_tail(&before, 0.6), 0);
    // no finished sentence before the last words
    assert_eq!(
        tacie_engine::split::punct_tail(
            &[
                (0.0, 0.4, "그"),
                (0.5, 0.8, "내러티브가"),
                (0.9, 1.1, "너무")
            ],
            0.1
        ),
        0
    );
}
