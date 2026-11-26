//! Writing and recovering index folders, and JSON with the same bytes as Python `json.dumps` (`pyjson`). The expected
//! text came from running `json.dumps(…, ensure_ascii=False[, indent=1])` in Python 3.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value, json};
use tacie_embed::{DIM, PASSAGE_TOKENS};
use tacie_index::npy::read_f32_2d;
use tacie_index::{EmbCache, Index, MODEL, Mask, RawChunk, pyjson, recover, seedable, write_index};

/// 가짜 임베더의 규칙이에요: crc32(text) % 1024 자리만 1.0.
fn fake_vector(text: &str) -> Vec<f32> {
    let mut v = vec![0.0; DIM];
    v[crc32fast::hash(text.as_bytes()) as usize % DIM] = 1.0;
    v
}

fn fake_embed(texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
    Ok(texts.iter().map(|t| fake_vector(t)).collect())
}

fn raw(v: Value) -> Vec<RawChunk> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_object().unwrap().clone())
        .collect()
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

fn keys(m: &Map<String, Value>) -> Vec<&str> {
    m.keys().map(String::as_str).collect()
}

fn mini() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mini")
}

fn open(dir: &Path) -> Index {
    Index::open(dir, Mask::None, &HashSet::new()).unwrap()
}

fn meta_of(dir: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(dir.join("meta.json")).unwrap()).unwrap()
}

#[test]
fn pyjson_matches_python_dumps() {
    let v = json!({"a": [1, {"b": "한\n\u{1}\"\\\x7f"}], "c": {}, "d": []});
    // Python: '{"a": [1, {"b": "한\\n\\u0001\\"\\\\\x7f"}], "c": {}, "d": []}'
    assert_eq!(
        pyjson::to_string(&v, None),
        concat!(
            r#"{"a": [1, {"b": "한\n\u0001\"\\"#,
            "\x7f",
            r#""}], "c": {}, "d": []}"#
        )
    );
    let w = json!({"a": [1, {"b": "x"}], "c": {}, "d": [], "e": null, "f": true});
    assert_eq!(
        pyjson::to_string(&w, Some(1)),
        "{\n \"a\": [\n  1,\n  {\n   \"b\": \"x\"\n  }\n ],\n \"c\": {},\n \"d\": [],\n \"e\": null,\n \"f\": true\n}"
    );
    // 짧은 이스케이프는 \b \f \n \r \t뿐이고 나머지 0x20 미만은 \u00xx(소문자)예요. 0x7f, U+2028, 이모지, '/'는 그대로예요.
    let s = json!("\u{0}\u{8}\u{b}\u{c}\r\t\u{1b}\u{1f}\u{7f}\u{2028}😀/");
    assert_eq!(
        pyjson::to_string(&s, None),
        "\"\\u0000\\b\\u000b\\f\\r\\t\\u001b\\u001f\u{7f}\u{2028}😀/\""
    );
    // 정수는 u64·i64 끝까지 그대로, 들여쓰기는 깊이마다 n칸이에요.
    let n = json!({"x": [[], {}, [18446744073709551615u64, [-9223372036854775808i64]], {"k": {}}]});
    assert_eq!(
        pyjson::to_string(&n, Some(2)),
        "{\n  \"x\": [\n    [],\n    {},\n    [\n      18446744073709551615,\n      [\n        -9223372036854775808\n      ]\n    ],\n    {\n      \"k\": {}\n    }\n  ]\n}"
    );
}

/// 소수는 Python `float.__repr__`(가장 짧은 자리, 소수점 자리가 -4 초과 16 이하면 고정, 아니면 `e±XX`)대로예요.
#[test]
fn pyjson_writes_floats_like_python_repr() {
    let cases: [(f64, &str); 19] = [
        (0.1, "0.1"),
        (1.0, "1.0"),
        (1e16, "1e+16"),
        (1e15, "1000000000000000.0"),
        (123456789012345680000.0, "1.2345678901234568e+20"),
        (0.0001, "0.0001"),
        (0.00001, "1e-05"),
        (1.5e-7, "1.5e-07"),
        (-0.0, "-0.0"),
        (2.5, "2.5"),
        (1e22, "1e+22"),
        (f64::MAX, "1.7976931348623157e+308"),
        (5e-324, "5e-324"),
        (0.1 + 0.2, "0.30000000000000004"),
        (1.0 / 3.0, "0.3333333333333333"),
        (100.0, "100.0"),
        (9999999999999998.0, "9999999999999998.0"),
        (-12.5, "-12.5"),
        (-0.000123, "-0.000123"),
    ];
    for (f, want) in cases {
        assert_eq!(pyjson::to_string(&json!(f), None), want, "{f:e}");
    }
}

