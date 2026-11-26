//! Checks that Jev request bodies match eval. The real API is not called.
//!
//! There are two copies of the prompt text: the engine (`src/jev.rs`, `PASSAGE_CHARS` in `src/pipeline.rs`) and
//! eval (`eval/enginelib/jev.py`, `PASSAGE_CHARS` in `strict.py`). Both are checked against the same fixture
//! (`eval/tests/fixtures/jev_requests.json`) (the eval side in `eval/tests/test_enginelib.py`). The fixture was
//! made by the old Python engine (2026-10-26). Changing only one side breaks one of them.
//!
//! The fixture's `raw` is the bytes Python `requests` sent (`json.dumps` defaults: a space after `,` and `:` as in
//! `", "` and `": "`, non-ASCII characters as `\uXXXX`). Rust (serde_json) sends without spaces and with UTF-8 as
//! is, so the bytes on the wire differ in only those two ways. So the received body is rewritten the Python way in
//! the same key order and compared with `raw` byte for byte. Any difference in characters, keys or key order breaks
//! it.
// These tests look at prompt text: they are skipped in release builds without the text (`--no-default-features`).
#![cfg(feature = "local-prompts")]

mod common {
    pub mod fake_jev;
}

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::fake_jev::FakeJev;
use serde_json::{Value, json};
use tacie_engine::jev::{Jev, JevKw};
use tacie_engine::pipeline::PASSAGE_CHARS;
use tacie_engine::settings;

fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../eval/tests/fixtures/jev_requests.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

/// Python `json.dumps(value)`(기본값)과 같은 글이에요. 고정 값에는 글, 객체, 배열, null만 나와요.
fn py_dumps(v: &Value) -> String {
    let mut out = String::new();
    write_py(v, &mut out);
    out
}

fn write_py(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_py_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_py(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_py_str(k, out);
                out.push_str(": ");
                write_py(x, out);
            }
            out.push('}');
        }
    }
}

/// `ensure_ascii=True`: ASCII 밖 글자는 UTF-16 단위마다 `\uxxxx`(소문자 16진수)예요.
fn write_py_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (' '..='~').contains(&c) => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    write!(out, "\\u{unit:04x}").unwrap();
                }
            }
        }
    }
    out.push('"');
}

#[test]
fn py_dumps_matches_python_json_dumps() {
    // python3 -c 'import json; print(json.dumps({"a": ["가\n", None, "😀\"\\"]}))'
    assert_eq!(
        py_dumps(&json!({"a": ["가\n", null, "😀\"\\"]})),
        r#"{"a": ["\uac00\n", null, "\ud83d\ude00\"\\"]}"#
    );
}

/// eval의 가짜(`jev_answers`)와 같은 200 응답이에요: noul은 0.25, choice는 고르게 나눈 값(소수 6자리)이에요.
fn answers(questions: &Value, input_tokens: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for (name, q) in questions.as_object().unwrap() {
        let a = if q["type"] == "choice" {
            let keys = q["criteria"].as_object().unwrap();
            let p = (1.0 / keys.len() as f64 * 1e6).round() / 1e6;
            let probs: serde_json::Map<String, Value> =
                keys.keys().map(|k| (k.clone(), json!(p))).collect();
            json!({"probabilities": probs})
        } else {
            json!({"noul": 0.25})
        };
        out.insert(name.clone(), a);
    }
    json!({"answers": out, "usage": {"input_tokens": input_tokens}})
}

