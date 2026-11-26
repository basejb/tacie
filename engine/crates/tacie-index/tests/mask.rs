use std::collections::HashSet;
use std::path::PathBuf;

use tacie_index::Mask;
use tacie_index::chunk::read_chunks;
use tacie_index::mask::allowed;

fn chunks() -> Vec<tacie_index::Chunk> {
    read_chunks(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mini/chunks.json"))
        .unwrap()
}

fn viewer(names: &[&str]) -> HashSet<String> {
    names.iter().map(|n| n.to_string()).collect()
}

#[test]
fn plans_mask_hides_only_plans_path() {
    let mut chunks = chunks();
    let u1 = viewer(&["slack:U1"]); // 비공개 조각도 보이게 해서 마스크만 봐요
    let plans: Mask = "plans".parse().unwrap();
    assert_eq!(
        allowed(&chunks, plans, &u1),
        [true, true, false, true, true, true]
    );
    assert!(allowed(&chunks, Mask::None, &u1).iter().all(|&ok| ok));

    // plans 밖의 superpowers 문서는 plans 마스크로는 보이고 superpowers 마스크로는 가려요.
    chunks[2].path = "docs/superpowers/specs/y.md".into();
    assert!(allowed(&chunks, Mask::Plans, &u1)[2]);
    assert!(!allowed(&chunks, Mask::Superpowers, &u1)[2]);

    assert_eq!("superpowers".parse::<Mask>().unwrap(), Mask::Superpowers);
    assert_eq!("none".parse::<Mask>().unwrap(), Mask::None);
    assert!("plan".parse::<Mask>().is_err());
}

#[test]
fn acl_chunk_is_hidden_unless_viewer_matches() {
    let mut chunks = chunks();
    assert_eq!(
        allowed(&chunks, Mask::None, &viewer(&[])),
        [true, true, true, true, false, true]
    );
    assert!(allowed(&chunks, Mask::None, &viewer(&["slack:U1"]))[4]);
    assert!(!allowed(&chunks, Mask::None, &viewer(&["slack:U2"]))[4]);

    // acl이 비어 있으면 아무도 못 봐요.
    chunks[0].acl = Some(vec![]);
    assert!(!allowed(&chunks, Mask::None, &viewer(&["slack:U1"]))[0]);
}

#[test]
fn explicit_null_acl_hides_chunk_but_missing_acl_is_public() {
    // 키가 없으면 모두에게 열리지만, 값이 null이면 아무도 못 봐요.
    let parse = |json: &str| -> tacie_index::Chunk { serde_json::from_str(json).unwrap() };
    let null_acl =
        parse(r#"{"src": "slack", "path": "slack/C1/1", "line": 1, "text": "t", "acl": null}"#);
    let no_acl = parse(r#"{"src": "code", "path": "a.py", "line": 1, "text": "t"}"#);
    assert_eq!(null_acl.acl, Some(vec![]));
    assert!(null_acl.extra.is_empty());
    assert_eq!(no_acl.acl, None);

    let chunks = [null_acl, no_acl];
    assert_eq!(allowed(&chunks, Mask::None, &viewer(&[])), [false, true]);
    assert_eq!(
        allowed(&chunks, Mask::None, &viewer(&["slack:U1"])),
        [false, true]
    );

    // 다시 쓰면 null은 []가 되고, 없던 acl은 계속 없어요.
    let back = serde_json::to_value(&chunks).unwrap();
    assert_eq!(back[0]["acl"], serde_json::json!([]));
    assert!(back[1].get("acl").is_none());
}
