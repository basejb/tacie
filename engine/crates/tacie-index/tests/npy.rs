use std::path::PathBuf;

use tacie_index::npy::{read_f32_2d, write_f32_2d};

fn mini(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mini")
        .join(name)
}

fn tmp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// 헤더 안의 글자 하나를 같은 길이의 다른 글자로 바꾼 사본이에요.
fn with_header(bytes: &[u8], from: &str, to: &str) -> Vec<u8> {
    assert_eq!(from.len(), to.len());
    let at = bytes[..128]
        .windows(from.len())
        .position(|w| w == from.as_bytes())
        .unwrap();
    let mut out = bytes.to_vec();
    out[at..at + to.len()].copy_from_slice(to.as_bytes());
    out
}

#[test]
fn reads_fixture_shape_6x1024_and_roundtrips() {
    let src = mini("emb.npy");
    let (data, rows, cols) = read_f32_2d(&src).unwrap();
    assert_eq!((rows, cols), (6, 1024));
    assert_eq!(data.len(), rows * cols);
    // 가짜 임베더 규칙이라 줄마다 1.0이 딱 하나 있어요.
    for row in data.chunks(cols) {
        assert_eq!(row.iter().filter(|&&v| v == 1.0).count(), 1);
        assert_eq!(row.iter().sum::<f32>(), 1.0);
    }

    let out = tmp("npy_roundtrip.npy");
    write_f32_2d(&out, &data, rows, cols).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&src).unwrap());
}

#[test]
fn rejects_other_dtypes_fortran_order_shapes_and_versions() {
    let bytes = std::fs::read(mini("emb.npy")).unwrap();
    let mut bad = vec![
        ("f8", with_header(&bytes, "'<f4'", "'<f8'")),
        ("fortran", with_header(&bytes, "False", "True ")),
        ("1d", with_header(&bytes, "(6, 1024)", "(6144,)  ")),
        ("truncated", bytes[..bytes.len() - 4].to_vec()),
        ("not npy", b"{\"not\": \"npy\"}".to_vec()),
    ];
    let mut v2 = bytes.clone();
    v2[6] = 2;
    bad.push(("v2", v2));
    for (name, b) in bad {
        let path = tmp(&format!("npy_bad_{name}.npy"));
        std::fs::write(&path, b).unwrap();
        assert!(read_f32_2d(&path).is_err(), "{name} should not read");
    }

    // 모양과 길이가 안 맞으면 쓰지 않아요.
    assert!(write_f32_2d(&tmp("npy_bad_write.npy"), &[0.0; 5], 2, 3).is_err());
}
