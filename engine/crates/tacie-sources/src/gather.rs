//! Gathers chunks from every source into one list: the repo first, then GitHub, then Slack. It also returns the source
//! info (`info`) to write in meta.json.
//!
//! - A source that failed to download but has a saved copy leaves its name in `warnings` and uses the saved copy
//!   (stderr `{name}: download failed, using the saved copy ({앞 120자})`). Without a saved copy the failure is
//!   returned as Err.
//! - A GitHub saved copy counts only in today's shape (state.json v2) and fully fetched (`fetched_at` present). If
//!   there is no saved copy and GitHub says the repo is not visible (`fetch().lost`), it is
//!   `Service{Key, "GitHub repo not visible"}`.
//! - A GitHub repo that got 404/410 `LOST_AT` times in a row or became a different repo is closed: no chunks, the
//!   warning `github-lost`, and `lost: true` in info. A single 404 gives the warning `github` and the saved copy.
//! - info key order: `github{repo, fetched_at, chunks, [lost]}`, `slack{team, fetched_at, chunks, channels}`.
//!
//! Raw Slack data is read once to build both the chunks and the info (nothing changes in between). It fails if `lost`
//! in `state.json` is neither a number nor a boolean (`Json`).

use std::path::Path;

use serde_json::{Map, Value};
use tacie_index::RawChunk;

use crate::error::{Kind, SourceError};
use crate::github::GitHub;
use crate::github::content::Snapshot;
use crate::github::docs::chunks as github_chunks;
use crate::github::fetch::fetch as github_fetch;
use crate::github::items::load_state;
use crate::github::probe::LOST_AT;
use crate::py;
use crate::repo;
use crate::slack::{self, SlackFetch};

/// `gather`에 줄 것이에요.
pub struct Gather<'a> {
    /// 자를 커밋의 파일이에요(`content::snapshot`). None이고 `code`도 None이면 레포 조각이 없어요.
    pub snapshot: Option<&'a Snapshot>,
    /// Graft 배선 그래프예요(없으면 스냅숏의 graft/.graph/wiring.json).
    pub graph: Option<&'a Path>,
    /// 그래프를 만든 뒤 바뀐 파일이에요(이름표를 붙이지 않아요).
    pub skip: &'a [String],
    /// GitHub 저장소(`owner/name`)예요. None이나 빈 글이면 GitHub은 빠져요.
    pub github_repo: Option<&'a str>,
    /// 그 저장소의 GitHub 원본 폴더예요.
    pub github_raw: &'a Path,
    /// 슬랙 원본 폴더예요. None이면 슬랙은 빠져요.
    pub slack_raw: Option<&'a Path>,
    /// 거짓이면 저장본이 있는 출처는 받지 않아요.
    pub fetch: bool,
    /// 이미 아는 레포 조각이에요(코드를 받지 못했을 때 지금 색인의 것). 있으면 `snapshot`보다 먼저예요.
    pub code: Option<Vec<RawChunk>>,
    /// 조각 캐시 파일이에요: blob이 바뀐 파일만 다시 잘라요. 스냅숏의 커밋이 비면 쓰지 않아요.
    pub cached: Option<&'a Path>,
}

/// 받기를 해 보고, 실패했는데 저장본이 있으면(`saved()`) 경고를 남기고 저장본으로 가요.
pub(crate) fn fetch_or_saved(
    name: &str,
    fetch: impl FnOnce() -> Result<(), SourceError>,
    saved: impl Fn() -> bool,
    warnings: &mut Vec<String>,
) -> Result<(), SourceError> {
    match fetch() {
        Ok(()) => Ok(()),
        Err(e) if !saved() => Err(e),
        Err(e) => {
            warnings.push(name.to_string());
            eprintln!(
                "{name}: download failed, using the saved copy ({})",
                py::take_chars(&e.to_string(), 120)
            );
            Ok(())
        }
    }
}

/// `lost >= LOST_AT`의 왼쪽이에요: 수나 참·거짓만 비교할 수 있어요.
fn lost_count(v: &Value) -> Result<f64, SourceError> {
    match v {
        Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| SourceError::Json("state.json's lost is not a number".to_string())),
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        _ => Err(SourceError::Json(
            "state.json's lost is not a number".to_string(),
        )),
    }
}

