//! Hooks the Jev client up to a fake Jev server (`common/fake_jev.rs`). The real API is not called.
//!
//! Request bodies are not changed, character for character and down to key order (the measured numbers and result
//! caches are tied to this text).

mod common {
    pub mod fake_jev;
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::fake_jev::FakeJev;
use serde_json::{Value, json};
use tacie_engine::errors::Kind;
use tacie_engine::jev::{Jev, JevKw, USD_PER_INPUT_TOKEN};
use tacie_engine::settings;
use tempfile::TempDir;

const KEY: &str = "sk-test-not-a-real-key";

type Last = Arc<Mutex<HashMap<String, Kind>>>;

/// 설정 폴더에 `provider`의 키 파일을 두고(None이면 비워 두고) Jev를 만들어요. 재시도 쉼은 0이에요.
fn client(provider: Option<&str>, url: &str, extra: &[(&str, &str)]) -> (Jev, Last, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    if let Some(p) = provider {
        std::fs::create_dir_all(dir.path().join(p)).unwrap();
        std::fs::write(
            dir.path().join(p).join("config.json"),
            json!({"apiKey": KEY}).to_string(),
        )
        .unwrap();
    }
    let mut env: HashMap<String, String> =
        HashMap::from([("TACIE_JEV_URL".to_string(), url.to_string())]);
    env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let last: Last = Arc::new(Mutex::new(HashMap::new()));
    let jev = Jev::from_config(dir.path(), &settings::resolve(&env), &env, last.clone())
        .with_backoff(|_| Duration::ZERO);
    (jev, last, dir)
}

fn kw(attempts: u32) -> JevKw {
    JevKw {
        timeout: Duration::from_secs(5),
        attempts,
    }
}

fn rerank_ok() -> Value {
    json!({"answers": {"p0": {"noul": 0.9}, "p1": {"noul": 0.2}}, "usage": {"input_tokens": 1000}})
}

/// 키 순서까지 보려고 직렬화한 글로 비교해요(serde_json Map의 ==는 순서를 보지 않아요).
#[cfg(feature = "local-prompts")]
fn same_text(got: &Value, want: &Value) {
    assert_eq!(
        serde_json::to_string(got).unwrap(),
        serde_json::to_string(want).unwrap()
    );
}

#[cfg(feature = "local-prompts")] // 프롬프트 글을 봐요
#[tokio::test]
async fn rerank_sends_p_keys_and_exact_prompt_texts() {
    let fake = FakeJev::start(vec![(200, rerank_ok()), (200, rerank_ok())]).await;
    let (jev, _, _dir) = client(Some("typesafe"), &fake.url, &[]);
    let context = [
        "민지: 엔진은 어떻게 떠요?".to_string(),
        "나: 앱이 띄워요".to_string(),
    ];
    let passages = ["첫 조각".to_string(), "둘째 조각".to_string()];

    let out = jev
        .rerank(&context, "나: 포트는요?", &passages, JevKw::default())
        .await
        .unwrap();
    assert_eq!(out.p, [0.9, 0.2]);
    assert!(out.ms >= 0.0);
    assert_eq!(out.usage["input_tokens"], 1000);

    let bodies = fake.bodies();
    assert_eq!(
        bodies[0]["questions"]["p0"]["instructions"],
        "Does passage p0 answer what the current message asks (using the earlier messages for context), or show where it is decided?"
    );
    same_text(
        &bodies[0],
        &json!({
            "model": "jev-latest",
            "state": {
                "recent_conversation": "민지: 엔진은 어떻게 떠요?\n나: 앱이 띄워요",
                "current_message": "나: 포트는요?",
                "passages": {"p0": "첫 조각", "p1": "둘째 조각"},
            },
            "questions": {
                "p0": {"type": "noul",
                       "instructions": "Does passage p0 answer what the current message asks (using the earlier messages for context), or show where it is decided?",
                       "criteria": {"true": "Passage p0 contains the answer or the exact place it is decided.",
                                    "false": "Passage p0 is unrelated or only mentions the topic."}},
                "p1": {"type": "noul",
                       "instructions": "Does passage p1 answer what the current message asks (using the earlier messages for context), or show where it is decided?",
                       "criteria": {"true": "Passage p1 contains the answer or the exact place it is decided.",
                                    "false": "Passage p1 is unrelated or only mentions the topic."}},
            },
        }),
    );
    let h = &fake.headers()[0];
    assert_eq!(h["authorization"], format!("Bearer {KEY}").as_str());
    assert!(
        h["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    assert!(h.get("typesafe-danger-zone-long-context").is_none());

    // 앞 말이 없거나 모두 빈 글이면 "(no earlier messages)"예요.
    jev.rerank(
        &[String::new()],
        "나: 포트는요?",
        &passages,
        JevKw::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        fake.bodies()[1]["state"]["recent_conversation"],
        "(no earlier messages)"
    );
}

#[cfg(feature = "local-prompts")] // 프롬프트 글을 봐요
#[tokio::test]
async fn gate_returns_p_lookup_source_probabilities_and_p_end() {
    let ok = json!({
        "answers": {"lookup": {"noul": 0.83},
                    "source": {"probabilities": {"code": 0.6, "docs": 0.3, "none": 0.1}},
                    "end": {"noul": 0.12}},
        "usage": {"input_tokens": 50, "cost": 0.001},
    });
    let fake = FakeJev::start(vec![(200, ok.clone()), (200, ok)]).await;
    let (jev, _, _dir) = client(Some("openrouter"), &fake.url, &[]);

    let out = jev
        .gate(&[], "민지: 재시도는 몇 번이에요?", JevKw::default())
        .await
        .unwrap();
    assert_eq!(out.p_lookup, 0.83);
    assert_eq!(out.p_end, Some(0.12));
    same_text(
        &Value::Object(out.source),
        &json!({"code": 0.6, "docs": 0.3, "none": 0.1}),
    );
    // 응답에 cost가 있으면 그대로 둬요.
    assert_eq!(out.usage, json!({"input_tokens": 50, "cost": 0.001}));
    assert!(out.ms >= 0.0);

    same_text(
        &fake.bodies()[0],
        &json!({
            "model": "typesafe/jev-1.13",
            "state": {"recent_conversation": "(start of meeting)",
                      "current_utterance": "민지: 재시도는 몇 번이에요?"},
            "questions": {
                "lookup": {"type": "noul",
                           "instructions": "Does the current utterance need a lookup in our company's code or documents? The lines are a live speech-recognition transcript, so punctuation is unreliable: a question often ends with a period or with no mark at all. Judge from the wording (question endings such as -요?/-나요/-까/-죠/-어요, question words such as 어디/뭐/얼마/왜/언제/어떻게), not from punctuation.",
                           "criteria": {
                               "true": "The current utterance asks for a fact that is written down in this company's own source code or internal documents: how our product behaves, where something is implemented, a rule, limit, default or setting, a design decision or its reason, a team process or convention. Showing the matching passage would help now. This holds even when the speaker asks a colleague in the room, as long as the answer is such a fact. So is asking how something works or what to do (~하면 되나요?, ~하면 안 돼요?, ~해야 돼요?), even when it sounds like asking for permission. So is asking what the team already decided and wrote down: which company, vendor or tool was chosen, when something was set to happen, who was assigned to it, a number that was set, or why.",
                               "false": "No lookup in company material is needed: small talk, opinions, feelings, planning a schedule or handing out work right now, status of someone's own work, rhetorical questions, questions about the outside world or news of other companies, questions whose answer is the listeners' consent, opinion or a decision being made right now, not a fact that could be written down (이렇게 해도 되는 거죠?, 이거 없애도 되죠?, ~하는 게 낫지 않을까요?), worries or thinking aloud (~할까 봐, ~해야 되는 건가 싶고), half-finished fragments, statements that ask nothing, or answers to someone else's question.",
                           }},
                "source": {"type": "choice",
                           "instructions": "Where would the answer to the current utterance be found?",
                           "criteria": {
                               "code": "The answer lives in source code: behavior, conditions, limits, values, where it is implemented.",
                               "docs": "The answer lives in written documents: decisions and their reasons, plans, specs, processes, conventions.",
                               "none": "No lookup is needed.",
                           }},
                // 같은 요청에서 회의를 끝내는 중인지도 물어요(`END_TRUE`, `END_FALSE`).
                "end": {"type": "noul",
                        "instructions": "Is the speaker ending this meeting now?",
                        "criteria": {
                            "true": "The speaker is closing this meeting right now: saying it is over, wrapping up, thanking everyone for their time, or saying goodbye to the group (오늘은 여기까지, 회의 마치겠습니다, 수고하셨습니다, 회의 종료).",
                            "false": "The meeting goes on: ending a topic or a task, something ending later or in the past, planning the next meeting, a question, or any other talk.",
                        }},
            },
        }),
    );

    // 앞 말이 있으면 줄바꿈으로 이어요.
    jev.gate(&["a".to_string(), "b".to_string()], "c", JevKw::default())
        .await
        .unwrap();
    assert_eq!(fake.bodies()[1]["state"]["recent_conversation"], "a\nb");
}

#[tokio::test]
async fn retries_on_429_then_succeeds_and_fills_cost() {
    let fake = FakeJev::start(vec![
        (429, json!("slow down")),
        (503, json!("busy")),
        (200, rerank_ok()),
    ])
    .await;
    let (jev, last, _dir) = client(Some("typesafe"), &fake.url, &[]);
    let waits = Arc::new(Mutex::new(Vec::new()));
    let seen = waits.clone();
    let jev = jev.with_backoff(move |attempt| {
        seen.lock().unwrap().push(attempt);
        Duration::ZERO
    });
    // 예전 거절은 200을 받으면 지워져요.
    last.lock().unwrap().insert("jev".into(), Kind::Key);

    let passages = ["a".to_string(), "b".to_string()];
    let out = jev.rerank(&[], "q", &passages, kw(3)).await.unwrap();
    assert_eq!(out.p, [0.9, 0.2]);
    let bodies = fake.bodies();
    assert_eq!(bodies.len(), 3);
    assert!(bodies.iter().all(|b| *b == bodies[0]));
    assert_eq!(*waits.lock().unwrap(), [0, 1]);
    // 응답에 cost가 없으면 input_tokens × 4.2e-8이에요.
    assert_eq!(USD_PER_INPUT_TOKEN, 4.2e-8);
    assert_eq!(out.usage["cost"].as_f64().unwrap(), 1000.0 * 4.2e-8);
    assert!(last.lock().unwrap().is_empty());
}

#[tokio::test]
async fn long_context_header_retry_for_typesafe_only() {
    // TypeSafe가 길다고 거절하면 헤더를 붙여 한 번 더 보내요. 이 재시도도 시도 하나를 써요.
    let passages = ["a".to_string(), "b".to_string()];
    let fake = FakeJev::start(vec![(413, json!("Too many tokens")), (200, rerank_ok())]).await;
    let (jev, _, _dir) = client(Some("typesafe"), &fake.url, &[]);
    let out = jev.rerank(&[], "q", &passages, kw(2)).await.unwrap();
    assert_eq!(out.p, [0.9, 0.2]);
    let headers = fake.headers();
    assert_eq!(headers.len(), 2);
    assert!(
        headers[0]
            .get("typesafe-danger-zone-long-context")
            .is_none()
    );
    assert_eq!(headers[1]["typesafe-danger-zone-long-context"], "true");
    assert_eq!(fake.bodies()[0], fake.bodies()[1]);

    // attempts 1이면 헤더를 붙인 요청은 나가지 않고 시도가 끝나요.
    let fake = FakeJev::start(vec![(413, json!("Too many tokens")), (200, rerank_ok())]).await;
    let (jev, _, _dir) = client(Some("typesafe"), &fake.url, &[]);
    let e = jev.rerank(&[], "q", &passages, kw(1)).await.unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (Kind::Busy, "jev: no attempts left")
    );
    assert_eq!(fake.headers().len(), 1);

    // attempts 2에서 헤더 요청이 503이면 그게 마지막 시도라 세 번째 요청은 없어요.
    let fake = FakeJev::start(vec![
        (413, json!("Too many tokens")),
        (503, json!("busy")),
        (200, rerank_ok()),
    ])
    .await;
    let (jev, _, _dir) = client(Some("typesafe"), &fake.url, &[]);
    let e = jev.rerank(&[], "q", &passages, kw(2)).await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (Kind::Down, "jev 503: busy"));
    let headers = fake.headers();
    assert_eq!(headers.len(), 2);
    assert_eq!(headers[1]["typesafe-danger-zone-long-context"], "true");

    // 헤더 재시도는 한 번뿐이에요.
    let fake = FakeJev::start(vec![
        (422, json!("context too long")),
        (422, json!("context too long")),
    ])
    .await;
    let (jev, _, _dir) = client(Some("typesafe"), &fake.url, &[]);
    let e = jev.rerank(&[], "q", &passages, kw(5)).await.unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (Kind::Down, "jev 422: context too long")
    );
    assert_eq!(fake.headers().len(), 2);