#[test]
fn write_handles_no_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let n = write_index(dir.path(), &[], &mut fake_embed, &Map::new(), None, None).unwrap();
    assert_eq!(n, 0);
    let (emb, rows, cols) = read_f32_2d(&dir.path().join("emb.npy")).unwrap();
    assert_eq!((emb.len(), rows, cols), (0, 0, 1024));
    assert_eq!(
        fs::read_to_string(dir.path().join("chunks.json")).unwrap(),
        "[]"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("meta.json")).unwrap(),
        "{}"
    );
}

/// mini 색인(예전 Python 엔진이 만든 고정 값이에요, 2026-10-26)을 읽어 같은 가짜 벡터로 다시 쓰면 세 파일이 바이트까지 같아요.
#[test]
fn rewritten_python_index_is_byte_identical() {
    let ix = open(&mini());
    let dir = tempfile::tempdir().unwrap();
    let n = write_index(dir.path(), &ix.raw, &mut fake_embed, &ix.meta, None, None).unwrap();
    assert_eq!(n, 6);
    for f in ["chunks.json", "meta.json", "emb.npy"] {
        assert_eq!(
            fs::read(dir.path().join(f)).unwrap(),
            fs::read(mini().join(f)).unwrap(),
            "{f}"
        );
    }
}

#[test]
fn chunks_json_keeps_key_order_and_nulls() {
    let chunks = raw(json!([
        {"src": "github", "path": "github/issue/7", "title": "이슈 #7 · 색인", "author": null, "at": "2026-10-24",
         "acl": ["*"], "line": 1, "url": "https://github.com/o/r/issues/7", "text": "github: 이슈 #7\n본문"},
        {"src": "slack", "path": "slack/C1/1.2", "title": null, "acl": null, "line": 3, "text": "slack: #eng\n탭\t끝"},
    ]));
    let dir = tempfile::tempdir().unwrap();
    write_index(
        dir.path(),
        &chunks,
        &mut fake_embed,
        &Map::new(),
        None,
        None,
    )
    .unwrap();
    let written = fs::read_to_string(dir.path().join("chunks.json")).unwrap();
    let as_value = Value::Array(chunks.iter().cloned().map(Value::Object).collect());
    assert_eq!(written, pyjson::to_string(&as_value, None));
    // Python json.dumps(chunks, ensure_ascii=False)
    assert_eq!(
        written,
        r#"[{"src": "github", "path": "github/issue/7", "title": "이슈 #7 · 색인", "author": null, "at": "2026-10-24", "acl": ["*"], "line": 1, "url": "https://github.com/o/r/issues/7", "text": "github: 이슈 #7\n본문"}, {"src": "slack", "path": "slack/C1/1.2", "title": null, "acl": null, "line": 3, "text": "slack: #eng\n탭\t끝"}]"#
    );

    let ix = open(dir.path());
    assert_eq!(
        keys(&ix.raw[0]),
        [
            "src", "path", "title", "author", "at", "acl", "line", "url", "text"
        ]
    );
    assert_eq!(*ix.raw, chunks);
    assert_eq!(ix.chunks[0].extra["author"], Value::Null);
    // null은 검색용 Chunk에서는 없음·닫힘으로 읽어도 raw에는 그대로 남아요.
    assert_eq!(ix.chunks[1].title, None);
    assert_eq!(ix.chunks[1].acl, Some(vec![]));
    assert_eq!(ix.raw[1]["title"], Value::Null);

    // 읽은 raw를 다시 쓰면 같은 바이트예요(예전에 만든 색인을 갱신해도 모양이 그대로예요).
    let again = tempfile::tempdir().unwrap();
    write_index(again.path(), &ix.raw, &mut fake_embed, &ix.meta, None, None).unwrap();
    assert_eq!(
        fs::read_to_string(again.path().join("chunks.json")).unwrap(),
        written
    );
}

#[test]
fn meta_json_is_indented_by_one() {
    let dir = tempfile::tempdir().unwrap();
    let meta = obj(
        json!({"repo": "o/r", "commit": null, "sources": {"repo": {"branch": "main"}, "slack": {}},
                          "embed": {"model": MODEL, "tokens": PASSAGE_TOKENS}}),
    );
    write_index(dir.path(), &[], &mut fake_embed, &meta, None, None).unwrap();
    // Python json.dumps(meta, indent=1, ensure_ascii=False)
    assert_eq!(
        fs::read_to_string(dir.path().join("meta.json")).unwrap(),
        "{\n \"repo\": \"o/r\",\n \"commit\": null,\n \"sources\": {\n  \"repo\": {\n   \"branch\": \"main\"\n  },\n  \"slack\": {}\n },\n \"embed\": {\n  \"model\": \"BAAI/bge-m3\",\n  \"tokens\": 512\n }\n}"
    );
}