/// 모든 출처의 조각(레포 먼저)과 출처 정보예요. 모듈 설명을 보세요.
pub fn gather(
    gh: &GitHub,
    slack: &dyn SlackFetch,
    g: Gather<'_>,
    warnings: &mut Vec<String>,
) -> Result<(Vec<RawChunk>, Map<String, Value>), SourceError> {
    let mut chunks = match (g.code, g.snapshot) {
        (Some(code), _) => code,
        (None, Some(snap)) => {
            let cached = g.cached.filter(|_| !snap.tree.commit.is_empty());
            repo::chunks(snap, g.graph, g.skip, cached)?
        }
        (None, None) => Vec::new(),
    };
    let mut info = Map::new();
    if let Some(repo) = g.github_repo.filter(|r| !r.is_empty()) {
        let raw = g.github_raw;
        // 오늘 모양(state.json v2)으로 끝까지 받은 것만 저장본이에요
        let saved = || !load_state(raw)["fetched_at"].is_null();
        if g.fetch || !saved() {
            let fetch = || {
                // GitHub이 볼 수 없는 저장소인데 저장본도 없으면 이어 갈 것이 없어요: 거절처럼 실패해요
                if github_fetch(gh, repo, raw)?.lost != 0 && !saved() {
                    return Err(SourceError::service(Kind::Key, "GitHub repo not visible"));
                }
                Ok(())
            };
            fetch_or_saved("github", fetch, saved, warnings)?;
        }
        let st = load_state(raw);
        let lost_v = &st["lost"];
        // 닫힌 쪽이에요: GitHub이 다시 보여 줄 때까지 그 문서는 하나도 찾지 않아요
        let lost = lost_count(lost_v)? >= LOST_AT as f64;
        let found = if lost {
            Vec::new()
        } else {
            github_chunks(raw)?
        };
        let n = found.len();
        chunks.extend(found);
        if lost {
            warnings.retain(|w| w != "github");
            warnings.push("github-lost".to_string());
        } else if py::truthy(lost_v) && !warnings.iter().any(|w| w == "github") {
            warnings.push("github".to_string()); // 404가 한 번: 다음 갱신이 더 알려 줄 때까지 저장본이에요
        }
        let mut gi = Map::new();
        let name = if py::truthy(&st["repo"]) {
            st["repo"].clone()
        } else {
            Value::from(repo)
        };
        gi.insert("repo".into(), name);
        gi.insert("fetched_at".into(), st["fetched_at"].clone());
        gi.insert("chunks".into(), Value::from(n));
        if lost {
            gi.insert("lost".into(), Value::Bool(true));
        }
        info.insert("github".into(), Value::Object(gi));
    }
    if let Some(dir) = g.slack_raw {
        let file = dir.join("slack.json");
        if g.fetch || !file.exists() {
            fetch_or_saved("slack", || slack.fetch(dir), || file.exists(), warnings)?;
        }
        let Some(raw) = slack::read_raw(dir)? else {
            return Err(SourceError::Other(
                "slack: slack.json was not written".to_string(),
            ));
        };
        let found = slack::chunks_of(&raw)?;
        let missing = |what: &str| SourceError::Json(format!("slack.json: {what}"));
        let team = raw
            .get("team_url")
            .ok_or_else(|| missing("there is no team_url"))?;
        let fetched_at = raw
            .get("fetched_at")
            .ok_or_else(|| missing("there is no fetched_at"))?;
        let mut channels = Vec::new();
        for c in raw["channels"].as_array().into_iter().flatten() {
            let private = c
                .get("is_private")
                .ok_or_else(|| missing("a channel has no is_private"))?;
            let name = c
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| missing("a channel has no name"))?;
            let mark = if py::truthy(private) { "🔒" } else { "#" };
            channels.push(Value::from(format!("{mark}{name}")));
        }
        let mut si = Map::new();
        si.insert("team".into(), team.clone());
        si.insert("fetched_at".into(), fetched_at.clone());
        si.insert("chunks".into(), Value::from(found.len()));
        si.insert("channels".into(), Value::Array(channels));
        chunks.extend(found);
        info.insert("slack".into(), Value::Object(si));
    }
    Ok((chunks, info))
}