    // 길이와 상관없는 400은 바로 오류예요.
    let fake = FakeJev::start(vec![(400, json!("bad request"))]).await;
    let (jev, _, _dir) = client(Some("typesafe"), &fake.url, &[]);
    let e = jev.rerank(&[], "q", &passages, kw(5)).await.unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (Kind::Down, "jev 400: bad request")
    );
    assert_eq!(fake.headers().len(), 1);

    // OpenRouter에는 이 헤더가 없어요.
    let fake = FakeJev::start(vec![(413, json!("Too many tokens")), (200, rerank_ok())]).await;
    let (jev, _, _dir) = client(Some("openrouter"), &fake.url, &[]);
    let e = jev.rerank(&[], "q", &passages, kw(5)).await.unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (Kind::Down, "jev 413: Too many tokens")
    );
    let headers = fake.headers();
    assert_eq!(headers.len(), 1);
    assert!(
        headers[0]
            .get("typesafe-danger-zone-long-context")
            .is_none()
    );
}

#[tokio::test]
async fn status_maps_to_kind_and_records_last() {
    let passages = ["a".to_string()];
    // 본문은 앞 300글자(바이트가 아니라 글자)만 실어요.
    let long = "가".repeat(400);
    let fake = FakeJev::start(vec![(401, json!(long))]).await;
    let (jev, last, _dir) = client(Some("typesafe"), &fake.url, &[]);
    let e = jev.rerank(&[], "q", &passages, kw(5)).await.unwrap_err();
    assert_eq!(e.kind, Kind::Key);
    assert_eq!(e.message, format!("jev 401: {}", "가".repeat(300)));
    assert_eq!(last.lock().unwrap().get("jev"), Some(&Kind::Key));
    assert_eq!(fake.bodies().len(), 1);

    let fake = FakeJev::start(vec![(402, json!("no credit"))]).await;
    let (jev, last, _dir) = client(Some("openrouter"), &fake.url, &[]);
    let e = jev.rerank(&[], "q", &passages, kw(5)).await.unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (Kind::Credit, "jev 402: no credit")
    );
    assert_eq!(last.lock().unwrap().get("jev"), Some(&Kind::Credit));

    // 마지막 시도의 429는 busy, 5xx는 down이에요. 이것들은 기억하지 않아요.
    let fake = FakeJev::start(vec![(500, json!("x")), (429, json!("slow"))]).await;
    let (jev, last, _dir) = client(Some("typesafe"), &fake.url, &[]);
    let e = jev.rerank(&[], "q", &passages, kw(2)).await.unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (Kind::Busy, "jev 429: slow"));
    assert!(last.lock().unwrap().is_empty());
    let fake2 = FakeJev::start(vec![(502, json!("bad gateway"))]).await;
    let (jev2, last2, _dir2) = client(Some("typesafe"), &fake2.url, &[]);
    let e = jev2.rerank(&[], "q", &passages, kw(1)).await.unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (Kind::Down, "jev 502: bad gateway")
    );
    assert!(last2.lock().unwrap().is_empty());

    // 연결이 안 되면 마지막 시도에서만 network예요. 오류 글에 키가 나오지 않아요.
    let closed = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}/v1/systemone", l.local_addr().unwrap())
    };
    let (jev, last, _dir) = client(Some("typesafe"), &closed, &[]);
    let e = jev.rerank(&[], "q", &passages, kw(2)).await.unwrap_err();
    assert_eq!(e.kind, Kind::Network);
    assert!(
        e.message.starts_with("jev request failed: "),
        "{}",
        e.message
    );
    // 원인까지 실어요("Connection refused"가 보여요).
    assert!(e.message.contains("refused"), "{}", e.message);
    assert!(!e.message.contains(KEY));
    assert!(last.lock().unwrap().is_empty());

    // 200인데 모양이 다르면 down이에요.
    let fake = FakeJev::start(vec![
        (200, json!({"answers": {}})),
        (200, json!("not json")),
    ])
    .await;
    let (jev, _, _dir) = client(Some("typesafe"), &fake.url, &[]);
    let e = jev.rerank(&[], "q", &passages, kw(1)).await.unwrap_err();
    assert_eq!(e.kind, Kind::Down);
    let e = jev.gate(&[], "q", kw(1)).await.unwrap_err();
    assert_eq!(e.kind, Kind::Down);

    // gate 답에 end가 빠져도 down이에요.
    let no_end = json!({
        "answers": {"lookup": {"noul": 0.83},
                    "source": {"probabilities": {"code": 0.6, "docs": 0.3, "none": 0.1}}},
        "usage": {"input_tokens": 50},
    });
    let fake = FakeJev::start(vec![(200, no_end)]).await;
    let (jev, _, _dir) = client(Some("typesafe"), &fake.url, &[]);
    let e = jev.gate(&[], "q", kw(1)).await.unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (Kind::Down, "jev: unexpected response (answers.end.noul)")
    );
}

