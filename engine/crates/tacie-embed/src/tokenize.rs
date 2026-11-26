//! The bge-m3 tokenizer. With the same rules as sentence-transformers (`truncation=True, max_length=n`) it adds
//! `<s> … </s>` and fills `<pad>` on the right.
//!
//! Passages are padded only up to the longest chunk in the batch. Queries pad only to a few set lengths
//! (`QUERY_BUCKETS`). The attention mask hides the padding, so vectors barely change (cosine 0.9999983 or more against
//! padding to 512, docs/01).

use std::path::Path;
use std::str::FromStr;
use std::sync::{Mutex, PoisonError};

use anyhow::{Context, anyhow};
use serde_json::{Value, json};
use tokenizers::{Tokenizer, TruncationParams};

/// 질문(`max_tokens` = None)을 자르는 길이예요. 모델은 8,192까지 받지만, 그만큼
/// 채우면 CPU에서 17–53 s, 약 22 GB가 들고 그동안 다른 질문이 막혀서 일부러 줄였어요(docs/04 2-1).
const QUERY_MAX_TOKENS: usize = 2048;

/// 질문(`max_tokens` = None)을 채우는 길이예요. 배치에서 가장 긴 글이 들어가는 가장 짧은 칸으로 채워요.
/// 질문은 `QUERY_MAX_TOKENS`(2,048)에서 잘리니 마지막 칸이 언제나 들어가요. 칸을 두는 까닭: 2083토큰 질문을
/// 8192로 채우면 CPU에서 17–53 s, 메모리 22 GB가 들었어요(그대로면 1.1 s, 2.4 GB).
const QUERY_BUCKETS: [usize; 7] = [32, 64, 128, 256, 512, 1024, 2048];

/// 글마다 한 줄인 `(input_ids, attention_mask)`예요.
type Encoded = (Vec<Vec<i64>>, Vec<Vec<i64>>);

pub struct Tok {
    // 자르는 길이가 호출마다 달라서, 설정을 바꾸고 인코딩하는 동안 잠가요.
    tk: Mutex<Tokenizer>,
    pad_id: i64,
}

impl Tok {
    /// `tokenizer.json`을 읽어 Hugging Face `transformers`가 같은 모델에 만드는 파이프라인으로 고쳐 올려요
    /// (`like_transformers`).
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let tk = std::fs::read_to_string(path)
            .map_err(anyhow::Error::from)
            .and_then(|s| Ok(serde_json::from_str::<Value>(&s)?))
            .and_then(|mut v| {
                like_transformers(&mut v);
                Tokenizer::from_str(&v.to_string()).map_err(|e| anyhow!(e))
            })
            .with_context(|| format!("tokenizer {}", path.display()))?;
        let pad_id = tk
            .token_to_id("<pad>")
            .ok_or_else(|| anyhow!("tokenizer {} has no <pad>", path.display()))?;
        Ok(Self {
            tk: Mutex::new(tk),
            pad_id: i64::from(pad_id),
        })
    }

    /// 모든 줄을 `padded_len` 길이로 채운 `(input_ids, attention_mask)`예요.
    pub fn encode(&self, texts: &[&str], max_tokens: Option<usize>) -> anyhow::Result<Encoded> {
        let encodings = {
            // 잡은 채 패닉한 호출이 있었어도 계속 써요: 자르는 길이는 아래에서 호출마다 다시 정해요
            let mut tk = self.tk.lock().unwrap_or_else(PoisonError::into_inner);
            tk.with_truncation(Some(TruncationParams {
                max_length: max_tokens.unwrap_or(QUERY_MAX_TOKENS),
                ..Default::default()
            }))
            .map_err(|e| anyhow!(e))?;
            tk.encode_batch(texts.to_vec(), true)
                .map_err(|e| anyhow!(e))?
        };
        let longest = encodings.iter().map(|e| e.len()).max().unwrap_or(0);
        let width = padded_len(longest, max_tokens);
        let widen = |xs: &[u32], fill: i64| {
            let mut row: Vec<i64> = xs.iter().map(|&x| i64::from(x)).collect();
            row.resize(width, fill);
            row
        };
        let ids = encodings
            .iter()
            .map(|e| widen(e.get_ids(), self.pad_id))
            .collect();
        let mask = encodings
            .iter()
            .map(|e| widen(e.get_attention_mask(), 0))
            .collect();
        Ok((ids, mask))
    }
}

