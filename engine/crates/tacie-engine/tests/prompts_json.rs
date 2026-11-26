//! Checks that the prompt copy the relay server fills in (`apps/server/src/prompts.json`) matches the engine's text
//! (`src/prompts.rs`).
//! When the engine's text changes, rewrite the copy with
//! `TACIE_WRITE_PROMPTS=1 cargo test -p tacie-engine --test prompts_json`, and deploy the relay server before
//! releasing the app (shipped apps send only the name).
#![cfg(feature = "local-prompts")]

use std::path::PathBuf;

use serde_json::{Value, json};
use tacie_engine::jev::{Ask, rerank_question};
use tacie_engine::openrouter::chat_body;
use tacie_engine::prompts::{self, Prompt};

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../apps/server/src/prompts.json")
}

#[test]
fn relay_copy_matches_the_engine() {
    let path = path();
    let old: Option<Value> = std::fs::read_to_string(&path).ok().map(|t| serde_json::from_str(&t).unwrap());
    // the open-source repository has no relay server: nothing to compare with there
    let Some(old) = old else {
        assert!(!path.parent().unwrap().exists(), "apps/server/src is there but prompts.json is not");
        eprintln!("no relay copy (apps/server): skipped");
        return;
    };
    // the engine's table, plus the texts old apps still ask the relay for by name, which only the copy keeps
    let mut want = prompts::table();
    for name in prompts::RELAY_ONLY {
        let kept = old["chat"].get(*name).cloned();
        assert!(kept.is_some(), "the relay copy lost {name}, which old apps still ask for");
        want["chat"][*name] = kept.unwrap();
    }
    let want = serde_json::to_string_pretty(&want).unwrap() + "\n";
    if std::env::var("TACIE_WRITE_PROMPTS").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &want).unwrap();
    }
    let have = std::fs::read_to_string(&path).unwrap();
    assert!(
        have == want,
        "apps/server/src/prompts.json differs from the engine's prompts: run TACIE_WRITE_PROMPTS=1 cargo test -p tacie-engine --test prompts_json"
    );
}

/// The texts only old apps use are not in the engine (they stay in the relay copy, which is not public).
#[test]
fn old_app_texts_are_not_in_the_engine() {
    let table = prompts::table();
    for name in prompts::RELAY_ONLY {
        assert!(table["chat"].get(*name).is_none(), "{name}");
    }
}

/// 중계 서버가 리랭크 틀에서 만드는 질문(`{k}`를 번호로)이 엔진이 직접 만드는 질문과 같아요.
#[test]
fn rerank_template_expands_to_the_engine_questions() {
    let table = prompts::table();
    let each = serde_json::to_string(&table["jev"]["rerank"]).unwrap();
    for k in [0, 7, 49] {
        let expanded: Value = serde_json::from_str(&each.replace("{k}", &k.to_string())).unwrap();
        assert_eq!(expanded, rerank_question(&k.to_string()));
    }
}

const RELAY_CHAT: &str = "https://ai.tacie.app/ai/openrouter";
const RELAY_JEV: &str = "https://ai.tacie.app/ai/jev";

/// 중계 서버로 가는 요청에는 프롬프트 글이 없고 이름과 값만 있어요. 다른 곳으로는 다 채운 글이에요.
#[test]
fn relay_requests_carry_names_not_prompts() {
    let prompt = Prompt {
        name: "answer",
        fields: vec![
            ("context", String::new()),
            ("question", "기본값이 몇이죠? {passages}".into()),
            ("passages", "<자료 번호=\"1\">x</자료>".into()),
        ],
    };
    let body = chat_body("m", 300, &prompt, RELAY_CHAT);
    assert_eq!(
        body,
        json!({"model": "m", "max_tokens": 300, "temperature": 0, "reasoning": {"enabled": false},
               "prompt": {"name": "answer", "fields": {"context": "", "question": "기본값이 몇이죠? {passages}",
                                                       "passages": "<자료 번호=\"1\">x</자료>"}}})
    );
    assert!(!body.to_string().contains("규칙"));

    let direct = chat_body("m", 300, &prompt, "http://127.0.0.1:9/v1");
    let text = direct["messages"][0]["content"].as_str().unwrap();
    assert!(direct.get("prompt").is_none());
    // 한 번에 채워요: 질문 속의 `{passages}`는 글자 그대로예요.
    assert!(text.contains("질문: 기본값이 몇이죠? {passages}\n"));
    assert!(text.starts_with("팀 회의나 대화에서"));

    let gate = Ask::Gate.body("jev-latest", json!({"a": 1}), RELAY_JEV);
    assert_eq!(
        gate,
        json!({"model": "jev-latest", "state": {"a": 1}, "prompt": {"name": "gate"}})
    );
    let gate_en = Ask::GateEn.body("jev-latest", json!({"a": 1}), RELAY_JEV);
    assert_eq!(gate_en["prompt"], json!({"name": "gate_en"}));
    let rerank = Ask::Rerank(3).body("jev-latest", json!({}), RELAY_JEV);
    assert_eq!(rerank["prompt"], json!({"name": "rerank", "count": 3}));
    assert!(rerank.get("questions").is_none());
    // 받은 질문은 중계 서버로도 그대로 가요(eval의 `decide`).
    let raw = Ask::Raw(json!({"q": {}})).body("jev-latest", json!({}), RELAY_JEV);
    assert_eq!(raw["questions"], json!({"q": {}}));
}

/// 판단은 앞 발화까지 보고 회의 언어를 정해요. "화자 N:" 표시는 세지 않아요.
#[test]
fn gate_language_follows_the_meeting() {
    use tacie_engine::jev::gate_lang;
    use tacie_engine::lang::Lang;
    let en: Vec<String> = [
        "화자 1: did we decide on double A or triple A batteries",
        "화자 2: I think so",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(gate_lang(&en, "화자 1: right?"), Lang::En);
    let ko: Vec<String> = [
        "화자 1: 재시도 간격은 얼마로 정했죠",
        "화자 2: 그거 PR에 있을 거예요",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(gate_lang(&ko, "화자 1: right?"), Lang::Ko);
    assert_eq!(
        gate_lang(&[], "화자 1: 이 API timeout 기본값이 몇이죠?"),
        Lang::Ko
    );
}

/// 패널 프롬프트(한국어와 `_en`)가 사본에 있어요. iPhone 짝(docs/40 4-3절)과 iPhone 도구 묶음도요.
#[test]
fn panel_prompts_are_in_the_relay_copy() {
    let table = prompts::table();
    for set in ["panel_iphone", "panel_iphone_meeting", "panel_iphone_notes"] {
        assert!(table["tools"][set].as_array().is_some_and(|a| !a.is_empty()), "{set}");
    }
    for name in ["panel_prep", "panel_why", "panel_followup", "panel_chat", "panel_chat_iphone"] {
        for name in [name.to_string(), format!("{name}_en")] {
            let entry = &table["chat"][name.as_str()];
            assert!(entry["text"].as_str().is_some_and(|t| !t.is_empty()), "{name}");
            for field in entry["fields"].as_array().unwrap() {
                let field = format!("{{{}}}", field.as_str().unwrap());
                assert!(entry["text"].as_str().unwrap().contains(&field), "{name} {field}");
            }
        }
    }
}
