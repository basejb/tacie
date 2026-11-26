use std::fs;
use std::path::PathBuf;

use tacie_index::EmbCache;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("tacie-embcache-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn vec_of(x: f32) -> Vec<f32> {
    vec![x; 4]
}

#[test]
fn path_encodes_model_and_tokens() {
    let root = tmp("path");
    let c = EmbCache::new(&root, "BAAI/bge-m3", 512);
    assert_eq!(c.path, root.join("BAAI__bge-m3@512").join("vectors.sqlite"));
    assert_eq!((c.model(), c.tokens()), ("BAAI/bge-m3", 512));
}

#[test]
fn put_then_get_by_position_including_duplicates() {
    let c = EmbCache::new(&tmp("put"), "m", 8);
    c.put(&["a", "b"], &[vec_of(1.0), vec_of(2.0)]).unwrap();
    let got = c.get(&["a", "b", "a", "zzz"]).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got[&0], vec_of(1.0));
    assert_eq!(got[&1], vec_of(2.0));
    assert_eq!(got[&2], vec_of(1.0));
}

#[test]
fn retag_sweeps_vectors_no_tag_uses() {
    let c = EmbCache::new(&tmp("retag"), "m", 8);
    c.put(&["a", "b"], &[vec_of(1.0), vec_of(2.0)]).unwrap();
    assert_eq!(c.retag("r#main", &["a"]).unwrap(), 1);
    assert!(c.get(&["b"]).unwrap().is_empty());
    assert_eq!(c.get(&["a"]).unwrap().len(), 1);
}

#[test]
fn drop_prefix_escapes_like_wildcards() {
    let c = EmbCache::new(&tmp("drop"), "m", 8);
    c.put(&["a", "b"], &[vec_of(1.0), vec_of(2.0)]).unwrap();
    c.retag("x_1", &["a", "b"]).unwrap();
    c.retag("xa1", &["b"]).unwrap();
    assert_eq!(c.drop_prefix("x_").unwrap(), 1);
    assert!(c.get(&["a"]).unwrap().is_empty());
    assert_eq!(c.get(&["b"]).unwrap().len(), 1);
}

/// fixtures/vectors.sqlite는 예전 Python 엔진이 만든 고정 값이에요(2026-10-26).
#[test]
fn python_written_cache_is_readable() {
    let root = tmp("py");
    let c = EmbCache::new(&root, "BAAI/bge-m3", 512);
    fs::create_dir_all(c.path.parent().unwrap()).unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vectors.sqlite");
    fs::copy(fixture, &c.path).unwrap();
    let got = c.get(&["a", "b"]).unwrap();
    assert_eq!(got[&0], vec![0.5f32; 1024]);
    assert_eq!(got[&1], (0..1024).map(|i| i as f32).collect::<Vec<_>>());
}

#[test]
fn garbage_file_is_moved_aside_and_cache_restarts_empty() {
    let root = tmp("bad");
    let c = EmbCache::new(&root, "m", 8);
    fs::create_dir_all(c.path.parent().unwrap()).unwrap();
    fs::write(&c.path, b"nope").unwrap();
    assert!(c.get(&["a"]).unwrap().is_empty());
    let bad = c.path.with_file_name("vectors.sqlite+bad");
    assert_eq!(fs::read(&bad).unwrap(), b"nope");
    c.put(&["a"], &[vec_of(3.0)]).unwrap();
    assert_eq!(c.get(&["a"]).unwrap()[&0], vec_of(3.0));
}

#[test]
fn cache_big_lists_fit_sqlite_limits() {
    let c = EmbCache::new(&tmp("big"), "m/x", 8);
    let texts: Vec<String> = (0..2500).map(|i| format!("t{i}")).collect();
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    c.put(&texts, &vec![vec_of(1.0); 2500]).unwrap();
    c.retag("a#main", &texts).unwrap();
    assert_eq!(c.get(&texts).unwrap().len(), 2500);
    assert_eq!(c.get(&["t5"]).unwrap()[&0].len(), 4);
    assert_eq!(c.drop_prefix("a#").unwrap(), 2500);
    assert!(c.get(&texts).unwrap().is_empty());
}

/// 4바이트로 나눠지지 않는 BLOB은 실패해요: 남는 바이트를 버리고 짧은 벡터를 주지 않아요.
#[test]
fn blob_of_odd_length_is_an_error() {
    let c = EmbCache::new(&tmp("odd"), "m", 8);
    c.put(&["a"], &[vec_of(1.0)]).unwrap();
    let db = rusqlite::Connection::open(&c.path).unwrap();
    db.execute("UPDATE vec SET v = ?", [vec![0u8; 5]]).unwrap();
    drop(db);
    assert!(c.get(&["a"]).is_err());
    assert!(c.get(&["zzz"]).unwrap().is_empty());
}