#[tokio::test]
async fn no_provider_is_key_error_without_a_request() {
    let fake = FakeJev::start(vec![(200, rerank_ok())]).await;
    let (jev, last, _dir) = client(None, &fake.url, &[("TACIE_JEV_MODEL", "jev-x")]);
    assert_eq!(
        (jev.provider_name(), jev.model(), jev.model_default()),
        (None, None, None)
    );
    let e = jev
        .decide(json!({}), json!({}), JevKw::default())
        .await
        .unwrap_err();
    assert_eq!(e.kind, Kind::Key);
    assert_eq!(e.message, "no Jev key");
    assert!(fake.bodies().is_empty());
    assert!(last.lock().unwrap().is_empty());
}

#[tokio::test]
async fn env_fail_switch_is_down() {
    let fake = FakeJev::start(vec![(200, rerank_ok())]).await;
    let (jev, _, _dir) = client(Some("typesafe"), &fake.url, &[("TACIE_JEV_FAIL", "1")]);
    let e = jev
        .rerank(&[], "q", &["a".to_string()], JevKw::default())
        .await
        .unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (Kind::Down, "jev disabled by TACIE_JEV_FAIL")
    );
    assert!(fake.bodies().is_empty());

    // 키가 없으면 그쪽이 먼저예요. "1"이 아니면 꺼지지 않아요.
    let (jev, _, _dir) = client(None, &fake.url, &[("TACIE_JEV_FAIL", "1")]);
    let e = jev
        .decide(json!({}), json!({}), JevKw::default())
        .await
        .unwrap_err();
    assert_eq!(e.kind, Kind::Key);
    let (jev, _, _dir) = client(Some("typesafe"), &fake.url, &[("TACIE_JEV_FAIL", "true")]);
    let passages = ["a".to_string(), "b".to_string()];
    jev.rerank(&[], "q", &passages, JevKw::default())
        .await
        .unwrap();
    assert_eq!(fake.bodies().len(), 1);
}