fn golden_questions(g: &Value, name: &str, i: usize) -> Value {
    let raw: Value = serde_json::from_str(g[name]["requests"][i]["raw"].as_str().unwrap()).unwrap();
    raw["questions"].clone()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn jev_request_bodies_match_the_eval_fixture() {
    let g = fixture();
    let tokens = &g["input_tokens"];
    let fake = FakeJev::start(vec![
        (200, answers(&golden_questions(&g, "gate", 0), tokens)),
        (200, answers(&golden_questions(&g, "rerank", 0), tokens)),
        // 처음 한 번은 길다고 거절해요: 긴 맥락 헤더를 붙여 다시 보내야 해요(eval 테스트와 같아요).
        (400, json!("context too long for this model")),
        (
            200,
            answers(&golden_questions(&g, "decide_long", 1), tokens),
        ),
    ])
    .await;

    // eval과 같은 TypeSafe 키 파일 하나, 모델은 기본값(`jev-latest`)이에요.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(g["provider"].as_str().unwrap())).unwrap();
    std::fs::write(
        dir.path().join("typesafe/config.json"),
        json!({"apiKey": "test-fake"}).to_string(),
    )
    .unwrap();
    let env = HashMap::from([("TACIE_JEV_URL".to_string(), fake.url.clone())]);
    let jev = Jev::from_config(
        dir.path(),
        &settings::resolve(&env),
        &env,
        Arc::new(Mutex::new(HashMap::new())),
    )
    .with_backoff(|_| Duration::ZERO);
    assert_eq!(jev.provider_name(), g["provider"].as_str());
    assert_eq!(jev.model().as_deref(), g["model"].as_str());

    let c = &g["calls"];
    // eval이 부르는 기본 인자(60초, 다섯 번)예요. 앱은 JevKw::PIPELINE이지만 본문은 같아요.
    let kw = JevKw::EVAL;
    let gate = jev
        .gate(
            &strings(&c["gate"]["context"]),
            c["gate"]["line"].as_str().unwrap(),
            kw,
        )
        .await
        .unwrap();
    let rerank = jev
        .rerank(
            &strings(&c["rerank"]["context"]),
            c["rerank"]["line"].as_str().unwrap(),
            &strings(&c["rerank"]["passages"]),
            kw,
        )
        .await
        .unwrap();
    let (long, _) = jev
        .decide(
            c["decide_long"]["state"].clone(),
            c["decide_long"]["questions"].clone(),
            kw,
        )
        .await
        .unwrap();

    let want: Vec<&Value> = ["gate", "rerank", "decide_long"]
        .iter()
        .flat_map(|name| g[*name]["requests"].as_array().unwrap())
        .collect();
    let bodies = fake.bodies();
    let headers = fake.headers();
    assert_eq!(bodies.len(), want.len());
    assert_eq!(bodies.len(), 4);
    for (i, (body, w)) in bodies.iter().zip(&want).enumerate() {
        assert_eq!(py_dumps(body), w["raw"].as_str().unwrap(), "request {i}");
        for h in w["headers"].as_array().unwrap() {
            assert!(
                headers[i].contains_key(h.as_str().unwrap()),
                "request {i} lacks {h}"
            );
        }
    }
    let long_header: Vec<bool> = headers
        .iter()
        .map(|h| h.contains_key("typesafe-danger-zone-long-context"))
        .collect();
    assert_eq!(long_header, [false, false, false, true]);

    // 돌려주는 값도 eval과 같아요(ms는 빼고).
    let r = &g["gate"]["result"];
    assert_eq!(gate.p_lookup, r["p_lookup"].as_f64().unwrap());
    assert_eq!(Value::Object(gate.source), r["source"]);
    assert_eq!(gate.p_end, r["p_end"].as_f64());
    assert_eq!(gate.usage, r["usage"]);
    let r = &g["rerank"]["result"];
    assert_eq!(json!(rerank.p), r["p"]);
    assert_eq!(rerank.usage, r["usage"]);
    assert_eq!(long, g["decide_long"]["result"]);
}

/// 리랭커에 보내는 조각 글자 수예요(eval은 `strict.PASSAGE_CHARS`로 잘라 보내요).
#[test]
fn passage_chars_match_the_eval_fixture() {
    assert_eq!(
        Some(PASSAGE_CHARS as u64),
        fixture()["passage_chars"].as_u64()
    );
}
