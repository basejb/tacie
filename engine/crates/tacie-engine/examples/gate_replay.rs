//! Calls the engine's gate (`Jev::gate`) as is and writes p_lookup and the chosen language for each line (for eval
//! comparison; reads the key files).
//! Usage: `cargo run --example gate_replay -p tacie-engine -- <items.json> <out.json>`
//! items.json is `[{"id", "context": [lines...], "line"}]`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tacie_engine::jev::{Jev, JevKw, gate_lang};
use tacie_engine::settings;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let items: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    let env: HashMap<String, String> = std::env::vars().collect();
    let config = std::path::PathBuf::from(env["HOME"].clone()).join(".config");
    let jev = Arc::new(Jev::from_config(
        &config,
        &settings::resolve(&env),
        &env,
        Arc::new(Mutex::new(HashMap::new())),
    ));
    let sem = Arc::new(tokio::sync::Semaphore::new(8));
    let mut tasks = Vec::new();
    for it in items {
        let (jev, sem) = (jev.clone(), sem.clone());
        tasks.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            let ctx: Vec<String> = it["context"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            let line = it["line"].as_str().unwrap();
            let lang = format!("{:?}", gate_lang(&ctx, line));
            let kw = JevKw {
                timeout: std::time::Duration::from_secs(30),
                attempts: 5,
            };
            let g = jev.gate(&ctx, line, kw).await;
            (
                it["id"].as_str().unwrap().to_string(),
                lang,
                g.map(|g| g.p_lookup).ok(),
            )
        }));
    }
    let mut out = serde_json::Map::new();
    for t in tasks {
        let (id, lang, p) = t.await.unwrap();
        out.insert(id, json!({"lang": lang, "p": p}));
    }
    std::fs::write(&args[2], serde_json::to_string(&out).unwrap()).unwrap();
}
