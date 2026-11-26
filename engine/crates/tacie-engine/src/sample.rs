//! Sample data for onboarding (docs/11 12절). Only requests and meetings called with `sample: true` in the practice
//! step search this index.
//! The user's own index is not looked at (instead of skipping the permission filter, the index itself is swapped).
//!
//! The sample index is smaller than the candidate count (`CAP`). So the question is not embedded and every visible
//! chunk goes to the reranker as a candidate: it answers even before the search model is downloaded, and it does
//! not matter if the transcribed text differs slightly from the sample question.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use tacie_embed::Embed;
use tacie_index::{CAP, Index, Mask};

use crate::errors::{Kind, ServiceError};

/// 모델이 없어서 임베딩하지 못했을 때의 글이에요(종류는 `Kind::Model`).
pub const MODEL_MISSING: &str = "search model is not downloaded yet";

/// 예시 조각의 id는 이만큼 더해서 내보내요: `/answer`가 받은 id만 보고 어느 색인의 조각인지 알아요(플래그를
/// 빠뜨려도 같은 번호의 내 자료 조각이 답 모델로 가지 않아요).
pub const SAMPLE_BASE: usize = 1_000_000;

#[derive(Serialize, Deserialize, Clone)]
pub struct Question {
    pub text: String,
    /// 연습의 어디에 쓰는지예요(`ask`: 3단계 버튼, `spoken`: 4단계 회의 연습에서 들려주는 말). 앱에 그대로 넘겨요.
    #[serde(rename = "use", default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

/// 예시 자료를 만들 때 읽는 질문 한 줄이에요(`sample/questions.json`, `sample/en/questions.json`). `answer`는 답이
/// 적힌 파일이고, 색인을 만들 때 엄격 확인(`answer_rank`)에만 써요. 번들의 `questions.json`에는 `Question`만 남겨요.
#[derive(Deserialize)]
pub struct BuildQuestion {
    #[serde(flatten)]
    pub question: Question,
    #[serde(default)]
    pub answer: Vec<String>,
}

/// 엄격 확인의 기준이에요: 질문으로 찾은 위 3조각(카드 3장) 안에 정답 파일이 있어야 해요.
pub const STRICT_TOP: usize = 3;

/// 검색 순서대로 늘어선 조각 경로에서 정답 파일이 처음 나오는 순위(1부터)예요. 없으면 None이에요.
pub fn answer_rank(ranked_paths: &[&str], answer: &[String]) -> Option<usize> {
    ranked_paths
        .iter()
        .position(|p| answer.iter().any(|a| a == p))
        .map(|i| i + 1)
}

pub struct Sample {
    pub index: Arc<Index>,
    pub questions: Vec<Question>,
}

impl Sample {
    /// `dir`에는 색인(`chunks.json`, `emb.npy`, `meta.json`)과 `questions.json`(`[{text, use}]`)이 있어요. 조각은
    /// 모두 공개라 보는 사람도 mask도 없어요. 조각이 `CAP`보다 많으면 열지 않아요(전부를 후보로 쓰니까요).
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        let index = Index::open(dir, Mask::None, &HashSet::new())
            .with_context(|| format!("open sample index {}", dir.display()))?;
        ensure!(
            index.chunks.len() <= CAP,
            "sample index has {} chunks, more than the {CAP} candidates it is searched with",
            index.chunks.len()
        );
        let path = dir.join("questions.json");
        let questions: Vec<Question> = serde_json::from_slice(
            &std::fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("parse {}", path.display()))?;
        Ok(Self {
            index: Arc::new(index),
            questions,
        })
    }

    /// 볼 수 있는 조각 전부예요: 예시 질문의 후보예요.
    pub fn candidates(&self) -> Vec<usize> {
        (0..self.index.chunks.len())
            .filter(|&i| self.index.allowed.get(i) == Some(&true))
            .collect()
    }
}

/// 모델 파일이 없을 때 엔진이 대신 쓰는 임베더예요. 엔진은 뜨고, 임베딩이 필요한 곳만 `model`로 실패해요.
/// 모델을 다 받으면 앱이 엔진을 다시 띄워요.
pub struct MissingModel;

impl Embed for MissingModel {
    fn embed(&self, _texts: &[&str], _max_tokens: Option<usize>) -> anyhow::Result<Vec<Vec<f32>>> {
        Err(ServiceError {
            kind: Kind::Model,
            message: MODEL_MISSING.to_string(),
        }
        .into())
    }
}

/// 임베딩 실패의 종류예요: 모델이 없으면 `Model`, 나머지는 `Down`이에요.
pub fn embed_failure_kind(e: &anyhow::Error) -> Kind {
    e.chain()
        .find_map(|c| c.downcast_ref::<ServiceError>())
        .map_or(Kind::Down, |s| s.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_rank_is_the_first_answer_file_one_based() {
        let ranked = ["README.md", "docs/deploy.md", "scripts/deploy.sh"];
        let answer = vec![
            "scripts/deploy.sh".to_string(),
            "docs/deploy.md".to_string(),
        ];
        assert_eq!(answer_rank(&ranked, &answer), Some(2));
        assert_eq!(answer_rank(&ranked, &["src/config.ts".to_string()]), None);
        assert_eq!(answer_rank(&ranked, &[]), None);
    }

    #[test]
    fn build_question_keeps_text_and_use_and_reads_answer() {
        let q: BuildQuestion = serde_json::from_str(
            r#"{"text": "What's the webhook timeout?", "use": "ask", "answer": ["src/config.ts"]}"#,
        )
        .unwrap();
        assert_eq!(q.question.text, "What's the webhook timeout?");
        assert_eq!(q.question.purpose.as_deref(), Some("ask"));
        assert_eq!(q.answer, vec!["src/config.ts"]);
        // the bundled questions.json keeps only {text, use}
        assert_eq!(
            serde_json::to_string(&q.question).unwrap(),
            r#"{"text":"What's the webhook timeout?","use":"ask"}"#
        );
    }

    /// Both bundled samples (Korean `sample/index`, English `sample/en/index`) open and carry the practice's
    /// questions: three or more `ask` buttons and one `spoken` line.
    #[test]
    fn bundled_samples_open_in_both_languages() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../sample");
        for dir in [root.join("index"), root.join("en/index")] {
            let s = Sample::open(&dir).unwrap_or_else(|e| panic!("{}: {e:#}", dir.display()));
            let uses: Vec<_> = s.questions.iter().map(|q| q.purpose.as_deref()).collect();
            assert!(
                uses.iter().filter(|u| **u == Some("ask")).count() >= 3,
                "{}",
                dir.display()
            );
            assert_eq!(
                uses.iter().filter(|u| **u == Some("spoken")).count(),
                1,
                "{}",
                dir.display()
            );
            assert!(!s.candidates().is_empty());
        }
    }
}
