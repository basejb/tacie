//! Tests the meeting session (`Session::hear`, `hear_audio`, `find`, `utterance`, `ledger`, `summary`,
//! `Engine::new_session`) with a fake gate, reranker, transcriber and ledger model. Real models and APIs are not
//! called.

mod common {
    pub mod fakes;
    pub mod fixture;
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::fakes::{
    FakeAnswerer, FakeEmbed, FakeGater, FakeNoteWriter, FakeReranker, FakeTranscriber, FakeVoice,
    fake_gate, services, transcript,
};
use common::fixture::{engine_with, index, index_of};
use serde_json::{Value, json};
use tacie_engine::errors::Kind;
use tacie_engine::pipeline::Engine;
use tacie_engine::services::Services;
use tacie_engine::session::{LEDGER_WAIT, Session, heard_line};
use tacie_engine::voices::{Channel, Person, Restored, Voices};
use tacie_index::Index;

/// 판단과 리랭커를 넣은 엔진이에요.
fn engine_of(
    ix: Index,
    gater: &Arc<FakeGater>,
    reranker: &Arc<FakeReranker>,
    answerer: &Arc<FakeAnswerer>,
) -> (Engine, Arc<FakeEmbed>) {
    let services = Services {
        gater: gater.clone(),
        ..services(reranker.clone(), answerer.clone())
    };
    engine_with(ix, services)
}

/// 판단만 쓰는 테스트예요(리랭커는 부르지 않아요).
fn gate_engine(gater: &Arc<FakeGater>) -> Engine {
    engine_of(
        index(),
        gater,
        &Arc::new(FakeReranker::all(0.9)),
        &Arc::new(FakeAnswerer::ok("")),
    )
    .0
}

/// p_lookup만 주는 판단이에요(ms는 1이에요).
fn gater(p_lookup: f64) -> Arc<FakeGater> {
    Arc::new(FakeGater::ok(p_lookup))
}

/// `Session::hear`를 불러요: 기준은 기본값(gate 0.7, end 0.8)이에요.
async fn hear(
    s: &Session,
    e: &Engine,
    text: &str,
    speaker: Option<&str>,
    gap_ms: Option<u64>,
    now: f64,
) -> Value {
    s.hear(
        e,
        text,
        speaker,
        e.settings.gate,
        Some(now),
        gap_ms,
        None,
        e.settings.end,
    )
    .await
}

/// `Session::utterance`를 불러요: 기준은 기본값(gate 0.7, show 0.8)이에요.
async fn utterance(s: &Session, e: &Engine, text: &str, speaker: Option<&str>, now: f64) -> Value {
    s.utterance(
        e,
        text,
        speaker,
        e.settings.gate,
        e.settings.show,
        Some(now),
    )
    .await
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn session_uses_recent_lines_as_context_and_marks_repeats() {
    let p = Arc::new(Mutex::new(0.9));
    let gate_p = p.clone();
    let gater = Arc::new(FakeGater::from_fn(move |_, _| {
        Ok(fake_gate(*gate_p.lock().unwrap(), None))
    }));
    let reranker = Arc::new(FakeReranker::ok(&[0.1, 0.2, 0.95, 0.3]));
    let (engine, embed) = engine_of(index(), &gater, &reranker, &Arc::new(FakeAnswerer::ok("")));
    let s = engine.new_session();
    *p.lock().unwrap() = 0.2;
    let chat = utterance(&s, &engine, "점심 뭐 먹어요", Some("민준"), 0.0).await;
    assert_eq!(
        (&chat["fired"], &chat["cards"]),
        (&json!(false), &json!([]))
    );
    *p.lock().unwrap() = 0.9;
    let first = utterance(&s, &engine, "그거 401 나면 어디 봐요?", Some("지수"), 10.0).await;
    assert_eq!(
        (
            &first["fired"],
            &first["shown"],
            &first["cards"][0]["repeat"]
        ),
        (&json!(true), &json!(true), &json!(false))
    );
    // 찾기는 (이번 글, 앞 대화, 이번 줄)을 받아요: 글은 임베더가, 앞 대화와 줄은 리랭커가 받았어요.
    assert_eq!(embed.texts().last().unwrap()[0], "그거 401 나면 어디 봐요?");
    let (context, line, _) = reranker.calls().pop().unwrap();
    assert_eq!(
        (context, line.as_str()),
        (
            lines(&["민준: 점심 뭐 먹어요"]),
            "지수: 그거 401 나면 어디 봐요?"
        )
    );
    let again = utterance(&s, &engine, "아 그 401 문서 다시", None, 100.0).await;
    assert_eq!(again["cards"][0]["repeat"], true);
    let later = utterance(&s, &engine, "401 문서 한 번 더", None, 500.0).await;
    assert_eq!(later["cards"][0]["repeat"], false);
    let summary = s.summary().await;
    assert_eq!(
        (
            &summary["utterances"],
            &summary["fired"],
            &summary["unanswered"]
        ),
        (&json!(4), &json!(3), &json!([]))
    );
}

/// 화면에 보이지 않은 카드는 `repeat`을 세는 데 들어가지 않아요.
#[tokio::test]
async fn only_shown_cards_count_for_repeats() {
    let (engine, _) = engine_of(
        index(),
        &gater(0.9),
        &Arc::new(FakeReranker::ok(&[0.1, 0.2, 0.95, 0.3])),
        &Arc::new(FakeAnswerer::ok("")),
    );
    let s = engine.new_session();
    let hidden = s
        .utterance(
            &engine,
            "메일 탭 401 어디 봐요?",
            None,
            0.7,
            0.99,
            Some(0.0),
        )
        .await;
    assert_eq!(hidden["shown"], false);
    let shown = s
        .utterance(&engine, "401 문서 다시요?", None, 0.7, 0.8, Some(10.0))
        .await;
    assert_eq!(
        (&shown["shown"], &shown["cards"][0]["repeat"]),
        (&json!(true), &json!(false))
    );
}

#[tokio::test]
async fn hear_then_find_and_answer_from_whole_chunks() {
    let answerer = Arc::new(FakeAnswerer::ok("dev:doctor부터 돌려요. [1]"));
    let services = Services {
        gater: gater(0.9),
        transcriber: Arc::new(FakeTranscriber::ok("메일 탭 401 나면 어디 봐요?")),
        ..services(
            Arc::new(FakeReranker::ok(&[0.1, 0.2, 0.95, 0.3])),
            answerer.clone(),
        )
    };
    let (engine, _) = engine_with(index(), services);
    let s = engine.new_session();
    let heard = s.hear_audio(&engine, b"RIFF....", Some("지수"), None).await;
    assert_eq!(
        (&heard["fired"], &heard["cards"]),
        (&json!(true), &json!([]))
    );
    assert!(heard.get("_line").is_none() && heard.get("_context").is_none());
    let found = s
        .find(
            &engine,
            heard["utterance_id"].as_str().unwrap(),
            engine.settings.show,
        )
        .await
        .unwrap();
    assert_eq!(found["cards"][0]["path"], "docs/메일/03_로그인.md");
    assert_eq!(found["cards"][0]["chunk"], 2);
    let ids: Vec<usize> = found["cards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["chunk"].as_u64().unwrap() as usize)
        .collect();
    let out = engine
        .compose_answer(heard["text"].as_str().unwrap(), &[], &ids)
        .await;
    assert!(out["answer"].as_str().unwrap().starts_with("dev:doctor"));
    // 머리 줄(file:, section:)을 뺀 조각 본문이에요.
    let passages = &answerer.calls()[0].2;
    assert!(
        passages[0]["snippet"]
            .as_str()
            .unwrap()
            .starts_with("## 401이 날 때")
    );
}

#[tokio::test]
async fn short_pause_joins_pieces_of_one_question() {
    let gater = Arc::new(FakeGater::from_fn(|_, line| {
        let p = if line.contains("어디서") && line.contains("부품") {
            0.9
        } else {
            0.2
        };
        Ok(fake_gate(p, None))
    }));
    let engine = gate_engine(&gater);
    let s = engine.new_session();
    let a = hear(
        &s,
        &engine,
        "그 부품들 실제로 어떻게 생겼는지는",
        None,
        Some(3000),
        0.0,
    )
    .await;
    let b = hear(&s, &engine, "어디서 볼 수 있어요?", None, Some(800), 1.0).await;
    assert_eq!(a["fired"], false);
    assert_eq!(b["fired"], true);
    assert_eq!(b["merged_from"], a["utterance_id"]);
    assert_eq!(
        gater.calls().pop().unwrap(),
        (
            vec![],
            "나: 그 부품들 실제로 어떻게 생겼는지는 어디서 볼 수 있어요?".to_string()
        )
    );
    // 앞 조각이 판단을 넘었으니 잇지 않아요.
    let c = hear(&s, &engine, "다음 얘기 할게요", None, Some(500), 2.0).await;
    assert_eq!(c["merged_from"], Value::Null);
    assert_eq!(
        gater.calls().pop().unwrap().0,
        lines(&["나: 그 부품들 실제로 어떻게 생겼는지는 어디서 볼 수 있어요?"])
    );
}

#[tokio::test]
async fn pieces_of_different_speakers_are_not_joined() {
    let engine = gate_engine(&gater(0.2));
    let s = engine.new_session();
    let a = hear(
        &s,
        &engine,
        "그 부품들 실제로 어떻게 생겼는지는",
        Some("상대"),
        Some(3000),
        0.0,
    )
    .await;
    let b = hear(
        &s,
        &engine,
        "어디서 볼 수 있어요?",
        Some("나"),
        Some(500),
        1.0,
    )
    .await;
    assert_eq!(
        (&b["merged_from"], &b["text"]),
        (&Value::Null, &json!("어디서 볼 수 있어요?"))
    );
    let c = hear(&s, &engine, "저도 궁금했어요", Some("나"), Some(500), 2.0).await;
    assert_eq!(c["merged_from"], b["utterance_id"]);
    assert_eq!(a["merged_from"], Value::Null);
}

#[tokio::test]
async fn backchannel_skips_gate_and_context() {
    let gater = gater(0.2);
    let engine = gate_engine(&gater);
    let s = engine.new_session();
    hear(&s, &engine, "점심 뭐 먹어요", None, Some(3000), 0.0).await;
    let b = hear(&s, &engine, "아 네", None, Some(1500), 1.0).await;
    assert_eq!(b["skipped"], "backchannel");
    assert_eq!(b["fired"], false);
    assert_eq!(b["p_lookup"], Value::Null);
    assert!(!b["utterance_id"].as_str().unwrap().is_empty());
    let seen: Vec<String> = gater.calls().into_iter().map(|(_, line)| line).collect();
    assert_eq!(seen, lines(&["나: 점심 뭐 먹어요"]));
    assert_eq!(s.recent().await, lines(&["나: 점심 뭐 먹어요"]));
    assert_eq!(s.summary().await["utterances"], 2);
}

#[tokio::test]
async fn backchannel_inside_question_is_skipped_when_joining() {
    for speaker in [None, Some("상대")] {
        let gater = gater(0.2);
        let engine = gate_engine(&gater);
        let s = engine.new_session();
        let a = hear(&s, &engine, "그 설정은", speaker, Some(3000), 0.0).await;
        hear(&s, &engine, "음", speaker, Some(800), 1.0).await;
        let c = hear(&s, &engine, "어디서 바꿔요?", speaker, Some(800), 2.0).await;
        let line = format!("{}: 그 설정은 어디서 바꿔요?", speaker.unwrap_or("나"));
        assert_eq!(c["merged_from"], a["utterance_id"], "{speaker:?}");
        assert_eq!(c["text"], "그 설정은 어디서 바꿔요?", "{speaker:?}");
        assert_eq!(gater.calls().pop().unwrap().1, line, "{speaker:?}");
        assert_eq!(s.recent().await, [line], "{speaker:?}");
    }
}

#[tokio::test]
async fn long_pause_around_backchannel_breaks_joining() {
    let engine = gate_engine(&gater(0.2));
    let s = engine.new_session();
    hear(&s, &engine, "그 설정은", None, Some(3000), 0.0).await;
    hear(&s, &engine, "음", None, Some(1200), 1.0).await;
    let c = hear(&s, &engine, "어디서 바꿔요?", None, Some(800), 2.0).await;
    assert_eq!(
        (&c["merged_from"], &c["text"]),
        (&Value::Null, &json!("어디서 바꿔요?"))
    );
}

#[tokio::test]
async fn question_after_leading_backchannel_stands_alone() {
    let engine = gate_engine(&gater(0.9));
    let s = engine.new_session();
    hear(&s, &engine, "네", None, Some(3000), 0.0).await;
    let q = hear(&s, &engine, "그거 어디서 바꿔요?", None, Some(500), 1.0).await;
    assert_eq!(
        (&q["merged_from"], &q["text"]),
        (&Value::Null, &json!("그거 어디서 바꿔요?"))
    );
}

#[tokio::test]
async fn consecutive_backchannels_stay_out() {
    let engine = gate_engine(&gater(0.2));
    let s = engine.new_session();
    hear(&s, &engine, "네", None, Some(3000), 0.0).await;
    let b = hear(&s, &engine, "네 맞아요", None, Some(300), 1.0).await;
    assert_eq!(b["skipped"], "backchannel");
    assert_eq!(b["merged_from"], Value::Null);
    assert!(s.recent().await.is_empty());
}

/// 판단의 세 번째 질문이에요: p_end가 END_THR 이상이면 말한 사람이 회의를 끝내는 중이에요.
#[tokio::test]
async fn end_of_meeting_is_flagged_from_the_gate() {
    let p_end = Arc::new(Mutex::new(0.9));
    let gate_end = p_end.clone();
    let closing = Arc::new(FakeGater::from_fn(move |_, _| {
        Ok(fake_gate(0.02, Some(*gate_end.lock().unwrap())))
    }));
    let engine = gate_engine(&closing);
    let s = engine.new_session();
    let bye = hear(
        &s,
        &engine,
        "그럼 오늘 회의는 여기까지 하죠",
        None,
        None,
        0.0,
    )
    .await;
    assert_eq!(
        (&bye["ending"], &bye["p_end"], &bye["fired"]),
        (&json!(true), &json!(0.9), &json!(false))
    );
    *p_end.lock().unwrap() = 0.3;
    let topic = hear(
        &s,
        &engine,
        "이 안건은 여기서 끝내고 넘어가죠",
        None,
        None,
        5.0,
    )
    .await;
    assert_eq!(
        (&topic["ending"], &topic["p_end"]),
        (&json!(false), &json!(0.3))
    );
    // 판단이 실패하거나 p_end가 없는 예전 가짜는 회의를 끝내지 않아요.
    let engine2 = gate_engine(&gater(0.2));
    let s2 = engine2.new_session();
    let plain = hear(&s2, &engine2, "점심 뭐 먹어요", None, None, 0.0).await;
    assert_eq!(
        (&plain["p_end"], &plain["ending"]),
        (&Value::Null, &json!(false))
    );
    let bc = hear(&s2, &engine2, "네", None, None, 1.0).await;
    assert_eq!(
        (&bc["skipped"], &bc["ending"]),
        (&json!("backchannel"), &json!(false))
    );
    let failing = gate_engine(&Arc::new(FakeGater::fail(Kind::Down, "jev 503")));
    let s3 = failing.new_session();
    let rec = hear(&s3, &failing, "그럼 여기까지 하죠", None, None, 0.0).await;
    assert_eq!(
        (&rec["p_end"], &rec["ending"], &rec["error_kind"]),
        (&Value::Null, &json!(false), &json!("down"))
    );
}

/// test_end_threshold_comes_from_settings의 세션 부분이에요(설정 부분은 settings 테스트에 있어요).
#[tokio::test]
async fn end_threshold_comes_from_settings() {
    let engine = gate_engine(&Arc::new(FakeGater::ending(0.02, Some(0.85))));
    assert_eq!(engine.settings.end, 0.8);
    let s = engine.new_session();
    let rec = s
        .hear(
            &engine,
            "수고하셨습니다",
            None,
            engine.settings.gate,
            Some(0.0),
            None,
            None,
            0.9,
        )
        .await;
    assert_eq!(rec["ending"], false);
    // 기본 기준(0.8)이면 같은 판단이 회의 끝이에요.
    let rec = hear(&s, &engine, "수고하셨습니다", None, None, 1.0).await;
    assert_eq!(rec["ending"], true);
}

/// stderr를 잡는 대신 그 줄을 만드는 `heard_line`을 봐요.
#[tokio::test]
async fn heard_log_line_stays_on_one_line() {
    let first = heard_line("s1u1", Some(0.9), None, None, false, "첫째 줄\r\n둘째 줄");
    let second = heard_line("s1u2", None, None, None, true, "아 네\n그래요");
    for l in [&first, &second] {
        assert!(
            l.starts_with("[heard]") && !l.contains(['\r', '\n']),
            "{l:?}"
        );
    }
    assert_eq!(first, "[heard] s1u1 p=0.90 첫째 줄  둘째 줄");
    assert_eq!(second, "[heard] s1u2 skipped=backchannel 아 네 그래요");
    assert_eq!(
        heard_line(
            "s1u3",
            Some(0.2),
            Some(0.85),
            Some("s1u2"),
            false,
            "어디서 바꿔요?"
        ),
        "[heard] s1u3 p=0.20 end=0.85 (joined s1u2) 어디서 바꿔요?"
    );
    // 로그를 켜도 기록은 같아요(줄은 stderr로 가요).
    let mut engine = gate_engine(&gater(0.9));
    engine.log_text = true;
    let s = engine.new_session();
    let a = hear(&s, &engine, "첫째 줄\r\n둘째 줄", None, Some(3000), 0.0).await;
    let b = hear(&s, &engine, "네", None, Some(3000), 10.0).await;
    assert_eq!(
        (&a["fired"], &b["skipped"]),
        (&json!(true), &json!("backchannel"))
    );
}

/// 기록의 키 순서는 바꾸지 않아요(앱과 계약 테스트가 JSON을 그대로 비교해요).
#[tokio::test]
async fn record_keys_follow_python_order() {
    let gater = Arc::new(FakeGater::from_fn(|_, line| {
        Ok(fake_gate(if line.contains('?') { 0.9 } else { 0.2 }, None))
    }));
    let (engine, _) = engine_of(
        index(),
        &gater,
        &Arc::new(FakeReranker::ok(&[0.1, 0.2, 0.95, 0.3])),
        &Arc::new(FakeAnswerer::ok("")),
    );
    let s = engine.new_session();
    let head = [
        "utterance_id",
        "text",
        "speaker",
        "voice",
        "person",
        "at",
        "gap_ms",
        "stt_confidence",
        "cards",
        "shown",
        "merged_from",
    ];
    let with = |tail: &[&'static str]| -> Vec<&'static str> { [&head[..], tail].concat() };

    let bc = hear(&s, &engine, "네", None, Some(3000), 0.0).await;
    assert_eq!(
        keys(&bc),
        [
            "utterance_id",
            "text",
            "speaker",
            "voice",
            "person",
            "at",
            "gap_ms",
            "cards",
            "shown",
            "merged_from",
            "skipped",
            "p_lookup",
            "fired",
            "p_end",
            "ending",
            "stt_confidence"
        ]
    );
    let gated = ["p_lookup", "fired", "p_end", "ending", "timing_ms"];
    let quiet = hear(&s, &engine, "점심 뭐 먹어요", None, Some(3000), 1.0).await;
    assert_eq!(keys(&quiet), with(&gated));
    assert_eq!(keys(&quiet["timing_ms"]), ["gate"]);
    let asked = hear(&s, &engine, "메일 탭 401 어디 봐요?", None, Some(3000), 2.0).await;
    assert_eq!(keys(&asked), with(&gated));
    assert_eq!(asked["fired"], true);

    let found = s
        .find(&engine, asked["utterance_id"].as_str().unwrap(), 0.8)
        .await
        .unwrap();
    assert_eq!(
        keys(&found),
        with(&[
            "p_lookup",
            "fired",
            "p_end",
            "ending",
            "timing_ms",
            "best_p",
            "low_confidence",
            "cost_usd"
        ])
    );
    assert_eq!(keys(&found["timing_ms"]), ["gate", "search", "rerank"]);
    assert_eq!(
        keys(&found["cards"][0]),
        [
            "id",
            "path",
            "kind",
            "source",
            "line_start",
            "line_end",
            "title",
            "byline",
            "snippet",
            "score",
            "url",
            "github_url",
            "local_path",
            "chunk",
            "repeat"
        ]
    );

    // 판단이 실패한 기록이에요. 찾기를 부르면 timing_ms가 끝에 새로 생겨요.
    let failing = Arc::new(FakeGater::fail(Kind::Key, "no Jev key"));
    let (engine2, _) = engine_of(
        index(),
        &failing,
        &Arc::new(FakeReranker::ok(&[0.1, 0.2, 0.95, 0.3])),
        &Arc::new(FakeAnswerer::ok("")),
    );
    let s2 = engine2.new_session();
    let err = hear(&s2, &engine2, "메일 탭 401 어디 봐요?", None, None, 0.0).await;
    let errored = [
        "p_lookup",
        "fired",
        "p_end",
        "ending",
        "error",
        "error_kind",
    ];
    assert_eq!(keys(&err), with(&errored));
    assert_eq!(
        (&err["fired"], &err["error"], &err["error_kind"]),
        (&json!(false), &json!("no Jev key"), &json!("key"))
    );
    let found = s2
        .find(&engine2, err["utterance_id"].as_str().unwrap(), 0.8)
        .await
        .unwrap();
    assert_eq!(
        keys(&found),
        with(&[
            "p_lookup",
            "fired",
            "p_end",
            "ending",
            "error",
            "error_kind",
            "timing_ms",
            "best_p",
            "low_confidence",
            "cost_usd"
        ])
    );
    assert_eq!(keys(&found["timing_ms"]), ["search", "rerank"]);

    // 색인이 비었어요: 판단을 부르지 않아요.
    let quiet_gater = gater.clone();
    let (empty, _) = engine_of(
        index_of(&json!([]), &json!({})),
        &quiet_gater,
        &Arc::new(FakeReranker::all(0.9)),
        &Arc::new(FakeAnswerer::ok("")),
    );
    let before = gater.calls().len();
    let s3 = empty.new_session();
    let rec = hear(&s3, &empty, "결제 재시도 몇 번이에요?", None, None, 0.0).await;
    assert_eq!(keys(&rec), with(&["p_lookup", "fired", "p_end", "ending"]));
    assert_eq!(gater.calls().len(), before);
}

/// 빈 색인이면 세션도 판단을 부르지 않아요.
#[tokio::test]
async fn empty_index_session_makes_no_gate_call() {
    let gater = gater(0.9);
    let reranker = Arc::new(FakeReranker::all(0.9));
    let (engine, _) = engine_of(
        index_of(&json!([]), &json!({})),
        &gater,
        &reranker,
        &Arc::new(FakeAnswerer::ok("")),
    );
    let s = engine.new_session();
    let rec = utterance(&s, &engine, "결제 재시도 몇 번이에요?", None, 0.0).await;
    assert_eq!((&rec["fired"], &rec["cards"]), (&json!(false), &json!([])));
    assert!(rec.get("error").is_none());
    assert_eq!(s.summary().await["utterances"], 1);
    assert!(gater.calls().is_empty() && reranker.calls().is_empty());
}

/// 앱이 `gate_empty`로 연 회의는 색인이 비어도 판단해요: 질문은 잡히고 카드는 없어요(앱이 적어 뒀다가 다시 찾아요).
#[tokio::test]
async fn an_empty_index_session_gates_when_asked() {
    let gater = gater(0.9);
    let reranker = Arc::new(FakeReranker::all(0.9));
    let (engine, _) = engine_of(
        index_of(&json!([]), &json!({})),
        &gater,
        &reranker,
        &Arc::new(FakeAnswerer::ok("")),
    );
    let s = engine.new_session_with(false, true, Voices::default());
    let rec = utterance(&s, &engine, "결제 재시도 몇 번이에요?", None, 0.0).await;
    assert_eq!((&rec["fired"], &rec["cards"]), (&json!(true), &json!([])));
    assert_eq!(gater.calls().len(), 1);
    assert!(reranker.calls().is_empty(), "nothing to rank");
}

#[tokio::test]
async fn record_values_follow_python() {
    let engine = gate_engine(&gater(0.2));
    let s = engine.new_session();
    let rec = s
        .hear(
            &engine,
            "점심 뭐 먹어요",
            Some("민준"),
            0.7,
            Some(12.5),
            Some(3000),
            Some(0.75),
            0.8,
        )
        .await;
    assert_eq!(
        rec,
        json!({"utterance_id": s.id.clone() + "u1", "text": "점심 뭐 먹어요", "speaker": "민준", "voice": null, "person": null, "at": 12.5,
               "gap_ms": 3000, "stt_confidence": 0.75, "cards": [], "shown": false, "merged_from": null,
               "p_lookup": 0.2, "fired": false, "p_end": null, "ending": false, "timing_ms": {"gate": 1.0}})
    );
    // 시각을 주지 않으면 지금 시각(초)이에요.
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let rec = s
        .hear(&engine, "다음 안건", None, 0.7, None, None, None, 0.8)
        .await;
    let at = rec["at"].as_f64().unwrap();
    assert!(at >= before && at < before + 60.0, "{at}");
    assert_eq!(
        (
            &rec["utterance_id"],
            &rec["speaker"],
            &rec["gap_ms"],
            &rec["stt_confidence"]
        ),
        (
            &json!(s.id.clone() + "u2"),
            &Value::Null,
            &Value::Null,
            &Value::Null
        )
    );
}

#[tokio::test]
async fn summary_lists_answered_and_unanswered() {
    let reranker = Arc::new(FakeReranker::ok(&[0.1, 0.2, 0.5, 0.3]));
    let (engine, _) = engine_of(
        index(),
        &gater(0.9),
        &reranker,
        &Arc::new(FakeAnswerer::ok("")),
    );
    let s = engine.new_session();
    let low = utterance(&s, &engine, "메일 탭 401 어디 봐요?", None, 0.0).await;
    assert_eq!(
        (&low["fired"], &low["shown"]),
        (&json!(true), &json!(false))
    );
    // 판단만 넘고 아직 찾지 않은 발화는 best_p가 없어요.
    let unfound = hear(&s, &engine, "세션은 어디서 만들어요?", None, None, 1.0).await;
    let summary = s.summary().await;
    assert_eq!(
        keys(&summary),
        [
            "session_id",
            "utterances",
            "fired",
            "answered",
            "unanswered"
        ]
    );
    assert_eq!(
        summary,
        json!({"session_id": s.id, "utterances": 2, "fired": 2, "answered": [],
               "unanswered": [{"utterance_id": low["utterance_id"], "text": "메일 탭 401 어디 봐요?", "best_p": 0.5},
                              {"utterance_id": unfound["utterance_id"], "text": "세션은 어디서 만들어요?", "best_p": null}]})
    );

    let shown = Arc::new(FakeReranker::ok(&[0.1, 0.2, 0.95, 0.3]));
    let (engine, _) = engine_of(
        index(),
        &gater(0.9),
        &shown,
        &Arc::new(FakeAnswerer::ok("")),
    );
    let s = engine.new_session();
    let rec = utterance(&s, &engine, "메일 탭 401 어디 봐요?", None, 0.0).await;
    let summary = s.summary().await;
    assert_eq!(summary["answered"], json!([rec]));
    assert_eq!(summary["unanswered"], json!([]));
}

/// `find`는 그런 발화가 없거나 맞장구면(`_context`가 없어요) None이고, 서버는 404로 답해요.
#[tokio::test]
async fn find_unknown_or_backchannel_utterance_is_none() {
    let reranker = Arc::new(FakeReranker::all(0.9));
    let (engine, _) = engine_of(
        index(),
        &gater(0.9),
        &reranker,
        &Arc::new(FakeAnswerer::ok("")),
    );
    let s = engine.new_session();
    let bc = hear(&s, &engine, "네", None, None, 0.0).await;
    assert!(s.find(&engine, "s9u9", 0.8).await.is_none());
    assert!(
        s.find(&engine, bc["utterance_id"].as_str().unwrap(), 0.8)
            .await
            .is_none()
    );
    assert!(reranker.calls().is_empty());
}

#[tokio::test]
async fn sessions_are_numbered_kept_and_dropped() {
    let engine = gate_engine(&gater(0.2));
    let a = engine.new_session();
    let b = engine.new_session();
    assert_eq!((a.id.as_str(), b.id.as_str()), ("s1", "s2"));
    assert!(Arc::ptr_eq(&engine.session("s1").unwrap(), &a));
    let rec = hear(&b, &engine, "점심 뭐 먹어요", None, None, 0.0).await;
    assert_eq!(rec["utterance_id"], "s2u1");
    assert!(engine.drop_session("s1"));
    assert!(!engine.drop_session("s1"));
    assert!(engine.session("s1").is_none() && engine.session("s3").is_none());
    assert_eq!(engine.new_session().id, "s3"); // 지운 번호는 다시 쓰지 않아요
}

/// 서버 핸들러가 세션을 부를 수 있게 세션의 future는 스레드를 넘어갈 수 있어야 해요(Send).
#[tokio::test]
async fn session_calls_are_send() {
    fn send<T: Send>(t: T) -> T {
        t
    }
    let engine = gate_engine(&gater(0.9));
    let s = engine.new_session();
    let rec = send(s.hear(
        &engine,
        "세션은 어디서 만들어요?",
        None,
        0.7,
        Some(0.0),
        None,
        None,
        0.8,
    ))
    .await;
    send(s.find(&engine, rec["utterance_id"].as_str().unwrap(), 0.8)).await;
    send(s.utterance(&engine, "다른 질문이에요?", None, 0.7, 0.8, Some(1.0))).await;
    send(s.summary()).await;
}

/// 앱이 기다리다 그만두면(시간 초과, 회의 멈춤) 서버는 판단을 기다리던 `hear`를 버려요. 버린 발화는 기록에도
/// 앞 대화에도 남지 않아야 해요: 다음 조각은 버린 호출이 없었던 것처럼 이어져요.
#[tokio::test]
async fn dropped_hear_leaves_the_session_unchanged() {
    let gater = gater(0.2);
    let engine = gate_engine(&gater);
    let s = engine.new_session();
    let a = hear(&s, &engine, "점심 뭐 먹어요", None, Some(3000), 0.0).await;
    let before = lines(&["나: 점심 뭐 먹어요"]);
    assert_eq!(s.recent().await, before);

    gater.set_delay(Duration::from_secs(60));
    // 새 줄(잇지 않음)과 앞 줄에 잇는 조각을 하나씩 버려요.
    for (text, gap) in [("그 설정은", 3000), ("어디서", 800)] {
        let dropped = tokio::time::timeout(
            Duration::from_millis(20),
            hear(&s, &engine, text, None, Some(gap), 1.0),
        )
        .await;
        assert!(dropped.is_err(), "{text}");
        assert_eq!(s.recent().await, before, "{text}");
        assert_eq!(s.summary().await["utterances"], 1, "{text}");
    }

    gater.set_delay(Duration::ZERO);
    let c = hear(&s, &engine, "어디서 바꿔요?", None, Some(800), 2.0).await;
    let joined = "나: 점심 뭐 먹어요 어디서 바꿔요?";
    assert_eq!(c["utterance_id"], "s1u2");
    assert_eq!(c["merged_from"], a["utterance_id"]);
    // 앞 줄은 이은 줄 안에만 있어요: 앞 대화에 다시 나오지 않아요.
    assert_eq!(gater.calls().pop().unwrap(), (vec![], joined.to_string()));
    assert_eq!(s.recent().await, [joined]);
    assert_eq!(s.summary().await["utterances"], 2);
}

/// 판단, 받아쓰기, 장부 모델을 넣은 엔진이에요.
fn meeting_engine(
    ix: Index,
    gater: &Arc<FakeGater>,
    stt: &Arc<FakeTranscriber>,
    notes: &Arc<FakeNoteWriter>,
) -> Engine {
    let services = Services {
        gater: gater.clone(),
        transcriber: stt.clone(),
        note_writer: notes.clone(),
        ..services(
            Arc::new(FakeReranker::all(0.9)),
            Arc::new(FakeAnswerer::ok("")),
        )
    };
    engine_with(ix, services).0
}

/// p_lookup 0.2 판단과 정해 둔 글을 주는 장부 모델이에요(받아쓰기는 없어요).
fn ledger_engine(notes: &Arc<FakeNoteWriter>) -> Engine {
    let stt = Arc::new(FakeTranscriber::fail(Kind::Down, "not faked"));
    meeting_engine(index(), &gater(0.2), &stt, notes)
}

fn note_writer(raw: &str) -> Arc<FakeNoteWriter> {
    Arc::new(FakeNoteWriter::ok(raw))
}

/// 받아쓰는 중인 발화가 없으면 장부는 기다리지 않아요: 3초 안에 와야 해요(남은 수가 새면 5초를 기다려요).
/// 실제 시계라서 바쁜 기계에서도 5초와 가를 만큼만 여유를 둬요.
async fn ledger_soon(s: &Session, e: &Engine) -> Value {
    let started = Instant::now();
    let out = s.ledger(e, None).await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "ledger waited {:?}",
        started.elapsed()
    );
    out
}

#[tokio::test]
async fn empty_transcript_is_not_logged() {
    let gater = gater(0.2);
    let stt = Arc::new(FakeTranscriber::ok(""));
    let engine = meeting_engine(index(), &gater, &stt, &note_writer("{}"));
    let s = engine.new_session();
    let rec = s.hear_audio(&engine, b"RIFF", None, None).await;
    assert_eq!(rec["utterance_id"], Value::Null);
    assert_eq!(
        serde_json::to_string(&rec).unwrap(),
        r#"{"utterance_id":null,"text":"","fired":false,"timing_ms":{"stt":1.0},"stt":"server"}"#
    );
    assert_eq!(s.summary().await["utterances"], 0);
    assert_eq!(stt.calls(), [b"RIFF".to_vec()]);
    assert!(gater.calls().is_empty());
    ledger_soon(&s, &engine).await;
}

/// 받아쓰기가 실패하면 기록하지 않고 이유를 돌려줘요(종류와 글, 글은 200자까지).
#[tokio::test]
async fn failed_transcription_is_not_logged() {
    let gater = gater(0.2);
    let stt = Arc::new(FakeTranscriber::fail(Kind::Network, "stt request failed"));
    let engine = meeting_engine(index(), &gater, &stt, &note_writer("{}"));
    let s = engine.new_session();
    let rec = s
        .hear_audio(&engine, b"RIFF", Some("상대"), Some(3000))
        .await;
    assert_eq!(
        serde_json::to_string(&rec).unwrap(),
        r#"{"utterance_id":null,"text":"","fired":false,"error":"stt request failed","error_kind":"network","stt":"server"}"#
    );
    let long = Arc::new(FakeTranscriber::fail(Kind::Down, &"가".repeat(300)));
    let engine2 = meeting_engine(index(), &gater, &long, &note_writer("{}"));
    let rec = engine2
        .new_session()
        .hear_audio(&engine2, b"RIFF", None, None)
        .await;
    assert_eq!(rec["error"], "가".repeat(200));
    assert_eq!(s.summary().await["utterances"], 0);
    assert!(gater.calls().is_empty());
    ledger_soon(&s, &engine).await;
}

/// test_stt_confidence_is_recorded_only의 세션 부분이에요(받아쓰기 확신은 기록만 하고 판단에 쓰지 않아요).
#[tokio::test]
async fn stt_confidence_is_recorded_only() {
    let stt = Arc::new(FakeTranscriber::script(vec![Ok(transcript(
        "점심 뭐 먹어요",
        Some(0.42),
    ))]));
    let engine = meeting_engine(index(), &gater(0.2), &stt, &note_writer("{}"));
    let rec = engine
        .new_session()
        .hear_audio(&engine, b"RIFF", None, None)
        .await;
    assert_eq!(rec["stt_confidence"], 0.42);
    assert_eq!(rec["fired"], false);
    assert!(rec.get("low_confidence").is_none());
}

#[tokio::test]
async fn hear_audio_returns_word_timings_last() {
    use tacie_engine::services::SpokenWord;
    let mut t = transcript("링크는 7일", None);
    t.words = vec![
        SpokenWord {
            text: "링크는".into(),
            start: 0.12,
            end: 0.58,
        },
        SpokenWord {
            text: "7일".into(),
            start: 0.9,
            end: 1.3,
        },
    ];
    let stt = Arc::new(FakeTranscriber::script(vec![Ok(t)]));
    let engine = meeting_engine(index(), &gater(0.2), &stt, &note_writer("{}"));
    let s = engine.new_session();
    let out = s.hear_audio(&engine, b"RIFF", None, None).await;
    assert_eq!(
        out["words"],
        json!([{"text":"링크는","start":0.12,"end":0.58},{"text":"7일","start":0.9,"end":1.3}])
    );
    assert_eq!(out.as_object().unwrap().keys().last().unwrap(), "words");
    // 글이 빈 응답에는 words가 없어요.
    let empty = Arc::new(FakeTranscriber::ok(""));
    let engine = meeting_engine(index(), &gater(0.2), &empty, &note_writer("{}"));
    let rec = engine
        .new_session()
        .hear_audio(&engine, b"RIFF", None, None)
        .await;
    assert!(rec.get("words").is_none());
}

/// 받아쓴 발화는 `hear` 기록에 받아쓰기 시간을 맨 앞에 넣은 `timing_ms`를 달아요. 키 순서는 바꾸지 않아요:
/// 판단한 기록은 `timing_ms`가 제자리(끝)에 있고, 없던 기록(맞장구, 판단 실패, 빈 색인)은 끝에 새로 붙어요.
#[tokio::test]
async fn transcribed_records_put_stt_time_first() {
    let head = [
        "utterance_id",
        "text",
        "speaker",
        "voice",
        "person",
        "at",
        "gap_ms",
        "stt_confidence",
        "cards",
        "shown",
        "merged_from",
        "p_lookup",
        "fired",
        "p_end",
        "ending",
    ];
    let with = |tail: &[&'static str]| -> Vec<&'static str> { [&head[..], tail].concat() };
    let heard = |text: &str| Arc::new(FakeTranscriber::ok(text));
    let notes = note_writer("{}");

    let gater = gater(0.2);
    let stt = heard("점심 뭐 먹어요");
    let engine = meeting_engine(index(), &gater, &stt, &notes);
    let s = engine.new_session();
    let rec = s
        .hear_audio(&engine, b"RIFF", Some("민준"), Some(3000))
        .await;
    assert_eq!(keys(&rec), with(&["timing_ms", "stt"]));
    assert_eq!(
        serde_json::to_string(&rec["timing_ms"]).unwrap(),
        r#"{"stt":1.0,"gate":1.0}"#
    );
    assert_eq!(
        (&rec["text"], &rec["speaker"], &rec["gap_ms"]),
        (&json!("점심 뭐 먹어요"), &json!("민준"), &json!(3000))
    );
    assert_eq!(
        gater.calls().pop().unwrap(),
        (vec![], "민준: 점심 뭐 먹어요".to_string())
    );

    let engine = meeting_engine(index(), &gater, &heard("네"), &notes);
    let rec = engine
        .new_session()
        .hear_audio(&engine, b"RIFF", None, None)
        .await;
    assert_eq!(
        keys(&rec),
        [
            "utterance_id",
            "text",
            "speaker",
            "voice",
            "person",
            "at",
            "gap_ms",
            "cards",
            "shown",
            "merged_from",
            "skipped",
            "p_lookup",
            "fired",
            "p_end",
            "ending",
            "stt_confidence",
            "timing_ms",
            "stt"
        ]
    );
    assert_eq!(rec["timing_ms"], json!({"stt": 1.0}));

    let failing = Arc::new(FakeGater::fail(Kind::Key, "no Jev key"));
    let engine = meeting_engine(index(), &failing, &heard("점심 뭐 먹어요"), &notes);
    let rec = engine
        .new_session()
        .hear_audio(&engine, b"RIFF", None, None)
        .await;
    assert_eq!(
        keys(&rec),
        with(&["error", "error_kind", "timing_ms", "stt"])
    );
    assert_eq!(rec["timing_ms"], json!({"stt": 1.0}));

    let empty = meeting_engine(
        index_of(&json!([]), &json!({})),
        &gater,
        &heard("점심 뭐 먹어요"),
        &notes,
    );
    let rec = empty
        .new_session()
        .hear_audio(&empty, b"RIFF", None, None)
        .await;
    assert_eq!(keys(&rec), with(&["timing_ms", "stt"]));
    assert_eq!(rec["timing_ms"], json!({"stt": 1.0}));

    // 받아쓰기 시간은 돌려주는 기록에만 붙어요: 찾기 뒤 기록에는 없어요.
    let asking = Arc::new(FakeGater::ok(0.9));
    let engine = meeting_engine(index(), &asking, &heard("메일 탭 401 어디 봐요?"), &notes);
    let s = engine.new_session();
    let rec = s.hear_audio(&engine, b"RIFF", None, None).await;
    assert_eq!(keys(&rec["timing_ms"]), ["stt", "gate"]);
    let found = s
        .find(&engine, rec["utterance_id"].as_str().unwrap(), 0.8)
        .await
        .unwrap();
    assert_eq!(keys(&found["timing_ms"]), ["gate", "search", "rerank"]);
}

#[tokio::test]
async fn session_ledger_sends_clean_lines_and_caches() {
    let notes = note_writer(r#"{"items": []}"#);
    let engine = ledger_engine(&notes);
    let s = engine.new_session();
    hear(&s, &engine, "그 설정은", Some("상대"), Some(3000), 0.0).await;
    hear(&s, &engine, "네", Some("상대"), Some(800), 1.0).await;
    hear(&s, &engine, "어디서 바꿔요?", Some("상대"), Some(800), 2.0).await;
    hear(&s, &engine, "배포는 목요일로 해요", None, Some(3000), 3.0).await;
    s.ledger(&engine, None).await;
    let seen = notes.calls();
    assert!(
        seen[0].contains("[s1u3] 상대: 그 설정은 어디서 바꿔요?"),
        "{}",
        seen[0]
    );
    assert!(!seen[0].contains("[s1u1]") && !seen[0].contains("[s1u2]"));
    assert!(seen[0].contains("[s1u4] 화자: 배포는 목요일로 해요"));
    assert_eq!(
        seen[0],
        "[s1u3] 상대: 그 설정은 어디서 바꿔요?\n[s1u4] 화자: 배포는 목요일로 해요"
    );
    let first = s.ledger(&engine, None).await;
    assert_eq!(notes.calls().len(), 1); // 새 발화가 없어요: 지난 결과예요
    hear(&s, &engine, "하나 더요", None, Some(3000), 4.0).await;
    let again = s.ledger(&engine, None).await;
    assert_eq!(notes.calls().len(), 2);
    assert_eq!(first, again);
}

#[tokio::test]
async fn session_ledger_skips_the_model_when_nothing_was_said() {
    let notes = note_writer("{}");
    let engine = ledger_engine(&notes);
    let s = engine.new_session();
    assert_eq!(s.ledger(&engine, None).await["items"], json!([]));
    assert!(notes.calls().is_empty());
    hear(&s, &engine, "네", None, Some(3000), 0.0).await;
    let out = s.ledger(&engine, None).await;
    assert_eq!(out["items"], json!([]));
    assert!(notes.calls().is_empty());
    assert_eq!(
        out,
        json!({"kind": "meeting", "kind_source": "auto", "title": null, "summary": [], "topics": [], "items": [],
               "dropped": 0, "ms": 0, "cost": 0})
    );
}

#[tokio::test]
async fn session_ledger_failures() {
    let offline = Arc::new(FakeNoteWriter::fail(Kind::Network, "ledger request failed"));
    let engine = ledger_engine(&offline);
    let s = engine.new_session();
    hear(&s, &engine, "배포는 목요일로 해요", None, Some(3000), 0.0).await;
    assert_eq!(
        s.ledger(&engine, None).await,
        json!({"items": [], "error": "ledger request failed", "error_kind": "network"})
    );
    // 실패한 결과는 기억하지 않아요: 다시 물으면 다시 불러요.
    s.ledger(&engine, None).await;
    assert_eq!(offline.calls().len(), 2);

    let unreadable = note_writer("JSON 아님");
    let engine2 = ledger_engine(&unreadable);
    let s2 = engine2.new_session();
    hear(&s2, &engine2, "배포는 목요일로 해요", None, Some(3000), 0.0).await;
    assert_eq!(
        s2.ledger(&engine2, None).await,
        json!({"items": [], "error": "unreadable"})
    );
    s2.ledger(&engine2, None).await;
    assert_eq!(unreadable.calls().len(), 2);
}

#[tokio::test]
async fn session_ledger_returns_title_and_summary() {
    let raw = json!({"title": "배포 일정", "items": [],
                     "summary": [{"text": "목요일에 배포해요.", "utterance_ids": ["s1u1"]}]})
    .to_string();
    let engine = ledger_engine(&note_writer(&raw));
    let s = engine.new_session();
    hear(&s, &engine, "배포는 목요일로 해요", None, Some(3000), 0.0).await;
    let out = s.ledger(&engine, None).await;
    assert_eq!(
        (&out["title"], &out["summary"]),
        (&json!("배포 일정"), &json!(["목요일에 배포해요."]))
    );
}

/// test_ledger_leaves_out_items_that_stand_only_on_questions_tacie_took을 세션으로 돌려요(notes 테스트는
/// `notes_from`에 `asked`를 직접 넘겨요): 판단을 넘은 발화가 `asked`예요.
#[tokio::test]
async fn session_ledger_leaves_out_items_on_questions_tacie_took() {
    let raw = json!({"items": [
        {"kind": "open_question", "text": "서명 위치를 정하지 못했어요.", "utterance_ids": ["s1u1"]},
        {"kind": "open_question", "text": "누가 맡을지 정하지 못했어요.", "utterance_ids": ["s1u2"]},
        {"kind": "decision", "text": "서명은 위에 둬요.", "utterance_ids": ["s1u1", "s1u2"]},
        {"kind": "decision", "text": "서명 위치는 위예요.", "utterance_ids": ["s1u1"]}]}) // 질문을 결정으로 읽었어요
    .to_string();
    let asking = Arc::new(FakeGater::from_fn(|_, line| {
        Ok(fake_gate(if line.contains('?') { 0.9 } else { 0.2 }, None))
    }));
    let stt = Arc::new(FakeTranscriber::fail(Kind::Down, "not faked"));
    let engine = meeting_engine(index(), &asking, &stt, &note_writer(&raw));
    let s = engine.new_session();
    hear(
        &s,
        &engine,
        "서명 기본 위치가 위예요, 아래예요?",
        None,
        Some(3000),
        0.0,
    )
    .await;
    hear(
        &s,
        &engine,
        "그건 누가 맡을지 정해야겠네요",
        None,
        Some(3000),
        1.0,
    )
    .await;
    let out = s.ledger(&engine, None).await;
    let texts: Vec<&str> = out["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| it["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["누가 맡을지 정하지 못했어요.", "서명은 위에 둬요."]);
    assert_eq!(out["dropped"], 2);
}

/// 회의가 끝날 때 마지막 말이 아직 받아쓰는 중이에요: 장부는 그 발화가 기록될 때까지 기다려요.
#[tokio::test]
async fn ledger_waits_for_utterances_still_being_transcribed() {
    let slow =
        Arc::new(FakeTranscriber::ok("그건 제가 볼게요").with_delay(Duration::from_millis(500)));
    let notes = note_writer(r#"{"items": []}"#);
    let engine = meeting_engine(index(), &gater(0.2), &slow, &notes);
    let s = engine.new_session();
    hear(&s, &engine, "배포는 목요일로 해요", None, Some(3000), 0.0).await;
    let (last, (out, waited)) =
        tokio::join!(s.hear_audio(&engine, b"RIFF", None, Some(3000)), async {
            tokio::time::sleep(Duration::from_millis(50)).await; // 마지막 말이 아직 받아쓰기에 있어요
            let started = Instant::now();
            (s.ledger(&engine, None).await, started.elapsed())
        });
    // 받아쓰기가 끝나면 장부를 깨워요: 5초를 다 기다리지 않아요(실제 시계라 4초까지 봐줘요).
    assert!(waited < Duration::from_secs(4), "{waited:?}");
    assert_eq!(last["utterance_id"], "s1u2");
    assert!(out.get("error").is_none(), "{out}");
    let seen = notes.calls();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].contains("그건 제가 볼게요"), "{}", seen[0]);
}

/// 받아쓰기는 5초까지만 기다려요: 그 뒤에는 그때까지의 기록으로 써요. 시계를 멈춰 두고 돌려요(실제로 5초를 쓰지 않아요).
#[tokio::test(start_paused = true)]
async fn ledger_waits_at_most_five_seconds() {
    assert_eq!(LEDGER_WAIT, Duration::from_secs(5));
    let stuck =
        Arc::new(FakeTranscriber::ok("그건 제가 볼게요").with_delay(Duration::from_secs(60)));
    let notes = note_writer(r#"{"items": []}"#);
    let engine = meeting_engine(index(), &gater(0.2), &stuck, &notes);
    let s = engine.new_session();
    hear(&s, &engine, "배포는 목요일로 해요", None, Some(3000), 0.0).await;
    let (last, (out, waited)) =
        tokio::join!(s.hear_audio(&engine, b"RIFF", None, Some(3000)), async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let started = tokio::time::Instant::now();
            let out = s.ledger(&engine, None).await;
            (out, started.elapsed())
        });
    assert!(
        waited >= LEDGER_WAIT && waited < LEDGER_WAIT + Duration::from_secs(1),
        "{waited:?}"
    );
    assert!(out.get("error").is_none(), "{out}");
    // 5초 뒤의 장부에는 아직 받아쓰던 발화가 없어요. 그 발화는 나중에 기록돼요.
    assert_eq!(
        notes.calls(),
        ["[s1u1] 화자: 배포는 목요일로 해요".to_string()]
    );
    assert_eq!(last["utterance_id"], "s1u2");
}

/// 앱이 기다리다 그만두면 서버는 받아쓰던 `hear_audio`를 버려요. 버려도 받아쓰는 수는 내려가야 해요:
/// 아니면 장부가 늘 5초를 기다려요.
#[tokio::test]
async fn dropped_hear_audio_still_releases_busy() {
    let stuck =
        Arc::new(FakeTranscriber::ok("그건 제가 볼게요").with_delay(Duration::from_secs(60)));
    let notes = note_writer(r#"{"items": []}"#);
    let engine = meeting_engine(index(), &gater(0.2), &stuck, &notes);
    let s = engine.new_session();
    hear(&s, &engine, "배포는 목요일로 해요", None, Some(3000), 0.0).await;
    let dropped = tokio::time::timeout(
        Duration::from_millis(20),
        s.hear_audio(&engine, b"RIFF", None, Some(3000)),
    )
    .await;
    assert!(dropped.is_err());
    assert_eq!(stuck.calls().len(), 1);
    let out = ledger_soon(&s, &engine).await;
    assert!(out.get("error").is_none(), "{out}");
    assert_eq!(
        notes.calls(),
        ["[s1u1] 화자: 배포는 목요일로 해요".to_string()]
    );
}

/// 서버 핸들러가 부를 수 있게 받아쓰기와 장부의 future도 Send예요.
#[tokio::test]
async fn transcribe_and_ledger_calls_are_send() {
    fn send<T: Send>(t: T) -> T {
        t
    }
    let engine = meeting_engine(
        index(),
        &gater(0.2),
        &Arc::new(FakeTranscriber::ok("점심 뭐 먹어요")),
        &note_writer(r#"{"items": []}"#),
    );
    let s = engine.new_session();
    send(s.hear_audio(&engine, b"RIFF", None, None)).await;
    send(s.ledger(&engine, None)).await;
}

// 목소리로 화자 묶기(docs/04 4절): 받아쓰기와 나란히 임베딩을 뽑아 `voice` 이름을 붙이고 앞 대화 줄에 써요.

const VOICE_A: [f32; 3] = [1.0, 0.0, 0.0];
const VOICE_B: [f32; 3] = [0.0, 1.0, 0.0];

fn voice_engine(gater: &Arc<FakeGater>, stt: FakeTranscriber, voice: Option<FakeVoice>) -> Engine {
    let stt = Arc::new(stt);
    let engine = meeting_engine(index(), gater, &stt, &note_writer("{}"));
    match voice {
        Some(v) => engine.with_voice(Some(Arc::new(v))),
        None => engine,
    }
}

fn two_voices() -> FakeVoice {
    FakeVoice::by_wav(&[
        (b"A1", &VOICE_A, 3.0),
        (b"A2", &VOICE_A, 3.0),
        (b"B1", &VOICE_B, 3.0),
    ])
}

/// 받아쓰기가 소리만 적은 발화("[게임 소리]")는 화자가 아니에요: 목소리로 묶지 않고 판단하지도 않아요.
/// 그 뒤에 처음 말한 사람이 화자 1이에요(2026-11-02 원격 회의에서 게임 소리가 화자 2가 됐어요).
#[tokio::test]
async fn sound_only_lines_get_no_speaker() {
    let gater = gater(0.2);
    let stt = FakeTranscriber::script(vec![
        Ok(transcript("[게임 소리]", None)),
        Ok(transcript("배포는 언제 해요", None)),
    ]);
    let engine = voice_engine(&gater, stt, Some(two_voices()));
    let s = engine.new_session();
    let sound = s.hear_audio(&engine, b"B1", None, Some(3000)).await;
    assert_eq!(
        (&sound["voice"], &sound["skipped"], &sound["text"]),
        (&Value::Null, &json!("sound"), &json!("[게임 소리]"))
    );
    let said = s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    assert_eq!(said["voice"], "화자 1");
    assert_eq!(said["merged_from"], Value::Null);
    assert_eq!(
        gater.calls(),
        [(vec![], "화자 1: 배포는 언제 해요".to_string())]
    );
}

/// 한국어 회의에 섞여 나온 다른 언어 줄("你？", 아랍어)은 기록하지 않아요: 화자도 판단도 없어요.
/// 한국어 사이에 낀 다른 언어 단어만 빼고 나머지 말은 그대로 판단해요.
#[tokio::test]
async fn foreign_script_lines_are_dropped_and_words_trimmed() {
    let gater = gater(0.2);
    let stt = FakeTranscriber::script(vec![
        Ok(transcript("你？", None)),
        Ok(transcript("배포는 مرحبا 언제 해요", None)),
    ]);
    let engine = voice_engine(&gater, stt, Some(two_voices()));
    let s = engine.new_session();
    let dropped = s.hear_audio(&engine, b"B1", None, Some(3000)).await;
    assert_eq!(
        (
            &dropped["utterance_id"],
            &dropped["text"],
            &dropped["stt_cleaned"]
        ),
        (&Value::Null, &json!(""), &json!("foreign_dropped"))
    );
    let said = s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    assert_eq!(said["text"], "배포는 언제 해요");
    assert_eq!(said["voice"], "화자 1");
    assert_eq!(
        gater.calls(),
        [(vec![], "화자 1: 배포는 언제 해요".to_string())]
    );
}

/// 새 화자를 만들 만큼 길게 말했는지는 WAV 길이가 아니라 단어 시각으로 봐요: 3초 WAV에 0.2초만 말한 다른 목소리는
/// 새 화자가 아니라 가장 가까운 화자에게 붙어요.
#[tokio::test]
async fn short_speech_in_long_wav_does_not_start_a_speaker() {
    use tacie_engine::services::SpokenWord;
    // 2026-11-02에는 "你？"였어요. 지금은 다른 언어 글자라 받아쓰기 단계에서 빠져서(`stt_clean`) 한국어로 바꿨어요.
    let mut short = transcript("뭐라고?", None);
    short.words = vec![SpokenWord {
        text: "뭐라고?".into(),
        start: 1.1,
        end: 1.32,
    }];
    let gater = gater(0.2);
    let stt = FakeTranscriber::script(vec![Ok(transcript("배포는 언제 해요", None)), Ok(short)]);
    let engine = voice_engine(&gater, stt, Some(two_voices()));
    let s = engine.new_session();
    s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    let rec = s.hear_audio(&engine, b"B1", None, Some(3000)).await;
    assert_eq!(rec["voice"], "화자 1");
}

/// 앞 7초는 샘플 값 1000, 뒤 7초는 3000인 WAV예요(`FakeVoice::by_level`이 앞뒤를 다른 목소리로 줘요).
fn two_level_wav(first: i16, second: i16) -> Vec<u8> {
    let half = 16000 * 7;
    let s: Vec<i16> = (0..half * 2)
        .map(|i| if i < half { first } else { second })
        .collect();
    tacie_engine::split::wav_bytes(&s)
}

/// 0.3–13.7초에 고르게 퍼진 단어 8개예요. 7초 부근(4번째와 5번째 사이)이 가장 길게 쉬어요.
fn eight_words() -> tacie_engine::services::Transcript {
    use tacie_engine::services::SpokenWord;
    let mut t = transcript("하나 둘 셋 넷 다섯 여섯 일곱 여덟", None);
    let at = [
        (0.3, 1.5),
        (1.8, 3.2),
        (3.5, 4.9),
        (5.2, 6.5),
        (7.4, 8.6),
        (8.9, 10.2),
        (10.5, 12.0),
        (12.3, 13.7),
    ];
    t.words = "하나 둘 셋 넷 다섯 여섯 일곱 여덟"
        .split(' ')
        .zip(at)
        .map(|(w, (s, e))| SpokenWord {
            text: w.into(),
            start: s,
            end: e,
        })
        .collect();
    t
}

fn level_voices() -> FakeVoice {
    FakeVoice::by_level(&[(1000, &VOICE_A), (3000, &VOICE_B)])
}

/// 쉼 없이 두 사람이 말한 조각은 목소리가 바뀐 곳에서 두 줄로 나뉘어요(2026-11-02 스파이크).
#[tokio::test]
async fn two_voices_in_one_utterance_become_two_lines() {
    let gater = gater(0.2);
    let engine = voice_engine(
        &gater,
        FakeTranscriber::script(vec![Ok(eight_words())]),
        Some(level_voices()),
    );
    let s = engine.new_session();
    let rec = s
        .hear_audio(
            &engine,
            &two_level_wav(1000, 3000),
            Some("상대"),
            Some(3000),
        )
        .await;
    assert_eq!(
        (&rec["voice"], &rec["text"]),
        (&json!("화자 1"), &json!("하나 둘 셋 넷"))
    );
    let then = rec["then"].as_array().expect("split pieces");
    assert_eq!(then.len(), 1);
    assert_eq!(
        (&then[0]["voice"], &then[0]["text"]),
        (&json!("화자 2"), &json!("다섯 여섯 일곱 여덟"))
    );
    assert_eq!(then[0]["offset_ms"], 6950); // (6.5 + 7.4) / 2
    assert_eq!(then[0]["merged_from"], Value::Null);
    assert!((then[0]["words"][0]["start"].as_f64().unwrap() - 0.45).abs() < 1e-6);
    assert!(then[0].get("timing_ms").is_none());
    assert!(rec.get("timing_ms").is_some());
    // 조각마다 판단해요: 뒤 조각은 앞 조각 줄을 앞 대화로 받아요
    let calls = gater.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[1],
        (
            vec!["화자 1: 하나 둘 셋 넷".to_string()],
            "화자 2: 다섯 여섯 일곱 여덟".to_string()
        )
    );
}

/// 앱이 15초에서 자른 조각(쉼 0)의 맨 앞에 앞 사람의 말끝이 남으면, 그 말은 앞 줄 목소리로 돌아가 앞 줄에 이어 붙어요.
/// 맨 앞 3초 안의 바뀜은 창으로 찾지 못해요(2026-11-07 유튜브 대담 "…성장을 / 한 회사입니다.").
#[tokio::test]
async fn a_cut_speakers_last_words_go_back_to_their_line() {
    use tacie_engine::services::SpokenWord;
    let gater = gater(0.2);
    // 두 번째 조각: 0–1.6초는 앞 사람(1000), 나머지는 다른 사람(3000)이에요
    let mut tail = transcript(
        "회사입니다 내년도에는 얼마를 예상하시나요 그리고 또 하나",
        None,
    );
    let at = [
        (0.2, 1.4),
        (2.0, 3.6),
        (4.0, 5.4),
        (5.8, 8.0),
        (8.4, 10.0),
        (10.4, 12.0),
        (12.4, 13.8),
    ];
    tail.words = "회사입니다 내년도에는 얼마를 예상하시나요 그리고 또 하나"
        .split(' ')
        .zip(at)
        .map(|(w, (s, e))| SpokenWord {
            text: w.into(),
            start: s,
            end: e,
        })
        .collect();
    let engine = voice_engine(
        &gater,
        FakeTranscriber::script(vec![Ok(eight_words()), Ok(tail)]),
        Some(level_voices()),
    );
    let s = engine.new_session();
    let first = s
        .hear_audio(
            &engine,
            &two_level_wav(1000, 1000),
            Some("상대"),
            Some(3000),
        )
        .await;
    assert_eq!(first["voice"], "화자 1");
    let head = 16000 * 16 / 10;
    let samples: Vec<i16> = (0..16000 * 14)
        .map(|i| if i < head { 1000 } else { 3000 })
        .collect();
    let rec = s
        .hear_audio(
            &engine,
            &tacie_engine::split::wav_bytes(&samples),
            Some("상대"),
            Some(0),
        )
        .await;
    assert_eq!(
        (&rec["voice"], &rec["merged_from"]),
        (&json!("화자 1"), &first["utterance_id"])
    );
    assert!(
        rec["text"].as_str().unwrap().ends_with("여덟 회사입니다"),
        "{rec}"
    );
    let then = rec["then"].as_array().expect("the rest is its own line");
    assert_eq!(
        (&then[0]["voice"], &then[0]["text"]),
        (
            &json!("화자 2"),
            &json!("내년도에는 얼마를 예상하시나요 그리고 또 하나")
        )
    );
}

/// 쉬고 시작한 조각(쉼 0.3초 이상)은 앞 줄 목소리로 돌려보내지 않아요.
#[tokio::test]
async fn a_new_utterance_after_a_pause_keeps_its_head() {
    use tacie_engine::services::SpokenWord;
    let gater = gater(0.2);
    let mut tail = transcript(
        "회사입니다 내년도에는 얼마를 예상하시나요 그리고 또 하나",
        None,
    );
    let at = [
        (0.2, 1.4),
        (2.0, 3.6),
        (4.0, 5.4),
        (5.8, 8.0),
        (8.4, 10.0),
        (10.4, 12.0),
        (12.4, 13.8),
    ];
    tail.words = "회사입니다 내년도에는 얼마를 예상하시나요 그리고 또 하나"
        .split(' ')
        .zip(at)
        .map(|(w, (s, e))| SpokenWord {
            text: w.into(),
            start: s,
            end: e,
        })
        .collect();
    let engine = voice_engine(
        &gater,
        FakeTranscriber::script(vec![Ok(eight_words()), Ok(tail)]),
        Some(level_voices()),
    );
    let s = engine.new_session();
    s.hear_audio(
        &engine,
        &two_level_wav(1000, 1000),
        Some("상대"),
        Some(3000),
    )
    .await;
    let head = 16000 * 16 / 10;
    let samples: Vec<i16> = (0..16000 * 14)
        .map(|i| if i < head { 1000 } else { 3000 })
        .collect();
    let rec = s
        .hear_audio(
            &engine,
            &tacie_engine::split::wav_bytes(&samples),
            Some("상대"),
            Some(900),
        )
        .await;
    assert_eq!(rec["voice"], "화자 2");
    assert!(rec.get("then").is_none());
}

#[tokio::test]
async fn single_voice_long_utterance_is_not_split() {
    let gater = gater(0.2);
    let engine = voice_engine(
        &gater,
        FakeTranscriber::script(vec![Ok(eight_words())]),
        Some(level_voices()),
    );
    let s = engine.new_session();
    let rec = s
        .hear_audio(
            &engine,
            &two_level_wav(1000, 1000),
            Some("상대"),
            Some(3000),
        )
        .await;
    assert!(rec.get("then").is_none());
    assert_eq!(rec["text"], "하나 둘 셋 넷 다섯 여섯 일곱 여덟");
}

#[tokio::test]
async fn no_word_timings_means_no_split() {
    let gater = gater(0.2);
    let stt = FakeTranscriber::script(vec![Ok(transcript(
        "하나 둘 셋 넷 다섯 여섯 일곱 여덟",
        None,
    ))]);
    let engine = voice_engine(&gater, stt, Some(level_voices()));
    let s = engine.new_session();
    let rec = s
        .hear_audio(
            &engine,
            &two_level_wav(1000, 3000),
            Some("상대"),
            Some(3000),
        )
        .await;
    assert!(rec.get("then").is_none());
}

#[tokio::test]
async fn hear_audio_without_voice_model_is_unchanged() {
    let gater = gater(0.2);
    let engine = voice_engine(&gater, FakeTranscriber::ok("배포는 언제 해요"), None);
    let s = engine.new_session();
    let rec = s.hear_audio(&engine, b"A1", None, None).await;
    assert_eq!(rec["voice"], Value::Null);
    assert_eq!(gater.calls()[0].1, "나: 배포는 언제 해요");
}

#[tokio::test]
async fn in_person_lines_use_voice_names() {
    let gater = gater(0.2);
    let stt = FakeTranscriber::script(vec![
        Ok(transcript("배포는 언제 해요", None)),
        Ok(transcript("다음 주 화요일이에요", None)),
    ]);
    let engine = voice_engine(&gater, stt, Some(two_voices()));
    let s = engine.new_session();
    let first = s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    let second = s.hear_audio(&engine, b"B1", None, Some(3000)).await;
    assert_eq!(
        (&first["voice"], &second["voice"]),
        (&json!("화자 1"), &json!("화자 2"))
    );
    assert_eq!(
        gater.calls()[1],
        (
            vec!["화자 1: 배포는 언제 해요".to_string()],
            "화자 2: 다음 주 화요일이에요".to_string()
        )
    );
}

#[tokio::test]
async fn remote_me_is_clustered_as_a_speaker() {
    let gater = gater(0.2);
    let engine = voice_engine(
        &gater,
        FakeTranscriber::ok("배포는 언제 해요"),
        Some(two_voices()),
    );
    let s = engine.new_session();
    let me = s.hear_audio(&engine, b"A1", Some("나"), Some(3000)).await;
    assert_eq!(me["voice"], json!("화자 1"));
    assert_eq!(gater.calls()[0].1, "화자 1: 배포는 언제 해요");
}

#[tokio::test]
async fn remote_me_and_them_never_share_a_speaker() {
    let gater = gater(0.2);
    let engine = voice_engine(
        &gater,
        FakeTranscriber::ok("배포는 언제 해요"),
        Some(two_voices()),
    );
    let s = engine.new_session();
    let me = s.hear_audio(&engine, b"A1", Some("나"), Some(3000)).await;
    let them = s.hear_audio(&engine, b"A1", Some("상대"), Some(3000)).await;
    assert_eq!(
        (&me["voice"], &them["voice"]),
        (&json!("화자 1"), &json!("화자 2"))
    );
}

#[tokio::test]
async fn different_voices_are_not_joined() {
    let stt = || {
        FakeTranscriber::script(vec![
            Ok(transcript("그 배포 스크립트가", None)),
            Ok(transcript("어디 있었죠", None)),
        ])
    };
    // 쉼이 짧고(300 ms) 앞 조각이 판단을 넘지 않았어요: 같은 목소리면 이어요.
    let engine = voice_engine(&gater(0.2), stt(), Some(two_voices()));
    let s = engine.new_session();
    let first = s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    let joined = s.hear_audio(&engine, b"A2", None, Some(300)).await;
    assert_eq!(joined["merged_from"], first["utterance_id"]);
    // 다른 목소리면 잇지 않아요.
    let engine = voice_engine(&gater(0.2), stt(), Some(two_voices()));
    let s = engine.new_session();
    s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    let apart = s.hear_audio(&engine, b"B1", None, Some(300)).await;
    assert_eq!(apart["merged_from"], Value::Null);
    assert_eq!(apart["text"], "어디 있었죠");
}

#[tokio::test]
async fn voice_failure_does_not_fail_the_utterance() {
    let gater = gater(0.9);
    let engine = voice_engine(
        &gater,
        FakeTranscriber::ok("배포는 언제 해요"),
        Some(FakeVoice::failing()),
    );
    let s = engine.new_session();
    let rec = s.hear_audio(&engine, b"A1", None, None).await;
    assert_eq!((&rec["fired"], &rec["voice"]), (&json!(true), &Value::Null));
    assert_eq!(gater.calls()[0].1, "나: 배포는 언제 해요");
}

#[tokio::test]
async fn empty_transcript_does_not_create_a_voice() {
    let gater = gater(0.2);
    let stt = FakeTranscriber::script(vec![
        Ok(transcript("", None)),
        Ok(transcript("배포는 언제 해요", None)),
    ]);
    let engine = voice_engine(&gater, stt, Some(two_voices()));
    let s = engine.new_session();
    let empty = s.hear_audio(&engine, b"A1", None, None).await;
    assert_eq!(empty["utterance_id"], Value::Null);
    let rec = s.hear_audio(&engine, b"B1", None, None).await;
    assert_eq!(rec["voice"], json!("화자 1"));
}

/// 원격 회의에서도 앱이 어느 쪽이 말했는지 못 가르면(겹쳐 말하기, 스피커) `X-Speaker` 없이 와요. 이미 나·상대를
/// 받은 세션은 원격이라 그런 발화를 대면 회의로 묶지 않아요(예전처럼 "나").
#[tokio::test]
async fn remote_untagged_utterances_are_not_clustered() {
    let gater = gater(0.2);
    let engine = voice_engine(
        &gater,
        FakeTranscriber::ok("배포는 언제 해요"),
        Some(two_voices()),
    );
    let s = engine.new_session();
    s.hear_audio(&engine, b"B1", Some("상대"), Some(3000)).await;
    let untagged = s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    assert_eq!(untagged["voice"], Value::Null);
    assert_eq!(gater.calls()[1].1, "나: 배포는 언제 해요");
}

#[tokio::test]
async fn backchannels_are_not_clustered() {
    // 맞장구는 짧고 소리가 섞여서 묶지 않아요(실측은 0.5초 미만 발화를 뺐어요).
    let stt = FakeTranscriber::script(vec![
        Ok(transcript("네네.", None)),
        Ok(transcript("배포는 언제 해요", None)),
    ]);
    let engine = voice_engine(&gater(0.2), stt, Some(two_voices()));
    let s = engine.new_session();
    let bc = s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    let next = s.hear_audio(&engine, b"B1", None, Some(3000)).await;
    assert_eq!(
        (&bc["skipped"], &bc["voice"]),
        (&json!("backchannel"), &Value::Null)
    );
    assert_eq!(next["voice"], json!("화자 1"));
}

#[tokio::test]
async fn a_missing_voice_keeps_the_short_pause_join() {
    // 뒤 조각의 임베딩이 없어도(실패, 너무 짧음) 같은 자리의 짧은 쉼이면 예전처럼 이어요. 이은 줄은 앞 조각의 이름이에요.
    let gater = gater(0.2);
    let stt = FakeTranscriber::script(vec![
        Ok(transcript("그 배포 스크립트가", None)),
        Ok(transcript("어디 있었죠", None)),
    ]);
    let engine = voice_engine(&gater, stt, Some(two_voices()));
    let s = engine.new_session();
    let first = s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    let joined = s.hear_audio(&engine, b"no print", None, Some(300)).await;
    assert_eq!(joined["merged_from"], first["utterance_id"]);
    assert_eq!(joined["voice"], json!("화자 1"));
    assert_eq!(gater.calls()[1].1, "화자 1: 그 배포 스크립트가 어디 있었죠");
}

#[tokio::test]
async fn ledger_lines_use_voice_names() {
    let notes = note_writer("{}");
    let stt = Arc::new(FakeTranscriber::script(vec![
        Ok(transcript("배포는 목요일로 해요", None)),
        Ok(transcript("그럼 공지는 제가 쓸게요", None)),
    ]));
    let engine =
        meeting_engine(index(), &gater(0.2), &stt, &notes).with_voice(Some(Arc::new(two_voices())));
    let s = engine.new_session();
    s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    s.hear_audio(&engine, b"B1", None, Some(3000)).await;
    s.ledger(&engine, None).await;
    let sent = notes.calls().join("\n");
    assert!(
        sent.contains("[s1u1] 화자 1: 배포는 목요일로 해요"),
        "{sent}"
    );
    assert!(
        sent.contains("[s1u2] 화자 2: 그럼 공지는 제가 쓸게요"),
        "{sent}"
    );
}

/// 원격 회의에서 표시 없이 온 발화와 앱이 이름을 준 화자는 묶지 않으니 임베딩도 뽑지 않아요(돈은 안 들지만 CPU예요).
#[tokio::test]
async fn unclustered_utterances_make_no_embed_calls() {
    let voice = Arc::new(two_voices());
    let gater = gater(0.2);
    let stt = Arc::new(FakeTranscriber::ok("배포는 언제 해요"));
    let engine =
        meeting_engine(index(), &gater, &stt, &note_writer("{}")).with_voice(Some(voice.clone()));
    let s = engine.new_session();
    s.hear_audio(&engine, b"B1", Some("상대"), Some(3000)).await;
    assert_eq!(voice.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    s.hear_audio(&engine, b"A1", Some("민준"), Some(3000)).await;
    assert_eq!(voice.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

// 기억한 사람과 다시 연 회의.

fn unit(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter().map(|x| x / n).collect()
}

#[tokio::test]
async fn heard_carries_person() {
    let gater = gater(0.2);
    let engine = voice_engine(
        &gater,
        FakeTranscriber::ok("배포는 언제 해요"),
        Some(two_voices()),
    );
    let people = vec![Person {
        id: "p1".into(),
        vector: unit(&VOICE_A),
    }];
    let s = engine.new_session_with(false, false, Voices::with(people, vec![]));
    let rec = s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    assert_eq!(
        (&rec["voice"], &rec["person"]),
        (&json!("화자 1"), &json!("p1"))
    );
    let k = keys(&rec);
    let at = k.iter().position(|k| *k == "voice").unwrap();
    assert_eq!(k[at + 1], "person");
    let other = s.hear_audio(&engine, b"B1", None, Some(3000)).await;
    assert_eq!(
        (&other["voice"], &other["person"]),
        (&json!("화자 2"), &Value::Null)
    );
    // 화자 묶기가 없는 발화도 키는 있어요.
    let bc = engine
        .new_session()
        .hear(&engine, "네", None, 0.7, None, None, None, 0.8)
        .await;
    assert_eq!(bc["person"], Value::Null);
}

#[tokio::test]
async fn voices_route_excludes_lines() {
    let a = [1.0, 0.0, 0.0];
    let b = [0.8, 0.6, 0.0];
    let voice = FakeVoice::by_wav(&[(b"a1", &a, 3.0), (b"a2", &a, 3.0), (b"b1", &b, 2.5)]);
    let engine = voice_engine(
        &gater(0.2),
        FakeTranscriber::ok("배포는 언제 해요"),
        Some(voice),
    );
    let s = engine.new_session();
    let mut uids = Vec::new();
    for wav in [b"a1", b"a2", b"b1"] {
        let rec = s.hear_audio(&engine, wav, None, Some(3000)).await;
        assert_eq!(rec["voice"], json!("화자 1"));
        uids.push(rec["utterance_id"].as_str().unwrap().to_string());
    }
    let all = s.voices(&[]).await;
    assert_eq!(keys(&all), ["voices"]);
    let one = &all["voices"][0];
    assert_eq!(
        keys(one),
        ["voice", "channel", "vector", "seconds", "person", "near"]
    );
    assert_eq!(
        (
            &one["voice"],
            &one["channel"],
            &one["seconds"],
            &one["person"],
            &one["near"]
        ),
        (
            &json!("화자 1"),
            &json!("room"),
            &json!(8.5),
            &Value::Null,
            &Value::Null
        )
    );
    let want = unit(&[2.8, 0.6, 0.0]);
    let got: Vec<f32> = serde_json::from_value(one["vector"].clone()).unwrap();
    assert!(
        got.iter().zip(&want).all(|(x, y)| (x - y).abs() < 1e-5),
        "{got:?}"
    );

    let some = s.voices(&[uids[2].clone()]).await;
    let one = &some["voices"][0];
    assert_eq!(one["vector"], json!([1.0, 0.0, 0.0]));
    assert_eq!(one["seconds"], json!(6.0));
    // 말이 모두 빠진 목소리는 없어요.
    assert_eq!(s.voices(&uids).await, json!({"voices": []}));
}

#[tokio::test]
async fn restored_session_does_not_reuse_numbers() {
    let restored = |n: usize, v: &[f32]| Restored {
        number: n,
        channel: Channel::Room,
        vector: unit(v),
        diar: None,
        seconds: 10.0,
        person: None,
    };
    let engine = voice_engine(
        &gater(0.2),
        FakeTranscriber::ok("배포는 언제 해요"),
        Some(FakeVoice::by_wav(&[(b"C1", &[0.0, 0.0, 1.0], 3.0)])),
    );
    let s = engine.new_session_with(
        false,
        false,
        Voices::with(vec![], vec![restored(1, &VOICE_A), restored(2, &VOICE_B)]),
    );
    let rec = s.hear_audio(&engine, b"C1", None, Some(3000)).await;
    assert_eq!(rec["voice"], json!("화자 3"));
}

// 화자 이름.

#[tokio::test]
async fn names_change_gate_context() {
    let gater = gater(0.2);
    let stt = FakeTranscriber::script(vec![
        Ok(transcript("배포는 언제 해요", None)),
        Ok(transcript("다음 주 화요일이에요", None)),
        Ok(transcript("공지는 제가 쓸게요", None)),
    ]);
    let engine = voice_engine(&gater, stt, Some(two_voices()));
    let s = engine.new_session();
    s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    s.set_names(HashMap::from([("화자 1".to_string(), "민재".to_string())]))
        .await;
    s.hear_audio(&engine, b"A2", None, Some(3000)).await;
    assert_eq!(
        gater.calls()[1],
        (
            vec!["화자 1: 배포는 언제 해요".to_string()],
            "민재: 다음 주 화요일이에요".to_string()
        )
    );
    // 이름 표는 통째로 바뀌어요: 빈 표면 다시 번호예요.
    s.set_names(HashMap::new()).await;
    s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    assert!(
        gater.calls()[2].1.starts_with("화자 1: "),
        "{:?}",
        gater.calls()[2]
    );
}

#[tokio::test]
async fn session_ledger_owner_is_speaker_key() {
    let raw = json!({"items": [
        {"kind": "commitment", "text": "공지를 쓰기로 했어요.", "utterance_ids": ["s1u2"], "owner": "민재", "due": null},
        {"kind": "commitment", "text": "배포를 맡기로 했어요.", "utterance_ids": ["s1u1"], "owner": "화자 2", "due": null},
        {"kind": "commitment", "text": "문서를 보기로 했어요.", "utterance_ids": ["s1u1"], "owner": "팀장", "due": null}]})
    .to_string();
    let notes = note_writer(&raw);
    let stt = Arc::new(FakeTranscriber::script(vec![
        Ok(transcript("배포는 목요일로 해요", None)),
        Ok(transcript("그럼 공지는 제가 쓸게요", None)),
    ]));
    let engine =
        meeting_engine(index(), &gater(0.2), &stt, &notes).with_voice(Some(Arc::new(two_voices())));
    let s = engine.new_session();
    s.hear_audio(&engine, b"B1", None, Some(3000)).await;
    s.hear_audio(&engine, b"A1", None, Some(3000)).await;
    s.set_names(HashMap::from([("화자 2".to_string(), "민재".to_string())]))
        .await;
    let out = s.ledger(&engine, None).await;
    assert_eq!(
        notes.calls(),
        ["[s1u1] 화자 1: 배포는 목요일로 해요\n[s1u2] 민재: 그럼 공지는 제가 쓸게요"]
    );
    assert_eq!(notes.attendee_calls(), [vec!["화자 1", "민재"]]);
    let owners: Vec<&Value> = out["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|it| &it["owner"])
        .collect();
    assert_eq!(owners, [&json!("화자 2"), &json!("화자 2"), &json!("팀장")]);
}

/// 목소리가 없는 줄(앱이 준 나·상대)은 그 이름이 참석자이자 키예요. 이름 없는 "화자"는 참석자가 아니에요.
#[tokio::test]
async fn session_ledger_attendees_without_voices() {
    let notes = note_writer(r#"{"items": []}"#);
    let engine = ledger_engine(&notes);
    let s = engine.new_session();
    hear(
        &s,
        &engine,
        "배포는 목요일로 해요",
        Some("상대"),
        Some(3000),
        0.0,
    )
    .await;
    hear(&s, &engine, "제가 공지 쓸게요", Some("나"), Some(3000), 1.0).await;
    hear(&s, &engine, "그건 다음에 봐요", None, Some(3000), 2.0).await;
    hear(
        &s,
        &engine,
        "네 그러죠 좋아요 그럼",
        Some("상대"),
        Some(3000),
        3.0,
    )
    .await;
    s.ledger(&engine, None).await;
    assert_eq!(notes.attendee_calls(), [vec!["상대", "나"]]);
}

/// 받아쓰기가 한글로 적은 팀 용어는 판단 전에 원래 용어로 바뀌어요(docs/04 5절 "받아쓴 글의 팀 용어 고치기").
/// 용어는 색인 문서에서 뽑고, 표기는 이 Mac의 표기 파일에서 읽어요.
#[tokio::test]
async fn heard_text_has_team_terms_fixed_before_the_gate() {
    let doc = |i: usize| {
        json!({"src": "docs", "path": format!("docs/{i}.md"), "line": 1,
               "text": "OpenRouter로 받아쓰기를 보내요."})
    };
    let chunks = json!([doc(0), doc(1), doc(2), doc(3), doc(4)]);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("term_readings.json");
    std::fs::write(&file, r#"{"OpenRouter": ["오픈라우터", "오픈 라우터"]}"#).unwrap();
    let gate = gater(0.1);
    let services = Services {
        gater: gate.clone(),
        transcriber: Arc::new(FakeTranscriber::ok("오픈 라우터에서 보낼 때 맞죠?")),
        ..services(
            Arc::new(FakeReranker::all(0.9)),
            Arc::new(FakeAnswerer::ok("")),
        )
    };
    let (engine, _) = engine_with(index_of(&chunks, &json!({"repo": "o/r"})), services);
    let engine = engine.with_terms_file(Some(file));
    let s = engine.new_session();
    let heard = s.hear_audio(&engine, b"RIFF", None, None).await;
    assert_eq!(heard["text"], "OpenRouter에서 보낼 때 맞죠?");
}

/// 로컬 모델 가짜예요: `fail`이면 늘 실패해요.
struct StubLocal {
    fail: bool,
}

impl tacie_engine::routed_stt::LocalModel for StubLocal {
    fn transcribe(
        &self,
        _wav: &[u8],
        _prompt: &str,
    ) -> anyhow::Result<tacie_engine::services::Transcript> {
        if self.fail {
            anyhow::bail!("가짜 실패");
        }
        let mut t = transcript("링크는 7일", None);
        t.via = tacie_engine::services::SttVia::Local;
        Ok(t)
    }
}

/// `RoutedStt`를 받아쓰기로 끼운 엔진이에요. 켜서 모델이 올라올 때까지 기다린 뒤 돌려줘요.
async fn routed_engine(fail: bool, on: bool) -> Engine {
    use tacie_engine::routed_stt::{LoadModel, LocalModel, RoutedStt};
    let load: LoadModel =
        Arc::new(move |_, _| Ok(Arc::new(StubLocal { fail }) as Arc<dyn LocalModel>));
    let server = Arc::new(FakeTranscriber::ok("서버 글"));
    let routed = RoutedStt::new(server, load, Arc::new(|| false), Arc::new(String::new));
    routed.set(on, Some(std::path::PathBuf::from("/model.bin")));
    for _ in 0..200 {
        if !routed.status().loading {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let services = Services {
        gater: gater(0.2),
        transcriber: routed.clone(),
        note_writer: note_writer("{}"),
        ..services(
            Arc::new(FakeReranker::all(0.9)),
            Arc::new(FakeAnswerer::ok("")),
        )
    };
    engine_with(index(), services).0.with_stt(Some(routed))
}

#[tokio::test]
async fn audio_reply_says_where_stt_ran() {
    let server = routed_engine(false, false).await;
    let out = server
        .new_session()
        .hear_audio(&server, b"RIFF", None, None)
        .await;
    assert_eq!(out["stt"], "server");
    assert!(out.get("stt_broken").is_none());
    let local = routed_engine(false, true).await;
    let out = local
        .new_session()
        .hear_audio(&local, b"RIFF", None, None)
        .await;
    assert_eq!(out["stt"], "local");
    // 글이 비어 돌아온 응답에도 붙어요.
    let empty = Arc::new(FakeTranscriber::ok(""));
    let engine = meeting_engine(index(), &gater(0.2), &empty, &note_writer("{}"));
    let out = engine
        .new_session()
        .hear_audio(&engine, b"RIFF", None, None)
        .await;
    assert_eq!(out["stt"], "server");
}

#[tokio::test]
async fn broken_local_reply_has_flag() {
    let engine = routed_engine(true, true).await;
    let s = engine.new_session();
    let mut last = Value::Null;
    for _ in 0..8 {
        last = s.hear_audio(&engine, b"RIFF", None, None).await;
        assert_eq!(last["error_kind"], "model");
        assert_eq!(last["stt"], "local");
        assert_eq!(last["fired"], false);
        assert_eq!(last["text"], "");
        assert!(last["utterance_id"].is_null());
    }
    assert_eq!(last["stt_broken"], true);
}

/// 저장한 조각들을 진짜 화자 모델로 세션에 다시 넣어 단어마다 목소리를 적어요(평가용, 손으로만 돌려요).
/// `TACIE_REPLAY`: `[{"wav": 경로, "gap_ms": 숫자|null, "words": [[시작초, 끝초, 글], ...]}]`, 결과는 `TACIE_REPLAY_OUT`에
/// 조각마다 `[[글, 목소리], ...]`예요. 받아쓰기는 조각의 단어 그대로라 돈이 들지 않아요.
#[tokio::test]
#[ignore]
async fn replay_saved_chunks() {
    use tacie_engine::services::SpokenWord;
    let path = std::env::var("TACIE_REPLAY").expect("TACIE_REPLAY");
    let chunks: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let model = std::env::var("TACIE_VOICE_MODEL").expect("TACIE_VOICE_MODEL");
    let mut voice = tacie_embed::voice::VoiceEmbedder::load(std::path::Path::new(&model)).unwrap();
    // `TACIE_DIAR_MODEL`: 화자 나누기 모델(CAM++ 파인튜닝)로 묶어요. 없으면 ERes2Net 하나로 묶어요.
    if let Ok(diar) = std::env::var("TACIE_DIAR_MODEL") {
        voice.add_diar(std::path::Path::new(&diar)).unwrap();
    }
    let script = chunks
        .iter()
        .map(|c| {
            let words: Vec<SpokenWord> = c["words"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| SpokenWord {
                    text: w[2].as_str().unwrap().into(),
                    start: w[0].as_f64().unwrap(),
                    end: w[1].as_f64().unwrap(),
                })
                .collect();
            let text = words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let mut t = transcript(&text, None);
            t.words = words;
            Ok(t)
        })
        .collect();
    let gater = gater(0.0);
    let stt = Arc::new(FakeTranscriber::script(script));
    let engine =
        meeting_engine(index(), &gater, &stt, &note_writer("{}")).with_voice(Some(Arc::new(voice)));
    let s = engine.new_session();
    let mut out = Vec::new();
    for c in &chunks {
        let wav = std::fs::read(c["wav"].as_str().unwrap()).unwrap();
        let rec = s
            .hear_audio(&engine, &wav, Some("상대"), c["gap_ms"].as_u64())
            .await;
        let mut lines = vec![rec.clone()];
        lines.extend(rec["then"].as_array().cloned().unwrap_or_default());
        let mut words = Vec::new();
        for l in &lines {
            for w in l["words"].as_array().cloned().unwrap_or_default() {
                words.push(json!([w["text"], l["voice"]]));
            }
        }
        out.push(Value::Array(words));
    }
    let out_path = std::env::var("TACIE_REPLAY_OUT").unwrap();
    std::fs::write(&out_path, Value::Array(out).to_string()).unwrap();
    // 회의 끝의 목소리들(대표 벡터, 초)이에요. 작은 화자 합치기를 엔진 밖에서 재 볼 때 써요.
    std::fs::write(
        format!("{out_path}.voices.json"),
        s.voices(&[]).await.to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn ledger_cache_rewrites_on_other_kind() {
    use tacie_engine::notes_kind::NotesKind;
    let notes = note_writer(r#"{"items": []}"#);
    let engine = ledger_engine(&notes);
    let s = engine.new_session();
    hear(&s, &engine, "배포는 목요일로 해요", None, Some(3000), 0.0).await;
    let auto = s.ledger(&engine, None).await;
    assert_eq!(auto["kind"], "meeting");
    assert_eq!(notes.calls().len(), 1);
    let interview = s.ledger(&engine, Some(NotesKind::Interview)).await;
    assert_eq!(interview["kind"], "interview");
    assert_eq!(notes.calls().len(), 2);
    s.ledger(&engine, Some(NotesKind::Interview)).await;
    assert_eq!(notes.calls().len(), 2); // 같은 유형, 새 발화 없음: 지난 결과예요
    let again = s.ledger(&engine, None).await;
    assert_eq!(again["kind"], "interview"); // 고르지 않으면 지난 결과 그대로예요
    assert_eq!(notes.calls().len(), 2);
}

#[tokio::test]
async fn ledger_cache_says_user_when_the_pick_matches_the_auto_kind() {
    use tacie_engine::notes_kind::NotesKind;
    let notes = note_writer(r#"{"items": []}"#);
    let engine = ledger_engine(&notes);
    let s = engine.new_session();
    hear(&s, &engine, "배포는 목요일로 해요", None, Some(3000), 0.0).await;
    assert_eq!(s.ledger(&engine, None).await["kind_source"], "auto");
    let picked = s.ledger(&engine, Some(NotesKind::Meeting)).await;
    assert_eq!(notes.calls().len(), 1); // same kind, nothing new heard: no second call
    assert_eq!(picked["kind_source"], "user");
}

/// iPhone 엔진(`gate_only`, docs/27 8-7): 검색 모델과 색인 없이도 판단은 불러요. 질문이면 카드를 찾지 않고 질문과
/// 앞 대화를 돌려줘요(앱이 서버에 지난 회의를 물어요). Mac 엔진은 이 표시가 꺼져 있어요.
#[tokio::test]
async fn gate_only_engine_gates_without_model_and_hands_back_the_question() {
    let gater = gater(0.9);
    let reranker = Arc::new(FakeReranker::all(0.9));
    let (engine, _) = engine_of(
        index_of(&json!([]), &json!({})),
        &gater,
        &reranker,
        &Arc::new(FakeAnswerer::ok("")),
    );
    let engine = engine.with_model_ready(false).with_gate_only(true);
    let s = engine.new_session();
    utterance(
        &s,
        &engine,
        "오늘은 결제 얘기부터 해요.",
        Some("화자 1"),
        0.0,
    )
    .await;
    let rec = utterance(
        &s,
        &engine,
        "예전에 웹훅 중복 어떻게 하기로 했죠?",
        Some("화자 2"),
        1.0,
    )
    .await;
    assert_eq!((&rec["fired"], &rec["cards"]), (&json!(true), &json!([])));
    assert!(rec.get("error").is_none() && rec.get("error_kind").is_none());
    assert_eq!(
        rec["context"],
        json!(["화자 1: 오늘은 결제 얘기부터 해요."])
    );
    assert_eq!(gater.calls().len(), 2);
    assert!(reranker.calls().is_empty());
}

/// 표시가 꺼진 엔진(Mac)은 모델이 없으면 지금처럼 판단을 부르지 않아요.
#[tokio::test]
async fn without_gate_only_a_missing_model_still_skips_the_gate() {
    let gater = gater(0.9);
    let (engine, _) = engine_of(
        index(),
        &gater,
        &Arc::new(FakeReranker::all(0.9)),
        &Arc::new(FakeAnswerer::ok("")),
    );
    let engine = engine.with_model_ready(false);
    let s = engine.new_session();
    let rec = utterance(
        &s,
        &engine,
        "예전에 웹훅 중복 어떻게 하기로 했죠?",
        None,
        0.0,
    )
    .await;
    assert_eq!(rec["error_kind"], "model");
    assert!(gater.calls().is_empty());
    assert!(rec.get("context").is_none());
}