/// Hugging Face `transformers`(5의 `XLMRobertaTokenizer`)는 `tokenizer.json`을 그대로 쓰지 않고 정규화를
/// `Precompiled` 하나로, 앞 나누기를 `WhitespaceSplit` 뒤 `Metaspace`로 다시 만들어요. 파일 그대로면 글 끝이나
/// 특수 토큰 앞의 공백이 `▁` 토큰 하나로 남아서, `basejb/tacie` 조각 1,053개 중 6개의 벡터가 transformers와 달랐어요
/// (코사인 0.9989–0.9999, docs/01 "Rust 엔진 자료 갱신"). 고친 파이프라인은 23,671개 글에서 transformers와 토큰이
/// 모두 같았어요. 이미 그 모양이거나 모르는 모양이면 그대로 둬요.
fn like_transformers(tk: &mut Value) {
    let precompiled = tk["normalizer"]["normalizers"]
        .as_array()
        .and_then(|ns| ns.iter().find(|n| n["type"] == "Precompiled"))
        .cloned();
    if tk["normalizer"]["type"] == "Sequence"
        && let Some(p) = precompiled
    {
        tk["normalizer"] = p;
    }
    if tk["pre_tokenizer"]["type"] == "Metaspace" {
        let metaspace = tk["pre_tokenizer"].take();
        tk["pre_tokenizer"] = json!({"type": "Sequence",
                                     "pretokenizers": [{"type": "WhitespaceSplit"}, metaspace]});
    }
}