#[test]
fn with_meta_shares_chunks_and_vectors() {
    let dir = tempfile::tempdir().unwrap();
    let chunks = raw(
        json!([{"src": "repo", "path": "a", "line": 1, "text": "alpha"},
                            {"src": "repo", "path": "b", "line": 1, "acl": ["slack:U1"], "text": "beta"}]),
    );
    write_index(
        dir.path(),
        &chunks,
        &mut fake_embed,
        &obj(json!({"repo": "o/old"})),
        None,
        None,
    )
    .unwrap();
    let viewer: HashSet<String> = ["slack:U1".to_string()].into();
    let ix = Index::open(dir.path(), Mask::Plans, &viewer).unwrap();
    let meta = obj(json!({"repo": "o/old", "built_at": "2026-10-26T00:00:00+0900"}));
    let next = ix.with_meta(meta.clone());
    assert!(Arc::ptr_eq(&ix.chunks, &next.chunks));
    assert!(Arc::ptr_eq(&ix.raw, &next.raw));
    assert!(Arc::ptr_eq(&ix.emb, &next.emb));
    assert_eq!(next.meta, meta);
    assert_eq!(keys(&next.meta), ["repo", "built_at"]);
    assert_eq!(ix.meta, obj(json!({"repo": "o/old"})));
    assert_eq!(
        (&next.allowed, next.mask, &next.viewer),
        (&ix.allowed, ix.mask, &ix.viewer)
    );
}

#[test]
fn write_rejects_vectors_of_another_size() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let cache = EmbCache::new(&tmp.path().join("cache"), MODEL, PASSAGE_TOKENS);
    let chunks = raw(json!([{"src": "repo", "path": "a", "text": "alpha"}]));
    let mut short = |texts: &[&str]| -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![1.0; 4]).collect())
    };
    let err = write_index(
        &out,
        &chunks,
        &mut short,
        &Map::new(),
        None,
        Some((&cache, "r#main")),
    );
    assert!(err.is_err());
    // np.stack에서 멈춰요: 캐시에도, 폴더에도 아무것도 쓰지 않아요.
    assert!(!out.join("emb.npy").exists() && !out.join("chunks.json").exists());
    assert!(cache.get(&["alpha"]).unwrap().is_empty());
}

#[test]
fn cache_seeds_from_a_matching_index_only() {
    let tmp = tempfile::tempdir().unwrap();
    let chunks = raw(json!([{"src": "repo", "path": "a", "text": "alpha"},
                            {"src": "repo", "path": "b", "text": "beta"}]));
    let calls: RefCell<Vec<Vec<String>>> = RefCell::new(Vec::new());
    let mut embed = |texts: &[&str]| {
        calls
            .borrow_mut()
            .push(texts.iter().map(|t| t.to_string()).collect());
        fake_embed(texts)
    };
    let old_index = |name: &str, embed_meta: Option<Value>| {
        let d = tmp.path().join(name);
        let meta = embed_meta.map_or_else(Map::new, |e| obj(json!({"embed": e})));
        write_index(&d, &chunks, &mut fake_embed, &meta, None, None).unwrap();
        d
    };
    let cache_for = |name: &str| EmbCache::new(&tmp.path().join(name), MODEL, PASSAGE_TOKENS);

    let other = old_index(
        "other",
        Some(json!({"model": "other-model", "tokens": 512})),
    );
    assert!(!seedable(&other, &cache_for("c1")));
    let n = write_index(
        &tmp.path().join("o1"),
        &chunks,
        &mut embed,
        &Map::new(),
        Some(&other),
        Some((&cache_for("c1"), "r#main")),
    )
    .unwrap();
    assert_eq!(n, 2);
    let mut last = calls.borrow().last().unwrap().clone();
    last.sort();
    assert_eq!(last, ["alpha", "beta"]);

    let plain = old_index("plain", None); // meta가 "embed"를 적기 전에 만든 색인: bge-m3, 512예요
    assert!(seedable(&plain, &cache_for("c2")));
    // 키 순서는 보지 않아요.
    let reordered = old_index("reordered", Some(json!({"tokens": 512, "model": MODEL})));
    assert!(seedable(&reordered, &cache_for("c2")));
    let n = write_index(
        &tmp.path().join("o2"),
        &chunks,
        &mut embed,
        &Map::new(),
        Some(&plain),
        Some((&cache_for("c2"), "r#main")),
    )
    .unwrap();
    assert_eq!(n, 0);
    assert_eq!(cache_for("c2").get(&["alpha", "beta"]).unwrap().len(), 2);
    assert_eq!(
        fs::read(tmp.path().join("o2/emb.npy")).unwrap(),
        fs::read(plain.join("emb.npy")).unwrap()
    );

    // 모델이 캐시의 것과 달라요: 가져오지 않아요.
    let odd = EmbCache::new(&tmp.path().join("c3"), "other-model", 512);
    assert!(!seedable(&plain, &odd));
    let n = write_index(
        &tmp.path().join("o3"),
        &chunks,
        &mut embed,
        &Map::new(),
        Some(&plain),
        Some((&odd, "r#main")),
    )
    .unwrap();
    assert_eq!(n, 2);
    assert_eq!(calls.borrow().len(), 2);
}