#[test]
fn from_config_picks_provider_model_and_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path();
    let write = |rel: &str, v: Value| {
        std::fs::create_dir_all(cfg.join(rel).parent().unwrap()).unwrap();
        std::fs::write(cfg.join(rel), v.to_string()).unwrap();
    };
    let make = |env: &[(&str, &str)]| {
        let env: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Jev::from_config(cfg, &settings::resolve(&env), &env, Default::default())
    };
    let names = |j: &Jev| (j.provider_name(), j.model(), j.model_default());
    let s = |v: &str| Some(v.to_string());

    write("openrouter/config.json", json!({"apiKey": "x"}));
    assert_eq!(
        names(&make(&[])),
        (
            Some("openrouter"),
            s("typesafe/jev-1.13"),
            s("typesafe/jev-1.13")
        )
    );

    // TypeSafe 키 파일이 있으면 그쪽이 앞서요. 모델은 파일의 model, 없으면 jev-latest예요.
    write("typesafe/config.json", json!({"apiKey": "x"}));
    assert_eq!(
        names(&make(&[])),
        (Some("typesafe"), s("jev-latest"), s("jev-latest"))
    );
    write(
        "typesafe/config.json",
        json!({"apiKey": "x", "model": "jev-9"}),
    );
    assert_eq!(
        names(&make(&[])),
        (Some("typesafe"), s("jev-9"), s("jev-9"))
    );

    // TACIE_JEV_MODEL은 쓰는 모델만 바꾸고 기본값은 그대로예요.
    assert_eq!(
        names(&make(&[("TACIE_JEV_MODEL", "jev-x")])),
        (Some("typesafe"), s("jev-x"), s("jev-9"))
    );

    // 기본값 묶음이에요: 파이프라인은 2.5초·2번, 평가 스크립트는 60초·5번(`JevKw::PIPELINE`, `JevKw::EVAL`).
    assert_eq!(
        JevKw::default(),
        JevKw {
            timeout: Duration::from_millis(2500),
            attempts: 2
        }
    );
    assert_eq!(
        JevKw::EVAL,
        JevKw {
            timeout: Duration::from_secs(60),
            attempts: 5
        }
    );
}
