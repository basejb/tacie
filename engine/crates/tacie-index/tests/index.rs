use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Deserialize;
use tacie_embed::{DIM, Embed};
use tacie_index::npy::{read_f32_2d, write_f32_2d};
use tacie_index::{Index, Mask};

fn mini() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mini")
}

/// 임베더가 받은 글, max_tokens, 돌려준 벡터예요.
type Call = (Vec<String>, Option<usize>, Vec<Vec<f32>>);

/// 가짜 임베더예요: crc32(text) % 1024 자리만 1.0.
/// mini 색인은 이 규칙으로 예전 Python 엔진이 만든 고정 값이에요(2026-10-26).
#[derive(Default)]
struct FakeEmbed {
    calls: Mutex<Vec<Call>>,
}

impl FakeEmbed {
    fn vector(text: &str) -> Vec<f32> {
        let mut v = vec![0.0; DIM];
        v[crc32fast::hash(text.as_bytes()) as usize % DIM] = 1.0;
        v
    }
}

impl Embed for FakeEmbed {
    fn embed(&self, texts: &[&str], max_tokens: Option<usize>) -> anyhow::Result<Vec<Vec<f32>>> {
        let out: Vec<Vec<f32>> = texts.iter().map(|t| Self::vector(t)).collect();
        let seen = texts.iter().map(|t| t.to_string()).collect();
        self.calls
            .lock()
            .unwrap()
            .push((seen, max_tokens, out.clone()));
        Ok(out)
    }
}

#[derive(Deserialize)]
struct Query {
    text: String,
    context: Vec<String>,
    line: String,
    mask: String,
    viewer: Vec<String>,
    texts: Vec<String>,
    vectors: Vec<Vec<f32>>,
    expected: Vec<usize>,
}

#[test]
fn dense_never_returns_a_masked_chunk() {
    let index = Index::open(&mini(), Mask::Plans, &HashSet::new()).unwrap();
    assert_eq!(index.allowed, [true, true, false, true, false, true]);
    assert!(index.searchable());

    // 가려진 조각(2: plans 문서, 4: 비공개 슬랙)의 글로 찾아도, k가 넉넉해도 그 조각은 나오지 않아요.
    for hidden in [2, 4] {
        let got = index.dense(&FakeEmbed::vector(&index.chunks[hidden].text), 6);
        assert_eq!(got.len(), 4);
        assert!(!got.contains(&2) && !got.contains(&4), "{got:?}");
    }

    // 보이는 조각의 글이면 그 조각이 맨 앞이고, k개까지만 줘요.
    let got = index.dense(&FakeEmbed::vector(&index.chunks[5].text), 2);
    assert_eq!((got.len(), got[0]), (2, 5));
}

#[test]
fn candidates_match_python_fixture() {
    let queries: Vec<Query> =
        serde_json::from_str(&fs::read_to_string(mini().join("queries.json")).unwrap()).unwrap();
    assert_eq!(queries.len(), 3);
    assert_eq!(queries.iter().filter(|q| q.context.is_empty()).count(), 1);

    for q in &queries {
        let viewer: HashSet<String> = q.viewer.iter().cloned().collect();
        let index = Index::open(&mini(), q.mask.parse().unwrap(), &viewer).unwrap();
        assert_eq!(index.meta["repo"], "basejb/tacie");

        let fake = FakeEmbed::default();
        let got = index
            .candidates(&fake, &q.text, &q.context, &q.line)
            .unwrap();
        assert_eq!(got, q.expected, "{}", q.line);

        // 고정 값을 만들 때 임베딩한 글, 벡터와 같고, 질문이라 max_tokens는 None이에요.
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "{}", q.line);
        let (texts, max_tokens, vectors) = &calls[0];
        assert_eq!(texts, &q.texts);
        assert_eq!(*max_tokens, None);
        assert_eq!(vectors, &q.vectors);
    }
}

#[test]
fn open_needs_one_vector_per_chunk_and_meta_is_optional() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("index_open");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::copy(mini().join("chunks.json"), dir.join("chunks.json")).unwrap();
    fs::copy(mini().join("emb.npy"), dir.join("emb.npy")).unwrap();

    let index = Index::open(&dir, Mask::Plans, &HashSet::new()).unwrap();
    assert!(index.meta.is_empty());
    assert_eq!(index.emb.len(), 6 * DIM);

    // 벡터 수가 조각 수와 다르거나 1024차원이 아니면 열지 않아요.
    let (data, _, _) = read_f32_2d(&mini().join("emb.npy")).unwrap();
    write_f32_2d(&dir.join("emb.npy"), &data[..5 * DIM], 5, DIM).unwrap();
    assert!(Index::open(&dir, Mask::Plans, &HashSet::new()).is_err());
    write_f32_2d(&dir.join("emb.npy"), &data[..6 * 512], 6, 512).unwrap();
    assert!(Index::open(&dir, Mask::Plans, &HashSet::new()).is_err());

    // 빈 색인도 열리지만 찾을 게 없어요.
    fs::write(dir.join("chunks.json"), "[]").unwrap();
    write_f32_2d(&dir.join("emb.npy"), &[], 0, DIM).unwrap();
    let empty = Index::open(&dir, Mask::Plans, &HashSet::new()).unwrap();
    assert!(!empty.searchable());
}