/// 캐시에 있는 글은 캐시에서, 옛 색인에 있는 글은 거기서(같은 글이 여럿이면 뒤의 것) 가져오고 나머지만 임베딩해요.
/// 캐시에는 가져온 것과 새 것이 들어가고, 태그는 이번 글 전부를 가리켜요.
#[test]
fn write_takes_cache_then_old_index_then_embeds_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = EmbCache::new(&tmp.path().join("cache"), MODEL, PASSAGE_TOKENS);
    let marked = |x: f32| {
        let mut v = vec![0.0; DIM];
        v[0] = x;
        v
    };
    cache.put(&["cached"], &[marked(7.0)]).unwrap();
    let old = tmp.path().join("old");
    let old_chunks = raw(json!([{"text": "seeded"}, {"text": "seeded"}]));
    let mut nth = 0.0;
    let mut numbered = |texts: &[&str]| -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|_| {
                nth += 1.0;
                marked(nth)
            })
            .collect())
    };
    write_index(&old, &old_chunks, &mut numbered, &Map::new(), None, None).unwrap();

    let chunks =
        raw(json!([{"text": "new"}, {"text": "seeded"}, {"text": "cached"}, {"text": "new2"}]));
    let seen: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let mut embed = |texts: &[&str]| -> anyhow::Result<Vec<Vec<f32>>> {
        seen.borrow_mut()
            .extend(texts.iter().map(|t| t.to_string()));
        Ok(texts.iter().map(|_| marked(9.0)).collect())
    };
    let out = tmp.path().join("out");
    let n = write_index(
        &out,
        &chunks,
        &mut embed,
        &Map::new(),
        Some(&old),
        Some((&cache, "r#main")),
    )
    .unwrap();
    assert_eq!(n, 2);
    assert_eq!(*seen.borrow(), ["new", "new2"]);
    let (emb, rows, _) = read_f32_2d(&out.join("emb.npy")).unwrap();
    let firsts: Vec<f32> = (0..rows).map(|r| emb[r * DIM]).collect();
    assert_eq!(firsts, [9.0, 2.0, 7.0, 9.0]); // "seeded"는 옛 색인의 뒤 줄(2.0)이에요
    let got = cache.get(&["new", "seeded", "cached", "new2"]).unwrap();
    assert_eq!(got.len(), 4);
    assert_eq!(got[&1][0], 2.0);
    // 이번 글만 태그에 남아요: 다른 태그가 없는 글은 retag가 지워요.
    cache.put(&["stale"], &[marked(1.0)]).unwrap();
    write_index(
        &out,
        &chunks[..1],
        &mut embed,
        &Map::new(),
        None,
        Some((&cache, "r#main")),
    )
    .unwrap();
    assert_eq!(cache.get(&["new", "seeded", "stale"]).unwrap().len(), 1);
}

#[test]
fn recover_puts_back_a_cut_off_swap() {
    let tmp = tempfile::tempdir().unwrap();
    let (ix, old, new) = (
        tmp.path().join("index"),
        tmp.path().join("index.old"),
        tmp.path().join("index.new"),
    );
    let write = |dir: &Path, repo: &str| {
        write_index(
            dir,
            &[],
            &mut fake_embed,
            &obj(json!({"repo": repo})),
            None,
            None,
        )
        .unwrap();
    };
    assert_eq!(recover(&ix).unwrap(), None); // 되돌릴 게 없어요
    assert!(!ix.exists());
    write(&new, "o/new");
    fs::remove_file(new.join("meta.json")).unwrap(); // 다 쓰기 전에 끊겼어요
    assert_eq!(recover(&ix).unwrap(), None);
    assert!(!ix.exists());
    write(&new, "o/new");
    write(&old, "o/old");
    assert_eq!(recover(&ix).unwrap(), Some(old.clone()));
    assert!(!old.exists());
    assert_eq!(meta_of(&ix)["repo"], "o/old");
    assert_eq!(recover(&ix).unwrap(), None); // 살아 있는 index/는 바꾸지 않아요
    assert!(new.is_dir());
    fs::remove_dir_all(&ix).unwrap();
    assert_eq!(recover(&ix).unwrap(), Some(new.clone()));
    assert!(!new.exists());
    assert_eq!(meta_of(&ix)["repo"], "o/new");
}