/// 문단(`Some(n)`)은 배치에서 가장 긴 조각 길이(`longest`, 잘려 와서 n을 넘지 않아요)예요. 예전에는 늘 n(512)까지
/// 채웠는데, 짧은 조각이 많은 노션·문서에서 계산의 절반 넘게가 패딩이었어요(조각 400개 82.4 s → 길이순으로 묶어
/// 39.0 s, 2026-10-27 docs/01). CPU 세션은 아레나와 메모리 패턴을 꺼서 모양이 많아도 메모리가 늘지 않아요(docs/01).
/// 질문(None)은 `longest`가 들어가는 가장 짧은 `QUERY_BUCKETS` 칸이에요. 질문은 2,048에서 잘려 오니 칸을 넘지
/// 않아요. 넘으면 `longest` 그대로 두는 건 지키는 갈래일 뿐이에요.
fn padded_len(longest: usize, max_tokens: Option<usize>) -> usize {
    match max_tokens {
        Some(n) => longest.min(n),
        None => QUERY_BUCKETS
            .into_iter()
            .find(|&b| b >= longest)
            .unwrap_or(longest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 잡은 채 패닉한 호출이 있어도 다음 인코딩은 돼요(갱신 중의 패닉이 그 뒤의 /ask를 막으면 안 돼요).
    #[test]
    fn a_panic_holding_the_tokenizer_does_not_stop_encoding() {
        use tokenizers::models::wordlevel::WordLevel;

        let model = WordLevel::builder()
            .vocab(
                [("<pad>", 0), ("<unk>", 1)]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            )
            .unk_token("<unk>".to_string())
            .build()
            .unwrap();
        let tok = Tok {
            tk: Mutex::new(Tokenizer::new(model)),
            pad_id: 0,
        };
        let poisoned = std::thread::scope(|s| {
            s.spawn(|| {
                let _held = tok.tk.lock().unwrap();
                panic!("poison the tokenizer");
            })
            .join()
        });
        assert!(poisoned.is_err() && tok.tk.is_poisoned());
        let (ids, mask) = tok.encode(&["x"], Some(512)).unwrap();
        assert_eq!((ids[0].len(), mask[0].len()), (1, 1));
    }

    #[test]
    fn rebuilds_the_pipeline_the_way_transformers_does() {
        let charsmap = json!({"type": "Precompiled", "precompiled_charsmap": "AAAA"});
        let metaspace = json!({"type": "Metaspace", "replacement": "\u{2581}", "prepend_scheme": "always",
                               "split": true});
        let mut tk = json!({
            "normalizer": {"type": "Sequence", "normalizers": [
                charsmap, {"type": "Replace", "pattern": {"Regex": " {2,}"}, "content": " "}]},
            "pre_tokenizer": metaspace,
            "model": {"type": "Unigram"},
        });
        like_transformers(&mut tk);
        let want = json!({
            "normalizer": charsmap,
            "pre_tokenizer": {"type": "Sequence", "pretokenizers": [{"type": "WhitespaceSplit"}, metaspace]},
            "model": {"type": "Unigram"},
        });
        assert_eq!(tk, want);
        like_transformers(&mut tk); // 이미 고친 모양은 그대로예요
        assert_eq!(tk, want);
        let mut other = json!({"normalizer": null, "pre_tokenizer": {"type": "ByteLevel"}});
        let before = other.clone();
        like_transformers(&mut other);
        assert_eq!(other, before);
    }

    /// Hugging Face `transformers` 5.20이 같은 글에 내는 토큰 id예요. 끝의 공백과 특수 토큰 앞의 공백이 `▁`(6)로
    /// 남지 않아요. 모델 폴더(`TACIE_MODEL_DIR`)가 없으면 건너뛰어요.
    #[test]
    fn token_ids_match_the_python_engine() {
        let Some(dir) = std::env::var_os("TACIE_MODEL_DIR") else {
            eprintln!("skip: TACIE_MODEL_DIR unset");
            return;
        };
        let tok = Tok::load(&Path::new(&dir).join("tokenizer.json")).unwrap();
        let cases: [(&str, &[i64]); 4] = [
            (
                "색인을 갱신해요 ",
                &[0, 6, 16199, 1571, 413, 6, 193117, 5301, 91063, 2],
            ),
            ("        // 같은 ", &[0, 4666, 13928, 2]),
            (
                "`<s> … </s>`를 붙이고",
                &[0, 9473, 0, 153, 2, 9473, 688, 91246, 41040, 2],
            ),
            ("let x = 1;\n    ", &[0, 2633, 1022, 2203, 106, 74, 2]),
        ];
        for (text, want) in cases {
            let (ids, mask) = tok.encode(&[text], None).unwrap();
            let n = mask[0].iter().filter(|&&m| m == 1).count();
            assert_eq!(&ids[0][..n], want, "{text:?}");
        }
    }

    #[test]
    fn pads_to_buckets_and_passages_up_to_max_tokens() {
        assert_eq!(padded_len(9, Some(512)), 9);
        assert_eq!(padded_len(300, Some(512)), 300);
        assert_eq!(padded_len(512, Some(512)), 512);
        assert_eq!(padded_len(200, Some(100)), 100);
        assert_eq!(padded_len(40, None), 64);
        assert_eq!(padded_len(64, None), 64);
        assert_eq!(padded_len(2048, None), 2048);
        assert_eq!(padded_len(2083, None), 2083);

        // 아래는 실제 토크나이저로 봐요. 모델 폴더(`TACIE_MODEL_DIR`)가 없으면 건너뛰어요.
        let Some(dir) = std::env::var_os("TACIE_MODEL_DIR") else {
            eprintln!("skip: TACIE_MODEL_DIR unset");
            return;
        };
        let tok = Tok::load(&Path::new(&dir).join("tokenizer.json")).unwrap();

        let (ids, mask) = tok
            .encode(
                &["포트는 51234예요", "회의 중에 답하는 에이전트"],
                Some(512),
            )
            .unwrap();
        let width = ids.iter().map(Vec::len).max().unwrap();
        assert!(width < 32, "padded to the longest of the batch, not 512");
        assert!(
            mask.iter().any(|m| m.iter().all(|&x| x == 1)),
            "the longest row has no padding"
        );
        for (row, m) in ids.iter().zip(&mask) {
            assert_eq!(row.len(), width);
            assert_eq!(m.len(), width);
            let n = m.iter().filter(|&&x| x == 1).count();
            assert_eq!((row[0], row[n - 1]), (0, 2), "<s> … </s>");
            assert!(row[n..].iter().all(|&x| x == 1), "pad id 1 on the tail");
            assert!(m[n..].iter().all(|&x| x == 0), "mask 0 on the tail");
        }

        let text = "엔진은 앱이 띄우고, 첫 줄에 포트 번호를 적은 뒤 색인을 올려요. 색인은 전용 클론에서 따로 만들어요.";
        let (ids, mask) = tok.encode(&[text], None).unwrap();
        assert_eq!(mask[0].iter().sum::<i64>(), 40);
        assert_eq!(ids[0].len(), 64);
        assert!(ids[0][40..].iter().all(|&x| x == 1));

        // 긴 질문은 2,048토큰에서 잘려요(docs/04 2-1).
        let long = "가나다 라마 ".repeat(6000);
        let (ids, mask) = tok.encode(&[&long], None).unwrap();
        assert_eq!(ids[0].len(), 2048);
        assert_eq!(mask[0].iter().sum::<i64>(), 2048);
    }
}
