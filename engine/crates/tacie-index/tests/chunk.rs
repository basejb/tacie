use std::path::PathBuf;

use serde_json::Value;
use tacie_index::chunk::{read_chunks, read_meta};

fn mini(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mini")
        .join(name)
}

#[test]
fn chunk_roundtrip_keeps_unknown_fields() {
    let path = mini("chunks.json");
    let chunks = read_chunks(&path).unwrap();
    assert_eq!(chunks.len(), 6);

    let code = &chunks[0];
    assert_eq!(
        (code.src.as_str(), code.path.as_str(), code.line),
        ("code", "engine/index.py", 63)
    );
    assert!(
        code.title.is_none() && code.url.is_none() && code.acl.is_none() && code.extra.is_empty()
    );

    let gh = &chunks[3];
    assert_eq!(gh.title.as_deref(), Some("PR #12 · 색인 갱신"));
    assert_eq!(
        gh.url.as_deref(),
        Some("https://github.com/basejb/tacie/pull/12")
    );
    assert_eq!(gh.acl, Some(vec!["*".to_string()]));
    assert_eq!(gh.extra["author"], "basejb");
    assert_eq!(gh.extra["at"], "2026-10-24");
    assert_eq!(chunks[5].extra["replies"], 2);

    // 다시 쓰면 모르는 필드까지 그대로예요.
    let back = serde_json::to_value(&chunks).unwrap();
    let orig: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(back, orig);

    assert_eq!(
        read_meta(&mini("meta.json")).unwrap()["repo"],
        "basejb/tacie"
    );
    assert!(read_meta(&mini("no-such-meta.json")).unwrap().is_empty());
}
